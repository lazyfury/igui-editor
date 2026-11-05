//! File open/save, recents and session tab sync.

use super::*;

impl AppState {
    pub fn clear_recent(&mut self) {
        if self.session.recent_projects.is_empty() && self.session.recent_files.is_empty() {
            return;
        }
        self.session.clear_recents();
        self.config_dirty = true;
        crate::macos::clear_system_recent();
        self.status("recent projects and files cleared");
        self.dirty = true;
    }

    pub fn open_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        match self.ws.open(&path) {
            Ok(_) => {
                let name = self
                    .ws
                    .active_doc()
                    .map(|doc| doc.name().to_string())
                    .unwrap_or_default();
                self.sidebar.selected = Some(path.clone());
                self.tabs.reveal_active = true;
                self.request_git_refresh();
                self.remember_disk(&path);
                self.session.push_recent_file(&path);
                crate::macos::note_recent(&path);
                self.config_dirty = true;
                self.sync_layout();
                self.sync_session_tabs();
                self.find.dirty = true;
                self.focus = Focus::Editor;
                self.editor.preedit = None;
                self.editor.drag = Drag::None;
                self.status(format!("opened {name}"));
            }
            Err(message) => self.error(message),
        }
        self.dirty = true;
    }

    pub fn save_active(&mut self) {
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only()) {
            if self.ws.active_doc().is_some_and(|doc| doc.is_image_view()) {
                self.status("image preview is read-only");
            } else {
                self.status("read-only Git view");
            }
            return;
        }
        match self.ws.save_active() {
            Ok(message) => {
                self.note_saved();
                self.status(message);
            }
            Err(message) => {
                if self.ws.active_doc().is_some_and(|doc| doc.path().is_none()) {
                    self.pending_dialog = Some(DialogRequest {
                        kind: DialogKind::SaveAs,
                        then_close: None,
                    });
                } else {
                    self.error(message);
                }
            }
        }
        self.dirty = true;
    }

    /// Applies a Save As path to the active tab.
    pub fn save_as(&mut self, path: PathBuf, then_close: Option<usize>) {
        match self.ws.save_active_as(path) {
            Ok(message) => {
                self.note_saved();
                self.status(message);
                if let Some(index) = then_close {
                    self.close_tab_now(index);
                }
            }
            Err(message) => self.error(message),
        }
        self.dirty = true;
    }

    pub(crate) fn note_saved(&mut self) {
        if let Some(doc) = self.ws.active_doc() {
            self.recovery.remove(doc.id());
        }
        if let Some(path) = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        {
            self.session.push_recent_file(&path);
            self.config_dirty = true;
            self.sidebar.selected = Some(path.clone());
            self.request_git_refresh();
            self.remember_disk(&path);
            self.lsp.notify_doc_saved(&path);
            crate::macos::note_recent(&path);
        }
        self.sync_session_tabs();
    }

    /// Start directory and suggested name for the next file dialog.
    pub fn dialog_context(&self) -> (Option<PathBuf>, String) {
        let start = self
            .ws
            .active_doc()
            .and_then(|doc| {
                doc.path()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
            .or_else(home_dir);
        let suggested = self
            .ws
            .active_doc()
            .map(|doc| doc.name().to_string())
            .unwrap_or_else(|| "untitled.txt".into());
        (start, suggested)
    }

    /// Records the open, saved tabs so a restart can restore them.
    pub fn sync_session_tabs(&mut self) {
        let tabs: Vec<PathBuf> = self
            .ws
            .tabs
            .iter()
            .filter_map(|doc| doc.path().map(Path::to_path_buf))
            .collect();
        let active = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf));
        self.session.set_tabs(tabs);
        // A synthetic Git/docs view has no path: keep the last real file tab
        // as the restored selection instead of clearing it.
        if active.is_some() {
            self.session.set_active_tab(active);
        }
        self.config_dirty = true;
    }
}
