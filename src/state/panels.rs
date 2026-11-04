//! Toggleable chrome panels and their grouping.
//!
//! The header paints one icon button per panel on its right side, grouped by
//! the edge the panel lives on. [`Panel`] is the single source of truth for a
//! panel's icon, label, action and open state; the header, keymap and menus all
//! read the same table, so a new panel only needs one entry here.

use super::*;
use crate::ui::icon::Icon;

/// A chrome panel that can be expanded/collapsed from the header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    /// Left dock: the workspace file tree.
    Explorer,
    /// Left dock: the Git changes/history panel.
    Git,
    /// Right dock: the LSP document outline.
    Outline,
    /// Bottom dock: the native terminal.
    Terminal,
}

/// Where a panel is docked. Consecutive panels of one group share a header
/// group; groups are separated by a hairline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelGroup {
    Left,
    Right,
    Bottom,
}

impl PanelGroup {
    pub const ALL: [PanelGroup; 3] = [PanelGroup::Left, PanelGroup::Right, PanelGroup::Bottom];
}

impl Panel {
    /// Header order: left dock, right dock, bottom dock.
    pub const ALL: [Panel; 4] = [Panel::Explorer, Panel::Git, Panel::Outline, Panel::Terminal];

    pub const fn group(self) -> PanelGroup {
        match self {
            Panel::Explorer | Panel::Git => PanelGroup::Left,
            Panel::Outline => PanelGroup::Right,
            Panel::Terminal => PanelGroup::Bottom,
        }
    }

    pub const fn action(self) -> Action {
        Action::ShowPanel(self)
    }

    pub(crate) const fn icon(self) -> Icon {
        match self {
            Panel::Explorer => Icon::PanelLeft,
            Panel::Git => Icon::GitBranch,
            Panel::Outline => Icon::PanelRight,
            Panel::Terminal => Icon::PanelBottom,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Panel::Explorer => "Explorer",
            Panel::Git => "Git",
            Panel::Outline => "Outline",
            Panel::Terminal => "Terminal",
        }
    }

    /// Sidebar tab id for the left-dock panels.
    pub const fn sidebar_tab(self) -> Option<&'static str> {
        match self {
            Panel::Explorer => Some("explorer"),
            Panel::Git => Some("git"),
            Panel::Outline | Panel::Terminal => None,
        }
    }

    pub fn is_open(self, state: &AppState) -> bool {
        match self {
            Panel::Explorer => state.show_sidebar && state.sidebar_tab == "explorer",
            Panel::Git => state.show_sidebar && state.sidebar_tab == "git",
            Panel::Outline => state.show_outline,
            Panel::Terminal => state.terminal.open,
        }
    }
}

impl AppState {
    /// Expands the panel, switching the left dock to its tab; collapses it when
    /// that panel is already visible.
    pub fn toggle_panel(&mut self, panel: Panel) {
        if let Some(tab) = panel.sidebar_tab() {
            if panel.is_open(self) {
                self.show_sidebar = false;
            } else {
                self.show_sidebar = true;
                self.set_sidebar_tab(tab);
                if tab == "git" {
                    self.request_git_refresh();
                }
            }
            self.session.sidebar = self.show_sidebar;
            self.config_dirty = true;
            self.needs_rebuild = true;
            self.dirty = true;
            return;
        }

        match panel {
            Panel::Outline => {
                self.show_outline = !self.show_outline;
                self.session.outline = self.show_outline;
                if !self.show_outline {
                    self.outline.hover = None;
                }
            }
            Panel::Terminal => self.toggle_terminal(),
            Panel::Explorer | Panel::Git => return,
        }
        self.config_dirty = true;
        self.needs_rebuild = true;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;

    fn state() -> AppState {
        let mut ws = Workspace::new(std::env::temp_dir());
        ws.new_tab();
        AppState::new(
            crate::theme::Theme::dark(),
            ws,
            Session::default(),
            std::env::temp_dir().join("igui-editor-panel-test.conf"),
        )
    }

    #[test]
    fn groups_cover_every_panel_in_header_order() {
        let mut seen = Vec::new();
        for group in PanelGroup::ALL {
            for panel in Panel::ALL {
                if panel.group() == group {
                    seen.push(panel);
                }
            }
        }
        assert_eq!(seen, Panel::ALL);
        assert_eq!(
            Panel::Explorer.group(),
            Panel::Git.group(),
            "left-dock panels share a group"
        );
    }

    #[test]
    fn explorer_and_git_share_the_left_dock() {
        let mut state = state();
        state.show_sidebar = false;
        state.toggle_panel(Panel::Explorer);
        assert!(state.show_sidebar);
        assert_eq!(state.sidebar_tab, "explorer");
        assert!(Panel::Explorer.is_open(&state));
        assert!(!Panel::Git.is_open(&state));

        state.toggle_panel(Panel::Git);
        assert!(state.show_sidebar);
        assert_eq!(state.sidebar_tab, "git");
        assert!(Panel::Git.is_open(&state));

        state.toggle_panel(Panel::Git);
        assert!(!state.show_sidebar);
        assert!(!Panel::Git.is_open(&state));
    }

    #[test]
    fn right_and_bottom_panels_toggle_their_own_flag() {
        let mut state = state();
        state.show_outline = false;
        state.toggle_panel(Panel::Outline);
        assert!(state.show_outline);
        assert!(Panel::Outline.is_open(&state));
        state.toggle_panel(Panel::Outline);
        assert!(!state.show_outline);

        // The terminal needs a real PTY, so its spawn path lives in
        // `--selfcheck`; here only the default state is asserted.
        assert!(!Panel::Terminal.is_open(&state));
    }
}
