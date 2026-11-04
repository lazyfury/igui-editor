//! Confirmation modal: closing a dirty tab, or applying a replace-in-files plan.
//!
//! Painted as an app overlay *after* the tree (like the menu drop-down) and
//! hit-tested from the same layout function, so the dialog never needs z-order
//! or a second window. Keys are handled by `input.rs`; the buttons are also
//! clickable, and both paths end in `AppState::resolve_modal`.

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::{AppState, Modal, ModalButton};

const PANEL_W: f32 = 480.0;
const PANEL_H: f32 = 190.0;
const BUTTON_W: f32 = 108.0;
const BUTTON_H: f32 = 32.0;

/// Buttons for the dirty-close modal.
pub const CLOSE_BUTTONS: [ModalButton; 3] =
    [ModalButton::Cancel, ModalButton::Discard, ModalButton::Save];
/// Buttons for the replace-in-files modal.
pub const REPLACE_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Apply];
/// Buttons for closing several tabs (no per-tab Save).
pub const CLOSE_TABS_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Discard];
/// Buttons for closing a project and its tabs.
pub const CLOSE_FOLDER_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Discard];
/// Buttons for switching projects after discarding dirty tabs.
pub const SWITCH_PROJECT_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Discard];
/// Buttons for an external-change prompt.
pub const EXTERNAL_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Reload];
/// Buttons for the macOS quit prompt.
pub const QUIT_BUTTONS: [ModalButton; 2] = [ModalButton::Cancel, ModalButton::Discard];

fn buttons_for(modal: Modal) -> &'static [ModalButton] {
    match modal {
        Modal::CloseDirtyTab { .. } => &CLOSE_BUTTONS,
        Modal::CloseTabs { .. } => &CLOSE_TABS_BUTTONS,
        Modal::CloseFolder { .. } => &CLOSE_FOLDER_BUTTONS,
        Modal::SwitchProject { .. } => &SWITCH_PROJECT_BUTTONS,
        Modal::ExternalChange { .. } => &EXTERNAL_BUTTONS,
        Modal::Quit { .. } => &QUIT_BUTTONS,
        Modal::ReplaceInFiles { .. } => &REPLACE_BUTTONS,
    }
}

/// The modal's panel and its button rectangles, centered in `viewport`.
pub fn layout(viewport: Rect, modal: Option<Modal>) -> (Rect, Vec<(ModalButton, Rect)>) {
    let buttons_for = modal.map(buttons_for).unwrap_or(&CLOSE_BUTTONS);
    let panel = Rect::from_center_size(viewport.center(), Size::new(PANEL_W, PANEL_H));
    let mut buttons = Vec::with_capacity(buttons_for.len());
    let mut x = panel.right() - 16.0 - BUTTON_W;
    for button in buttons_for.iter().rev() {
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
    let (panel, buttons) = layout(viewport, Some(modal));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);

    let (title, body) = match modal {
        Modal::CloseDirtyTab { tab } => {
            let name = state
                .ws
                .tabs
                .get(tab)
                .map(|doc| doc.name().to_string())
                .unwrap_or_else(|| "this file".into());
            (
                "Unsaved changes".to_string(),
                format!("Save changes to {name} before closing?"),
            )
        }
        Modal::CloseTabs { keep, from } => {
            let count = (from..state.ws.tabs.len())
                .filter(|index| keep != Some(*index))
                .count();
            (
                "Close tabs".to_string(),
                format!(
                    "Close {count} tab{}? Unsaved changes will be lost.",
                    if count == 1 { "" } else { "s" }
                ),
            )
        }
        Modal::CloseFolder { dirty } => (
            "Close project".to_string(),
            format!(
                "Close the project and {dirty} unsaved tab{}? Unsaved changes will be lost.",
                if dirty == 1 { "" } else { "s" }
            ),
        ),
        Modal::SwitchProject { dirty } => {
            let name = state
                .pending_project
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "another project".into());
            (
                "Switch project".to_string(),
                format!(
                    "Switch to {name} and close {dirty} unsaved tab{}? Unsaved changes will be lost.",
                    if dirty == 1 { "" } else { "s" }
                ),
            )
        }
        Modal::Quit { dirty } => (
            "Unsaved changes".to_string(),
            format!(
                "Discard changes in {dirty} tab{} and quit?",
                if dirty == 1 { "" } else { "s" }
            ),
        ),
        Modal::ExternalChange { tab } => {
            let name = state
                .ws
                .tabs
                .get(tab)
                .map(|doc| doc.name().to_string())
                .unwrap_or_else(|| "this file".into());
            (
                "File changed on disk".to_string(),
                format!("Reload {name} and lose the unsaved changes in this buffer?"),
            )
        }
        Modal::ReplaceInFiles { files, occurrences } => {
            let replacement = state
                .pending_replace
                .as_ref()
                .map(|job| job.replacement.clone())
                .unwrap_or_default();
            let suffix = if occurrences == 1 { "" } else { "s" };
            let file_suffix = if files == 1 { "" } else { "s" };
            (
                "Replace in files".to_string(),
                format!(
                    "Replace {occurrences} occurrence{suffix} in {files} file{file_suffix} with “{}”?",
                    kit::truncate(measurer, &replacement, m.font_ui, panel.width() - 60.0)
                ),
            )
        }
    };

    let title_baseline = panel.top() + 44.0;
    kit::text(
        ctx,
        &title,
        panel.left() + 18.0,
        title_baseline,
        m.font_ui,
        p.text,
    );
    kit::text(
        ctx,
        &kit::truncate(measurer, &body, m.font_ui, panel.width() - 36.0),
        panel.left() + 18.0,
        title_baseline + 30.0,
        m.font_ui,
        p.dim,
    );

    state.modal_buttons = buttons.clone();
    for (button, rect) in &buttons {
        let primary = matches!(
            button,
            ModalButton::Save | ModalButton::Apply | ModalButton::Reload
        );
        let danger = matches!(button, ModalButton::Discard);
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
            ModalButton::Cancel if matches!(modal, Modal::ExternalChange { .. }) => "Keep",
            ModalButton::Discard if matches!(modal, Modal::Quit { .. }) => "Discard & Quit",
            ModalButton::Discard if matches!(modal, Modal::CloseFolder { .. }) => "Discard & Close",
            ModalButton::Discard if matches!(modal, Modal::SwitchProject { .. }) => {
                "Discard & Switch"
            }
            ModalButton::Cancel => "Cancel",
            ModalButton::Discard => "Discard",
            ModalButton::Save => "Save",
            ModalButton::Apply => "Replace all",
            ModalButton::Reload => "Reload",
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
