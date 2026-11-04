//! Dock model for the left, right and bottom chrome regions.
//!
//! A dock owns an ordered tab list and at most one active tab; it is either
//! open or closed. The **center** region is deliberately not a dock: its tabs
//! are documents with their own undo/dirty/close lifecycle in
//! `model::workspace`, and folding them in here would create a second document
//! lifecycle for no gain.
//!
//! [`Panel`] stays the single table for a tab: dock, icon, label, command, open
//! state and badge. The header buttons, the dock tab strips, the menus and the
//! keymap all read this table, so a new tab only needs one entry here.

use super::*;
use crate::ui::icon::Icon;

/// A docked region around the editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockId {
    Left,
    Right,
    Bottom,
}

impl DockId {
    pub const ALL: [DockId; 3] = [DockId::Left, DockId::Right, DockId::Bottom];

    /// Index into the per-dock UI/hit arrays.
    pub const fn index(self) -> usize {
        match self {
            DockId::Left => 0,
            DockId::Right => 1,
            DockId::Bottom => 2,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            DockId::Left => "Left",
            DockId::Right => "Right",
            DockId::Bottom => "Bottom",
        }
    }
}

/// One tab of a dock (a chrome panel). Header order is
/// [`Panel::ALL`]: the left dock's two tabs, then the right and bottom tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Explorer,
    Git,
    Inherited,
    Signals,
    Outline,
    Terminal,
    Problems,
}

impl Panel {
    /// Header-button order. Panels of one dock stay contiguous so the header
    /// can group them; Git comes first so it is always reachable.
    pub const ALL: [Panel; 7] = [
        Panel::Git,
        Panel::Explorer,
        Panel::Inherited,
        Panel::Signals,
        Panel::Outline,
        Panel::Terminal,
        Panel::Problems,
    ];

    /// Tabs of `dock`, in strip order.
    pub const fn tabs_in(dock: DockId) -> &'static [Panel] {
        match dock {
            DockId::Left => &[
                Panel::Explorer,
                Panel::Git,
                Panel::Inherited,
                Panel::Signals,
            ],
            DockId::Right => &[Panel::Outline],
            // PROBLEMS comes first: diagnostics are the reason to look at the
            // bottom dock while a build/check is running; the shell is one tab away.
            DockId::Bottom => &[Panel::Problems, Panel::Terminal],
        }
    }

    pub const fn dock(self) -> DockId {
        match self {
            Panel::Explorer | Panel::Git | Panel::Inherited | Panel::Signals => DockId::Left,
            Panel::Outline => DockId::Right,
            Panel::Terminal | Panel::Problems => DockId::Bottom,
        }
    }

    /// Stable id used by the session file and by `keymaps.json` commands.
    pub const fn id(self) -> &'static str {
        match self {
            Panel::Explorer => "explorer",
            Panel::Git => "git",
            Panel::Inherited => "inherited",
            Panel::Signals => "signals",
            Panel::Outline => "outline",
            Panel::Terminal => "terminal",
            Panel::Problems => "problems",
        }
    }

    pub fn from_id(id: &str) -> Option<Panel> {
        Panel::ALL.into_iter().find(|panel| panel.id() == id)
    }

    pub const fn action(self) -> Action {
        Action::ShowPanel(self)
    }

    pub(crate) const fn icon(self) -> Icon {
        match self {
            Panel::Explorer => Icon::PanelLeft,
            Panel::Git => Icon::GitBranch,
            Panel::Inherited => Icon::ListTree,
            Panel::Signals => Icon::Radio,
            Panel::Outline => Icon::PanelRight,
            Panel::Terminal => Icon::Terminal,
            Panel::Problems => Icon::CircleAlert,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Panel::Explorer => "EXPLORER",
            Panel::Git => "GIT",
            Panel::Inherited => "INHERITED",
            Panel::Signals => "SIGNALS",
            Panel::Outline => "OUTLINE",
            Panel::Terminal => "TERMINAL",
            Panel::Problems => "PROBLEMS",
        }
    }

    /// Left-dock tabs are stored by id in the session (`sidebar_tab`).
    pub const fn sidebar_tab(self) -> Option<&'static str> {
        match self {
            Panel::Explorer => Some("explorer"),
            Panel::Git => Some("git"),
            Panel::Inherited => Some("inherited"),
            Panel::Signals => Some("signals"),
            Panel::Outline | Panel::Terminal | Panel::Problems => None,
        }
    }

    pub fn is_open(self, state: &AppState) -> bool {
        state.dock_active(self.dock()) == Some(self)
    }

    /// Optional short counter painted after the tab label.
    pub fn badge(self, state: &AppState) -> Option<String> {
        match self {
            // `1E 2W` mirrors the footer chip, so the tab says what is wrong
            // before the panel is even read.
            Panel::Problems => {
                let problems = &state.problems;
                (problems.errors + problems.warnings > 0)
                    .then(|| format!("{}E {}W", problems.errors, problems.warnings))
            }
            Panel::Outline => {
                let path = state.ws.active_doc()?.path()?;
                let count = state.lsp.symbols_for(path).len();
                (count > 0).then(|| count.to_string())
            }
            Panel::Inherited => {
                let count = state.godot_property_count();
                (count > 0).then(|| count.to_string())
            }
            Panel::Signals => {
                let count = state.godot_signal_count();
                (count > 0).then(|| count.to_string())
            }
            Panel::Explorer | Panel::Git | Panel::Terminal => None,
        }
    }
}

impl AppState {
    pub fn dock_open(&self, dock: DockId) -> bool {
        match dock {
            DockId::Left => self.show_sidebar,
            DockId::Right => self.show_outline,
            DockId::Bottom => self.terminal.open,
        }
    }

    /// Active tab of an open dock. `None` when the dock is closed.
    pub fn dock_active(&self, dock: DockId) -> Option<Panel> {
        if !self.dock_open(dock) {
            return None;
        }
        Some(match dock {
            DockId::Left => Panel::from_id(&self.sidebar_tab)
                .filter(|panel| panel.dock() == DockId::Left)
                .unwrap_or(Panel::Explorer),
            DockId::Right => Panel::Outline,
            DockId::Bottom => self.bottom_tab,
        })
    }

    /// Opens `panel`'s dock and makes the panel its active tab.
    pub fn dock_show(&mut self, panel: Panel) {
        match panel.dock() {
            DockId::Left => {
                let changed = !self.show_sidebar;
                self.show_sidebar = true;
                self.session.sidebar = true;
                self.set_sidebar_tab(panel.sidebar_tab().unwrap_or("explorer"));
                if changed {
                    self.needs_rebuild = true;
                }
            }
            DockId::Right => {
                if !self.show_outline {
                    self.show_outline = true;
                    self.session.outline = true;
                    self.needs_rebuild = true;
                }
            }
            DockId::Bottom => {
                // The dock body is chosen at build time, so a tab change needs a
                // rebuild just like an open/close does.
                let changed = self.bottom_tab != panel;
                let was_open = self.terminal.open;
                self.bottom_tab = panel;
                self.session.bottom_tab = panel.id().into();
                match panel {
                    Panel::Terminal => self.open_terminal(),
                    _ => {
                        self.terminal.focused = false;
                        if self.focus == Focus::Terminal {
                            self.focus = Focus::Editor;
                        }
                        self.terminal.open = true;
                        self.session.terminal_open = true;
                        if panel == Panel::Problems {
                            self.refresh_problems();
                        }
                    }
                }
                if changed || !was_open {
                    self.needs_rebuild = true;
                }
            }
        }
        self.config_dirty = true;
        self.dirty = true;
    }

    /// Closes one dock, keeping its tabs and service state. Terminal shells keep
    /// running so reopening the dock does not lose the session.
    pub fn dock_hide(&mut self, dock: DockId) {
        if !self.dock_open(dock) {
            return;
        }
        match dock {
            DockId::Left => {
                self.show_sidebar = false;
                self.session.sidebar = false;
            }
            DockId::Right => {
                self.show_outline = false;
                self.session.outline = false;
                self.outline.hover = None;
            }
            DockId::Bottom => {
                self.hide_bottom_dock();
                return;
            }
        }
        self.config_dirty = true;
        self.needs_rebuild = true;
        self.dirty = true;
    }

    /// Opens a closed dock on its active tab, or closes it when open.
    pub fn dock_toggle(&mut self, dock: DockId) {
        if self.dock_open(dock) {
            self.dock_hide(dock);
            return;
        }
        let active = match dock {
            DockId::Left => Panel::from_id(&self.sidebar_tab)
                .filter(|panel| panel.dock() == DockId::Left)
                .unwrap_or(Panel::Explorer),
            DockId::Right => Panel::Outline,
            DockId::Bottom => self.bottom_tab,
        };
        self.dock_show(active);
    }

    /// Header-button command: opens the dock on `panel`, switches tabs when the
    /// dock is already open on another tab, and closes it when `panel` is the
    /// visible tab.
    pub fn toggle_panel(&mut self, panel: Panel) {
        if panel.is_open(self) {
            self.dock_hide(panel.dock());
        } else {
            self.dock_show(panel);
        }
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
            std::env::temp_dir().join("igui-editor-dock-test.conf"),
        )
    }

    #[test]
    fn header_order_keeps_each_dock_contiguous() {
        let mut groups = Vec::new();
        for panel in Panel::ALL {
            if groups.last() != Some(&panel.dock()) {
                groups.push(panel.dock());
            }
        }
        assert_eq!(
            groups,
            [DockId::Left, DockId::Right, DockId::Bottom],
            "header buttons must stay grouped by dock"
        );
        assert_eq!(Panel::ALL[0], Panel::Git);
    }

    #[test]
    fn every_tab_belongs_to_the_dock_that_lists_it() {
        for dock in DockId::ALL {
            for panel in Panel::tabs_in(dock) {
                assert_eq!(panel.dock(), dock, "{panel:?}");
                assert_eq!(Panel::from_id(panel.id()), Some(*panel));
            }
        }
        let mut seen = Vec::new();
        for dock in DockId::ALL {
            seen.extend_from_slice(Panel::tabs_in(dock));
        }
        seen.sort_by_key(|panel| panel.id());
        let mut all = Panel::ALL.to_vec();
        all.sort_by_key(|panel| panel.id());
        assert_eq!(seen, all, "every panel must be listed by one dock");
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
        assert!(!Panel::Explorer.is_open(&state));

        state.toggle_panel(Panel::Git);
        assert!(!state.show_sidebar);
        assert!(!Panel::Git.is_open(&state));
        assert_eq!(state.session.sidebar_tab, "git");
    }

    #[test]
    fn right_dock_toggles_its_own_flag() {
        let mut state = state();
        state.show_outline = false;
        state.dock_show(Panel::Outline);
        assert!(state.show_outline);
        assert_eq!(state.dock_active(DockId::Right), Some(Panel::Outline));
        state.dock_hide(DockId::Right);
        assert!(!state.show_outline);
        assert_eq!(state.dock_active(DockId::Right), None);
    }

    #[test]
    fn bottom_dock_switches_between_terminal_and_problems() {
        let mut state = state();
        state.bottom_tab = Panel::Problems;
        state.terminal.open = false;
        state.dock_show(Panel::Problems);
        assert!(state.terminal.open);
        assert_eq!(state.dock_active(DockId::Bottom), Some(Panel::Problems));
        assert_eq!(state.session.bottom_tab, "problems");

        state.dock_show(Panel::Terminal);
        assert_eq!(state.dock_active(DockId::Bottom), Some(Panel::Terminal));
        assert_eq!(state.session.bottom_tab, "terminal");

        state.dock_hide(DockId::Bottom);
        assert!(!state.terminal.open);
        assert_eq!(state.dock_active(DockId::Bottom), None);
        // The chosen tab survives closing, so reopening shows it again.
        assert_eq!(state.bottom_tab, Panel::Terminal);
    }

    #[test]
    fn toggling_the_visible_tab_closes_the_dock() {
        let mut state = state();
        state.dock_show(Panel::Explorer);
        state.toggle_panel(Panel::Explorer);
        assert!(!state.show_sidebar);
        assert!(!state.session.sidebar);
    }
}
