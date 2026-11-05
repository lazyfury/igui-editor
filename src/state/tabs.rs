//! Tab lifecycle, close guards and tab/tree context menus.

use super::*;

impl AppState {
    // -- document lifecycle ---------------------------------------------

    /// Focuses tab `index` (no-op when out of range or already active) and
    /// refreshes the view state that depends on the active document.
    pub fn activate_tab(&mut self, index: usize) {
        if index >= self.ws.tabs.len() || index == self.ws.active {
            return;
        }
        self.ws.select(index);
        self.tabs.reveal_active = true;
        self.editor.preedit = None;
        self.editor.drag = Drag::None;
        self.focus = Focus::Editor;
        self.find.dirty = true;
        self.sync_layout();
        self.sync_session_tabs();
        self.dirty = true;
    }

    /// Closes every tab except `keep` (context menu). Dirty targets require a
    /// single discard confirmation.
    pub fn request_close_others(&mut self, keep: usize) {
        if keep >= self.ws.tabs.len() || self.ws.tabs.len() <= 1 {
            return;
        }
        self.request_close_many(Some(keep), 0);
    }

    /// Closes every tab to the right of `from` (context menu).
    pub fn request_close_right(&mut self, from: usize) {
        if from + 1 >= self.ws.tabs.len() {
            return;
        }
        self.request_close_many(None, from + 1);
    }

    pub(crate) fn request_close_many(&mut self, keep: Option<usize>, from: usize) {
        let targets: Vec<usize> = (from..self.ws.tabs.len())
            .filter(|index| keep != Some(*index))
            .collect();
        if targets.is_empty() {
            return;
        }
        let dirty = targets.iter().any(|index| self.ws.tabs[*index].is_dirty());
        self.tab_menu = None;
        self.menu_open = None;
        if dirty {
            self.modal = Some(Modal::CloseTabs { keep, from });
        } else {
            self.close_tabs_now(keep, from);
        }
        self.dirty = true;
    }

    pub(crate) fn close_tabs_now(&mut self, keep: Option<usize>, from: usize) {
        let mut targets: Vec<usize> = (from..self.ws.tabs.len())
            .filter(|index| keep != Some(*index))
            .collect();
        let count = targets.len();
        targets.sort_unstable_by(|a, b| b.cmp(a));
        for index in targets {
            self.close_tab_now(index);
        }
        if count > 0 {
            self.status(format!(
                "closed {count} tab{}",
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    /// Runs a file-tree context-menu command.
    pub fn run_tree_menu(&mut self, path: PathBuf, is_dir: bool, action: TreeMenuAction) {
        self.tree_menu = None;
        match action {
            TreeMenuAction::Open => {
                if is_dir {
                    self.ws.tree.toggle(&path);
                } else {
                    self.open_path(path);
                }
            }
            TreeMenuAction::Rename => self.open_rename(path),
            TreeMenuAction::NewFile => {
                let dir = if is_dir {
                    path
                } else {
                    path.parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| self.ws.root.clone())
                };
                self.open_new_file_in(dir);
            }
            TreeMenuAction::NewFolder => {
                let dir = if is_dir {
                    path
                } else {
                    path.parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| self.ws.root.clone())
                };
                self.open_new_folder_in(dir);
            }
            TreeMenuAction::Delete => self.request_delete(path),
            TreeMenuAction::CopyPath => {
                let text = path.display().to_string();
                self.set_clipboard(&text);
                self.status(format!("copied {text}"));
            }
            TreeMenuAction::Reveal => self.reveal_path(&path),
        }
        self.dirty = true;
    }

    pub(crate) fn reveal_path(&mut self, path: &Path) {
        let result = {
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg("-R")
                    .arg(path)
                    .spawn()
            }
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open")
                    .arg(path.parent().unwrap_or(path))
                    .spawn()
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("explorer")
                    .arg(format!("/select,{}", path.display()))
                    .spawn()
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "reveal is unsupported on this platform",
                ))
            }
        };
        match result {
            Ok(_) => self.status(format!("revealed {}", path.display())),
            Err(error) => self.error(format!("reveal failed: {error}")),
        }
    }

    /// Runs a tab context-menu command.
    pub fn run_tab_menu(&mut self, action: TabMenuAction, tab: usize) {
        self.tab_menu = None;
        match action {
            TabMenuAction::Close => self.request_close_tab(tab),
            TabMenuAction::CloseOthers => self.request_close_others(tab),
            TabMenuAction::CloseRight => self.request_close_right(tab),
            TabMenuAction::CopyPath => {
                let Some(path) = self
                    .ws
                    .tabs
                    .get(tab)
                    .and_then(|doc| doc.path().map(Path::to_path_buf))
                else {
                    self.error("untitled buffer has no path");
                    return;
                };
                let text = path.display().to_string();
                self.set_clipboard(&text);
                self.status(format!("copied {text}"));
            }
            TabMenuAction::RevealInSidebar => self.reveal_tab(tab),
        }
        self.dirty = true;
    }

    pub(crate) fn reveal_tab(&mut self, tab: usize) {
        let Some(path) = self
            .ws
            .tabs
            .get(tab)
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        else {
            self.error("untitled buffer has no path");
            return;
        };
        if !self.ws.folder_open || !path.starts_with(&self.ws.root) {
            self.error("file is outside the open project");
            return;
        }
        let Some(parent) = path.parent() else {
            return;
        };
        self.ws.tree.expand_to(parent);
        let row_h = self.theme.m.row_h;
        let rows = self.ws.tree.rows();
        if let Some(index) = rows.iter().position(|row| row.path == path) {
            let view_h = self.sidebar.rect.size.height.max(0.0);
            self.sidebar.scroll = (index as f32 * row_h - view_h * 0.5).max(0.0);
        }
        self.sidebar.selected = Some(path.clone());
        if !self.show_sidebar {
            self.show_sidebar = true;
            self.session.sidebar = true;
            self.config_dirty = true;
            self.needs_rebuild = true;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.status(format!("revealed {name}"));
    }

    /// Opens the quit confirmation for `dirty` unsaved buffers.
    pub fn request_quit(&mut self, dirty: usize) {
        self.tab_menu = None;
        self.tab_drag = None;
        self.menu_open = None;
        self.modal = Some(Modal::Quit { dirty });
        self.dirty = true;
    }

    /// Closes the active tab, asking for confirmation when it is dirty.
    pub fn request_close_active(&mut self) {
        self.request_close_tab(self.ws.active);
    }

    /// Closes tab `index`, asking for confirmation when it is dirty.
    pub fn request_close_tab(&mut self, index: usize) {
        let Some(doc) = self.ws.tabs.get(index) else {
            return;
        };
        if doc.is_dirty() {
            self.ws.select(index);
            self.tabs.reveal_active = true;
            self.modal = Some(Modal::CloseDirtyTab { tab: index });
            self.menu_open = None;
            self.dirty = true;
        } else {
            self.close_tab_now(index);
        }
    }

    /// Closes `index` (bypassing the dirty check) and refreshes the session.
    pub fn close_tab_now(&mut self, index: usize) {
        let closing = self
            .ws
            .tabs
            .get(index)
            .map(|doc| (doc.id(), doc.path().map(Path::to_path_buf)));
        self.ws.force_close(index);
        if let Some((id, path)) = closing {
            self.forget_document(id, path.as_deref());
        }
        self.after_tabs_changed();
    }

    /// Re-opens the workspace after a tab change.
    pub fn after_tabs_changed(&mut self) {
        self.ws.validate_active();
        let open: HashSet<PathBuf> = self
            .ws
            .tabs
            .iter()
            .filter_map(|doc| doc.path().map(Path::to_path_buf))
            .collect();
        self.images.prune(&open);
        self.tabs.reveal_active = true;
        self.request_git_refresh();
        self.sync_layout();
        self.sync_session_tabs();
        self.find.dirty = true;
        self.focus = Focus::Editor;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;

    fn state(root: &Path) -> AppState {
        let mut ws = Workspace::new(root);
        ws.new_tab();
        AppState::new(
            crate::theme::Theme::dark(),
            ws,
            Session::default(),
            root.join("igui-editor-tabs-test.conf"),
        )
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-tabs-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn index_of(state: &AppState, path: &Path) -> usize {
        state
            .ws
            .tabs
            .iter()
            .position(|doc| doc.path() == Some(path))
            .expect("the file tab is open")
    }

    #[test]
    fn closing_tabs_prunes_every_per_document_map() {
        let dir = temp("prune");
        let path = dir.join("notes.md");
        std::fs::write(&path, "# Notes\n").unwrap();
        let mut state = state(&dir);
        let untitled_id = state.ws.tabs[0].id();

        state.open_path(&path);
        let file_id = state.ws.active_doc().unwrap().id();
        state.set_view(ViewMode::Edit);
        state.set_language(Some("markdown".into()));
        assert_eq!(
            state.language_by_path.get(&path).map(String::as_str),
            Some("markdown"),
            "a path override is persisted for the file"
        );

        state.ws.select(0);
        state.set_view(ViewMode::Edit);
        state.set_language(Some("markdown".into()));
        state.tick_reliability();

        assert!(state.doc_views.contains_key(&file_id));
        assert!(state.doc_views.contains_key(&untitled_id));
        assert!(state.doc_revs.contains_key(&file_id));
        assert!(state.language_by_id.contains_key(&untitled_id));
        assert!(state.external.contains_key(&path));

        state.close_tab_now(1);
        state.close_tab_now(0);

        assert!(state.ws.tabs.is_empty());
        assert!(state.doc_views.is_empty(), "no DocUi after closing tabs");
        assert!(state.doc_revs.is_empty(), "no revisions after closing tabs");
        assert!(
            state.language_by_id.is_empty(),
            "untitled language overrides die with the buffer"
        );
        assert!(
            state.external.is_empty(),
            "a closed file is no longer polled for external changes"
        );
        assert_eq!(
            state.language_by_path.get(&path).map(String::as_str),
            Some("markdown"),
            "the per-path language override outlives the tab"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reopening_a_file_gets_fresh_document_ui_defaults() {
        let dir = temp("reopen");
        let path = dir.join("notes.md");
        std::fs::write(&path, "# Notes\n").unwrap();
        let mut state = state(&dir);

        state.open_path(&path);
        let first_id = state.ws.active_doc().unwrap().id();
        state.set_view(ViewMode::Edit);
        assert_eq!(state.view_of_active(), ViewMode::Edit);
        assert!(state.doc_views.contains_key(&first_id));

        state.close_tab_now(index_of(&state, &path));
        assert!(!state.doc_views.contains_key(&first_id));

        state.open_path(&path);
        let second_id = state.ws.active_doc().unwrap().id();
        assert_ne!(first_id, second_id, "document ids are not reused");
        assert!(
            !state.doc_views.contains_key(&second_id),
            "reopening must not resurrect the old view entry"
        );
        assert_eq!(
            state.view_of_active(),
            ViewMode::Split,
            "Markdown falls back to its path-based default view"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_paths_prunes_external_stamps() {
        let dir = temp("delete");
        let scripts = dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let open = scripts.join("player.gd");
        let closed = scripts.join("old.gd");
        std::fs::write(&open, "extends Node\n").unwrap();
        std::fs::write(&closed, "extends Node\n").unwrap();
        let mut state = state(&dir);

        state.open_path(&open);
        assert!(state.external.contains_key(&open));
        state.close_tab_now(index_of(&state, &open));
        assert!(
            !state.external.contains_key(&open),
            "closing the tab drops its external stamp"
        );

        // A stamp is keyed by path and may outlive its tab; deleting the path
        // must clear every entry for it and, for a folder, everything below it.
        state.remember_disk(&open);
        state.remember_disk(&closed);
        state.delete_path_now(scripts.clone());
        assert!(
            state
                .external
                .keys()
                .all(|path| !path.starts_with(&scripts)),
            "deleted files must not stay in the external map: {:?}",
            state.external.keys().collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
