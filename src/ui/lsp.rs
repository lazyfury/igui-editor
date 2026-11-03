//! LSP overlays: hover card and completion list.
//!
//! Both are painted after the tree (above the editor, below menus/modals) and
//! positioned from the real caret rectangle recorded by the editor painter.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const HOVER_W: f32 = 460.0;
const HOVER_MAX_H: f32 = 260.0;
const COMPLETION_W: f32 = 380.0;
const COMPLETION_ROW_H: f32 = 22.0;
const COMPLETION_MAX_ROWS: usize = 10;

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if state.lsp.completion.is_some() {
        paint_completion(state, measurer, ctx);
    } else if state.lsp.hover.is_some() {
        paint_hover(state, measurer, ctx);
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
    let mut lines = Vec::new();
    for raw in hover.text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.trim_start().starts_with("```") {
            continue;
        }
        let cleaned = trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("### "))
            .unwrap_or(trimmed);
        if cleaned.is_empty() {
            lines.push(String::new());
        } else {
            wrap_line(cleaned, max_width - 20.0, measurer, font, &mut lines);
        }
    }
    if lines.len() > 13 {
        lines.truncate(13);
        lines.push("…".into());
    }
    if lines.is_empty() {
        return;
    }
    let height = (lines.len() as f32 * line_h + 16.0).min(HOVER_MAX_H);
    let caret = state.editor.caret_rect;
    let mut origin = Vec2::new(caret.right() + 8.0, caret.top() - height - 6.0);
    if origin.x + max_width > viewport.right() - 8.0 {
        origin.x = caret.left() - max_width - 8.0;
    }
    if origin.x < viewport.left() + 8.0 {
        origin.x = viewport.left() + 8.0;
    }
    if origin.y < viewport.top() + 8.0 {
        origin.y = caret.bottom() + 6.0;
    }
    if origin.y + height > viewport.bottom() - 8.0 {
        origin.y = (viewport.bottom() - height - 8.0).max(viewport.top() + 8.0);
    }
    let panel = Rect::from_min_size(origin, Size::new(max_width, height));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    let mut y = panel.top() + 8.0;
    for line in lines.iter().take(13) {
        kit::text(
            ctx,
            line,
            panel.left() + 10.0,
            y + measurer.ascent(font),
            font,
            if line.is_empty() { p.faint } else { p.text },
        );
        y += line_h;
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
    let height = 8.0 + rows as f32 * COMPLETION_ROW_H + 24.0;
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
    let mut y = panel.top() + 6.0;
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
        Vec2::new(panel.left() + 10.0, panel.bottom() - 22.0),
        Size::new(panel.width() - 20.0, 20.0),
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
