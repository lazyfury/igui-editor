//! Explorer rename prompt and path updates.

use super::*;

impl AppState {
    /// Enter in the focused Explorer renames the selected row.
    pub fn rename_selected(&mut self) {
        let Some(path) = self.sidebar.selected.clone() else {
            self.status("select a file or folder first");
            return;
        };
        self.open_rename(path);
    }

    /// Opens the inline rename prompt for `path`.
    pub fn open_rename(&mut self, path: PathBuf) {
        if !path.exists() {
            self.error(format!("does not exist: {}", path.display()));
            return;
        }
        if self.ws.folder() == Some(path.as_path()) {
            self.error("cannot rename the project root");
            return;
        }
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            self.error("cannot rename this path");
            return;
        };
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            self.error("cannot rename this path");
            return;
        };
        if self.new_file.open || self.modal.is_some() || self.recovery_prompt.is_some() {
            return;
        }
        self.settings_open = false;
        self.quick_open.open = false;
        self.lsp.dismiss_transient();
        if self.search.open {
            self.close_project_search();
        }
        self.close_find();
        self.menu_open = None;
        self.tab_menu = None;
        self.tree_menu = None;
        self.new_file.kind = NewFileKind::Rename;
        self.new_file.dir = dir;
        self.new_file.name = TextEdit::new(name);
        self.new_file.name.select_all();
        self.new_file.target = Some(path);
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.new_file.open = true;
        self.focus = Focus::Explorer;
        self.terminal.focused = false;
        self.dirty = true;
    }

    pub(crate) fn submit_rename(&mut self) {
        let Some(target) = self.new_file.target.clone() else {
            self.new_file.error = Some("nothing to rename".into());
            self.dirty = true;
            return;
        };
        let name = self.new_file.name.text().trim().to_string();
        if name.is_empty() {
            self.new_file.error = Some("enter a name".into());
            self.dirty = true;
            return;
        }
        let relative = Path::new(&name);
        if relative.is_absolute()
            || relative.components().count() != 1
            || !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            self.new_file.error = Some("enter a single name without / or ..".into());
            self.dirty = true;
            return;
        }
        let dest = self.new_file.dir.join(relative);
        if dest == target {
            self.close_new_file();
            self.focus = Focus::Explorer;
            self.dirty = true;
            return;
        }
        if dest.exists() {
            self.new_file.error = Some(format!("already exists: {name}"));
            self.dirty = true;
            return;
        }
        if let Err(error) = std::fs::rename(&target, &dest) {
            self.new_file.error = Some(format!("rename failed: {error}"));
            self.dirty = true;
            return;
        }
        let updates: Vec<(usize, PathBuf, PathBuf)> = self
            .ws
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, doc)| {
                let path = doc.path()?;
                if path != target && !path.starts_with(&target) {
                    return None;
                }
                let suffix = path.strip_prefix(&target).unwrap_or(Path::new(""));
                Some((index, path.to_path_buf(), dest.join(suffix)))
            })
            .collect();
        for (index, old, new) in &updates {
            if let Some(doc) = self.ws.tabs.get_mut(*index) {
                doc.set_path(new.clone());
            }
            self.external.remove(old);
            self.external.insert(new.clone(), disk_stamp(new));
        }
        for path in &mut self.session.recent_files {
            if path == &target || path.starts_with(&target) {
                let suffix = path.strip_prefix(&target).unwrap_or(Path::new(""));
                *path = dest.join(suffix);
            }
        }
        for path in &mut self.session.recent_projects {
            if path == &target || path.starts_with(&target) {
                let suffix = path.strip_prefix(&target).unwrap_or(Path::new(""));
                *path = dest.join(suffix);
            }
        }
        for (path, _) in &mut self.session.languages {
            if path == &target || path.starts_with(&target) {
                let suffix = path.strip_prefix(&target).unwrap_or(Path::new(""));
                *path = dest.join(suffix);
            }
        }
        self.sidebar.selected = Some(dest.clone());
        self.new_file.open = false;
        self.new_file.target = None;
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.focus = Focus::Explorer;
        self.ws.tree.refresh();
        if let Some(parent) = dest.parent() {
            self.ws.tree.expand_to(parent);
        }
        // Tell the language servers about the move: the old path is gone, the
        // new one exists (directories report both ends).
        self.note_file_event(target.clone(), crate::model::watch::FileEventKind::Deleted);
        self.note_file_event(dest.clone(), crate::model::watch::FileEventKind::Created);
        self.request_git_refresh();
        self.sync_session_tabs();
        self.find.dirty = true;
        self.config_dirty = true;
        let old_name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.display().to_string());
        self.status(format!("renamed {old_name} → {name}"));
        self.dirty = true;
    }
}
