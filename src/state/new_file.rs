//! Quick-create-file prompt state.

use super::*;

impl AppState {
    /// Opens the quick-create-file prompt.
    pub fn open_new_file(&mut self) {
        let (dir, _) = self.dialog_context();
        let dir =
            dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.open_new_file_in(dir);
    }

    /// Opens the quick-create prompt rooted at `dir` (tree context menu).
    pub fn open_new_file_in(&mut self, dir: PathBuf) {
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
        self.new_file.dir = dir;
        self.new_file.name = TextEdit::new("untitled.md");
        self.new_file.name.select_all();
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.new_file.open = true;
        self.focus = Focus::Editor;
        self.terminal.focused = false;
        self.dirty = true;
    }

    pub fn close_new_file(&mut self) {
        self.new_file.open = false;
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.dirty = true;
    }

    /// Creates and opens the entered file. Errors stay inline in the prompt.
    pub fn submit_new_file(&mut self) {
        let name = self.new_file.name.text().trim().to_string();
        if name.is_empty() {
            self.new_file.error = Some("enter a file name".into());
            self.dirty = true;
            return;
        }
        let relative = Path::new(&name);
        if relative.is_absolute()
            || relative.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            self.new_file.error = Some("use a relative name without ..".into());
            self.dirty = true;
            return;
        }
        let path = self.new_file.dir.join(relative);
        if let Some(parent) = path.parent() {
            if !parent.is_dir() {
                self.new_file.error = Some(format!("folder does not exist: {}", parent.display()));
                self.dirty = true;
                return;
            }
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.new_file.error = Some(format!("already exists: {name}"));
                self.dirty = true;
                return;
            }
            Err(error) => {
                self.new_file.error = Some(format!("create failed: {error}"));
                self.dirty = true;
                return;
            }
        }
        let name = name.clone();
        self.new_file.open = false;
        self.new_file.error = None;
        if self.ws.folder_open {
            self.ws.tree.reload();
            if let Some(parent) = path.parent() {
                self.ws.tree.expand_to(parent);
            }
        }
        self.open_path(path);
        self.status(format!("created {name}"));
        self.dirty = true;
    }
}
