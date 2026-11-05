//! Markdown preview pane: a read-only, scrollable rendering of the active
//! document.
//!
//! The layout is a flat list of draw primitives computed from the block model
//! (`model::markdown`) at a given width. The cache is keyed by
//! `(document id, revision, width, theme mode)`, so typing only relayouts when
//! the document actually changed and a resize/theme switch invalidates it too.
//!
//! Links and images remain display-only: links are styled but do not navigate
//! and images render as `▢ alt`. Tables are rendered with per-column alignment.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use igui_core::{Color, FontWeight, NodeId, Rect, Size, Vec2};
use igui_render::TextAlign;
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::model::markdown::{self, Align, Block, BlockKind, Run, Style, Table};
use crate::state::AppState;
use crate::theme::{Mode, Theme};

const PAD: f32 = 18.0;
const SCROLLBAR: f32 = 8.0;
/// Above this size the preview parses only a prefix and shows a banner.
pub const PREVIEW_MAX_BYTES: usize = 512 * 1024;
/// Above this size the preview is disabled (banner only).
pub const PREVIEW_LARGE_BYTES: usize = 8 * 1024 * 1024;
/// Above this size, edits are coalesced before a full parse + layout.
pub const PREVIEW_DEBOUNCE_BYTES: usize = 48 * 1024;
/// Trailing idle time before a coalesced preview rebuild.
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(120);

/// One backend-neutral preview drawing primitive (content-space coordinates).
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Text {
        x: f32,
        baseline: f32,
        text: String,
        size: f32,
        weight: FontWeight,
        color: Color,
        /// Inline-code background.
        bg: Option<Rect>,
        underline: bool,
        strike: bool,
    },
    Fill {
        rect: Rect,
        color: Color,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheKey {
    doc: u64,
    rev: u64,
    width: u32,
    mode: Mode,
    font: u32,
}

#[derive(Clone, Debug)]
struct Cache {
    key: CacheKey,
    prims: Vec<Prim>,
    height: f32,
}

/// Preview scroll + cached layout, stored on the app state.
#[derive(Default)]
pub struct PreviewUi {
    pub scroll: f32,
    pub total_h: f32,
    ///  The visible pane (for wheel/scrollbar hit tests).
    pub rect: Rect,
    pub vbar: Rect,
    pub vthumb: Rect,
    /// Grab offset while the scrollbar thumb is dragged.
    pub drag_grab: Option<f32>,
    cache: Option<Cache>,
    /// Parsed blocks and the `(document id, revision)` they came from.
    blocks: Vec<Block>,
    parsed_key: Option<(u64, u64)>,
    /// Block anchors for scroll sync (populated by the layout). `Rc` keeps the
    /// per-frame scroll-sync path from copying the list.
    pub anchors: Rc<Vec<(usize, f32)>>,
    /// The document was cut at [`PREVIEW_MAX_BYTES`].
    truncated: bool,
    /// The document is too large to preview at all.
    too_large: bool,
    /// Trailing debounce deadline while typing in a large Markdown file.
    rebuild_at: Option<Instant>,
    /// Revision that scheduled `rebuild_at` (a new revision pushes it out).
    deferred_rev: Option<u64>,
}

impl PreviewUi {
    /// Whether a coalesced rebuild is waiting for its debounce deadline.
    pub fn rebuild_pending(&self) -> bool {
        self.rebuild_at.is_some()
    }

    /// The debounce deadline, for the frame scheduler.
    pub fn rebuild_deadline(&self) -> Option<Instant> {
        self.rebuild_at
    }

    /// Drops a pending rebuild when the preview pane is no longer mounted.
    pub fn cancel_rebuild(&mut self) {
        self.rebuild_at = None;
        self.deferred_rev = None;
    }
}

impl std::fmt::Debug for PreviewUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewUi")
            .field("scroll", &self.scroll)
            .field("total_h", &self.total_h)
            .field("rect", &self.rect)
            .finish()
    }
}

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "preview");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let max = (state.preview.total_h - state.preview.rect.height()).max(0.0);
            crate::ui::scroll::apply(&mut state.preview.scroll, delta.y, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => {
                    // Clicking the rendered pane returns typing to the document
                    // (split view) or leaves nothing to type into (preview only).
                    state.focus = crate::state::Focus::Editor;
                    if state.preview.vbar.contains(pos) && state.preview.vthumb.width() > 0.0 {
                        state.preview.drag_grab = Some(pos.y - state.preview.vthumb.top());
                        scroll_to(&mut state, pos.y);
                    }
                }
                PointerPhase::Move => {
                    if state.preview.drag_grab.is_some() {
                        scroll_to(&mut state, pos.y);
                    }
                }
                PointerPhase::Up => state.preview.drag_grab = None,
                PointerPhase::DoubleClick => {}
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    id
}

fn scroll_to(state: &mut AppState, y: f32) {
    let Some(grab) = state.preview.drag_grab else {
        return;
    };
    let bar = state.preview.vbar;
    let thumb_h = state.preview.vthumb.height();
    let travel = (bar.height() - thumb_h).max(1.0);
    let top = (y - grab).clamp(bar.top(), bar.bottom() - thumb_h);
    let ratio = (top - bar.top()) / travel;
    let max = (state.preview.total_h - bar.height()).max(0.0);
    state.preview.scroll = ratio * max;
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let p = theme.p;
    let measurer = env.measurer;

    kit::fill(env.ctx, rect, p.bg);

    let view = Rect::from_min_size(
        Vec2::new(rect.left() + 1.0, rect.top()),
        Size::new(
            (rect.width() - SCROLLBAR - 1.0).max(16.0),
            rect.height().max(16.0),
        ),
    );
    state.preview.rect = rect;

    let Some((doc_id, rev, len)) = state
        .ws
        .active_doc()
        .map(|doc| (doc.id(), doc.rev(), doc.text().len()))
    else {
        state.preview.rebuild_at = None;
        state.preview.deferred_rev = None;
        return;
    };
    let body = state.settings.editor().font;
    let key = CacheKey {
        doc: doc_id,
        rev,
        width: view.width().round() as u32,
        mode: theme.mode,
        font: body.round() as u32,
    };
    let stale = state
        .preview
        .cache
        .as_ref()
        .is_none_or(|cache| cache.key != key);
    if stale {
        // Parsing + laying out a large document costs milliseconds; doing it
        // on every keystroke pins a core. Coalesce edits for large Markdown
        // files and keep painting the previous layout until the user pauses.
        let same_layout = state.preview.cache.as_ref().is_some_and(|cache| {
            cache.key.doc == key.doc
                && cache.key.width == key.width
                && cache.key.mode == key.mode
                && cache.key.font == key.font
        });
        let now = Instant::now();
        if same_layout && len > PREVIEW_DEBOUNCE_BYTES {
            if state.preview.deferred_rev != Some(rev) {
                state.preview.deferred_rev = Some(rev);
                state.preview.rebuild_at = Some(now + PREVIEW_DEBOUNCE);
            }
            if state.preview.rebuild_at.is_some_and(|due| now < due) {
                // Paint the previous layout; `needs_frame` wakes us at the
                // deadline so the pending rebuild is not lost.
            } else {
                rebuild(state, measurer, &theme, view, key, p, body);
            }
        } else {
            rebuild(state, measurer, &theme, view, key, p, body);
        }
    } else {
        state.preview.rebuild_at = None;
        state.preview.deferred_rev = None;
    }
    let Some(cache) = state.preview.cache.as_ref() else {
        return;
    };
    let height = cache.height;
    state.preview.total_h = height;
    let max_scroll = (height - view.height()).max(0.0);
    state.preview.scroll = state.preview.scroll.clamp(0.0, max_scroll);

    env.ctx.save();
    env.ctx.clip_rect(view);
    let offset = Vec2::new(view.left(), view.top() - state.preview.scroll);
    let visible_top = state.preview.scroll;
    let visible_bottom = visible_top + view.height();
    for prim in &cache.prims {
        match prim {
            Prim::Fill { rect, color } => {
                if rect.bottom() < visible_top || rect.top() > visible_bottom {
                    continue;
                }
                env.ctx.fill_rect(rect.translate(offset), *color);
            }
            Prim::Text {
                x,
                baseline,
                text,
                size,
                weight,
                color,
                bg,
                underline,
                strike,
            } => {
                // Baselines are content-space; allow a generous ink margin so
                // descenders, inline-code fills and strike/underline rules are
                // never clipped by the cull.
                if baseline + size * 0.6 < visible_top || baseline - size * 1.6 > visible_bottom {
                    continue;
                }
                if let Some(bg) = bg {
                    env.ctx.fill_rect(bg.translate(offset), p.raised);
                }
                let baseline = *baseline + offset.y;
                env.ctx.draw_text_weighted(
                    text.as_str(),
                    Vec2::new(*x + offset.x, baseline),
                    *size,
                    *weight,
                    igui_render::TextAlign::Left,
                    *color,
                );
                if *underline {
                    env.ctx.fill_rect(
                        Rect::from_min_size(
                            Vec2::new(*x + offset.x, baseline + 2.0),
                            Size::new(kit::width(measurer, text, *size), 1.0),
                        ),
                        *color,
                    );
                }
                if *strike {
                    env.ctx.fill_rect(
                        Rect::from_min_size(
                            Vec2::new(*x + offset.x, baseline - size * 0.28),
                            Size::new(kit::width(measurer, text, *size), 1.0),
                        ),
                        p.faint,
                    );
                }
            }
        }
    }
    env.ctx.restore();

    // Scrollbar (only when the content overflows).
    let vbar = Rect::from_min_size(
        Vec2::new(rect.right() - SCROLLBAR, rect.top()),
        Size::new(SCROLLBAR, rect.height()),
    );
    let mut vthumb = Rect::ZERO;
    if max_scroll > 0.0 {
        let thumb_h = (view.height() * view.height() / height)
            .min(view.height())
            .max(1.0);
        let travel = (view.height() - thumb_h).max(0.0);
        let y = vbar.top() + (state.preview.scroll / max_scroll) * travel;
        vthumb = Rect::from_min_size(
            Vec2::new(vbar.left() + 2.0, y),
            Size::new(SCROLLBAR - 4.0, thumb_h),
        );
        env.ctx.fill_rect(vthumb, p.border);
    }
    state.preview.vbar = vbar;
    state.preview.vthumb = vthumb;
}

// -- layout ------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn rebuild(
    state: &mut AppState,
    measurer: &dyn TextMeasurer,
    theme: &Theme,
    view: Rect,
    key: CacheKey,
    p: crate::theme::Palette,
    body: f32,
) {
    let text = state
        .ws
        .active_doc()
        .filter(|doc| state.preview.parsed_key != Some((doc.id(), doc.rev())))
        .map(|doc| doc.text().to_string());
    if let Some(text) = text {
        let len = text.len();
        state.preview.too_large = len > PREVIEW_LARGE_BYTES;
        state.preview.truncated = len > PREVIEW_MAX_BYTES && !state.preview.too_large;
        state.preview.blocks = if state.preview.too_large {
            Vec::new()
        } else if state.preview.truncated {
            markdown::parse(&text[..truncate_boundary(&text, PREVIEW_MAX_BYTES)])
        } else {
            markdown::parse(&text)
        };
        state.preview.parsed_key = Some((key.doc, key.rev));
    }
    let (mut prims, mut height, anchors) = if state.preview.too_large {
        (
            vec![banner(
                "preview disabled for files over 8 MB — use the editor",
                p.faint,
                0.0,
            )],
            44.0,
            Vec::new(),
        )
    } else {
        layout(&state.preview.blocks, measurer, view.width(), theme, body)
    };
    if state.preview.truncated {
        prims.push(banner(
            "… preview truncated at 512 KB — use the editor for the rest",
            p.faint,
            height,
        ));
        height += 28.0;
    }
    state.preview.anchors = Rc::new(anchors);
    state.preview.cache = Some(Cache { key, prims, height });
    state.preview.rebuild_at = None;
    state.preview.deferred_rev = None;
    state.dirty = true;
}

// -- layout ------------------------------------------------------------------

pub(crate) fn layout(
    blocks: &[Block],
    measurer: &dyn TextMeasurer,
    width: f32,
    theme: &Theme,
    body: f32,
) -> (Vec<Prim>, f32, Vec<(usize, f32)>) {
    let p = theme.p;
    let body_line = (body * 1.54).round().max(14.0);
    let code_font = (body - 1.0).max(9.0);
    let code_line = (code_font * 1.5).round().max(12.0);
    let content_w = (width - PAD * 2.0).max(40.0);
    let mut prims = Vec::new();
    let mut anchors: Vec<(usize, f32)> = Vec::with_capacity(blocks.len() + 1);
    let mut y = PAD;

    for block in blocks {
        anchors.push((block.start_line, y));
        let quote_x = block.quote as f32 * 12.0;
        let indent_x = block.indent as f32 * 18.0;
        let x = PAD + quote_x + indent_x;
        let max_w = (content_w - quote_x - indent_x).max(30.0);
        let base_color = if block.quote > 0 { p.dim } else { p.text };
        let start_y = y;

        match &block.kind {
            BlockKind::Heading { level, runs } => {
                let (size, weight): (f32, FontWeight) = match level {
                    1 => (body + 8.0, FontWeight::BOLD),
                    2 => (body + 5.0, FontWeight::BOLD),
                    3 => (body + 3.0, FontWeight::SEMI_BOLD),
                    4 => (body + 1.0, FontWeight::SEMI_BOLD),
                    _ => (body, FontWeight::MEDIUM),
                };
                let line_h = (size * 1.35).round().max(body_line);
                y = place_runs(
                    &mut prims,
                    measurer,
                    runs,
                    x,
                    max_w,
                    y,
                    size,
                    line_h,
                    weight,
                    p.text,
                    theme,
                    TextAlign::Left,
                );
                y += if *level <= 2 { 12.0 } else { 8.0 };
            }
            BlockKind::Paragraph(runs) => {
                y = place_runs(
                    &mut prims,
                    measurer,
                    runs,
                    x,
                    max_w,
                    y,
                    body,
                    body_line,
                    FontWeight::NORMAL,
                    base_color,
                    theme,
                    TextAlign::Left,
                );
                y += 8.0;
            }
            BlockKind::Item {
                ordered,
                number,
                runs,
            } => {
                let marker = if *ordered {
                    format!("{number}.")
                } else {
                    "•".to_string()
                };
                let marker_w = kit::width(measurer, &marker, body) + 8.0;
                prims.push(Prim::Text {
                    x,
                    baseline: baseline(y, body_line, body, measurer),
                    text: marker,
                    size: body,
                    weight: FontWeight::NORMAL,
                    color: p.faint,
                    bg: None,
                    underline: false,
                    strike: false,
                });
                y = place_runs(
                    &mut prims,
                    measurer,
                    runs,
                    x + marker_w,
                    (max_w - marker_w).max(20.0),
                    y,
                    body,
                    body_line,
                    FontWeight::NORMAL,
                    base_color,
                    theme,
                    TextAlign::Left,
                );
                y += 2.0;
            }
            BlockKind::Code { lang, lines } => {
                let line_h = code_line;
                let height = lines.len().max(1) as f32 * line_h + 16.0;
                prims.push(Prim::Fill {
                    rect: Rect::from_min_size(Vec2::new(PAD, y), Size::new(content_w, height)),
                    color: p.raised,
                });
                if !lang.is_empty() {
                    let label = lang.to_uppercase();
                    prims.push(Prim::Text {
                        x: PAD + content_w - kit::width(measurer, &label, 10.0) - 8.0,
                        baseline: y + 12.0,
                        text: label,
                        size: 10.0,
                        weight: FontWeight::NORMAL,
                        color: p.faint,
                        bg: None,
                        underline: false,
                        strike: false,
                    });
                }
                for (index, line) in lines.iter().enumerate() {
                    if line.is_empty() {
                        continue;
                    }
                    prims.push(Prim::Text {
                        x: PAD + 10.0,
                        baseline: y + 8.0 + index as f32 * line_h + 13.0,
                        text: line.clone(),
                        size: code_font,
                        weight: FontWeight::NORMAL,
                        color: p.text,
                        bg: None,
                        underline: false,
                        strike: false,
                    });
                }
                y += height + 10.0;
            }
            BlockKind::Rule => {
                prims.push(Prim::Fill {
                    rect: Rect::from_min_size(Vec2::new(PAD, y + 4.0), Size::new(content_w, 1.0)),
                    color: p.border,
                });
                y += 16.0;
            }
            BlockKind::Image { alt } => {
                prims.push(Prim::Text {
                    x,
                    baseline: baseline(y, body_line, body, measurer),
                    text: format!("▢ {alt}"),
                    size: body,
                    weight: FontWeight::NORMAL,
                    color: p.faint,
                    bg: None,
                    underline: false,
                    strike: false,
                });
                y += body_line + 4.0;
            }
            BlockKind::Table(table) => {
                y = layout_table(
                    &mut prims, measurer, table, x, max_w, y, theme, body, body_line,
                );
                y += 12.0;
            }
        }

        if block.quote > 0 {
            for level in 0..block.quote {
                let lx = PAD + level as f32 * 12.0 - 6.0;
                prims.push(Prim::Fill {
                    rect: Rect::from_min_size(
                        Vec2::new(lx, start_y),
                        Size::new(2.0, (y - start_y).max(4.0)),
                    ),
                    color: p.accent,
                });
            }
        }
    }

    if let Some(last) = blocks.last() {
        anchors.push((last.end_line, y));
    }
    (prims, y + PAD, anchors)
}

fn baseline(top: f32, line_h: f32, size: f32, measurer: &dyn TextMeasurer) -> f32 {
    kit::line_baseline(measurer, top, line_h, size)
}

#[derive(Debug)]
struct Token {
    segments: Vec<(String, Style)>,
    /// Advance width at the block's base weight (bold runs measured bold).
    width: f32,
    space_before: bool,
    hard_break: bool,
}

fn tokenize(
    runs: &[Run],
    measurer: &dyn TextMeasurer,
    size: f32,
    weight: FontWeight,
) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut segments: Vec<(String, Style)> = Vec::new();
    let mut pending_space = false;
    let mut space_before = false;

    let measure = |segments: &[(String, Style)]| -> f32 {
        segments
            .iter()
            .map(|(text, style)| {
                kit::width_weighted(measurer, text, size, style_weight(*style, weight))
            })
            .sum()
    };
    let flush =
        |tokens: &mut Vec<Token>, segments: &mut Vec<(String, Style)>, space_before: bool| {
            if segments.is_empty() {
                return;
            }
            let width = measure(segments);
            tokens.push(Token {
                segments: std::mem::take(segments),
                width,
                space_before,
                hard_break: false,
            });
        };

    for run in runs {
        for ch in run.text.chars() {
            if ch == '\n' {
                flush(&mut tokens, &mut segments, space_before);
                space_before = false;
                pending_space = false;
                tokens.push(Token {
                    segments: Vec::new(),
                    width: 0.0,
                    space_before: false,
                    hard_break: true,
                });
            } else if ch.is_whitespace() {
                flush(&mut tokens, &mut segments, space_before);
                space_before = false;
                pending_space = true;
            } else {
                if segments.is_empty() {
                    space_before = pending_space && !tokens.is_empty();
                    pending_space = false;
                }
                if let Some((text, style)) = segments.last_mut() {
                    if *style == run.style {
                        text.push(ch);
                        continue;
                    }
                }
                segments.push((ch.to_string(), run.style));
            }
        }
    }
    flush(&mut tokens, &mut segments, space_before);
    tokens
}

/// Splits a token wider than the line into character chunks, so long words and
/// CJK paragraphs wrap instead of overflowing.
fn split_long_tokens(
    tokens: Vec<Token>,
    measurer: &dyn TextMeasurer,
    size: f32,
    weight: FontWeight,
    max_w: f32,
) -> Vec<Token> {
    let mut out = Vec::new();
    for token in tokens {
        if token.hard_break || token.width <= max_w || token.segments.is_empty() {
            out.push(token);
            continue;
        }
        let mut segments: Vec<(String, Style)> = Vec::new();
        let mut width = 0.0;
        let mut space_before = token.space_before;
        for (text, style) in &token.segments {
            for ch in text.chars() {
                // A single character does not need a temporary `String` just
                // to be measured; long CJK lines hit this per character.
                let advance = measurer.advance_weighted(ch, size, style_weight(*style, weight));
                if width + advance > max_w && !segments.is_empty() {
                    out.push(Token {
                        segments: std::mem::take(&mut segments),
                        width,
                        space_before,
                        hard_break: false,
                    });
                    width = 0.0;
                    space_before = false;
                }
                if let Some((last, last_style)) = segments.last_mut() {
                    if last_style == style {
                        last.push(ch);
                        width += advance;
                        continue;
                    }
                }
                segments.push((ch.to_string(), *style));
                width += advance;
            }
        }
        if !segments.is_empty() {
            out.push(Token {
                segments,
                width,
                space_before,
                hard_break: false,
            });
        }
    }
    out
}

fn wrap_tokens(tokens: Vec<Token>, max_w: f32, space_w: f32) -> Vec<Vec<Token>> {
    let mut lines: Vec<Vec<Token>> = Vec::new();
    let mut line: Vec<Token> = Vec::new();
    let mut used = 0.0;
    for token in tokens {
        if token.hard_break {
            lines.push(std::mem::take(&mut line));
            used = 0.0;
            continue;
        }
        let separator = if !line.is_empty() && token.space_before {
            space_w
        } else {
            0.0
        };
        if !line.is_empty() && used + separator + token.width > max_w {
            lines.push(std::mem::take(&mut line));
            used = 0.0;
        }
        let separator = if !line.is_empty() && token.space_before {
            space_w
        } else {
            0.0
        };
        used += separator + token.width;
        line.push(token);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn place_runs(
    prims: &mut Vec<Prim>,
    measurer: &dyn TextMeasurer,
    runs: &[Run],
    x0: f32,
    max_w: f32,
    y_start: f32,
    size: f32,
    line_h: f32,
    weight: FontWeight,
    color: Color,
    theme: &Theme,
    align: TextAlign,
) -> f32 {
    let p = theme.p;
    let max_w = max_w.max(8.0);
    let space_w = measurer.advance(' ', size);
    let tokens = tokenize(runs, measurer, size, weight);
    let tokens = split_long_tokens(tokens, measurer, size, weight, max_w);
    let lines = wrap_tokens(tokens, max_w, space_w);
    let ascent = measurer.ascent(size);

    let mut y = y_start;
    for line in &lines {
        let total: f32 = line
            .iter()
            .enumerate()
            .map(|(index, token)| {
                token.width
                    + if index > 0 && token.space_before {
                        space_w
                    } else {
                        0.0
                    }
            })
            .sum();
        let offset = match align {
            TextAlign::Left => 0.0,
            TextAlign::Center => ((max_w - total) * 0.5).max(0.0),
            TextAlign::Right => (max_w - total).max(0.0),
        };
        let baseline = kit::line_baseline_with_ascent(y, line_h, size, ascent);
        let mut x = x0 + offset;
        for (index, token) in line.iter().enumerate() {
            if index > 0 && token.space_before {
                x += space_w;
            }
            for (text, style) in &token.segments {
                let run_weight = style_weight(*style, weight);
                let advance = kit::width_weighted(measurer, text, size, run_weight);
                emit(
                    prims,
                    measurer,
                    text,
                    x,
                    baseline,
                    size,
                    run_weight,
                    run_color(*style, color, p),
                    *style,
                );
                x += advance;
            }
        }
        y += line_h;
    }
    y
}

/// Natural (unwrapped) width of a cell's runs.
fn runs_width(measurer: &dyn TextMeasurer, runs: &[Run], size: f32, weight: FontWeight) -> f32 {
    let mut width = 0.0;
    let mut pending_space = false;
    for run in runs {
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                pending_space = true;
                continue;
            }
            if pending_space && width > 0.0 {
                width += measurer.advance(' ', size);
            }
            pending_space = false;
            width += measurer.advance_weighted(ch, size, style_weight(run.style, weight));
        }
    }
    width
}

const TABLE_PAD_X: f32 = 8.0;
const TABLE_PAD_Y: f32 = 6.0;
const TABLE_MIN_COL: f32 = 56.0;
const TABLE_MAX_COL: f32 = 320.0;

/// Lays out one table: natural column widths (shrunk to fit), header row with a
/// raised background, one rule per row and square outer borders.
#[allow(clippy::too_many_arguments)]
fn layout_table(
    prims: &mut Vec<Prim>,
    measurer: &dyn TextMeasurer,
    table: &Table,
    x: f32,
    max_w: f32,
    y: f32,
    theme: &Theme,
    body: f32,
    body_line: f32,
) -> f32 {
    let p = theme.p;
    let columns = table.columns();
    let mut widths = vec![TABLE_MIN_COL; columns];
    for (column, width) in widths.iter_mut().enumerate() {
        let mut widest: f32 = 0.0;
        if let Some(cell) = table.header.get(column) {
            widest = widest.max(runs_width(measurer, cell, body, FontWeight::BOLD));
        }
        for row in &table.rows {
            if let Some(cell) = row.get(column) {
                widest = widest.max(runs_width(measurer, cell, body, FontWeight::NORMAL));
            }
        }
        *width = (widest + TABLE_PAD_X * 2.0).clamp(TABLE_MIN_COL, TABLE_MAX_COL);
    }
    let natural: f32 = widths.iter().sum();
    if natural > max_w {
        let scale = max_w / natural;
        for width in &mut widths {
            *width = (*width * scale).max(TABLE_MIN_COL);
        }
    }
    let total: f32 = widths.iter().sum();

    let mut row_y = y;
    let header_start = prims.len();
    let header_h = layout_table_row(
        prims,
        measurer,
        &table.header,
        &widths,
        table,
        x,
        row_y,
        theme,
        true,
        body,
        body_line,
    );
    prims.insert(
        header_start,
        Prim::Fill {
            rect: Rect::from_min_size(Vec2::new(x, row_y), Size::new(total, header_h)),
            color: p.raised,
        },
    );
    prims.push(Prim::Fill {
        rect: Rect::from_min_size(Vec2::new(x, row_y + header_h - 1.0), Size::new(total, 1.0)),
        color: p.border,
    });
    row_y += header_h;

    for row in &table.rows {
        let height = layout_table_row(
            prims, measurer, row, &widths, table, x, row_y, theme, false, body, body_line,
        );
        prims.push(Prim::Fill {
            rect: Rect::from_min_size(Vec2::new(x, row_y + height - 1.0), Size::new(total, 1.0)),
            color: p.border,
        });
        row_y += height;
    }

    // Square outer border.
    prims.push(Prim::Fill {
        rect: Rect::from_min_size(Vec2::new(x, y), Size::new(total, 1.0)),
        color: p.border,
    });
    prims.push(Prim::Fill {
        rect: Rect::from_min_size(Vec2::new(x, row_y - 1.0), Size::new(total, 1.0)),
        color: p.border,
    });
    prims.push(Prim::Fill {
        rect: Rect::from_min_size(Vec2::new(x, y), Size::new(1.0, row_y - y)),
        color: p.border,
    });
    prims.push(Prim::Fill {
        rect: Rect::from_min_size(Vec2::new(x + total - 1.0, y), Size::new(1.0, row_y - y)),
        color: p.border,
    });
    row_y
}

#[allow(clippy::too_many_arguments)]
fn layout_table_row(
    prims: &mut Vec<Prim>,
    measurer: &dyn TextMeasurer,
    cells: &[Vec<Run>],
    widths: &[f32],
    table: &Table,
    x: f32,
    y: f32,
    theme: &Theme,
    header: bool,
    body: f32,
    body_line: f32,
) -> f32 {
    let p = theme.p;
    let weight = if header {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    let mut bottom = y + body_line;
    let mut cell_x = x;
    for (column, width) in widths.iter().enumerate() {
        let runs = cells.get(column).map(Vec::as_slice).unwrap_or(&[]);
        let align = match table.align(column) {
            Align::Left => TextAlign::Left,
            Align::Center => TextAlign::Center,
            Align::Right => TextAlign::Right,
        };
        bottom = bottom.max(place_runs(
            prims,
            measurer,
            runs,
            cell_x + TABLE_PAD_X,
            (width - TABLE_PAD_X * 2.0).max(8.0),
            y + TABLE_PAD_Y,
            body,
            body_line,
            weight,
            p.text,
            theme,
            align,
        ));
        cell_x += width;
    }
    (bottom - y) + TABLE_PAD_Y
}

#[allow(clippy::too_many_arguments)]
fn emit(
    prims: &mut Vec<Prim>,
    measurer: &dyn TextMeasurer,
    text: &str,
    x: f32,
    baseline: f32,
    size: f32,
    weight: FontWeight,
    color: Color,
    style: Style,
) {
    if text.is_empty() {
        return;
    }
    let bg = if style == Style::Code {
        Some(Rect::from_min_size(
            Vec2::new(x - 2.0, baseline - size - 1.0),
            Size::new(kit::width(measurer, text, size) + 4.0, size + 3.0),
        ))
    } else {
        None
    };
    prims.push(Prim::Text {
        x,
        baseline,
        text: text.to_string(),
        size,
        weight,
        color,
        bg,
        underline: style == Style::Link,
        strike: style == Style::Strikethrough,
    });
}

fn style_weight(style: Style, base: FontWeight) -> FontWeight {
    match style {
        Style::Strong => FontWeight::BOLD.max(base),
        Style::Emphasis if base == FontWeight::NORMAL => FontWeight::MEDIUM,
        _ => base,
    }
}

fn run_color(style: Style, base: Color, p: crate::theme::Palette) -> Color {
    match style {
        Style::Code | Style::Link => p.accent,
        Style::Strikethrough => p.dim,
        _ => base,
    }
}

fn truncate_boundary(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut index = max;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn banner(text: &str, color: Color, y: f32) -> Prim {
    Prim::Text {
        x: PAD,
        baseline: y + 20.0,
        text: text.to_string(),
        size: 12.0,
        weight: FontWeight::NORMAL,
        color,
        bg: None,
        underline: false,
        strike: false,
    }
}
