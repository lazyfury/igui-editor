//! External file watching on the metadata poll.
//!
//! Only open files used to be polled, so a tool creating a file next to an open
//! one (Godot writes `<script>.gd.uid`) was invisible: the Explorer stayed
//! stale and the language servers were never told. This module snapshots the
//! directories the user can actually see — the loaded sidebar tree plus the
//! parents of open tabs — and queues the difference as
//! [`crate::model::watch::FileEvent`]s for the sidebar refresh and the language
//! servers.
//!
//! No OS watcher and no new dependency: one `read_dir` per watched directory on
//! the existing one-second poll, bounded by [`MAX_WATCH_DIRS`] and
//! [`crate::model::watch::MAX_DIR_ENTRIES`].

use super::*;
use crate::model::watch::{self, FileEvent, FileEventKind};

/// Directories one poll may read (the loaded tree plus open-tab parents).
const MAX_WATCH_DIRS: usize = 32;
/// Directory entries one poll may stat in total. A huge expanded folder stops
/// the scan instead of blocking the frame on tens of thousands of `stat`s.
const MAX_WATCH_ENTRIES: usize = 20_000;

impl AppState {
    /// Runs the filesystem poll: open-file metadata, then the watched
    /// directories. Called once a second (and directly by tests/self-check).
    pub fn poll_filesystem(&mut self) {
        self.check_external_changes();
        self.check_directory_changes();
    }

    /// Queues one editor-side change for the language servers.
    pub(crate) fn note_file_event(&mut self, path: PathBuf, kind: FileEventKind) {
        self.file_events.push(FileEvent { path, kind });
        self.dirty = true;
    }

    /// Compares the current listing of every watched directory with the last
    /// one and reacts to creates, deletes and content changes.
    fn check_directory_changes(&mut self) {
        let mut dirs = self.ws.tree.watch_dirs();
        for doc in &self.ws.tabs {
            if let Some(parent) = doc.path().and_then(Path::parent) {
                dirs.push(parent.to_path_buf());
            }
        }
        // Godot rewrites `.godot/global_script_class_cache.cfg` when its scan
        // changes the global class list. Watching it turns "a script was added
        // somewhere we do not list" into a document re-parse.
        for root in self.lsp.rescan_roots() {
            let cache = root.join(".godot");
            if cache.is_dir() {
                dirs.push(cache);
            }
        }
        dirs.sort_unstable();
        dirs.dedup();
        dirs.truncate(MAX_WATCH_DIRS);

        let mut events = Vec::new();
        let mut seen = HashSet::with_capacity(dirs.len());
        let mut budget = MAX_WATCH_ENTRIES;
        for dir in &dirs {
            let Some(current) = watch::snapshot(dir) else {
                continue;
            };
            budget = budget.saturating_sub(current.len());
            seen.insert(dir.clone());
            if let Some(previous) = self.watched_dirs.get(dir) {
                events.extend(watch::diff(previous, &current));
            }
            // The first sighting only records the baseline.
            self.watched_dirs.insert(dir.clone(), current);
            if budget == 0 {
                break;
            }
        }
        self.watched_dirs.retain(|dir, _| seen.contains(dir));
        if events.is_empty() {
            return;
        }

        // Only creates/deletes can change the row set; a content edit (our own
        // autosave, a formatter) must not re-read the whole visible tree.
        let structural = events
            .iter()
            .any(|event| event.kind != FileEventKind::Changed);
        if structural && self.ws.folder_open {
            self.ws.tree.refresh();
            // The file list is what Quick Open indexes.
            self.quick_open.index = None;
            self.sidebar.hover = None;
            if self
                .sidebar
                .selected
                .as_ref()
                .is_some_and(|selected| !selected.exists())
            {
                self.sidebar.selected = None;
            }
        }
        self.file_events.extend(events);
        self.dirty = true;
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
            root.join("igui-editor-watch-test.conf"),
        )
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-watch-{tag}-{}-{}",
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
    fn an_external_file_appears_in_the_tree_and_is_queued_for_the_servers() {
        let dir = temp("external");
        std::fs::write(dir.join("player.gd"), "extends Node\n").unwrap();
        let mut state = state(&dir);
        // Baseline snapshot (what the first poll after startup records).
        state.poll_filesystem();
        assert!(state.file_events.is_empty());

        std::fs::write(dir.join("player.gd.uid"), "uid://abc123\n").unwrap();
        state.poll_filesystem();

        let rows = state.ws.tree.rows();
        assert!(
            rows.iter().any(|row| row.name == "player.gd.uid"),
            "the external file must appear in the sidebar: {:?}",
            rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(
            state.file_events,
            vec![FileEvent {
                path: std::fs::canonicalize(&dir)
                    .unwrap_or_else(|_| dir.clone())
                    .join("player.gd.uid"),
                kind: FileEventKind::Created,
            }]
        );
        assert!(state.dirty);

        // A second poll without changes queues nothing new.
        state.poll_filesystem();
        assert_eq!(state.file_events.len(), 1);

        // Deleting it is reported too, and the row goes away.
        std::fs::remove_file(dir.join("player.gd.uid")).unwrap();
        state.poll_filesystem();
        assert!(state
            .file_events
            .iter()
            .any(|event| event.kind == FileEventKind::Deleted));
        assert!(!state
            .ws
            .tree
            .rows()
            .iter()
            .any(|row| row.name.ends_with(".uid")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_external_edit_is_reported_as_changed() {
        let dir = temp("changed");
        std::fs::write(dir.join("notes.txt"), "one\n").unwrap();
        let mut state = state(&dir);
        state.poll_filesystem();
        std::fs::write(dir.join("notes.txt"), "one\ntwo\n").unwrap();
        state.poll_filesystem();
        assert!(
            state
                .file_events
                .iter()
                .any(|event| event.kind == FileEventKind::Changed
                    && event.path.ends_with("notes.txt"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn editor_side_changes_are_queued_explicitly() {
        let dir = temp("editor");
        let mut state = state(&dir);
        state.note_file_event(dir.join("made.gd"), FileEventKind::Created);
        assert_eq!(state.file_events.len(), 1);
        assert_eq!(state.file_events[0].kind, FileEventKind::Created);
        let _ = std::fs::remove_dir_all(dir);
    }
}
