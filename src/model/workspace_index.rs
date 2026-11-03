//! A flat file index for Quick Open.
//!
//! Built on demand from the workspace root: hidden entries and common build/VCS
//! directories are skipped, the walk is capped (files and depth) and results are
//! ranked by [`super::fuzzy`].

use std::path::{Component, Path, PathBuf};

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
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
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
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.is_dir() {
                    if IGNORED_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    stack.push((entry.path(), depth + 1));
                } else if metadata.is_file() {
                    files.push(entry.path());
                    if files.len() >= MAX_FILES {
                        break;
                    }
                }
            }
        }
        Self { root, files }
    }

    /// Ranks files for `query`, best first (workspace-relative paths shown).
    pub fn query(&self, query: &str, recent: impl Fn(&Path) -> bool) -> Vec<(PathBuf, i32)> {
        let relative: Vec<(PathBuf, String)> = self
            .files
            .iter()
            .map(|path| {
                let display = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned();
                (path.clone(), display)
            })
            .collect();
        let mut scored: Vec<(PathBuf, i32)> = relative
            .iter()
            .filter_map(|(path, display)| {
                fuzzy::score(query, display, recent(path)).map(|score| (path.clone(), score))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.truncate(MAX_RESULTS);
        scored
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
