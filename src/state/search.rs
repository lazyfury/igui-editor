//! Project search worker state.

use super::*;

impl AppState {
    /// Opens (or closes) the project search panel (folder required).
    pub fn open_project_search(&mut self) {
        if self.search.open {
            self.close_project_search();
            return;
        }
        if !self.ws.folder_open {
            self.error("open a folder first (⇧⌘O)");
            return;
        }
        self.settings_open = false;
        self.quick_open.open = false;
        self.menu_open = None;
        self.tree_menu = None;
        self.lsp.dismiss_transient();
        self.search.open = true;
        self.search.query.select_all();
        self.search.focus = SearchFocus::Query;
        self.search.selected = 0;
        self.search.scroll = 0;
        self.search.hits.clear();
        self.search.files_scanned = 0;
        self.search.scanning = false;
        self.search.pending = None;
        if self.search.query.text().is_empty() {
            self.search.worker = None;
        } else {
            self.search_start();
        }
        self.dirty = true;
    }

    pub fn close_project_search(&mut self) {
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        self.search.open = false;
        self.search.pending = None;
        self.search.scanning = false;
        self.search.hits.clear();
        self.search.selected = 0;
        self.search.scroll = 0;
        self.dirty = true;
    }

    /// Debounces query/toggle edits (120 ms), then restarts the worker.
    pub fn search_query_changed(&mut self) {
        self.search.pending = Some(Instant::now());
        self.search.selected = 0;
        self.search.scroll = 0;
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        self.search.scanning = false;
        self.dirty = true;
    }

    pub(crate) fn search_start(&mut self) {
        self.search.pending = None;
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        let query = self.search.query.text().trim().to_string();
        if query.is_empty() {
            self.search.scanning = false;
            self.search.hits.clear();
            return;
        }
        self.search.hits.clear();
        self.search.files_scanned = 0;
        self.search.selected = 0;
        self.search.scroll = 0;
        self.search.scanning = true;
        let matcher = Matcher {
            query,
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
        };
        self.search.worker = Some(SearchWorker::spawn(self.ws.root.clone(), matcher));
        self.dirty = true;
    }

    /// Advances the debounce and drains worker messages. Called from `update`.
    pub fn search_tick(&mut self) {
        if !self.search.open {
            return;
        }
        if self
            .search
            .pending
            .is_some_and(|at| at.elapsed().as_millis() >= 120)
        {
            self.search_start();
        }
        let mut done = false;
        if let Some(worker) = &self.search.worker {
            for _ in 0..64 {
                match worker.try_recv() {
                    Some(SearchMessage::Hits(batch)) => {
                        self.search.hits.extend(batch);
                        if self.search.hits.len() >= crate::model::search::MAX_HITS {
                            self.search.hits.truncate(crate::model::search::MAX_HITS);
                            worker.cancel();
                        }
                        self.search.selected = self
                            .search
                            .selected
                            .min(self.search.hits.len().saturating_sub(1));
                        self.dirty = true;
                    }
                    Some(SearchMessage::Progress { files }) => {
                        self.search.files_scanned = files;
                        self.dirty = true;
                    }
                    Some(SearchMessage::Done { files }) => {
                        self.search.files_scanned = files;
                        done = true;
                        self.dirty = true;
                        break;
                    }
                    None => break,
                }
            }
        }
        if done {
            self.search.worker = None;
            self.search.scanning = false;
        }
    }

    pub fn search_busy(&self) -> bool {
        self.search.scanning || self.search.pending.is_some()
    }

    pub fn search_move(&mut self, delta: isize) {
        if self.search.hits.is_empty() {
            return;
        }
        let last = self.search.hits.len() - 1;
        let selected = self.search.selected as isize + delta;
        self.search.selected = selected.clamp(0, last as isize) as usize;
        self.dirty = true;
    }

    /// Opens the selected hit and selects the match.
    pub fn search_accept(&mut self) {
        let Some(hit) = self.search.hits.get(self.search.selected).cloned() else {
            return;
        };
        let file_name = hit
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = hit.line + 1;
        self.open_path(hit.path);
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_caret(hit.offset);
            doc.extend_to(hit.offset + (hit.end_in_line.saturating_sub(hit.start_in_line)));
        }
        self.editor.reveal = true;
        self.focus = Focus::Editor;
        self.status(format!("{file_name}:{line}"));
        self.dirty = true;
    }
}
