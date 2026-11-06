//! Pointer ownership for app-level overlays.
//!
//! Menus, prompts, settings, the git diff and the LSP cards are painted after
//! the scene tree but are **not** scene nodes. The engine therefore still routes
//! pointer moves to whatever control sits underneath them. Without an explicit
//! precedence function a prompt floating over the editor would keep the editor's
//! hover machinery alive, schedule a delayed LSP hover card on top of the
//! prompt, and leave stale hover highlights on tabs/sidebar/footer.
//!
//! [`pointer_capture`] mirrors the paint stacking order (top-most overlay wins)
//! and answers one question: which surface owns the pointer at `pos`? `app.rs`
//! uses it for move/press/release/wheel/double-click, the cursor override and
//! the hover-dwell tick, so hover state is updated for exactly one surface.
//!
//! Keyboard precedence stays in [`crate::input`]; this module is pointer-only.

use igui_core::{Rect, Vec2};
use igui_ui::TextMeasurer;

use super::AppState;

/// An app-owned overlay that can own the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    Recovery,
    Modal,
    Tooling,
    SignatureHelp,
    Completion,
    HoverCard,
    RenameSymbol,
    References,
    ProjectSearch,
    GotoLine,
    NewFile,
    QuickOpen,
    Settings,
    TreeMenu,
    TabMenu,
    Menu,
    GitDiff,
}

impl Overlay {
    /// LSP cards are transient tooltips rather than prompts: they suppress the
    /// editor's hover machinery while the pointer is on them, and a wheel
    /// gesture still scrolls the document after dismissing them.
    pub(crate) fn is_lsp(self) -> bool {
        matches!(
            self,
            Overlay::SignatureHelp | Overlay::Completion | Overlay::HoverCard
        )
    }
}

/// The pointer owner at a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PointerCapture {
    /// A prompt/menu/diff owns the whole window (`pos` is only used for its own
    /// hover rows).
    Window(Overlay),
    /// A floating card owns only `rect`; the rest of the window keeps working.
    Panel(Overlay, Rect),
}

impl PointerCapture {
    pub(crate) fn overlay(self) -> Overlay {
        match self {
            PointerCapture::Window(overlay) | PointerCapture::Panel(overlay, _) => overlay,
        }
    }

    /// Whether a wheel gesture belongs to the overlay.
    ///
    /// LSP cards are informational: scrolling dismisses them and scrolls the
    /// editor instead of being swallowed (signature help appears while typing,
    /// so a mouse wheel must not stop working).
    pub(crate) fn blocks_wheel(self) -> bool {
        !self.overlay().is_lsp()
    }
}

/// The overlay that owns the pointer at `pos`, top-most first.
///
/// The order is the reverse of `EditorApp::paint`'s overlay list. `hover_rect`
/// and `signature_help_rect` are recorded by the LSP painter, so they are valid
/// after a frame; a card that has not been painted yet captures nothing.
pub(crate) fn pointer_capture(state: &AppState, pos: Vec2) -> Option<PointerCapture> {
    if state.recovery_prompt.is_some() {
        Some(PointerCapture::Window(Overlay::Recovery))
    } else if state.modal.is_some() {
        Some(PointerCapture::Window(Overlay::Modal))
    } else if state.tooling.prompt.is_some() {
        Some(PointerCapture::Window(Overlay::Tooling))
    } else if state.rename_symbol.open {
        Some(PointerCapture::Window(Overlay::RenameSymbol))
    } else if state.references.open {
        Some(PointerCapture::Window(Overlay::References))
    } else if state.search.open {
        Some(PointerCapture::Window(Overlay::ProjectSearch))
    } else if state.goto_line.open {
        Some(PointerCapture::Window(Overlay::GotoLine))
    } else if state.new_file.open {
        Some(PointerCapture::Window(Overlay::NewFile))
    } else if state.quick_open.open {
        Some(PointerCapture::Window(Overlay::QuickOpen))
    } else if state.settings_open {
        Some(PointerCapture::Window(Overlay::Settings))
    } else if state.tree_menu.is_some() {
        Some(PointerCapture::Window(Overlay::TreeMenu))
    } else if state.tab_menu.is_some() {
        Some(PointerCapture::Window(Overlay::TabMenu))
    } else if state.menu_open.is_some() {
        Some(PointerCapture::Window(Overlay::Menu))
    } else if state.git.diff_visible {
        Some(PointerCapture::Window(Overlay::GitDiff))
    } else if state.lsp.signature_help.is_some() && state.lsp.signature_help_rect.contains(pos) {
        Some(PointerCapture::Panel(
            Overlay::SignatureHelp,
            state.lsp.signature_help_rect,
        ))
    } else if state.lsp.completion.is_some() {
        Some(PointerCapture::Window(Overlay::Completion))
    } else if state.lsp.hover.is_some() && state.lsp.hover_rect.contains(pos) {
        Some(PointerCapture::Panel(
            Overlay::HoverCard,
            state.lsp.hover_rect,
        ))
    } else {
        None
    }
}

/// Clears every pointer-derived hover state. Returns whether anything changed.
///
/// Called when a press leaves the previous hover surface, when the pointer
/// leaves the window, and while an overlay owns the pointer.
pub(crate) fn clear_pointer_hover(state: &mut AppState) -> bool {
    let changed = state.hover_target.is_some()
        || state.hover_leave_at.is_some()
        || state.menubar_hover.is_some()
        || state.menu_hover.is_some()
        || state.tabs.hover.is_some()
        || state.sidebar.hover.is_some()
        || state.outline.hover.is_some()
        || state.problems.hover.is_some()
        || state.git.hover.is_some()
        || state.language_hover
        || state.theme_hover
        || state.lsp_hover
        || state.lsp_toggle_hover
        || state.header_hover.is_some()
        || state.panel_hover.is_some()
        || state.dock_hover.is_some()
        || state.dock_close_hover.is_some()
        || state
            .tab_menu
            .as_ref()
            .is_some_and(|menu| menu.hover.is_some())
        || state
            .tree_menu
            .as_ref()
            .is_some_and(|menu| menu.hover.is_some());

    state.hover_target = None;
    state.hover_leave_at = None;
    state.menubar_hover = None;
    state.menu_hover = None;
    state.tabs.hover = None;
    state.sidebar.hover = None;
    state.outline.hover = None;
    state.problems.hover = None;
    state.git.hover = None;
    state.language_hover = false;
    state.theme_hover = false;
    state.lsp_hover = false;
    state.lsp_toggle_hover = false;
    state.header_hover = None;
    state.panel_hover = None;
    state.dock_hover = None;
    state.dock_close_hover = None;
    if let Some(menu) = state.tab_menu.as_mut() {
        menu.hover = None;
    }
    if let Some(menu) = state.tree_menu.as_mut() {
        menu.hover = None;
    }
    changed
}

/// Applies a pointer move to the owning overlay only.
///
/// Clears every surface below the capture, then updates the overlay's own hover
/// rows. Non-LSP overlays also dismiss the transient LSP UI: a prompt opened
/// while a hover card is visible must not leave the card painted above it.
pub(crate) fn capture_pointer_move(
    state: &mut AppState,
    capture: PointerCapture,
    pos: Vec2,
    measurer: &dyn TextMeasurer,
) -> bool {
    let mut changed = false;
    if !capture.overlay().is_lsp() {
        let lsp_visible = state.lsp.hover.is_some()
            || state.lsp.completion.is_some()
            || state.lsp.signature_help.is_some();
        if lsp_visible {
            state.lsp.dismiss_transient();
            state.clear_signature_help();
            changed = true;
        }
    }
    changed |= clear_pointer_hover(state);

    match capture {
        PointerCapture::Window(Overlay::Menu) => {
            let bar = state
                .menubar_hits
                .iter()
                .find(|(_, rect)| rect.contains(pos))
                .map(|(menu, _)| *menu);
            if state.menubar_hover != bar {
                state.menubar_hover = bar;
                changed = true;
            }
            let hover = crate::ui::menu::hover_at(state, pos, measurer);
            if state.menu_hover != hover {
                state.menu_hover = hover;
                changed = true;
            }
        }
        PointerCapture::Window(Overlay::TabMenu) => {
            let hover = crate::ui::tab_menu::hover_at(state, pos);
            if let Some(menu) = state.tab_menu.as_mut() {
                if menu.hover != hover {
                    menu.hover = hover;
                    changed = true;
                }
            }
        }
        PointerCapture::Window(Overlay::TreeMenu) => {
            let hover = crate::ui::tree_menu::hover_at(state, pos);
            if let Some(menu) = state.tree_menu.as_mut() {
                if menu.hover != hover {
                    menu.hover = hover;
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::theme::Theme;

    fn state() -> AppState {
        let config = std::env::temp_dir().join(format!(
            "igui-editor-overlay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        AppState::new(
            Theme::dark(),
            Workspace::detached(),
            Session::default(),
            config,
        )
    }

    fn at() -> Vec2 {
        Vec2::new(40.0, 40.0)
    }

    #[test]
    fn no_overlay_means_no_capture() {
        assert_eq!(pointer_capture(&state(), at()), None);
    }

    #[test]
    fn modal_and_recovery_outrank_prompts() {
        let mut state = state();
        state.quick_open.open = true;
        state.settings_open = true;
        state.modal = Some(super::super::Modal::Quit { dirty: 1 });
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::Modal))
        );
        state.recovery_prompt = Some(Vec::new());
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::Recovery))
        );
    }

    #[test]
    fn prompts_follow_the_paint_stacking_order() {
        let mut state = state();
        state.settings_open = true;
        state.quick_open.open = true;
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::QuickOpen))
        );
        state.goto_line.open = true;
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::GotoLine))
        );
        state.references.open = true;
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::References))
        );
    }

    #[test]
    fn lsp_cards_own_only_their_rect() {
        let mut state = state();
        state.lsp.hover = Some(crate::model::lsp::HoverUi {
            text: "hover".into(),
            native: None,
        });
        state.lsp.hover_rect =
            Rect::from_min_size(Vec2::new(100.0, 100.0), igui_core::Size::new(200.0, 80.0));
        assert_eq!(pointer_capture(&state, at()), None);
        let inside = Vec2::new(120.0, 120.0);
        assert!(matches!(
            pointer_capture(&state, inside),
            Some(PointerCapture::Panel(Overlay::HoverCard, _))
        ));
    }

    #[test]
    fn completion_and_signature_help_are_active_popups() {
        let mut state = state();
        state.lsp.completion = Some(crate::model::lsp::CompletionUi {
            items: Vec::new(),
            selected: 0,
            preview: None,
            preview_pending: false,
            native: None,
        });
        assert_eq!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Window(Overlay::Completion))
        );
        state.lsp.completion = None;
        state.lsp.signature_help_rect =
            Rect::from_min_size(Vec2::new(10.0, 10.0), igui_core::Size::new(100.0, 40.0));
        state.lsp.signature_help = Some(crate::model::lsp::SignatureHelpUi {
            signatures: Vec::new(),
            active_signature: 0,
            active_parameter: 0,
        });
        assert!(matches!(
            pointer_capture(&state, at()),
            Some(PointerCapture::Panel(Overlay::SignatureHelp, _))
        ));
        // Outside the card the editor keeps its hover behavior.
        assert_eq!(pointer_capture(&state, Vec2::new(400.0, 400.0)), None);
    }

    #[test]
    fn lsp_cards_let_the_wheel_through() {
        let hover = PointerCapture::Panel(Overlay::HoverCard, Rect::ZERO);
        assert!(!hover.blocks_wheel());
        let completion = PointerCapture::Window(Overlay::Completion);
        assert!(!completion.blocks_wheel());
        let quick_open = PointerCapture::Window(Overlay::QuickOpen);
        assert!(quick_open.blocks_wheel());
    }

    #[test]
    fn clear_pointer_hover_resets_every_surface() {
        let mut state = state();
        state.menubar_hover = Some(super::super::Menu::File);
        state.menu_hover = Some(1);
        state.tabs.hover = Some(2);
        state.sidebar.hover = Some(3);
        state.language_hover = true;
        state.theme_hover = true;
        state.lsp_hover = true;
        state.lsp_toggle_hover = true;
        state.header_hover = Some(super::super::HeaderButton::Close);
        state.panel_hover = Some(super::super::Panel::Explorer);
        state.dock_hover = Some(super::super::Panel::Terminal);
        assert!(clear_pointer_hover(&mut state));
        assert!(state.menubar_hover.is_none());
        assert!(state.menu_hover.is_none());
        assert!(state.tabs.hover.is_none());
        assert!(state.sidebar.hover.is_none());
        assert!(!state.language_hover);
        assert!(!state.theme_hover);
        assert!(!state.lsp_hover);
        assert!(!state.lsp_toggle_hover);
        assert!(state.header_hover.is_none());
        assert!(state.panel_hover.is_none());
        assert!(state.dock_hover.is_none());
        assert!(!clear_pointer_hover(&mut state));
    }
}
