//! Embedded Godot native-symbol documentation.
//!
//! The data comes from the LSP's custom `textDocument/nativeSymbol` request;
//! `model::bbcode` turns Godot's BBCode into styled, clickable spans.

use std::collections::BTreeMap;

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::model::bbcode::{self, DocLink, DocSpan, DocStyle};
use crate::model::lsp::NativeSymbol;
use crate::state::AppState;

const HEADER_H: f32 = 34.0;
const PAD: f32 = 14.0;
const LINE_PAD: f32 = 2.0;
const MAX_WIDTH: f32 = 760.0;
const MAX_HEIGHT: f32 = 680.0;

#[derive(Clone, Default)]
struct DocLine {
    spans: Vec<DocSpan>,
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    state.docs.links.clear();
    state.docs.close_hit = Rect::ZERO;
    state.docs.back_hit = Rect::ZERO;
    if !state.docs.open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let width = (viewport.width() - 80.0).clamp(420.0, MAX_WIDTH);
    let height = (viewport.height() - 80.0).clamp(300.0, MAX_HEIGHT);
    let rect = Rect::from_min_size(
        Vec2::new(
            (viewport.center().x - width * 0.5).clamp(
                viewport.left() + 16.0,
                (viewport.right() - width - 16.0).max(0.0),
            ),
            (viewport.center().y - height * 0.5).clamp(
                viewport.top() + 16.0,
                (viewport.bottom() - height - 16.0).max(0.0),
            ),
        ),
        Size::new(width, height),
    );
    state.docs.rect = rect;
    state.docs.body = Rect::from_min_size(
        Vec2::new(rect.left(), rect.top() + HEADER_H),
        Size::new(rect.width(), (rect.height() - HEADER_H).max(0.0)),
    );

    // Dim the editor, then draw a square documentation panel.
    ctx.fill_rect(viewport, Color::new(0.0, 0.0, 0.0, 0.48));
    kit::fill(ctx, rect, p.raised);
    ctx.stroke_rect(rect, 1.0, p.border);
    let header = Rect::from_min_size(rect.origin, Size::new(rect.width(), HEADER_H));
    kit::hline(
        ctx,
        header.left(),
        header.right(),
        header.bottom(),
        p.border,
    );
    kit::text(
        ctx,
        "GODOT DOCS",
        header.left() + PAD,
        kit::baseline(measurer, header, m.font_small),
        m.font_small,
        p.accent,
    );
    let target = if state.docs.symbol.is_empty() {
        state.docs.class.clone()
    } else if state.docs.class == state.docs.symbol {
        state.docs.symbol.clone()
    } else {
        format!("{}.{}", state.docs.class, state.docs.symbol)
    };
    let target = kit::truncate(
        measurer,
        &target,
        m.font_small,
        (header.width() - 180.0).max(80.0),
    );
    kit::text_center(
        ctx,
        measurer,
        &target,
        header,
        kit::baseline(measurer, header, m.font_small),
        m.font_small,
        p.dim,
    );
    let close = Rect::from_min_size(
        Vec2::new(header.right() - 66.0, header.top()),
        Size::new(58.0, header.height()),
    );
    state.docs.close_hit = close;
    kit::text_right(
        ctx,
        measurer,
        "ESC  ×",
        close.right() - 8.0,
        kit::baseline(measurer, close, m.font_small),
        m.font_small,
        p.faint,
    );
    if !state.docs.history.is_empty() {
        let back = Rect::from_min_size(
            Vec2::new(header.left() + 104.0, header.top()),
            Size::new(50.0, header.height()),
        );
        state.docs.back_hit = back;
        kit::text(
            ctx,
            "BACK",
            back.left(),
            kit::baseline(measurer, back, m.font_small),
            m.font_small,
            p.dim,
        );
    }

    let lines = build_lines(
        state,
        measurer,
        state.docs.body.width() - PAD * 2.0,
        m.font_small,
    );
    let line_h = (m.font_small + 5.0).max(18.0);
    let list_h = state.docs.body.height().max(1.0);
    state.docs.scroll_max = (lines.len() as f32 * line_h - list_h).max(0.0);
    crate::ui::scroll::clamp(&mut state.docs.scroll, state.docs.scroll_max);
    let first = (state.docs.scroll / line_h).floor().max(0.0) as usize;
    let visible = (list_h / line_h).ceil().max(1.0) as usize + 1;

    ctx.save();
    ctx.clip_rect(state.docs.body);
    for (index, line) in lines.iter().enumerate().skip(first).take(visible) {
        let y = state.docs.body.top() + index as f32 * line_h - state.docs.scroll;
        if y + line_h < state.docs.body.top() {
            continue;
        }
        let mut x = state.docs.body.left() + PAD;
        let baseline = y + (line_h - m.font_small) * 0.5 + measurer.ascent(m.font_small);
        for span in &line.spans {
            if span.text.is_empty() {
                continue;
            }
            let width = kit::width(measurer, &span.text, m.font_small);
            let color = style_color(span.style, &p);
            kit::text(ctx, &span.text, x, baseline, m.font_small, color);
            if let Some(link) = &span.link {
                kit::hline(ctx, x, x + width, y + line_h - LINE_PAD, p.accent);
                state.docs.links.push((
                    link.clone(),
                    Rect::from_min_size(Vec2::new(x, y), Size::new(width.max(1.0), line_h)),
                ));
            }
            x += width;
        }
    }
    ctx.restore();

    let footer = Rect::from_min_size(
        Vec2::new(rect.left() + PAD, rect.bottom() - 20.0),
        Size::new(rect.width() - PAD * 2.0, 18.0),
    );
    kit::hline(ctx, footer.left(), footer.right(), footer.top(), p.border);
    kit::text(
        ctx,
        "click links · ↑↓/wheel scroll · Esc close",
        footer.left(),
        kit::baseline(measurer, footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

fn build_lines(
    state: &AppState,
    measurer: &dyn TextMeasurer,
    max_width: f32,
    font: f32,
) -> Vec<DocLine> {
    let mut spans = Vec::new();
    let docs = &state.docs;
    if docs.loading {
        spans.push(DocSpan {
            text: "Loading native documentation…".into(),
            style: DocStyle::Normal,
            link: None,
        });
    } else if let Some(error) = &docs.error {
        spans.push(DocSpan {
            text: error.clone(),
            style: DocStyle::Normal,
            link: None,
        });
    } else if let Some(data) = &docs.data {
        let title = if data.detail.is_empty() {
            format!("{} {}", data.native_class, data.name)
        } else {
            data.detail.clone()
        };
        spans.push(DocSpan {
            text: title,
            style: DocStyle::Heading,
            link: None,
        });
        spans.push(DocSpan {
            text: "\n\n".into(),
            style: DocStyle::Normal,
            link: None,
        });
        spans.extend(bbcode::parse(&data.documentation, &data.native_class));
        if !data.children.is_empty() {
            spans.push(DocSpan {
                text: "\n\n".into(),
                style: DocStyle::Normal,
                link: None,
            });
            add_children(&mut spans, data);
        }
    }
    wrap_spans(spans, measurer, font, max_width.max(80.0))
}

fn add_children(spans: &mut Vec<DocSpan>, data: &NativeSymbol) {
    let mut groups: BTreeMap<u32, Vec<&NativeSymbol>> = BTreeMap::new();
    for child in &data.children {
        groups.entry(child.kind).or_default().push(child);
    }
    let order = [
        (7, "PROPERTIES"),
        (8, "FIELDS"),
        (14, "CONSTANTS"),
        (9, "CONSTRUCTORS"),
        (6, "METHODS"),
        (12, "FUNCTIONS"),
        (22, "SIGNALS"),
        (23, "ENUMS"),
    ];
    for (kind, title) in order {
        let Some(items) = groups.remove(&kind) else {
            continue;
        };
        spans.push(DocSpan {
            text: format!("{title} ({})", items.len()),
            style: DocStyle::Heading,
            link: None,
        });
        spans.push(DocSpan {
            text: "\n".into(),
            style: DocStyle::Normal,
            link: None,
        });
        for child in items {
            let label = if child.detail.is_empty() {
                child.name.clone()
            } else {
                child.detail.clone()
            };
            spans.push(DocSpan {
                text: label,
                style: DocStyle::Link,
                link: Some(DocLink::Native {
                    class: if child.native_class.is_empty() {
                        data.native_class.clone()
                    } else {
                        child.native_class.clone()
                    },
                    symbol: child.name.clone(),
                }),
            });
            spans.push(DocSpan {
                text: "\n".into(),
                style: DocStyle::Normal,
                link: None,
            });
        }
        spans.push(DocSpan {
            text: "\n".into(),
            style: DocStyle::Normal,
            link: None,
        });
    }
    for (kind, items) in groups {
        spans.push(DocSpan {
            text: format!("MEMBERS ({kind}) ({})", items.len()),
            style: DocStyle::Heading,
            link: None,
        });
        spans.push(DocSpan {
            text: "\n".into(),
            style: DocStyle::Normal,
            link: None,
        });
        for child in items {
            spans.push(DocSpan {
                text: if child.detail.is_empty() {
                    child.name.clone()
                } else {
                    child.detail.clone()
                },
                style: DocStyle::Link,
                link: Some(DocLink::Native {
                    class: if child.native_class.is_empty() {
                        data.native_class.clone()
                    } else {
                        child.native_class.clone()
                    },
                    symbol: child.name.clone(),
                }),
            });
            spans.push(DocSpan {
                text: "\n".into(),
                style: DocStyle::Normal,
                link: None,
            });
        }
    }
}

fn wrap_spans(
    spans: Vec<DocSpan>,
    measurer: &dyn TextMeasurer,
    font: f32,
    max_width: f32,
) -> Vec<DocLine> {
    let mut lines = vec![DocLine::default()];
    let mut current_width = 0.0f32;
    for span in spans {
        let parts: Vec<&str> = span.text.split('\n').collect();
        for (part_index, part) in parts.iter().enumerate() {
            for token in part.split_inclusive(' ') {
                let word = token.trim_end_matches(' ');
                let spaces = token.len().saturating_sub(word.len());
                if !word.is_empty() {
                    let word_width = kit::width(measurer, word, font);
                    if current_width > 0.0 && current_width + word_width > max_width {
                        lines.push(DocLine::default());
                        current_width = 0.0;
                    }
                    if word_width > max_width && current_width == 0.0 {
                        for ch in word.chars() {
                            let ch_width = kit::width(measurer, &ch.to_string(), font);
                            if current_width > 0.0 && current_width + ch_width > max_width {
                                lines.push(DocLine::default());
                                current_width = 0.0;
                            }
                            push_span(
                                lines.last_mut().expect("line"),
                                &mut current_width,
                                ch.to_string(),
                                ch_width,
                                span.style,
                                span.link.clone(),
                            );
                        }
                    } else {
                        push_span(
                            lines.last_mut().expect("line"),
                            &mut current_width,
                            word.to_string(),
                            word_width,
                            span.style,
                            span.link.clone(),
                        );
                    }
                }
                if spaces > 0 && current_width > 0.0 {
                    let spaces_width = kit::width(measurer, " ", font) * spaces as f32;
                    if current_width + spaces_width <= max_width {
                        push_span(
                            lines.last_mut().expect("line"),
                            &mut current_width,
                            " ".repeat(spaces),
                            spaces_width,
                            span.style,
                            span.link.clone(),
                        );
                    }
                }
            }
            if part_index + 1 < parts.len() {
                lines.push(DocLine::default());
                current_width = 0.0;
            }
        }
    }
    while lines.len() > 1 && lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn push_span(
    line: &mut DocLine,
    current_width: &mut f32,
    text: String,
    width: f32,
    style: DocStyle,
    link: Option<DocLink>,
) {
    if text.is_empty() {
        return;
    }
    *current_width += width;
    line.spans.push(DocSpan { text, style, link });
}

fn style_color(style: DocStyle, p: &crate::theme::Palette) -> Color {
    match style {
        DocStyle::Heading | DocStyle::Bold => p.accent,
        DocStyle::Code => p.syntax.string,
        DocStyle::Link => p.accent,
        DocStyle::Italic => p.dim,
        DocStyle::Normal => p.text,
    }
}

pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.docs.open {
        return false;
    }
    if state.docs.close_hit.contains(pos) {
        state.close_native_docs();
        return true;
    }
    if state.docs.back_hit.contains(pos) {
        state.native_docs_back();
        return true;
    }
    let link = state
        .docs
        .links
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(link, _)| link.clone());
    if let Some(link) = link {
        match link {
            DocLink::Native { class, symbol } => state.open_native_docs_at(class, symbol, true),
            DocLink::Url(url) => state.open_external_url(&url),
        }
        return true;
    }
    if !state.docs.rect.contains(pos) {
        state.close_native_docs();
    }
    true
}

pub fn wheel(state: &mut AppState, delta: f32) {
    if !state.docs.open {
        return;
    }
    state.docs.scroll = (state.docs.scroll + delta * 30.0).clamp(0.0, state.docs.scroll_max);
    state.dirty = true;
}
