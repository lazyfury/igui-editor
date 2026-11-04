//! Autosave, crash recovery and window geometry.

use super::*;

impl AppState {
    // -- reliability -----------------------------------------------------

    /// Autosave + recovery flush after the idle delay; tracks edits to reset it.
    pub fn tick_reliability(&mut self) {
        if self.recovery_prompt.is_some() || self.modal.is_some() {
            return;
        }
        let mut changed = false;
        for doc in &self.ws.tabs {
            let id = doc.id();
            let rev = doc.rev();
            match self.doc_revs.get(&id) {
                Some(known) if *known == rev => {}
                _ => {
                    self.doc_revs.insert(id, rev);
                    changed = true;
                }
            }
        }
        let live: std::collections::HashSet<u64> =
            self.ws.tabs.iter().map(|doc| doc.id()).collect();
        self.doc_revs.retain(|id, _| live.contains(id));
        if changed {
            self.last_activity = Instant::now();
        }
        if self.last_activity.elapsed().as_secs_f32() < 2.0 {
            return;
        }
        self.flush_reliability();
        self.last_activity = Instant::now();
    }

    /// Runs autosave + snapshots immediately (tests / shutdown paths).
    pub fn force_reliability_flush(&mut self) {
        self.flush_reliability();
    }

    pub(crate) fn flush_reliability(&mut self) {
        let mut saved = 0usize;
        let mut saved_paths = Vec::new();
        if self.settings.autosave {
            for doc in &mut self.ws.tabs {
                if doc.is_dirty() && doc.path().is_some() && doc.save().is_ok() {
                    saved += 1;
                    if let Some(path) = doc.path().map(Path::to_path_buf) {
                        saved_paths.push(path);
                    }
                }
            }
        }
        for path in saved_paths {
            self.remember_disk(&path);
        }
        let mut live = std::collections::HashSet::new();
        for doc in &self.ws.tabs {
            live.insert(doc.id());
            if doc.is_dirty() {
                let _ = self.recovery.save(
                    doc.id(),
                    doc.name(),
                    doc.path(),
                    doc.text(),
                    doc.caret(),
                    doc.anchor(),
                );
            } else {
                self.recovery.remove(doc.id());
            }
        }
        for entry in self.recovery.list() {
            if !live.contains(&entry.id) {
                self.recovery.remove(entry.id);
            }
        }
        if saved > 0 {
            self.status(format!("autosaved {saved} file(s)"));
            self.sync_session_tabs();
        }
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
                        doc.restore_snapshot(entry.text.clone(), entry.caret, entry.anchor);
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
