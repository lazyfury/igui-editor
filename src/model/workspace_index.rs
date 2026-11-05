//! A flat file index for Quick Open.
//!
//! Built on demand from the workspace root: hidden entries and common build/VCS
//! directories are skipped, the walk is capped (files and depth) and results are
//! ranked by [`super::fuzzy`].

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use super::fuzzy;

/// Directories never indexed.
pub const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
    "vendor",
    ".idea",
    ".vscode",
];

/// Maximum indexed files.
pub const MAX_FILES: usize = 20_000;
/// Maximum directory depth below the root.
pub const MAX_DEPTH: usize = 12;
/// Maximum results returned for one query.
pub const MAX_RESULTS: usize = 50;

/// A snapshot of the workspace files.
///
/// `displays` and `lower` are parallel to `files` and computed once at build
/// time. Quick Open re-ranks the index on every keystroke, so rebuilding
/// relative paths and lowercasing 20k paths per keypress would dominate the
/// palette's cost.
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub displays: Vec<String>,
    pub lower: Vec<String>,
}

impl FileIndex {
    /// Walks `root`, skipping ignored/hidden entries, capped at [`MAX_FILES`].
    pub fn build(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut files = Vec::new();
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_DEPTH || files.len() >= MAX_FILES {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                // `file_type` answers the normal dir/file question without a
                // stat per entry. Symlinks keep the previous `metadata()`
                // fallback so their handling does not change.
                let (is_dir, is_file) = if file_type.is_symlink() {
                    match entry.metadata() {
                        Ok(metadata) => (metadata.is_dir(), metadata.is_file()),
                        Err(_) => continue,
                    }
                } else {
                    (file_type.is_dir(), file_type.is_file())
                };
                if is_dir {
                    if IGNORED_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    stack.push((entry.path(), depth + 1));
                } else if is_file {
                    files.push(entry.path());
                    if files.len() >= MAX_FILES {
                        break;
                    }
                }
            }
        }
        let mut displays = Vec::with_capacity(files.len());
        let mut lower = Vec::with_capacity(files.len());
        for path in &files {
            let display = display_path(&root, path);
            lower.push(display.to_lowercase());
            displays.push(display);
        }
        Self {
            root,
            files,
            displays,
            lower,
        }
    }

    /// Ranks files for `query`, best first (workspace-relative paths shown).
    /// Only the capped results clone their `PathBuf`; the index data itself is
    /// borrowed for scoring.
    pub fn query(&self, query: &str, recent: impl Fn(&Path) -> bool) -> Vec<(PathBuf, i32)> {
        let query_lower = query.to_lowercase();
        let mut scored: Vec<(&PathBuf, i32)> = self
            .files
            .iter()
            .zip(&self.displays)
            .zip(&self.lower)
            .filter_map(|((path, display), lower)| {
                let recent = recent(path);
                let score = if query_lower.is_empty() {
                    // Keep the convenience API on the empty-query path; it
                    // returns before allocating a lowercase copy.
                    fuzzy::score(query, display, recent)
                } else {
                    fuzzy::score_lower(&query_lower, lower, recent)
                };
                score.map(|score| (path, score))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        scored.truncate(MAX_RESULTS);
        scored
            .into_iter()
            .map(|(path, score)| (path.clone(), score))
            .collect()
    }
}

/// Builds one [`FileIndex`] on a background thread and sends it back.
///
/// The worker owns no cancellation flag: dropping it drops the receiver, the
/// thread's `send` fails, and the thread exits without touching the UI.
pub struct IndexWorker {
    root: PathBuf,
    rx: Receiver<FileIndex>,
}

impl IndexWorker {
    /// Starts indexing `root` on a new thread.
    pub fn spawn(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let (tx, rx) = mpsc::channel();
        let worker_root = root.clone();
        std::thread::spawn(move || {
            let index = FileIndex::build(worker_root);
            let _ = tx.send(index);
        });
        Self { root, rx }
    }

    /// The root this worker was started for.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Non-blocking receive of the finished index.
    pub fn try_recv(&self) -> Option<FileIndex> {
        self.rx.try_recv().ok()
    }
}

/// Workspace-relative display for a path (falls back to the full path).
pub fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-index-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("README.md"), "readme").unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("src/nested/lib.rs"), "").unwrap();
        fs::write(dir.join("target/debug/artifact"), "").unwrap();
        fs::write(dir.join(".hidden"), "").unwrap();
        dir
    }

    #[test]
    fn skips_ignored_and_hidden_entries() {
        let dir = fixture("walk");
        let index = FileIndex::build(&dir);
        let names: Vec<String> = index
            .files
            .iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.contains(&"README.md".to_string()));
        assert!(names.contains(&"src/main.rs".to_string()));
        assert!(names.contains(&"src/nested/lib.rs".to_string()));
        assert!(!names.iter().any(|name| name.starts_with("target")));
        assert!(!names.iter().any(|name| name.contains(".git")));
        assert!(!names.iter().any(|name| name.contains(".hidden")));
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entries_keep_the_previous_walk_behavior() {
        use std::os::unix::fs::symlink;

        let dir = fixture("symlink");
        symlink(dir.join("src"), dir.join("linked-src")).unwrap();
        symlink(dir.join("README.md"), dir.join("linked-readme.md")).unwrap();
        let index = FileIndex::build(&dir);
        let names: Vec<String> = index
            .files
            .iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.contains(&"README.md".to_string()));
        assert!(names.contains(&"src/main.rs".to_string()));
        // `DirEntry::metadata` does not follow links, so symlinks stay skipped
        // exactly as they were before the `file_type()` fast path.
        assert!(!names.iter().any(|name| name.starts_with("linked-")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn worker_builds_the_index_and_stops_after_one_result() {
        let dir = fixture("worker");
        let worker = IndexWorker::spawn(dir.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut index = None;
        while std::time::Instant::now() < deadline {
            if let Some(built) = worker.try_recv() {
                index = Some(built);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let index = index.expect("the worker must send the index before the deadline");
        assert_eq!(index.root, dir);
        assert!(index.files.iter().any(|path| path.ends_with("main.rs")));
        assert!(worker.try_recv().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn index_caches_display_and_lowercase_paths() {
        let dir = fixture("cache");
        let index = FileIndex::build(&dir);
        assert_eq!(index.files.len(), index.displays.len());
        assert_eq!(index.files.len(), index.lower.len());
        assert!(index
            .displays
            .iter()
            .all(|display| !display.starts_with('/')));
        assert!(index
            .lower
            .iter()
            .all(|lower| *lower == lower.to_lowercase()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn query_ranks_by_file_name_and_respects_recent() {
        let dir = fixture("query");
        let index = FileIndex::build(&dir);
        let results = index.query("main", |_| false);
        assert!(results
            .first()
            .is_some_and(|(path, _)| path.ends_with("main.rs")));
        let recent = index.query("rs", |path| path.ends_with("lib.rs"));
        assert!(recent
            .first()
            .is_some_and(|(path, _)| path.ends_with("lib.rs")));
        assert!(index.query("no-such-file", |_| false).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn display_paths_are_relative_and_use_slashes() {
        let root = Path::new("/tmp/root");
        let path = Path::new("/tmp/root/src/main.rs");
        assert_eq!(display_path(root, path), "src/main.rs");
        assert_eq!(display_path(Path::new("/other"), Path::new("/x/y")), "x/y");
    }
}
