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

    /// Opens the quick-create-file prompt rooted at `dir` (tree context menu).
    pub fn open_new_file_in(&mut self, dir: PathBuf) {
        self.open_creator_in(dir, NewFileKind::File);
    }

    /// Opens the quick-create-folder prompt rooted at `dir`.
    pub fn open_new_folder_in(&mut self, dir: PathBuf) {
        self.open_creator_in(dir, NewFileKind::Folder);
    }

    fn open_creator_in(&mut self, dir: PathBuf, kind: NewFileKind) {
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
        self.new_file.kind = kind;
        self.new_file.dir = dir;
        self.new_file.target = None;
        self.new_file.name = TextEdit::new(match kind {
            NewFileKind::File => "untitled.md",
            NewFileKind::Folder => "new-folder",
            NewFileKind::Rename => "untitled",
        });
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
        self.new_file.target = None;
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.dirty = true;
    }

    /// Creates the entered file/folder. Errors stay inline in the prompt.
    pub fn submit_new_file(&mut self) {
        let kind = self.new_file.kind;
        if kind == NewFileKind::Rename {
            self.submit_rename();
            return;
        }
        let name = self.new_file.name.text().trim().to_string();
        if name.is_empty() {
            self.new_file.error = Some(match kind {
                NewFileKind::File => "enter a file name".into(),
                NewFileKind::Folder => "enter a folder name".into(),
                NewFileKind::Rename => "enter a name".into(),
            });
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
        let created = match kind {
            NewFileKind::File => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|_| ()),
            NewFileKind::Folder => std::fs::create_dir(&path),
            NewFileKind::Rename => unreachable!(),
        };
        match created {
            Ok(()) => {}
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
            // `refresh` keeps every expanded folder; the new entry shows up
            // without collapsing the user's tree.
            self.ws.tree.refresh();
            let expand = match kind {
                NewFileKind::File => path.parent(),
                NewFileKind::Folder => Some(path.as_path()),
                NewFileKind::Rename => None,
            };
            if let Some(parent) = expand {
                self.ws.tree.expand_to(parent);
            }
        }
        if kind == NewFileKind::File {
            self.note_file_event(path.clone(), crate::model::watch::FileEventKind::Created);
        }
        match kind {
            NewFileKind::File => {
                self.open_path(path);
                self.status(format!("created {name}"));
            }
            NewFileKind::Folder => {
                self.sidebar.selected = Some(path);
                self.focus = Focus::Explorer;
                self.status(format!("created folder {name}"));
            }
            NewFileKind::Rename => {}
        }
        self.dirty = true;
    }
}
