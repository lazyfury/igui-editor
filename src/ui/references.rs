//! References overlay: a centered, read-only list of `textDocument/references`
//! results. Geometry lives in [`layout`] and is shared by paint and clicks.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 720.0;
const ROW_H: f32 = 42.0;
const HEADER_H: f32 = 46.0;
const FOOTER_H: f32 = 28.0;
const PAD: f32 = 10.0;
const MAX_ROWS: usize = 10;

/// Panel/row/footer geometry (paint and hit tests use this one function).
pub struct Layout {
    pub panel: Rect,
    pub rows: Vec<Rect>,
    pub footer: Rect,
}

pub fn layout(state: &AppState) -> Layout {
    let viewport = state.viewport.logical_rect();
    let visible = state.references.rows.len().clamp(1, MAX_ROWS);
    let height = HEADER_H + visible as f32 * ROW_H + FOOTER_H;
    let width = WIDTH.min(viewport.width() - 48.0).max(320.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));
    let mut rows = Vec::with_capacity(visible);
    let mut y = panel.top() + HEADER_H;
    for _ in 0..visible {
        rows.push(Rect::from_min_size(
            Vec2::new(panel.left() + PAD, y),
            Size::new(panel.width() - PAD * 2.0, ROW_H),
        ));
        y += ROW_H;
    }
    let footer = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, y),
        Size::new(panel.width() - PAD * 2.0, FOOTER_H),
    );
    Layout {
        panel,
        rows,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.references.open {
        return;
    }
    let layout = layout(state);
    let visible = layout.rows.len();
    let count = state.references.rows.len();
    if count == 0 {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    state.references.rect = layout.panel;
    // Keep the selected row inside the visible window.
    let selected = state.references.selected.min(count - 1);
    if selected < state.references.scroll {
        state.references.scroll = selected;
    } else if selected >= state.references.scroll + visible {
        state.references.scroll = selected + 1 - visible;
    }
    let scroll = state.references.scroll;

    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.35));
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );

    let header = Rect::from_min_size(
        Vec2::new(layout.panel.left() + PAD, layout.panel.top()),
        Size::new(layout.panel.width() - PAD * 2.0, HEADER_H),
    );
    let baseline = kit::baseline(measurer, header, m.font_ui);
    kit::text(
        ctx,
        "REFERENCES",
        header.left() + 4.0,
        baseline,
        m.font_ui,
        p.text,
    );
    let symbol = kit::truncate(
        measurer,
        &state.references.symbol,
        m.font_ui,
        header.width() - 160.0,
    );
    let x = header.left() + 8.0 + kit::width(measurer, "REFERENCES", m.font_ui);
    kit::text(ctx, &symbol, x, baseline, m.font_ui, p.accent);
    kit::text_right(
        ctx,
        measurer,
        &format!("{count} RESULTS"),
        header.right() - 4.0,
        kit::baseline(measurer, header, m.font_small),
        m.font_small,
        p.faint,
    );
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        header.bottom(),
        p.border,
    );

    for (slot, rect) in layout.rows.iter().enumerate() {
        let index = scroll + slot;
        let Some(row) = state.references.rows.get(index) else {
            break;
        };
        if index == selected {
            kit::fill(ctx, *rect, p.hover);
            kit::vline(ctx, rect.left(), rect.top(), rect.bottom(), p.accent);
        }
        let label = kit::truncate(measurer, &row.label, m.font_small, rect.width() - 20.0);
        kit::text(
            ctx,
            &label,
            rect.left() + 10.0,
            rect.top() + 15.0,
            m.font_small,
            if index == selected { p.text } else { p.dim },
        );
        if !row.preview.is_empty() {
            let preview = kit::truncate(measurer, &row.preview, m.font_ui, rect.width() - 20.0);
            kit::text(
                ctx,
                &preview,
                rect.left() + 10.0,
                rect.top() + 33.0,
                m.font_ui,
                if index == selected { p.text } else { p.dim },
            );
        }
    }

    kit::text(
        ctx,
        "↑↓ select · Enter open · Esc close",
        layout.footer.left() + 4.0,
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
    kit::text_right(
        ctx,
        measurer,
        &format!("{}/{}", selected + 1, count),
        layout.footer.right() - 4.0,
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

/// Handles a pointer press. Returns whether it consumed the event.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.references.open {
        return false;
    }
    let layout = layout(state);
    if !layout.panel.contains(pos) {
        state.close_references();
        return true;
    }
    for (slot, rect) in layout.rows.iter().enumerate() {
        if rect.contains(pos) {
            let index = state.references.scroll + slot;
            if index < state.references.rows.len() {
                state.open_reference(index);
            }
            return true;
        }
    }
    true
}

/// Handles a key while the overlay is open. Returns whether it was consumed.
pub fn on_key(state: &mut AppState, key: Key, _modifiers: Modifiers) -> bool {
    if !state.references.open {
        return false;
    }
    match key {
        Key::Escape => state.close_references(),
        Key::Enter => state.open_reference(state.references.selected),
        Key::ArrowUp => state.move_references(-1),
        Key::ArrowDown => state.move_references(1),
        Key::Home => state.move_references(isize::MIN / 2),
        Key::End => state.move_references(isize::MAX / 2),
        _ => {}
    }
    true
}

pub fn scroll(state: &mut AppState, delta_y: f32) {
    if !state.references.open {
        return;
    }
    let visible = state.references.rows.len().clamp(1, MAX_ROWS);
    state.scroll_references(delta_y, visible);
}
