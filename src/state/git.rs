//! Git snapshot and read-only diff.

use super::*;

impl AppState {
    /// A git worker request is outstanding; the app must keep polling for the
    /// reply until it lands.
    pub fn git_busy(&self) -> bool {
        self.git.pending || self.git.loading || self.git.diff_loading
    }

    pub fn git_entry_for(&self, path: &Path) -> Option<&crate::model::git::GitEntry> {
        let root = self.git.status.root.as_ref()?;
        let relative = path.strip_prefix(root).ok()?;
        self.git
            .status
            .conflicts
            .iter()
            .chain(self.git.status.changes.iter())
            .chain(self.git.status.staged.iter())
            .find(|entry| entry.path == relative)
    }

    pub(crate) fn git_cwd(&self) -> Option<PathBuf> {
        self.ws
            .active_doc()
            .and_then(|doc| {
                doc.path()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
    }

    pub fn request_git_refresh(&mut self) {
        self.git.pending = true;
        self.git.cwd = self.git_cwd();
        self.dirty = true;
    }

    /// Drops repository snapshots and overlays tied to the previous project.
    pub(crate) fn reset_git_project(&mut self) {
        self.git.status = RepoStatus::default();
        self.git.loading = false;
        self.git.pending = false;
        self.git.selected = None;
        self.git.diff = None;
        self.git.diff_loading = false;
        self.git.diff_visible = false;
        self.git.diff_scroll = 0.0;
        self.git.rows.clear();
        self.git.scroll = 0.0;
        self.git.scroll_max = 0.0;
        self.git.error = None;
        self.git.hover = None;
        self.dirty = true;
    }

    pub(crate) fn ensure_git_worker(&mut self) {
        if self.git.worker.is_none() {
            self.git.worker = Some(GitWorker::spawn());
        }
    }

    pub(crate) fn show_git_view(
        &mut self,
        key: String,
        name: String,
        text: String,
        preview_start: Option<usize>,
    ) {
        let index = self.ws.show_view(key, name, text);
        self.ws.select(index);
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_git_preview_start(preview_start);
        }
        self.tabs.reveal_active = true;
        self.focus = Focus::Editor;
        self.editor.preedit = None;
        self.editor.drag = Drag::None;
        self.sync_layout();
        self.find.dirty = true;
        self.dirty = true;
    }

    pub fn open_git_diff(&mut self, path: PathBuf, staged: bool) {
        self.ensure_git_worker();
        self.git.selected = Some(path.clone());
        self.git.diff = None;
        self.git.diff_loading = true;
        self.git.diff_visible = false;
        self.git.diff_scroll = 0.0;
        self.status("reading diff…");
        self.focus = Focus::Editor;
        self.terminal.focused = false;
        self.git.diff_generation += 1;
        let generation = self.git.diff_generation;
        let cwd = self
            .git
            .cwd
            .clone()
            .or_else(|| self.git_cwd())
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(worker) = &self.git.worker {
            worker.send(GitRequest::Diff {
                generation,
                cwd,
                path,
                staged,
            });
        }
        self.dirty = true;
    }

    pub fn close_git_diff(&mut self) {
        self.git.diff_visible = false;
        self.git.diff_loading = false;
        self.git.diff = None;
        self.git.diff_scroll = 0.0;
        self.dirty = true;
    }

    /// Loads `<config>/keymaps.json` once at startup. Invalid JSON is reported
    /// but never prevents the editor from starting.
    pub(crate) fn rebuild_keymap(&mut self) {
        self.keymap = Keymap::for_id(self.settings.keymap);
        if let Some(overrides) = self.keymap_overrides.clone() {
            self.keymap.apply_overrides(&overrides);
        }
        self.keymap_pending.clear();
        self.vim
            .reset_for_keymap(self.settings.keymap == KeymapId::Vim);
    }

    /// Drains Git worker events and issues a pending status request.
    pub fn tick_git(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(worker) = &self.git.worker {
            loop {
                match worker.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        // A dead worker must not keep `needs_frame` polling forever.
        if disconnected {
            self.git.worker = None;
            self.git.pending = false;
            self.git.loading = false;
            self.git.diff_loading = false;
            self.git.error = Some("git worker stopped".into());
            self.dirty = true;
        }
        for event in events {
            match event {
                GitEvent::Status { generation, status } if generation == self.git.generation => {
                    self.git.status = status;
                    self.git.loading = false;
                    self.git.error = None;
                    self.dirty = true;
                }
                GitEvent::Diff { generation, diff } if generation == self.git.diff_generation => {
                    let staged = diff.staged;
                    let path = diff.path.clone();
                    let mut text = format!(
                        "diff --git a/{} b/{}\n{}\n{}",
                        path.display(),
                        path.display(),
                        if staged { "staged" } else { "worktree" },
                        diff.text
                    );
                    let mut preview_start = None;
                    if let Some(root) = self.git.status.root.as_ref() {
                        if let Ok(content) = std::fs::read_to_string(root.join(&path)) {
                            if content.len() <= 2 * 1024 * 1024 {
                                let before = text.lines().count();
                                text.push_str("\n──── file preview ────\n");
                                text.push_str(&content);
                                preview_start = Some(before + 2);
                            }
                        }
                    }
                    let name = format!("Diff: {}", path.display());
                    let key = format!("git:diff:{}:{staged}", path.display());
                    self.git.diff = Some(diff);
                    self.git.diff_loading = false;
                    self.show_git_view(key, name, text, preview_start);
                    self.status("git diff");
                }
                _ => {}
            }
        }
        if self.git.pending {
            self.git.pending = false;
            self.ensure_git_worker();
            self.git.generation += 1;
            let generation = self.git.generation;
            self.git.loading = true;
            self.git.error = None;
            let cwd = self.git.cwd.clone().or_else(|| self.git_cwd());
            if let Some(cwd) = cwd {
                if let Some(worker) = &self.git.worker {
                    worker.send(GitRequest::Status { generation, cwd });
                }
            } else {
                self.git.loading = false;
                self.git.status = RepoStatus::default();
            }
        }
        // Drop stale worker errors so the panel does not keep a previous repo.
        if let Some(status) = self.git.status.error.take() {
            if !status.is_empty() {
                self.git.error = Some(status);
                self.dirty = true;
            }
        }
    }
}
