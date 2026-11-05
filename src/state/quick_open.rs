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
        // Reopening the palette must not re-walk the tree. The index is only
        // rebuilt when it is missing or belongs to a previous project root.
        if !self
            .quick_open
            .index
            .as_ref()
            .is_some_and(|index| index.root == self.ws.root)
        {
            self.quick_open.index = Some(FileIndex::build(&self.ws.root));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::theme::Theme;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-quick-open-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn state(root: &Path) -> AppState {
        let mut ws = Workspace::new(root);
        ws.new_tab();
        AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            root.join("igui-editor-quick-open-test.conf"),
        )
    }

    fn index_for(root: PathBuf, path: PathBuf) -> FileIndex {
        let display = display_path(&root, &path);
        FileIndex {
            root,
            files: vec![path],
            displays: vec![display.clone()],
            lower: vec![display.to_lowercase()],
        }
    }

    #[test]
    fn opening_quick_open_reuses_the_index_for_the_same_root() {
        let dir = temp("reuse");
        std::fs::write(dir.join("sentinel.md"), "sentinel").unwrap();
        let mut state = state(&dir);
        let sentinel = state.ws.root.join("sentinel.md");
        state.quick_open.index = Some(index_for(state.ws.root.clone(), sentinel.clone()));

        state.open_quick_open();

        let index = state.quick_open.index.as_ref().expect("index kept");
        assert_eq!(index.files, vec![sentinel]);
        assert_eq!(state.quick_open.results.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_quick_open_builds_an_index_when_missing() {
        let dir = temp("missing");
        std::fs::write(dir.join("alpha.md"), "alpha").unwrap();
        let mut state = state(&dir);
        state.quick_open.index = None;

        state.open_quick_open();

        let index = state.quick_open.index.as_ref().expect("index built");
        assert_eq!(index.root, state.ws.root);
        assert!(index.files.iter().any(|path| path.ends_with("alpha.md")));
        assert_eq!(state.quick_open.results.len(), index.files.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_quick_open_rebuilds_an_index_for_another_root() {
        let dir = temp("other-root");
        std::fs::write(dir.join("alpha.md"), "alpha").unwrap();
        let mut state = state(&dir);
        let stale_root = dir.join("stale");
        let stale = stale_root.join("sentinel.md");
        state.quick_open.index = Some(index_for(stale_root, stale.clone()));

        state.open_quick_open();

        let index = state.quick_open.index.as_ref().expect("index rebuilt");
        assert_eq!(index.root, state.ws.root);
        assert!(!index.files.contains(&stale));
        assert!(index.files.iter().any(|path| path.ends_with("alpha.md")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
