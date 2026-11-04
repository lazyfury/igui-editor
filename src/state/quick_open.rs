//! Quick Open palette state.

use super::*;

impl AppState {
    /// Opens the quick-open palette (folder required).
    pub fn open_quick_open(&mut self) {
        if !self.ws.folder_open {
            self.error("open a project first (⇧⌘O)");
            return;
        }
        if self.quick_open.open {
            self.close_quick_open();
            return;
        }
        self.settings_open = false;
        self.menu_open = None;
        self.tree_menu = None;
        self.lsp.dismiss_transient();
        if self.search.open {
            self.close_project_search();
        }
        self.quick_open.open = true;
        self.quick_open.index = Some(FileIndex::build(&self.ws.root));
        self.quick_open.selected = 0;
        self.quick_open.scroll = 0;
        self.quick_open.query.select_all();
        self.refresh_quick_open();
        self.dirty = true;
    }

    pub fn close_quick_open(&mut self) {
        self.quick_open.open = false;
        self.quick_open.preedit = None;
        self.quick_open.results.clear();
        self.quick_open.selected = 0;
        self.quick_open.scroll = 0;
        self.dirty = true;
    }

    /// Re-ranks the index for the current query.
    pub fn refresh_quick_open(&mut self) {
        if self.quick_open.index.is_none() && self.ws.folder_open {
            self.quick_open.index = Some(FileIndex::build(&self.ws.root));
        }
        let Some(index) = &self.quick_open.index else {
            self.quick_open.results.clear();
            return;
        };
        let query = self.quick_open.query.text().to_string();
        let recent = &self.session.recent_files;
        self.quick_open.results = index
            .query(&query, |path| recent.iter().any(|entry| entry == path))
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        self.quick_open.selected = self
            .quick_open
            .selected
            .min(self.quick_open.results.len().saturating_sub(1));
        self.dirty = true;
    }

    pub fn quick_open_move(&mut self, delta: isize) {
        if self.quick_open.results.is_empty() {
            return;
        }
        let last = self.quick_open.results.len() - 1;
        let selected = self.quick_open.selected as isize + delta;
        self.quick_open.selected = selected.clamp(0, last as isize) as usize;
        self.dirty = true;
    }

    /// Opens the selected result.
    pub fn quick_open_accept(&mut self) {
        let Some(path) = self
            .quick_open
            .results
            .get(self.quick_open.selected)
            .cloned()
        else {
            return;
        };
        self.close_quick_open();
        self.open_path(path);
    }

    /// Display path for a Quick Open row.
    pub fn quick_open_display(&self, path: &Path) -> String {
        display_path(&self.ws.root, path)
    }
}
