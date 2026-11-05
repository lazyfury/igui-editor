//! Autosave, crash recovery and window geometry.

use super::*;

impl AppState {
    // -- reliability -----------------------------------------------------

    /// Autosave + recovery flush after the idle delay.
    ///
    /// A flush is scheduled only by an edit (`changed_at`) or by the backoff
    /// after a failed write (`retry_at`). A dirty buffer that is not being
    /// edited does not wake the app or rewrite its snapshot.
    pub fn tick_reliability(&mut self) {
        if self.recovery_prompt.is_some() || self.modal.is_some() {
            return;
        }
        let now = Instant::now();
        let mut changed = false;
        for doc in &self.ws.tabs {
            let id = doc.id();
            let rev = doc.rev();
            if self.doc_revs.get(&id) != Some(&rev) {
                self.doc_revs.insert(id, rev);
                changed = true;
            }
        }
        // Only rebuild the live set when a tab was closed (the common case has
        // the same count and no set work per frame).
        if changed || self.doc_revs.len() != self.ws.tabs.len() {
            let live: std::collections::HashSet<u64> =
                self.ws.tabs.iter().map(|doc| doc.id()).collect();
            self.doc_revs.retain(|id, _| live.contains(id));
            self.recovery_revs.retain(|id, _| live.contains(id));
        }
        if changed {
            self.reliability_changed_at = Some(now);
            // A fresh edit supersedes a pending failure backoff.
            self.reliability_retry_at = None;
        }
        let edit_due = self
            .reliability_changed_at
            .is_some_and(|at| now.duration_since(at) >= super::schedule::AUTOSAVE_DELAY);
        let retry_due = self.reliability_retry_at.is_some_and(|at| now >= at);
        if !(edit_due || retry_due) {
            return;
        }
        self.flush_reliability();
    }

    /// Runs autosave + snapshots immediately (tests / shutdown paths).
    pub fn force_reliability_flush(&mut self) {
        self.flush_reliability();
    }

    /// Writes dirty buffers and recovery snapshots. Returns the number of
    /// snapshots written; unchanged buffers are skipped.
    pub(crate) fn flush_reliability(&mut self) -> usize {
        let mut saved = 0usize;
        let mut saved_paths = Vec::new();
        let mut failures: Vec<(String, String)> = Vec::new();
        if self.settings.autosave {
            // Autosave writes verbatim: no tidy rewrite and no undo entry. A
            // failed write keeps the tab dirty so the next flush can retry.
            for doc in &mut self.ws.tabs {
                if !doc.is_dirty() || doc.path().is_none() {
                    continue;
                }
                match doc.save_verbatim() {
                    Ok(()) => {
                        saved += 1;
                        if let Some(path) = doc.path().map(Path::to_path_buf) {
                            saved_paths.push(path);
                        }
                    }
                    Err(error) => failures.push((doc.name().to_string(), error.to_string())),
                }
            }
        }
        for path in saved_paths {
            self.remember_disk(&path);
        }
        let mut live = std::collections::HashSet::new();
        let mut snapshots = 0usize;
        for doc in &mut self.ws.tabs {
            let id = doc.id();
            let rev = doc.rev();
            self.doc_revs.insert(id, rev);
            live.insert(id);
            if doc.is_dirty() {
                // A snapshot only changes when the buffer does; the 2 s idle
                // tick must not rewrite an unchanged dirty file.
                if self.recovery_revs.get(&id) != Some(&rev) {
                    let wrote = self.recovery.save(
                        id,
                        crate::model::recovery::Snapshot {
                            name: doc.name(),
                            path: doc.path(),
                            text: doc.text(),
                            caret: doc.caret(),
                            anchor: doc.anchor(),
                            eol: doc.line_ending(),
                        },
                    );
                    // A failed snapshot is retried on the next edit/backoff.
                    if wrote.is_ok() {
                        self.recovery_revs.insert(id, rev);
                        snapshots += 1;
                    }
                }
            } else {
                self.recovery.remove(id);
                self.recovery_revs.remove(&id);
            }
        }
        for entry in self.recovery.list() {
            if !live.contains(&entry.id) {
                self.recovery.remove(entry.id);
            }
        }
        if saved > 0 {
            self.sync_session_tabs();
        }
        if failures.is_empty() {
            if saved > 0 {
                self.status(format!("autosaved {saved} file(s)"));
            }
        } else {
            // Report a failed autosave instead of claiming success.
            let (name, error) = &failures[0];
            let message = if failures.len() == 1 {
                format!("autosave failed for {name}: {error}")
            } else {
                format!(
                    "autosave failed for {} files ({name}: {error})",
                    failures.len()
                )
            };
            self.error(message);
        }
        // The flush consumed the pending edit. A failed write is retried after
        // a backoff instead of on every idle tick.
        self.reliability_changed_at = None;
        self.reliability_retry_at = if failures.is_empty() {
            None
        } else {
            Some(Instant::now() + super::schedule::AUTOSAVE_RETRY)
        };
        snapshots
    }

    /// Applies the startup recovery prompt.
    pub fn resolve_recovery(&mut self, restore: bool) {
        let Some(entries) = self.recovery_prompt.take() else {
            return;
        };
        if restore {
            let mut last = None;
            for entry in &entries {
                let existing = entry.path.as_ref().and_then(|path| {
                    self.ws
                        .tabs
                        .iter()
                        .position(|doc| doc.path() == Some(path.as_path()))
                });
                if let Some(index) = existing {
                    if let Some(doc) = self.ws.tabs.get_mut(index) {
                        doc.restore_snapshot(
                            entry.text.clone(),
                            entry.caret,
                            entry.anchor,
                            entry.eol,
                        );
                    }
                    last = Some(index);
                } else {
                    let index = self.ws.add_recovered(entry);
                    if let Some(doc) = self.ws.tabs.get(index) {
                        self.doc_revs.insert(doc.id(), doc.rev());
                    }
                    last = Some(index);
                }
            }
            if let Some(index) = last {
                self.ws.select(index);
            }
            self.recovery.clear();
            // Restored buffers are dirty again but their snapshots were just
            // cleared; schedule one so a crash before the next edit cannot lose
            // the recovered text.
            self.reliability_changed_at = Some(Instant::now());
            self.reliability_retry_at = None;
            self.after_tabs_changed();
            self.status(format!("restored {} buffer(s)", entries.len()));
        } else {
            self.recovery.clear();
            self.status("discarded recovered buffers");
        }
        self.dirty = true;
    }

    /// Persists window size/position (logical pixels).
    pub fn set_window_geometry(&mut self, size: (f32, f32), position: (f32, f32)) {
        let geometry = WindowGeometry { size, position };
        if self.session.window != Some(geometry) {
            self.session.window = Some(geometry);
            self.config_dirty = true;
        }
    }

    /// Persists whether the window was maximized (zoomed) on exit.
    pub fn set_window_maximized(&mut self, maximized: bool) {
        if self.session.window_maximized != maximized {
            self.session.window_maximized = maximized;
            self.config_dirty = true;
        }
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
            root.join("igui-editor-reliability-test.conf"),
        )
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-reliability-{tag}-{}-{}",
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

    #[test]
    fn autosave_writes_the_verbatim_buffer_and_clears_dirty() {
        let dir = temp("verbatim");
        let path = dir.join("notes.txt");
        std::fs::write(&path, "one  \ntwo   ").unwrap();
        let mut state = state(&dir);
        state.ws.open(&path).unwrap();
        let doc = state.ws.active_doc_mut().unwrap();
        doc.set_caret(doc.text().len());
        doc.insert_text("x");
        assert!(doc.is_dirty());

        state.force_reliability_flush();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "one  \ntwo   x",
            "autosave must not tidy trailing whitespace or a missing newline"
        );
        assert!(!state.ws.active_doc().unwrap().is_dirty());
        let (message, is_error) = state.message().unwrap();
        assert!(!is_error, "successful autosave is not an error: {message}");
        assert_eq!(message, "autosaved 1 file(s)");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unchanged_dirty_buffer_is_not_snapshotted_again() {
        let dir = temp("dedupe");
        let path = dir.join("notes.txt");
        std::fs::write(&path, "one").unwrap();
        let mut state = state(&dir);
        state.ws.open(&path).unwrap();
        state.settings.autosave = false;
        let doc = state.ws.active_doc_mut().unwrap();
        doc.set_caret(doc.text().len());
        doc.insert_text("!");

        assert_eq!(state.flush_reliability(), 1, "first flush snapshots");
        assert!(state.reliability_changed_at.is_none());
        assert_eq!(state.flush_reliability(), 0, "unchanged buffer is skipped");
        assert_eq!(state.flush_reliability(), 0);

        state.ws.active_doc_mut().unwrap().insert_text("?");
        assert_eq!(state.flush_reliability(), 1, "an edit snapshots once");
        assert_eq!(state.flush_reliability(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_autosave_backs_off_instead_of_waking_every_tick() {
        let dir = temp("backoff");
        let path = dir.join("blocked.txt");
        std::fs::create_dir_all(&path).unwrap();
        let mut state = state(&dir);
        let doc = state.ws.active_doc_mut().unwrap();
        doc.set_path(path);
        doc.insert_text("keep me");

        state.flush_reliability();
        assert!(state.reliability_changed_at.is_none());
        let retry = state
            .reliability_retry_at
            .expect("a failed write schedules a retry");
        assert!(
            retry > Instant::now() + Duration::from_secs(20),
            "retry must be a backoff, not the next tick"
        );

        // A new edit supersedes the backoff and schedules a normal flush.
        state.ws.active_doc_mut().unwrap().insert_text("x");
        state.tick_reliability();
        assert!(state.reliability_retry_at.is_none());
        assert!(state.reliability_changed_at.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_autosave_keeps_the_tab_dirty_and_reports_an_error() {
        let dir = temp("failure");
        let path = dir.join("blocked.txt");
        // A directory with the target name makes the write/rename fail.
        std::fs::create_dir_all(&path).unwrap();
        let mut state = state(&dir);
        let doc = state.ws.active_doc_mut().unwrap();
        doc.set_path(path);
        doc.insert_text("keep me");
        assert!(doc.is_dirty());

        state.force_reliability_flush();
        assert!(
            state.ws.active_doc().unwrap().is_dirty(),
            "a failed autosave must not clear dirty"
        );
        let (message, is_error) = state.message().unwrap();
        assert!(is_error, "autosave failure must be an error: {message}");
        assert!(
            message.contains("autosave failed"),
            "unexpected message: {message}"
        );
        assert!(
            !message.contains("autosaved"),
            "a failure must not claim success: {message}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
