//! Confirmation modal: closing a tab with unsaved changes.
//!
//! Painted as an app overlay *after* the tree (like the menu drop-down) and
//! hit-tested from the same layout function, so the dialog never needs z-order
//! or a second window. Keys are handled by `input.rs`; the buttons are also
//! clickable, and both paths end in `AppState::resolve_modal`.

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit;
use crate::state::{AppState, ModalButton};

const PANEL_W: f32 = 420.0;
const PANEL_H: f32 = 172.0;
const BUTTON_W: f32 = 92.0;
const BUTTON_H: f32 = 28.0;

/// Buttons of the open modal, left to right.
pub const BUTTONS: [ModalButton; 3] =
    [ModalButton::Cancel, ModalButton::Discard, ModalButton::Save];

/// The modal's panel and its button rectangles, centered in `viewport`.
pub fn layout(viewport: Rect) -> (Rect, Vec<(ModalButton, Rect)>) {
    let panel = Rect::from_center_size(viewport.center(), Size::new(PANEL_W, PANEL_H));
    let mut buttons = Vec::with_capacity(BUTTONS.len());
    let mut x = panel.right() - 16.0 - BUTTON_W;
    for button in BUTTONS.iter().rev() {
        let rect = Rect::from_min_size(
            Vec2::new(x, panel.bottom() - 16.0 - BUTTON_H),
            Size::new(BUTTON_W, BUTTON_H),
        );
        buttons.push((*button, rect));
        x -= BUTTON_W + 8.0;
    }
    buttons.reverse();
    (panel, buttons)
}

/// Paints the dim layer, the panel and the buttons; records hit rectangles.
pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let Some(modal) = state.modal else {
        state.modal_buttons.clear();
        return;
    };
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;

    // A plain dim fill: no blur, no shadow — the app's square language.
    ctx.fill_rect(
        state.viewport.logical_rect(),
        Color::new(0.0, 0.0, 0.0, 0.55),
    );

    let viewport = state.viewport.logical_rect();
    let (panel, buttons) = layout(viewport);
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    // Accent rule marks the destructive flow the dialog belongs to.
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);

    let name = match modal {
        crate::state::Modal::CloseDirtyTab { tab } => state
            .ws
            .tabs
            .get(tab)
            .map(|doc| doc.name().to_string())
            .unwrap_or_else(|| "this file".into()),
    };

    let title_baseline = panel.top() + 40.0;
    kit::text(
        ctx,
        "Unsaved changes",
        panel.left() + 18.0,
        title_baseline,
        14.0,
        p.text,
    );
    kit::text(
        ctx,
        &format!("Save changes to {name} before closing?"),
        panel.left() + 18.0,
        title_baseline + 28.0,
        m.font_ui,
        p.dim,
    );

    state.modal_buttons = buttons.clone();
    for (button, rect) in &buttons {
        let primary = *button == ModalButton::Save;
        let danger = *button == ModalButton::Discard;
        let fill = if primary {
            p.accent
        } else if danger {
            p.hover
        } else {
            p.panel
        };
        kit::fill(ctx, *rect, fill);
        ctx.stroke_rect(*rect, 1.0, if primary { p.accent } else { p.border });
        let label = match button {
            ModalButton::Cancel => "Cancel",
            ModalButton::Discard => "Discard",
            ModalButton::Save => "Save",
        };
        let color = if primary {
            p.bg
        } else if danger {
            p.error
        } else {
            p.text
        };
        let width = kit::width(measurer, label, m.font_ui);
        kit::text(
            ctx,
            label,
            rect.center().x - width * 0.5,
            kit::baseline(measurer, *rect, m.font_ui),
            m.font_ui,
            color,
        );
    }
}

/// Which button a click at `pos` hits, if any.
pub fn hit(state: &AppState, pos: Vec2) -> Option<ModalButton> {
    state
        .modal_buttons
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(button, _)| *button)
}
