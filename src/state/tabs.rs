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
            let view_h = (self.sidebar.rect.size.height - crate::ui::sidebar::HEADER_H).max(0.0);
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

    /// Called when macOS asks to quit with dirty tabs.
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
        let id = self.ws.tabs.get(index).map(|doc| doc.id());
        self.ws.force_close(index);
        if let Some(id) = id {
            self.doc_revs.remove(&id);
            self.recovery.remove(id);
            self.language_by_id.remove(&id);
        }
        self.after_tabs_changed();
    }

    /// Re-opens the workspace after a tab change.
    pub fn after_tabs_changed(&mut self) {
        self.ws.validate_active();
        self.tabs.reveal_active = true;
        self.request_git_refresh();
        self.sync_layout();
        self.sync_session_tabs();
        self.find.dirty = true;
        self.focus = Focus::Editor;
        self.dirty = true;
    }
}
