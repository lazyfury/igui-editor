//! LSP overlays: hover card and completion list.
//!
//! Both are painted after the tree (above the editor, below menus/modals) and
//! positioned from the real caret rectangle recorded by the editor painter.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

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
        let cleaned = clean_inline(cleaned);
        if cleaned.is_empty() {
            lines.push(String::new());
        } else {
            wrap_line(&cleaned, max_width - 20.0, measurer, font, &mut lines);
        }
    }
    let has_docs = hover.native.is_some();
    let max_lines = if has_docs { 12 } else { 13 };
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push("…".into());
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
    let mut out = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        let rest = &line[index..];
        if rest.starts_with('[') {
            if let Some(label_end) = rest.find("](") {
                let after_label = label_end + 2;
                if let Some(target_end) = rest[after_label..].find(')') {
                    let label = &rest[1..label_end];
                    let target = &rest[after_label..after_label + target_end];
                    let shown = if label.is_empty() { target } else { label };
                    out.push_str(&clean_inline(shown));
                    index += after_label + target_end + 1;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                out.push_str(&clean_inline(&after[..end]));
                index += 2 + end + 2;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push_str(&after[..end]);
                index += 1 + end + 1;
                continue;
            }
        }
        let ch = rest.chars().next().expect("index < line.len()");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
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
