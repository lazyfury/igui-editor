//! LSP overlays: hover card and completion list.
//!
//! Both are painted after the tree (above the editor, below menus/modals) and
//! positioned from the real caret rectangle recorded by the editor painter.

use std::sync::Arc;

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;
use crate::syntax::{Highlight, SyntaxProvider, SyntaxRegistry};

const HOVER_W: f32 = 480.0;
const HOVER_MAX_H: f32 = 300.0;
const COMPLETION_W: f32 = 400.0;
const COMPLETION_ROW_H: f32 = 26.0;
const COMPLETION_MAX_ROWS: usize = 10;
const COMPLETION_PREVIEW_W: f32 = 380.0;
const COMPLETION_PREVIEW_MAX_H: f32 = 230.0;

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    state.lsp.hover_rect = Rect::ZERO;
    state.lsp.hover_doc_hit = Rect::ZERO;
    state.lsp.completion_doc_hit = Rect::ZERO;
    if state.lsp.completion.is_some() {
        paint_completion(state, measurer, ctx);
    } else if state.lsp.hover.is_some() {
        paint_hover(state, measurer, ctx);
    }
}

/// One display line in a hover card. Fenced code keeps its syntax tokens so
/// Rust-analyzer/Godot hover snippets read like the editor instead of a wall
/// of single-colored text.
struct HoverLine {
    text: String,
    /// A full fenced-code line (painted with a block background).
    code: bool,
    /// Inline-code byte ranges inside `text` (painted with a code background).
    code_ranges: Vec<(usize, usize)>,
    tokens: Vec<Highlight>,
}

fn hover_provider(
    registry: &SyntaxRegistry,
    info: &str,
    active: Option<Arc<dyn SyntaxProvider>>,
) -> Option<Arc<dyn SyntaxProvider>> {
    let language = info
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .find(|part| !part.is_empty())
        .unwrap_or("");
    if language.is_empty() {
        return active;
    }
    registry
        .by_name(language)
        .or_else(|| registry.provider_for_extension(language))
}

fn looks_like_declaration(line: &str) -> bool {
    let first = line
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == '(' || ch == '{' || ch == ':')
        .next()
        .unwrap_or("");
    matches!(
        first,
        "fn" | "func"
            | "class"
            | "struct"
            | "enum"
            | "trait"
            | "impl"
            | "let"
            | "var"
            | "const"
            | "pub"
            | "def"
            | "interface"
            | "type"
            | "namespace"
            | "signal"
            | "property"
    )
}

fn build_hover_lines(
    text: &str,
    registry: &SyntaxRegistry,
    active: Option<Arc<dyn SyntaxProvider>>,
    max_width: f32,
    measurer: &dyn TextMeasurer,
    font: f32,
) -> Vec<HoverLine> {
    let mut lines = Vec::new();
    let mut fence: Option<(Option<Arc<dyn SyntaxProvider>>, u32)> = None;
    let mut first_content = true;
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        let start = trimmed.trim_start();
        // Fallback for raw Godot BBCode; current Godot LSP normally converts
        // these to Markdown fences before sending the hover.
        if start == "[codeblocks]" || start == "[/codeblocks]" {
            first_content = false;
            continue;
        }
        if start.starts_with("[codeblock") || start == "[gdscript]" {
            let info = start.strip_prefix("[codeblock").unwrap_or("").trim();
            let info = info.strip_suffix(']').unwrap_or(info).trim();
            let info = info.strip_prefix("lang=").unwrap_or(info).trim();
            fence = Some((hover_provider(registry, info, active.clone()), 0));
            first_content = false;
            continue;
        }
        if start == "[/codeblock]" || start == "[/gdscript]" {
            fence = None;
            first_content = false;
            continue;
        }
        if start.starts_with("```") || start.starts_with("~~~") {
            if fence.is_some() {
                fence = None;
            } else {
                let info = trimmed.trim_start().trim_start_matches(['`', '~']).trim();
                fence = Some((hover_provider(registry, info, active.clone()), 0));
            }
            first_content = false;
            continue;
        }
        if let Some((provider, state)) = fence.as_mut() {
            let expanded = crate::model::wrap::expand_tabs(trimmed, 4);
            let (tokens, next) = provider
                .as_ref()
                .filter(|provider| expanded.len() <= provider.max_line_len())
                .map(|provider| provider.highlight_line(&expanded, *state))
                .unwrap_or_default();
            *state = next;
            lines.push(HoverLine {
                text: expanded,
                code: true,
                code_ranges: Vec::new(),
                tokens,
            });
            first_content = false;
            continue;
        }
        let cleaned = trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("### "))
            .unwrap_or(trimmed);
        let (cleaned, inline_ranges) = clean_inline_with_code(cleaned);
        if cleaned.is_empty() {
            lines.push(HoverLine {
                text: String::new(),
                code: false,
                code_ranges: Vec::new(),
                tokens: Vec::new(),
            });
        } else if !inline_ranges.is_empty() {
            // Godot's `[code]…[/code]` becomes `` `…` `` in LSP markdown.
            // Keep those spans as inline code and highlight code-like keywords.
            let tokens = inline_code_tokens(&cleaned, &inline_ranges, active.as_deref());
            let wrapped = wrap_styled_line(
                &cleaned,
                &inline_ranges,
                &tokens,
                max_width - 20.0,
                measurer,
                font,
            );
            lines.extend(wrapped);
        } else if first_content
            && looks_like_declaration(&cleaned)
            && active
                .as_ref()
                .is_some_and(|provider| cleaned.len() <= provider.max_line_len())
        {
            // Native Godot hovers and plain-text LSP declarations start with
            // `func …` / `fn …` rather than a fenced block. Highlight the
            // first declaration line as code too.
            let provider = active.as_ref().expect("checked above");
            let (tokens, _) = provider.highlight_line(&cleaned, provider.neutral_state());
            lines.push(HoverLine {
                text: cleaned,
                code: true,
                code_ranges: Vec::new(),
                tokens,
            });
        } else {
            let mut wrapped = Vec::new();
            wrap_line(&cleaned, max_width - 20.0, measurer, font, &mut wrapped);
            for line in wrapped {
                lines.push(HoverLine {
                    text: line,
                    code: false,
                    code_ranges: Vec::new(),
                    tokens: Vec::new(),
                });
            }
        }
        first_content = false;
    }
    lines
}

fn inline_code_tokens(
    text: &str,
    ranges: &[(usize, usize)],
    active: Option<&dyn SyntaxProvider>,
) -> Vec<Highlight> {
    let Some(provider) = active else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for &(start, end) in ranges {
        let span = &text[start..end];
        if span.is_empty() || span.len() > provider.max_line_len() {
            continue;
        }
        let (found, _) = provider.highlight_line(span, provider.neutral_state());
        for token in found {
            let token_start = start + token.start;
            let token_end = start + token.end;
            if token_start < token_end && token_end <= end {
                tokens.push(Highlight {
                    start: token_start,
                    end: token_end,
                    kind: token.kind,
                });
            }
        }
    }
    tokens
}

/// Byte ranges for visual lines, used when inline-code styling must survive
/// wrapping. Long words are split on character boundaries.
fn wrap_byte_spans(
    text: &str,
    max_width: f32,
    measurer: &dyn TextMeasurer,
    font: f32,
) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut index = 0usize;
    let mut line_start = 0usize;
    let mut line_end = 0usize;
    let mut line_w = 0.0f32;
    while index < text.len() {
        let rest = &text[index..];
        let raw_len = rest.find(' ').map_or(rest.len(), |position| position + 1);
        let raw_end = index + raw_len;
        let raw = &text[index..raw_end];
        let word = raw.trim_end();
        let word_end = index + word.len();
        if word.is_empty() {
            index = raw_end;
            continue;
        }
        let raw_w = kit::width(measurer, raw, font);
        let word_w = kit::width(measurer, word, font);
        if line_start == line_end && word_w > max_width {
            let mut chunk_start = index;
            let mut chunk_w = 0.0f32;
            for (offset, ch) in text[index..word_end].char_indices() {
                let advance = measurer.advance(ch, font);
                if chunk_w + advance > max_width && chunk_start < index + offset {
                    lines.push((chunk_start, index + offset));
                    chunk_start = index + offset;
                    chunk_w = 0.0;
                }
                chunk_w += advance;
            }
            if chunk_start < word_end {
                lines.push((chunk_start, word_end));
            }
            line_start = word_end;
            line_end = word_end;
            line_w = 0.0;
            index = raw_end;
            continue;
        }
        if line_start != line_end && line_w + raw_w > max_width {
            lines.push((line_start, line_end));
            line_start = index;
            line_end = word_end;
            line_w = word_w;
        } else {
            if line_start == line_end {
                line_start = index;
                line_w = 0.0;
            }
            line_end = word_end;
            line_w += raw_w;
        }
        index = raw_end;
    }
    if line_start < line_end {
        lines.push((line_start, line_end));
    }
    lines
}

fn clip_ranges(ranges: &[(usize, usize)], start: usize, end: usize) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .filter_map(|&(from, to)| {
            let from = from.max(start);
            let to = to.min(end);
            (from < to && to >= start).then(|| (from - start, to - start))
        })
        .collect()
}

fn clip_tokens(tokens: &[Highlight], start: usize, end: usize) -> Vec<Highlight> {
    tokens
        .iter()
        .filter_map(|token| {
            let from = token.start.max(start);
            let to = token.end.min(end);
            (from < to && to >= start).then(|| Highlight {
                start: from - start,
                end: to - start,
                kind: token.kind,
            })
        })
        .collect()
}

fn wrap_styled_line(
    text: &str,
    code_ranges: &[(usize, usize)],
    tokens: &[Highlight],
    max_width: f32,
    measurer: &dyn TextMeasurer,
    font: f32,
) -> Vec<HoverLine> {
    wrap_byte_spans(text, max_width, measurer, font)
        .into_iter()
        .map(|(start, end)| HoverLine {
            text: text[start..end].to_string(),
            code: false,
            code_ranges: clip_ranges(code_ranges, start, end),
            tokens: clip_tokens(tokens, start, end),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_hover_line(
    ctx: &mut PaintContext,
    theme: &crate::theme::Theme,
    measurer: &dyn TextMeasurer,
    line: &HoverLine,
    x: f32,
    line_top: f32,
    line_h: f32,
    baseline: f32,
    font: f32,
) {
    let p = theme.p;
    if line.text.is_empty() {
        return;
    }
    if !line.code {
        for &(start, end) in &line.code_ranges {
            let from = x + kit::width(measurer, &line.text[..start], font);
            let to = x + kit::width(measurer, &line.text[..end], font);
            kit::fill(
                ctx,
                Rect::from_min_size(
                    Vec2::new(from - 1.0, line_top + 1.0),
                    Size::new((to - from).max(1.0) + 2.0, (line_h - 2.0).max(1.0)),
                ),
                p.bg,
            );
        }
    }
    if line.tokens.is_empty() {
        kit::text(ctx, &line.text, x, baseline, font, p.text);
        return;
    }
    let mut cursor = 0usize;
    for token in &line.tokens {
        let start = token.start.min(line.text.len());
        let end = token.end.min(line.text.len());
        if start < cursor || end <= start {
            continue;
        }
        if start > cursor {
            kit::text(
                ctx,
                &line.text[cursor..start],
                x + kit::width(measurer, &line.text[..cursor], font),
                baseline,
                font,
                p.text,
            );
        }
        let token_text = &line.text[start..end];
        kit::text(
            ctx,
            token_text,
            x + kit::width(measurer, &line.text[..start], font),
            baseline,
            font,
            theme.token_color(token.kind),
        );
        cursor = end;
    }
    if cursor < line.text.len() {
        kit::text(
            ctx,
            &line.text[cursor..],
            x + kit::width(measurer, &line.text[..cursor], font),
            baseline,
            font,
            p.text,
        );
    }
}

fn paint_hover(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let Some(hover) = state.lsp.hover.as_ref() else {
        return;
    };
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let max_width = HOVER_W.min(viewport.width() - 32.0).max(240.0);
    let font = m.font_small;
    let line_h = font + 5.0;
    let mut lines = build_hover_lines(
        &hover.text,
        &state.syntax,
        state.language_of_active(),
        max_width,
        measurer,
        font,
    );
    let has_docs = hover.native.is_some();
    let max_lines = if has_docs { 12 } else { 13 };
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push(HoverLine {
            text: "…".into(),
            code: false,
            code_ranges: Vec::new(),
            tokens: Vec::new(),
        });
    }
    if lines.is_empty() {
        return;
    }
    let docs_h = if has_docs { 26.0 } else { 0.0 };
    let height = (lines.len() as f32 * line_h + 16.0 + docs_h).min(HOVER_MAX_H);
    let caret = state.editor.caret_rect;
    let anchor = state
        .lsp
        .hover_anchor
        .unwrap_or(Vec2::new(caret.left(), caret.top()));
    let mut origin = Vec2::new(anchor.x + 8.0, anchor.y - height - 6.0);
    if origin.x + max_width > viewport.right() - 8.0 {
        origin.x = anchor.x - max_width - 8.0;
    }
    if origin.x < viewport.left() + 8.0 {
        origin.x = viewport.left() + 8.0;
    }
    if origin.y < viewport.top() + 8.0 {
        origin.y = anchor.y + 12.0;
    }
    if origin.y + height > viewport.bottom() - 8.0 {
        origin.y = (viewport.bottom() - height - 8.0).max(viewport.top() + 8.0);
    }
    let panel = Rect::from_min_size(origin, Size::new(max_width, height));
    state.lsp.hover_rect = panel;
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    let mut y = panel.top() + 8.0;
    ctx.save();
    ctx.clip_rect(panel);
    for line in lines.iter().take(13) {
        if line.code {
            kit::fill(
                ctx,
                Rect::from_min_size(
                    Vec2::new(panel.left() + 6.0, y),
                    Size::new(panel.width() - 12.0, line_h),
                ),
                p.bg,
            );
        }
        let x = panel.left() + if line.code { 14.0 } else { 10.0 };
        let baseline = kit::line_baseline(measurer, y, line_h, font);
        draw_hover_line(ctx, &theme, measurer, line, x, y, line_h, baseline, font);
        y += line_h;
    }
    ctx.restore();
    if has_docs {
        let docs = Rect::from_min_size(
            Vec2::new(panel.left() + 10.0, panel.bottom() - 22.0),
            Size::new(panel.width() - 20.0, 18.0),
        );
        state.lsp.hover_doc_hit = docs;
        kit::hline(ctx, panel.left(), panel.right(), docs.top() - 3.0, p.border);
        kit::text(
            ctx,
            "OPEN GODOT DOCS",
            docs.left(),
            kit::baseline(measurer, docs, m.font_small),
            m.font_small,
            p.accent,
        );
    }
}

fn paint_completion(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let Some(completion) = state.lsp.completion.as_ref() else {
        return;
    };
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let font = m.font_small;
    let rows = completion.items.len().min(COMPLETION_MAX_ROWS);
    if rows == 0 {
        return;
    }
    let width = COMPLETION_W.min(viewport.width() - 32.0);
    let height = 10.0 + rows as f32 * COMPLETION_ROW_H + 28.0;
    let caret = state.editor.caret_rect;
    let mut origin = Vec2::new(caret.right() + 4.0, caret.bottom() + 4.0);
    if origin.x + width > viewport.right() - 8.0 {
        origin.x = (viewport.right() - width - 8.0).max(viewport.left() + 8.0);
    }
    if origin.y + height > viewport.bottom() - 8.0 {
        origin.y = (caret.top() - height - 4.0).max(viewport.top() + 8.0);
    }
    let panel = Rect::from_min_size(origin, Size::new(width, height));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);

    let mut first = 0usize;
    let selected = completion.selected.min(completion.items.len() - 1);
    if selected >= COMPLETION_MAX_ROWS {
        first = selected + 1 - COMPLETION_MAX_ROWS;
    }
    let mut y = panel.top() + 8.0;
    for (slot, item) in completion
        .items
        .iter()
        .skip(first)
        .take(COMPLETION_MAX_ROWS)
        .enumerate()
    {
        let index = first + slot;
        let row = Rect::from_min_size(
            Vec2::new(panel.left() + 4.0, y),
            Size::new(panel.width() - 8.0, COMPLETION_ROW_H),
        );
        if index == selected {
            kit::fill(ctx, row, p.hover);
            kit::vline(ctx, row.left(), row.top(), row.bottom(), p.accent);
        }
        let detail_width = if item.detail.is_empty() {
            0.0
        } else {
            (row.width() * 0.42).min(160.0)
        };
        let label = kit::truncate(
            measurer,
            &item.label,
            font,
            row.width() - detail_width - 24.0,
        );
        kit::text(
            ctx,
            &label,
            row.left() + 8.0,
            kit::baseline(measurer, row, font),
            font,
            if index == selected { p.text } else { p.dim },
        );
        if !item.detail.is_empty() {
            let detail = kit::truncate_left(measurer, &item.detail, m.font_small, detail_width);
            kit::text_right(
                ctx,
                measurer,
                &detail,
                row.right() - 8.0,
                kit::baseline(measurer, row, m.font_small),
                m.font_small,
                p.faint,
            );
        }
        y += COMPLETION_ROW_H;
    }
    let footer = Rect::from_min_size(
        Vec2::new(panel.left() + 10.0, panel.bottom() - 26.0),
        Size::new(panel.width() - 20.0, 22.0),
    );
    kit::text(
        ctx,
        "↑↓ move · Enter insert · Esc close",
        footer.left(),
        kit::baseline(measurer, footer, m.font_small),
        m.font_small,
        p.faint,
    );
    kit::text_right(
        ctx,
        measurer,
        &format!("{}/{}", selected + 1, completion.items.len()),
        footer.right(),
        kit::baseline(measurer, footer, m.font_small),
        m.font_small,
        p.faint,
    );
    paint_completion_preview(state, measurer, ctx, panel, viewport);
}

fn paint_completion_preview(
    state: &mut AppState,
    measurer: &dyn TextMeasurer,
    ctx: &mut igui_render::PaintContext,
    completion_panel: Rect,
    viewport: Rect,
) {
    let Some(completion) = state.lsp.completion.as_ref() else {
        return;
    };
    let text = match &completion.preview {
        Some(text) => text.clone(),
        None if completion.preview_pending => "Resolving documentation…".to_string(),
        None => return,
    };
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let font = m.font_small;
    let line_h = font + 5.0;
    let width = COMPLETION_PREVIEW_W.min(viewport.width() - 24.0);
    let mut lines = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.trim_start().starts_with("```") {
            continue;
        }
        let cleaned = trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("### "))
            .unwrap_or(trimmed);
        let cleaned = clean_inline(cleaned);
        if cleaned.is_empty() {
            lines.push(String::new());
        } else {
            wrap_line(&cleaned, width - 20.0, measurer, font, &mut lines);
        }
    }
    if lines.len() > 9 {
        lines.truncate(9);
        lines.push("…".into());
    }
    if lines.is_empty() {
        lines.push(text.lines().next().unwrap_or("").to_string());
    }
    let has_docs = completion.preview.is_some();
    let docs_h = if has_docs { 26.0 } else { 0.0 };
    let height =
        (lines.len() as f32 * line_h + 16.0 + docs_h).clamp(34.0, COMPLETION_PREVIEW_MAX_H);
    let mut x = completion_panel.right() + 8.0;
    if x + width > viewport.right() - 8.0 {
        x = completion_panel.left() - width - 8.0;
    }
    x = x.clamp(
        viewport.left() + 8.0,
        (viewport.right() - width - 8.0).max(0.0),
    );
    let mut y = completion_panel.top();
    if y + height > viewport.bottom() - 8.0 {
        y = (viewport.bottom() - height - 8.0).max(viewport.top() + 8.0);
    }
    let panel = Rect::from_min_size(Vec2::new(x, y), Size::new(width, height));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    let mut row = panel.top() + 8.0;
    for line in &lines {
        kit::text(
            ctx,
            line,
            panel.left() + 10.0,
            row + measurer.ascent(font),
            font,
            if line.is_empty() { p.faint } else { p.text },
        );
        row += line_h;
    }
    if has_docs {
        let docs = Rect::from_min_size(
            Vec2::new(panel.left() + 10.0, panel.bottom() - 22.0),
            Size::new(panel.width() - 20.0, 18.0),
        );
        state.lsp.completion_doc_hit = docs;
        kit::hline(ctx, panel.left(), panel.right(), docs.top() - 3.0, p.border);
        kit::text(
            ctx,
            if completion.native.is_some() {
                "OPEN GODOT DOCS"
            } else {
                "OPEN DOCS"
            },
            docs.left(),
            kit::baseline(measurer, docs, m.font_small),
            m.font_small,
            p.accent,
        );
    }
}

/// Strips the inline Markdown that a hover/completion card does not render.
///
/// Godot formats a declaration as
/// `Defined in [res://子目录/脚本.gd](file:///...percent-encoded...)`.
/// Painting that raw would show the brackets and a long `%E4...` URL instead of
/// the path, so links collapse to their label (which is the readable `res://`
/// path) and inline code/emphasis lose their markers.
fn clean_inline(line: &str) -> String {
    clean_inline_with_code(line).0
}

/// Removes inline Markdown markers while recording backtick spans as byte
/// ranges. Godot turns BBCode `[code]…[/code]` into these spans before sending
/// the hover, so this is where Godot comment keywords become code-like.
fn clean_inline_with_code(line: &str) -> (String, Vec<(usize, usize)>) {
    let mut out = String::with_capacity(line.len());
    let mut code = Vec::new();
    append_inline(line, &mut out, &mut code);
    (out, code)
}

fn append_inline(input: &str, out: &mut String, code: &mut Vec<(usize, usize)>) {
    let mut index = 0;
    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with('[') {
            if let Some(label_end) = rest.find("](") {
                let after_label = label_end + 2;
                if let Some(target_end) = rest[after_label..].find(')') {
                    let label = &rest[1..label_end];
                    let target = &rest[after_label..after_label + target_end];
                    let shown = if label.is_empty() { target } else { label };
                    append_inline(shown, out, code);
                    index += after_label + target_end + 1;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                append_inline(&after[..end], out, code);
                index += 2 + end + 2;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                let start = out.len();
                out.push_str(&after[..end]);
                if out.len() > start {
                    code.push((start, out.len()));
                }
                index += 1 + end + 1;
                continue;
            }
        }
        // Some servers (or future Godot versions) leave inline BBCode in the
        // hover text instead of converting it to Markdown backticks.
        if let Some(after) = rest
            .strip_prefix("[code]")
            .or_else(|| rest.strip_prefix("[kbd]"))
        {
            if let Some(end) = after.find("[/code]").or_else(|| after.find("[/kbd]")) {
                let open_len = if rest.starts_with("[code]") { 6 } else { 5 };
                let close_len = if after[end..].starts_with("[/code]") {
                    7
                } else {
                    6
                };
                let start = out.len();
                out.push_str(&after[..end]);
                if out.len() > start {
                    code.push((start, out.len()));
                }
                index += open_len + end + close_len;
                continue;
            }
        }
        let ch = rest.chars().next().expect("index < input.len()");
        out.push(ch);
        index += ch.len_utf8();
    }
}

fn wrap_line(
    line: &str,
    max_width: f32,
    measurer: &dyn TextMeasurer,
    font: f32,
    out: &mut Vec<String>,
) {
    let mut current = String::new();
    let mut width = 0.0;
    for word in line.split_inclusive(' ') {
        let word_width = kit::width(measurer, word, font);
        if !current.is_empty() && width + word_width > max_width {
            out.push(current.trim_end().to_string());
            current.clear();
            width = 0.0;
        }
        if word_width > max_width && current.is_empty() {
            for ch in word.chars() {
                let advance = measurer.advance(ch, font);
                if width + advance > max_width && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    width = 0.0;
                }
                current.push(ch);
                width += advance;
            }
            continue;
        }
        current.push_str(word);
        width += word_width;
    }
    if !current.is_empty() {
        out.push(current.trim_end().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::TokenKind;

    struct Probe;

    impl TextMeasurer for Probe {
        fn advance(&self, ch: char, font_size: f32) -> f32 {
            if ch == ' ' {
                font_size * 0.25
            } else {
                font_size * 0.55
            }
        }

        fn line_height(&self, font_size: f32) -> f32 {
            font_size * 1.4
        }
    }

    #[test]
    fn hover_code_fences_are_highlighted_with_the_fence_language() {
        let registry = SyntaxRegistry::with_builtins();
        let lines = build_hover_lines(
            "```rs\nfn main() {}\n```",
            &registry,
            None,
            480.0,
            &Probe,
            12.0,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].code);
        assert_eq!(lines[0].text, "fn main() {}");
        assert!(
            lines[0]
                .tokens
                .iter()
                .any(|token| token.kind == TokenKind::Keyword
                    && &lines[0].text[token.start..token.end] == "fn"),
            "missing keyword token: {:?}",
            lines[0].tokens
        );
    }

    #[test]
    fn inline_code_spans_keep_keyword_colors_and_code_ranges() {
        let registry = SyntaxRegistry::with_builtins();
        let gdscript = registry.by_name("gdscript");
        let lines = build_hover_lines(
            "Call `func` and `await` from `_ready`.",
            &registry,
            gdscript,
            480.0,
            &Probe,
            12.0,
        );
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].code_ranges.is_empty());
        assert_eq!(lines[0].text, "Call func and await from _ready.");
        for keyword in ["func", "await"] {
            assert!(
                lines[0]
                    .tokens
                    .iter()
                    .any(|token| token.kind == TokenKind::Keyword
                        && &lines[0].text[token.start..token.end] == keyword),
                "missing {keyword} token: {:?}",
                lines[0].tokens
            );
        }
    }

    #[test]
    fn raw_bbcode_codeblocks_are_accepted_as_a_fallback() {
        let registry = SyntaxRegistry::with_builtins();
        let gdscript = registry.by_name("gdscript");
        let lines = build_hover_lines(
            "[codeblock lang=gdscript]\nfunc _ready():\n\tpass\n[/codeblock]",
            &registry,
            gdscript,
            480.0,
            &Probe,
            12.0,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.code));
        assert!(lines[0]
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Keyword
                && &lines[0].text[token.start..token.end] == "func"));
    }

    #[test]
    fn wrapped_inline_code_keeps_its_background_and_tokens() {
        let registry = SyntaxRegistry::with_builtins();
        let gdscript = registry.by_name("gdscript");
        let lines = build_hover_lines(
            "Call `func` with a very long explanation that must wrap around the narrow card width because it keeps going.",
            &registry,
            gdscript,
            140.0,
            &Probe,
            12.0,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().any(|line| !line.code_ranges.is_empty()));
        assert!(lines.iter().any(|line| {
            line.tokens.iter().any(|token| {
                token.kind == TokenKind::Keyword && &line.text[token.start..token.end] == "func"
            })
        }));
    }

    #[test]
    fn raw_bbcode_code_spans_are_accepted_as_a_fallback() {
        let registry = SyntaxRegistry::with_builtins();
        let gdscript = registry.by_name("gdscript");
        let lines = build_hover_lines(
            "Use [code]func[/code] here.",
            &registry,
            gdscript,
            480.0,
            &Probe,
            12.0,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Use func here.");
        assert_eq!(lines[0].code_ranges.len(), 1);
        assert!(lines[0]
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Keyword
                && &lines[0].text[token.start..token.end] == "func"));
    }

    #[test]
    fn a_plain_declaration_line_uses_the_active_language() {
        let registry = SyntaxRegistry::with_builtins();
        let gdscript = registry.by_name("gdscript");
        let lines = build_hover_lines(
            "func Node._ready() -> void",
            &registry,
            gdscript,
            480.0,
            &Probe,
            12.0,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].code);
        assert!(
            lines[0]
                .tokens
                .iter()
                .any(|token| token.kind == TokenKind::Keyword
                    && &lines[0].text[token.start..token.end] == "func"),
            "missing keyword token: {:?}",
            lines[0].tokens
        );
    }

    #[test]
    fn prose_and_code_fences_keep_their_line_roles() {
        let registry = SyntaxRegistry::with_builtins();
        let lines = build_hover_lines(
            "before\n```rust\nlet value = 1;\n```\nafter",
            &registry,
            None,
            480.0,
            &Probe,
            12.0,
        );
        let roles: Vec<bool> = lines.iter().map(|line| line.code).collect();
        assert_eq!(roles, vec![false, true, false]);
        assert_eq!(lines[0].text, "before");
        assert_eq!(lines[2].text, "after");
    }

    #[test]
    fn markdown_links_show_the_readable_label_not_the_encoded_url() {
        assert_eq!(
            clean_inline(
                "Defined in [res://子目录/脚本.gd](file:///private/tmp/%E4%B8%AD%E6%96%87/%E8%84%9A%E6%9C%AC.gd)"
            ),
            "Defined in res://子目录/脚本.gd"
        );
    }

    #[test]
    fn inline_code_and_bold_markers_are_removed() {
        assert_eq!(clean_inline("`func` **Node** name"), "func Node name");
    }

    #[test]
    fn plain_text_and_chinese_are_untouched() {
        assert_eq!(
            clean_inline("中文路径 res://脚本.gd"),
            "中文路径 res://脚本.gd"
        );
    }
}
