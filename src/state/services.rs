//! Sidebar contributions exposed by the service registry.

use super::*;

impl AppState {
    pub fn sidebar_tabs(&self) -> &[SidebarContribution] {
        &self.services.sidebar_tabs
    }

    pub fn set_sidebar_tab(&mut self, id: &'static str) {
        if !self.services.sidebar_tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        if self.sidebar_tab != id {
            self.sidebar_tab = id.into();
            self.session.sidebar_tab = id.into();
            self.config_dirty = true;
            if id == "git" {
                self.request_git_refresh();
            }
            self.dirty = true;
        }
    }
}
