//! Startup recovery prompt: restore or discard crash snapshots.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 540.0;
const ROW_H: f32 = 22.0;
const PAD: f32 = 16.0;
const MAX_ROWS: usize = 6;

pub struct Layout {
    pub panel: Rect,
    pub rows: Vec<Rect>,
    pub discard: Rect,
    pub restore: Rect,
}

pub fn layout(viewport: Rect, count: usize) -> Layout {
    let rows = count.clamp(1, MAX_ROWS);
    let height = PAD + 30.0 + 10.0 + rows as f32 * ROW_H + 16.0 + 28.0 + PAD;
    let width = WIDTH.min(viewport.width() - 48.0).max(300.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));
    let mut rows_rect = Vec::with_capacity(rows);
    let mut y = panel.top() + PAD + 40.0;
    for _ in 0..rows {
        rows_rect.push(Rect::from_min_size(
            Vec2::new(panel.left() + PAD, y),
            Size::new(panel.width() - PAD * 2.0, ROW_H),
        ));
        y += ROW_H;
    }
    let button_y = panel.bottom() - PAD - 28.0;
    let restore = Rect::from_min_size(
        Vec2::new(panel.right() - PAD - 104.0, button_y),
        Size::new(104.0, 28.0),
    );
    let discard = Rect::from_min_size(
        Vec2::new(restore.left() - 8.0 - 104.0, button_y),
        Size::new(104.0, 28.0),
    );
    Layout {
        panel,
        rows: rows_rect,
        discard,
        restore,
    }
}

pub fn paint(state: &AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let Some(entries) = &state.recovery_prompt else {
        return;
    };
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let layout = layout(viewport, entries.len());

    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.45));
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );

    kit::text(
        ctx,
        "Recover unsaved changes?",
        layout.panel.left() + PAD,
        layout.panel.top() + PAD + 20.0,
        14.0,
        p.text,
    );
    kit::text(
        ctx,
        "these buffers were open when the app last exited unexpectedly",
        layout.panel.left() + PAD,
        layout.panel.top() + PAD + 38.0,
        m.font_small,
        p.dim,
    );

    for (index, row) in layout.rows.iter().enumerate() {
        let Some(entry) = entries.get(index) else {
            break;
        };
        let name = kit::truncate(measurer, &entry.name, m.font_ui, row.width() * 0.5);
        kit::text(
            ctx,
            &name,
            row.left() + 4.0,
            kit::baseline(measurer, *row, m.font_ui),
            m.font_ui,
            p.text,
        );
        let path = entry
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "untitled".into());
        let path = kit::truncate_left(measurer, &path, m.font_small, row.width() * 0.45);
        kit::text_right(
            ctx,
            measurer,
            &path,
            row.right() - 4.0,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            p.faint,
        );
    }
    if entries.len() > MAX_ROWS {
        kit::text(
            ctx,
            &format!("… and {} more", entries.len() - MAX_ROWS),
            layout.rows[MAX_ROWS - 1].left() + 4.0,
            layout.rows[MAX_ROWS - 1].bottom() + 14.0,
            m.font_small,
            p.faint,
        );
    }

    paint_button(ctx, measurer, &theme, layout.discard, "Discard All", false);
    paint_button(ctx, measurer, &theme, layout.restore, "Restore All", true);
}

fn paint_button(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    theme: &crate::theme::Theme,
    rect: Rect,
    label: &str,
    primary: bool,
) {
    let p = theme.p;
    let m = theme.m;
    kit::fill(ctx, rect, if primary { p.accent } else { p.panel });
    ctx.stroke_rect(rect, 1.0, if primary { p.accent } else { p.border });
    let width = kit::width(measurer, label, m.font_ui);
    kit::text(
        ctx,
        label,
        rect.center().x - width * 0.5,
        kit::baseline(measurer, rect, m.font_ui),
        m.font_ui,
        if primary { p.bg } else { p.text },
    );
}

/// Pointer press while the prompt is open. Returns `true` when consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if state.recovery_prompt.is_none() {
        return false;
    }
    let layout = layout(state.viewport.logical_rect(), entries_len(state));
    if layout.restore.contains(pos) {
        state.resolve_recovery(true);
        return true;
    }
    if layout.discard.contains(pos) {
        state.resolve_recovery(false);
        return true;
    }
    layout.panel.contains(pos)
}

/// Key press while the prompt is open.
pub fn on_key(state: &mut AppState, key: Key, _modifiers: Modifiers) -> bool {
    if state.recovery_prompt.is_none() {
        return false;
    }
    match key {
        Key::Escape => state.resolve_recovery(false),
        Key::Enter => state.resolve_recovery(true),
        _ => {}
    }
    true
}

fn entries_len(state: &AppState) -> usize {
    state
        .recovery_prompt
        .as_ref()
        .map_or(0, |entries| entries.len())
}
