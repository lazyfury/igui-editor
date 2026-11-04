//! Lazy directory tree for the sidebar.
//!
//! Only the root is read up front; expanding a directory reads it the first
//! time. Hidden entries are skipped, directories sort before files, and both
//! groups sort case-insensitively — the ordering a file browser implies.

use std::fs;
use std::path::{Path, PathBuf};

use super::watch::MAX_DIR_ENTRIES;

/// A flattened, visible row (what the sidebar paints and hit-tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
struct FsNode {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    children: Option<Vec<FsNode>>,
}

/// The sidebar's tree: a root directory plus lazily loaded children.
#[derive(Clone, Debug)]
pub struct FsTree {
    pub root: PathBuf,
    nodes: Vec<FsNode>,
    /// Flattened visible rows, rebuilt only when the tree changes. Paint and
    /// hit-testing borrow this slice instead of cloning the whole tree.
    flat: Vec<Row>,
    /// `read_dir` failure on the root, surfaced in the sidebar.
    pub error: Option<String>,
}

impl FsTree {
    /// An empty tree for "no project open": nothing to list, no IO.
    pub fn empty() -> Self {
        Self {
            root: PathBuf::new(),
            nodes: Vec::new(),
            flat: Vec::new(),
            error: None,
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut tree = Self {
            root,
            nodes: Vec::new(),
            flat: Vec::new(),
            error: None,
        };
        tree.reload();
        tree
    }

    /// Re-reads the root (and drops every expansion).
    pub fn reload(&mut self) {
        match read_dir(&self.root, 0) {
            Ok(nodes) => {
                self.nodes = nodes;
                self.error = None;
            }
            Err(error) => {
                self.nodes.clear();
                self.error = Some(error);
            }
        }
        self.rebuild_flat();
    }

    /// Re-reads the directories that are already loaded, keeping every
    /// expansion. Returns whether the visible rows changed.
    ///
    /// This is the external-change refresh: `reload` resets the tree (used when
    /// the project changes), `refresh` is what a file created by another tool
    /// must not do to the user's expanded folders.
    pub fn refresh(&mut self) -> bool {
        let previous = std::mem::take(&mut self.flat);
        match reload_loaded(&self.root, 0, &self.nodes) {
            Ok(nodes) => {
                self.nodes = nodes;
                self.error = None;
            }
            // Keep the last good tree when the root cannot be read.
            Err(error) => self.error = Some(error),
        }
        self.rebuild_flat();
        self.flat != previous
    }

    /// Directories whose children are loaded: the root plus every expanded
    /// node. The external-change poll snapshots exactly these.
    pub fn watch_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if !self.root.as_os_str().is_empty() {
            out.push(self.root.clone());
            collect_watch_dirs(&self.nodes, &mut out);
        }
        out
    }

    /// Expands or collapses the directory at `path`.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let changed = toggle_in(&mut self.nodes, path);
        if changed {
            self.rebuild_flat();
        }
        changed
    }

    /// Expands every directory between the root and `path` so it is visible.
    pub fn expand_to(&mut self, path: &Path) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }
        let changed = expand_ancestors(&mut self.nodes, path);
        if changed {
            self.rebuild_flat();
        }
        changed
    }

    /// Every visible row, in paint/hit-test order.
    pub fn rows(&self) -> &[Row] {
        &self.flat
    }

    fn rebuild_flat(&mut self) {
        self.flat.clear();
        let Self { nodes, flat, .. } = self;
        flatten(nodes, flat);
    }
}

fn flatten(nodes: &[FsNode], rows: &mut Vec<Row>) {
    for node in nodes {
        rows.push(Row {
            path: node.path.clone(),
            name: node.name.clone(),
            depth: node.depth,
            is_dir: node.is_dir,
            expanded: node.expanded,
        });
        if node.expanded {
            if let Some(children) = &node.children {
                flatten(children, rows);
            }
        }
    }
}

fn toggle_in(nodes: &mut [FsNode], path: &Path) -> bool {
    for node in nodes {
        if node.path == path {
            if !node.is_dir {
                return false;
            }
            node.expanded = !node.expanded;
            if node.expanded && node.children.is_none() {
                node.children = read_dir(&node.path, node.depth + 1).ok();
            }
            return true;
        }
        if node.expanded {
            if let Some(children) = &mut node.children {
                if toggle_in(children, path) {
                    return true;
                }
            }
        }
    }
    false
}

fn read_dir(path: &Path, depth: usize) -> Result<Vec<FsNode>, String> {
    let entries = fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut nodes = Vec::new();
    for entry in entries.flatten().take(MAX_DIR_ENTRIES) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hidden files stay hidden, like every editor's sidebar
        }
        // `metadata` follows symlinks; a dangling link is simply skipped.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let is_dir = metadata.is_dir();
        nodes.push(FsNode {
            path: entry.path(),
            name,
            is_dir,
            depth,
            expanded: false,
            children: None,
        });
    }
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(nodes)
}

/// Rebuilds one directory level, carrying over `expanded`/loaded children from
/// `previous` so a refresh never collapses the tree.
fn reload_loaded(path: &Path, depth: usize, previous: &[FsNode]) -> Result<Vec<FsNode>, String> {
    let mut nodes = read_dir(path, depth)?;
    for node in &mut nodes {
        if !node.is_dir {
            continue;
        }
        let Some(old) = previous.iter().find(|old| old.path == node.path) else {
            continue;
        };
        node.expanded = old.expanded;
        if let Some(children) = old.children.as_deref() {
            if let Ok(children) = reload_loaded(&node.path, depth + 1, children) {
                node.children = Some(children);
            }
        }
    }
    Ok(nodes)
}

fn collect_watch_dirs(nodes: &[FsNode], out: &mut Vec<PathBuf>) {
    for node in nodes {
        if !node.is_dir || node.children.is_none() {
            continue;
        }
        out.push(node.path.clone());
        collect_watch_dirs(node.children.as_deref().unwrap_or_default(), out);
    }
}

fn expand_ancestors(nodes: &mut [FsNode], path: &Path) -> bool {
    for node in nodes.iter_mut() {
        if !node.is_dir || !path.starts_with(&node.path) {
            continue;
        }
        let was_expanded = node.expanded;
        node.expanded = true;
        if node.children.is_none() {
            node.children = read_dir(&node.path, node.depth + 1).ok();
        }
        let mut changed = !was_expanded;
        if node.path != path {
            if let Some(children) = &mut node.children {
                changed |= expand_ancestors(children, path);
            }
        }
        return changed;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-fstree-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        fs::write(dir.join("a.md"), "a").unwrap();
        fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        dir
    }

    #[test]
    fn dirs_sort_before_files_and_hidden_entries_are_skipped() {
        let dir = temp_dir("sort");
        let tree = FsTree::new(&dir);
        let names: Vec<String> = tree.rows().iter().map(|row| row.name.clone()).collect();
        assert_eq!(names, ["src", "a.md", "b.txt"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn expand_is_lazy_and_toggles() {
        let dir = temp_dir("expand");
        let mut tree = FsTree::new(&dir);
        assert_eq!(tree.rows().len(), 3);
        let src = dir.join("src");
        assert!(tree.toggle(&src));
        let rows = tree.rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].name, "main.rs");
        assert_eq!(rows[1].depth, 1);
        assert!(tree.toggle(&src));
        assert_eq!(tree.rows().len(), 3);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_keeps_expansions_and_picks_up_external_files() {
        let dir = temp_dir("refresh");
        let mut tree = FsTree::new(&dir);
        let src = dir.join("src");
        assert!(tree.toggle(&src));
        assert!(tree.rows().iter().any(|row| row.name == "main.rs"));

        // A tool creates a file next to the expanded folder and inside it.
        fs::write(dir.join("new.gd.uid"), "uid://x").unwrap();
        fs::write(src.join("player.gd"), "extends Node").unwrap();
        assert!(tree.refresh());

        let rows = tree.rows();
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(names.contains(&"new.gd.uid"), "{names:?}");
        assert!(
            names.contains(&"player.gd"),
            "the expanded folder stays open"
        );
        let player = rows
            .iter()
            .find(|row| row.name == "player.gd")
            .expect("player.gd row");
        assert_eq!(player.depth, 1);

        // Nothing changed: refresh reports no visible change.
        assert!(!tree.refresh());

        // Deleting the file removes its row without collapsing `src`.
        fs::remove_file(src.join("player.gd")).unwrap();
        assert!(tree.refresh());
        assert!(!tree.rows().iter().any(|row| row.name == "player.gd"));
        assert!(tree.rows().iter().any(|row| row.name == "main.rs"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn watch_dirs_lists_the_root_and_loaded_folders() {
        let dir = temp_dir("watch");
        let mut tree = FsTree::new(&dir);
        assert_eq!(tree.watch_dirs(), vec![dir.clone()]);
        tree.toggle(&dir.join("src"));
        assert!(tree.watch_dirs().iter().any(|path| path.ends_with("src")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toggling_a_file_does_nothing() {
        let dir = temp_dir("file");
        let mut tree = FsTree::new(&dir);
        assert!(!tree.toggle(&dir.join("a.md")));
        let _ = fs::remove_dir_all(dir);
    }
}
