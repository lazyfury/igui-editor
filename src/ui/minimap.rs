//! Minimap strip: the structural thumbnail, its viewport slider and pointer
//! mapping.
//!
//! The strip sits between the text area and the vertical scrollbar and is
//! painted by the editor control, so its draw commands live in the same cached
//! `DrawList` (idle frames stay empty). Content comes from
//! [`crate::model::minimap`]; this module only turns line marks into rectangles
//! and maps pointer positions back to lines.
//!
//! Layout is recorded during paint (`MinimapUi`) and reused by the pointer
//! callbacks, matching the scrollbar/tab pattern: one layout function, no
//! parallel geometry.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;

use crate::kit::RectExt;
use crate::model::doc::Document;
use crate::model::minimap;
use crate::model::wrap::WrapLayout;
use crate::state::{AppState, Drag, Focus, MinimapUi};
use crate::theme::Theme;

/// Hide the map rather than shrink the text area below this width.
pub const MIN_TEXT_W: f32 = 360.0;
/// Ink alpha for the lightest bucket (a bucket with one inked line).
const INK_MIN: f32 = 0.10;
/// Ink alpha for a fully inked bucket.
const INK_MAX: f32 = 0.32;
/// Viewport slider fill and border alpha.
const SLIDER_FILL: f32 = 0.05;
const SLIDER_BORDER: f32 = 0.45;

/// Width the editor must reserve for the map, or 0 when it is disabled, above
/// the file caps, or would leave too little room for text.
pub fn reserved_width(state: &AppState, editor_rect: Rect, gutter_w: f32) -> f32 {
    let Some(doc) = state.ws.active_doc() else {
        return 0.0;
    };
    if !state.session.minimap || !minimap::is_active(doc) {
        return 0.0;
    }
    let m = state.theme.m;
    let remaining = editor_rect.width() - gutter_w - m.scrollbar - m.minimap;
    if remaining < MIN_TEXT_W {
        0.0
    } else {
        m.minimap
    }
}

/// Paints the strip and records its rectangles.
///
/// `first_row`/`last_row` are the visible rows the editor just computed (visual
/// rows under soft wrap, logical lines otherwise).
#[allow(clippy::too_many_arguments)]
pub fn paint(
    ui: &mut MinimapUi,
    doc: &Document,
    wrap: &WrapLayout,
    wrap_on: bool,
    map: Rect,
    first_row: usize,
    last_row: usize,
    max_scroll_y: f32,
    tab: usize,
    theme: &Theme,
    ctx: &mut PaintContext,
) {
    let line_count = doc.line_count();
    ui.rect = map;
    ui.slider = Rect::ZERO;
    ui.cache.sync(doc, tab);
    if map.width() <= 1.0 || map.height() <= 1.0 || line_count == 0 {
        return;
    }

    // Opaque background: the current-line tint and selection must not bleed
    // through the strip. One rule separates it from the text.
    ctx.fill_rect(map, theme.p.bg);
    ctx.fill_rect(
        Rect::from_min_size(map.origin, Size::new(1.0, map.height())),
        theme.p.border,
    );
    let inner = Rect::from_min_size(
        Vec2::new(map.left() + 1.0, map.top()),
        Size::new((map.width() - 1.0).max(1.0), map.height()),
    );

    let content_h = minimap::content_height(line_count, inner.height());
    let bucket_count = minimap::bucket_count(line_count, inner.height());
    let scale = ui.cache.max_columns().max(1) as f32;
    let row_h = content_h / bucket_count as f32;
    let ink = theme.p.text;
    {
        let rows = ui.cache.buckets(bucket_count);
        for (index, bucket) in rows.iter().enumerate() {
            if bucket.x1 <= bucket.x0 {
                continue;
            }
            let x0 =
                inner.left() + (bucket.x0.min(minimap::MAX_COLUMNS) as f32 / scale) * inner.width();
            let x1 =
                inner.left() + (bucket.x1.min(minimap::MAX_COLUMNS) as f32 / scale) * inner.width();
            let y = inner.top() + index as f32 * row_h;
            let alpha = INK_MIN + (INK_MAX - INK_MIN) * bucket.density();
            ctx.fill_rect(
                Rect::from_min_size(Vec2::new(x0, y), Size::new((x1 - x0).max(1.0), row_h)),
                ink.alpha_mul(alpha),
            );
        }
    }

    let (first_line, last_line) = visible_lines(wrap, wrap_on, first_row, last_row, line_count);
    ui.first_line = first_line;

    if max_scroll_y > 0.5 {
        let (top, height) = minimap::slider(line_count, first_line, last_line, content_h);
        let top = top.min((content_h - 1.0).max(0.0));
        let height = height.min(content_h - top).max(1.0);
        let slider = Rect::from_min_size(
            Vec2::new(inner.left(), inner.top() + top),
            Size::new(inner.width(), height),
        );
        ctx.fill_rect(slider, theme.p.text.alpha_mul(SLIDER_FILL));
        ctx.stroke_rect(slider, 1.0, theme.p.dim.alpha_mul(SLIDER_BORDER));
        ui.slider = slider;
    }
}

/// Inclusive `(first, last)` visible line span for the visible row range.
fn visible_lines(
    wrap: &WrapLayout,
    wrap_on: bool,
    first_row: usize,
    last_row: usize,
    line_count: usize,
) -> (usize, usize) {
    if line_count == 0 {
        return (0, 0);
    }
    let last = line_count - 1;
    if wrap_on && !wrap.is_empty() {
        let rows = wrap.len();
        let first_row = first_row.min(rows - 1);
        let last_row = last_row.min(rows - 1);
        let first = wrap.line_of_row(first_row).unwrap_or(0).min(last);
        let end = wrap
            .line_of_row(last_row)
            .unwrap_or(first)
            .min(last)
            .max(first);
        (first, end)
    } else {
        (
            first_row.min(last),
            last_row.min(last).max(first_row.min(last)),
        )
    }
}

/// Document line under a viewport `y` inside the map.
pub fn line_for_y(ui: &MinimapUi, y: f32, line_count: usize) -> usize {
    let content_h = minimap::content_height(line_count, ui.rect.height());
    minimap::line_at(y - ui.rect.top(), line_count, content_h)
}

/// Whether the map owns the pointer position (recorded rect; zero when hidden).
pub fn contains(state: &AppState, pos: Vec2) -> bool {
    let rect = state.editor.minimap.rect;
    !rect.is_empty() && rect.contains(pos)
}

/// Pointer-down on the strip: jump, or start dragging the slider, and never
/// move the caret. Returns whether the press was consumed.
pub fn handle_press(state: &mut AppState, pos: Vec2) -> bool {
    if !contains(state, pos) {
        return false;
    }
    let (slider, first_line) = {
        let ui = &state.editor.minimap;
        (ui.slider, ui.first_line)
    };
    let line_count = state.ws.active_doc().map_or(0, Document::line_count);
    if line_count == 0 {
        return false;
    }
    let line = line_for_y(&state.editor.minimap, pos.y, line_count);
    let in_slider = !slider.is_empty() && slider.contains(pos);
    let grab = if in_slider {
        (line as f32 - first_line as f32).max(0.0)
    } else {
        0.0
    };
    state.editor.drag = Drag::Minimap { grab };
    if !in_slider {
        scroll_to_line(state, line);
    }
    state.focus = Focus::Editor;
    state.editor.reveal = false;
    state.dirty = true;
    true
}

/// Pointer move while dragging the strip.
pub fn handle_drag(state: &mut AppState, pos: Vec2) {
    let Drag::Minimap { grab } = state.editor.drag else {
        return;
    };
    let line_count = state.ws.active_doc().map_or(0, Document::line_count);
    if line_count == 0 {
        return;
    }
    let line = line_for_y(&state.editor.minimap, pos.y, line_count);
    let target = (line as f32 - grab).floor().max(0.0) as usize;
    scroll_to_line(state, target.min(line_count - 1));
    state.dirty = true;
}

/// Scrolls so `line` is the first visible line (clamped by the next paint).
fn scroll_to_line(state: &mut AppState, line: usize) {
    let line_h = state.editor_metrics().line_h.max(1.0);
    let wrap_on = state.session.soft_wrap
        && !state.editor.wrap.is_empty()
        && state
            .ws
            .active_doc()
            .is_some_and(|doc| state.editor.wrap.matches(doc.id(), doc.rev()));
    let row = if wrap_on {
        state.editor.wrap.first_row_of_line(line)
    } else {
        line
    };
    if let Some(doc) = state.ws.active_doc_mut() {
        doc.scroll_y = row as f32 * line_h;
    }
}
