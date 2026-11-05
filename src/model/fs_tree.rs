//! Lazy directory tree for the sidebar.
//!
//! Only the root is read up front; expanding a directory reads it the first
//! time. Dot entries are hidden unless the user asks for them, generated/temp
//! files are shown dimmed or hidden by [`IgnoreMode`], directories sort before
//! files, and both groups sort case-insensitively — the ordering a file browser
//! implies.

use std::fs;
use std::path::{Path, PathBuf};

use super::ignore::{self, IgnoreMode};
use super::watch::MAX_DIR_ENTRIES;

/// What the Explorer shows at all, and how it marks tool noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    /// Include dot entries (`.git`, `.env`, …).
    pub show_hidden: bool,
    /// Presentation of generated/temp files ([`ignore::is_ignored`]).
    pub ignore: IgnoreMode,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            show_hidden: false,
            ignore: IgnoreMode::Dim,
        }
    }
}

/// A flattened, visible row (what the sidebar paints and hit-tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// Hidden dot entry or ignored tool file: paint it secondary.
    pub dim: bool,
}

#[derive(Clone, Debug)]
struct FsNode {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    dim: bool,
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
    filter: Filter,
}

impl FsTree {
    /// An empty tree for "no project open": nothing to list, no IO.
    pub fn empty() -> Self {
        Self {
            root: PathBuf::new(),
            nodes: Vec::new(),
            flat: Vec::new(),
            error: None,
            filter: Filter::default(),
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut tree = Self {
            root,
            nodes: Vec::new(),
            flat: Vec::new(),
            error: None,
            filter: Filter::default(),
        };
        tree.reload();
        tree
    }

    /// Re-reads the root (and drops every expansion).
    pub fn reload(&mut self) {
        match read_dir(&self.root, 0, self.filter) {
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

    /// Applies new visibility rules, keeping every expansion. Returns whether
    /// the visible rows changed.
    pub fn set_filter(&mut self, filter: Filter) -> bool {
        if self.filter == filter {
            return false;
        }
        self.filter = filter;
        if self.root.as_os_str().is_empty() {
            return false;
        }
        let previous = std::mem::take(&mut self.flat);
        match reload_loaded(&self.root, 0, &self.nodes, filter) {
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

    /// Re-reads the directories that are already loaded, keeping every
    /// expansion. Returns whether the visible rows changed.
    ///
    /// This is the external-change refresh: `reload` resets the tree (used when
    /// the project changes), `refresh` is what a file created by another tool
    /// must not do to the user's expanded folders.
    pub fn refresh(&mut self) -> bool {
        let previous = std::mem::take(&mut self.flat);
        match reload_loaded(&self.root, 0, &self.nodes, self.filter) {
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
        let changed = toggle_in(&mut self.nodes, path, self.filter);
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
        let changed = expand_ancestors(&mut self.nodes, path, self.filter);
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
            dim: node.dim,
        });
        if node.expanded {
            if let Some(children) = &node.children {
                flatten(children, rows);
            }
        }
    }
}

fn toggle_in(nodes: &mut [FsNode], path: &Path, filter: Filter) -> bool {
    for node in nodes {
        if node.path == path {
            if !node.is_dir {
                return false;
            }
            node.expanded = !node.expanded;
            if node.expanded && node.children.is_none() {
                node.children = read_dir(&node.path, node.depth + 1, filter).ok();
            }
            return true;
        }
        if node.expanded {
            if let Some(children) = &mut node.children {
                if toggle_in(children, path, filter) {
                    return true;
                }
            }
        }
    }
    false
}

fn read_dir(path: &Path, depth: usize, filter: Filter) -> Result<Vec<FsNode>, String> {
    let entries = fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut nodes = Vec::new();
    for entry in entries.flatten().take(MAX_DIR_ENTRIES) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.');
        if hidden && !filter.show_hidden {
            continue; // hidden files stay hidden unless the setting asks for them
        }
        let ignored = ignore::is_ignored(&name);
        if ignored && filter.ignore == IgnoreMode::Hide {
            continue;
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
            dim: hidden || (ignored && filter.ignore == IgnoreMode::Dim),
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
fn reload_loaded(
    path: &Path,
    depth: usize,
    previous: &[FsNode],
    filter: Filter,
) -> Result<Vec<FsNode>, String> {
    let mut nodes = read_dir(path, depth, filter)?;
    for node in &mut nodes {
        if !node.is_dir {
            continue;
        }
        let Some(old) = previous.iter().find(|old| old.path == node.path) else {
            continue;
        };
        node.expanded = old.expanded;
        if let Some(children) = old.children.as_deref() {
            if let Ok(children) = reload_loaded(&node.path, depth + 1, children, filter) {
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

fn expand_ancestors(nodes: &mut [FsNode], path: &Path, filter: Filter) -> bool {
    for node in nodes.iter_mut() {
        if !node.is_dir || !path.starts_with(&node.path) {
            continue;
        }
        let was_expanded = node.expanded;
        node.expanded = true;
        if node.children.is_none() {
            node.children = read_dir(&node.path, node.depth + 1, filter).ok();
        }
        let mut changed = !was_expanded;
        if node.path != path {
            if let Some(children) = &mut node.children {
                changed |= expand_ancestors(children, path, filter);
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
    fn show_hidden_reveals_dot_entries_and_keeps_expansions() {
        let dir = temp_dir("show-hidden");
        fs::write(dir.join(".env"), "SECRET=1").unwrap();
        let mut tree = FsTree::new(&dir);
        let src = dir.join("src");
        assert!(tree.toggle(&src));
        assert!(!tree.rows().iter().any(|row| row.name == ".env"));

        let filter = Filter {
            show_hidden: true,
            ignore: IgnoreMode::Dim,
        };
        assert!(tree.set_filter(filter));
        let rows = tree.rows();
        let env = rows
            .iter()
            .find(|row| row.name == ".env")
            .expect("the dot file appears when hidden entries are shown");
        assert!(env.dim, "a hidden row stays secondary");
        assert!(
            rows.iter().any(|row| row.name == ".hidden" && row.is_dir),
            "hidden directories are listed too"
        );
        assert!(
            rows.iter()
                .any(|row| row.name == "main.rs" && row.depth == 1),
            "applying the filter must not collapse the expansion"
        );

        assert!(
            tree.set_filter(Filter::default()),
            "hiding dot entries changes the rows"
        );
        assert!(!tree.rows().iter().any(|row| row.name == ".env"));
        assert!(tree.rows().iter().any(|row| row.name == "main.rs"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignored_tool_files_are_dimmed_or_hidden() {
        let dir = temp_dir("ignore");
        fs::write(dir.join("player.gd.uid"), "uid://x").unwrap();
        fs::write(dir.join("scratch.tmp"), "x").unwrap();
        let mut tree = FsTree::new(&dir);
        let dimmed = |tree: &FsTree, name: &str| {
            tree.rows()
                .iter()
                .find(|row| row.name == name)
                .map(|row| row.dim)
        };
        assert_eq!(dimmed(&tree, "player.gd.uid"), Some(true));
        assert_eq!(dimmed(&tree, "scratch.tmp"), Some(true));
        assert_eq!(dimmed(&tree, "a.md"), Some(false));

        assert!(tree.set_filter(Filter {
            show_hidden: false,
            ignore: IgnoreMode::Hide,
        }));
        assert_eq!(dimmed(&tree, "player.gd.uid"), None);
        assert_eq!(dimmed(&tree, "scratch.tmp"), None);
        assert_eq!(dimmed(&tree, "a.md"), Some(false));

        assert!(tree.set_filter(Filter {
            show_hidden: false,
            ignore: IgnoreMode::Off,
        }));
        assert_eq!(dimmed(&tree, "player.gd.uid"), Some(false));
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
