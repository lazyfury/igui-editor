//! Workspace root, external changes, drops and replace jobs.

use super::*;

impl AppState {
    /// Opens a folder as the workspace (Open Folder / dropped folder).
    pub fn set_root(&mut self, path: PathBuf) {
        if let Err(message) = self.ws.set_folder(path) {
            self.error(message);
            return;
        }
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = None;
        self.settings_open = false;
        self.session.folder = Some(self.ws.root.clone());
        self.request_git_refresh();
        self.quick_open.index = None;
        if self.quick_open.open {
            self.close_quick_open();
        }
        if self.search.open {
            self.close_project_search();
        }
        self.config_dirty = true;
        let name = self
            .ws
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.ws.root.display().to_string());
        self.status(format!("folder: {name}"));
        self.dirty = true;
    }

    pub(crate) fn remember_disk(&mut self, path: &Path) {
        self.external.insert(path.to_path_buf(), disk_stamp(path));
    }

    /// Polls metadata for every open file and reacts to outside edits. Clean
    /// tabs reload automatically; dirty tabs ask before losing their changes.
    pub fn check_external_changes(&mut self) {
        if self.modal.is_some() || self.recovery_prompt.is_some() {
            return;
        }
        for index in 0..self.ws.tabs.len() {
            let Some(path) = self
                .ws
                .tabs
                .get(index)
                .and_then(|doc| doc.path().map(Path::to_path_buf))
            else {
                continue;
            };
            let current = disk_stamp(&path);
            let Some(previous) = self.external.get(&path).copied() else {
                self.external.insert(path, current);
                continue;
            };
            if previous == current {
                continue;
            }
            self.external.insert(path.clone(), current);
            if current.is_none() {
                let name = self.ws.tabs[index].name().to_string();
                self.error(format!("{name} was deleted on disk"));
                self.dirty = true;
                continue;
            }
            let dirty = self.ws.tabs.get(index).is_some_and(|doc| doc.is_dirty());
            if !dirty {
                self.reload_tab(index, false);
            } else {
                self.modal = Some(Modal::ExternalChange { tab: index });
                self.menu_open = None;
                self.tab_menu = None;
                self.dirty = true;
                return;
            }
        }
    }

    /// Runs the 1-second metadata poll.
    pub fn tick_external_changes(&mut self) {
        if self.last_external_check.elapsed() < super::schedule::EXTERNAL_POLL {
            return;
        }
        self.last_external_check = Instant::now();
        self.check_external_changes();
    }

    pub(crate) fn reload_tab(&mut self, tab: usize, report: bool) {
        let Some(path) = self
            .ws
            .tabs
            .get(tab)
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(doc) = self.ws.tabs.get_mut(tab) else {
            return;
        };
        let name = doc.name().to_string();
        match doc.reload() {
            Ok(()) => {
                self.remember_disk(&path);
                self.highlight.reset();
                self.find.dirty = true;
                self.sync_layout();
                if report {
                    self.status(format!("reloaded {name} from disk"));
                } else {
                    self.status(format!("{name} changed on disk — reloaded"));
                }
                self.dirty = true;
            }
            Err(error) => self.error(format!("reload {name} failed: {error}")),
        }
    }

    /// Closes the folder and all of its tabs. Unsaved tabs are confirmed first.
    pub fn close_folder(&mut self) {
        if !self.ws.folder_open {
            self.error("no folder open");
            return;
        }
        let dirty = self.ws.tabs.iter().filter(|doc| doc.is_dirty()).count();
        if dirty > 0 {
            self.modal = Some(Modal::CloseFolder { dirty });
            self.menu_open = None;
            self.menu_hover = None;
            self.dirty = true;
            return;
        }
        self.close_folder_now();
    }

    pub(crate) fn close_folder_now(&mut self) {
        let ids: Vec<u64> = self.ws.tabs.iter().map(|doc| doc.id()).collect();
        if !self.ws.close_folder() {
            self.error("no folder open");
            return;
        }
        for id in ids {
            self.doc_revs.remove(&id);
            self.doc_views.remove(&id);
            self.recovery.remove(id);
            self.language_by_id.remove(&id);
        }
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = None;
        self.sidebar.actions.clear();
        self.session.folder = None;
        self.config_dirty = true;
        if self.quick_open.open {
            self.close_quick_open();
        }
        self.quick_open.index = None;
        if self.search.open {
            self.close_project_search();
        }
        self.after_tabs_changed();
        self.status("folder closed");
    }

    /// Opens dropped paths: a folder becomes the workspace, files become tabs.
    pub fn open_dropped(&mut self, paths: Vec<PathBuf>) {
        let mut folder = None;
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                if folder.is_none() {
                    folder = Some(path);
                }
            } else if path.is_file() {
                files.push(path);
            }
        }
        if let Some(folder) = folder {
            self.set_root(folder);
        }
        let had_files = !files.is_empty();
        let mut opened = 0usize;
        for file in files.into_iter().take(20) {
            self.open_path(file);
            opened += 1;
        }
        if opened > 0 {
            self.focus = Focus::Editor;
            self.status(format!(
                "opened {opened} dropped file{}",
                if opened == 1 { "" } else { "s" }
            ));
        } else if self.ws.folder_open {
            if !had_files {
                self.status("folder dropped");
            }
        } else if !had_files {
            self.status("nothing to open");
        }
        self.dirty = true;
    }

    /// Plans a replace-in-files run from the current hits and asks for
    /// confirmation. Open tabs are skipped.
    pub fn request_replace_in_files(&mut self, replacement: String) {
        if self.search.hits.is_empty() {
            self.error("nothing to replace");
            return;
        }
        let matcher = Matcher {
            query: self.search.query.text().trim().to_string(),
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
        };
        if matcher.query.is_empty() || replacement.is_empty() {
            self.error("enter a search term and a replacement");
            return;
        }
        let paths = self.search.hits.iter().map(|hit| hit.path.clone());
        let open: std::collections::HashSet<PathBuf> = self
            .ws
            .tabs
            .iter()
            .filter_map(|doc| doc.path().map(Path::to_path_buf))
            .collect();
        let (plans, errors) =
            replace::plan(paths, &matcher, &replacement, |path| open.contains(path));
        if plans.is_empty() {
            let detail = errors
                .first()
                .cloned()
                .unwrap_or_else(|| "no matches".into());
            self.error(format!("nothing replaced: {detail}"));
            return;
        }
        let files = plans.len();
        let occurrences: usize = plans.iter().map(|plan| plan.count).sum();
        self.pending_replace = Some(ReplaceJob {
            plans,
            replacement,
            files,
            occurrences,
        });
        self.modal = Some(Modal::ReplaceInFiles { files, occurrences });
        self.menu_open = None;
        self.dirty = true;
    }
}
