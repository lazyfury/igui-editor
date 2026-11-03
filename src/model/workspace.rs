//! Tabs + the sidebar tree: what the shell is looking at.
//!
//! `Workspace` is also where file IO lives (`open`, `save`), so the UI layer
//! never touches `std::fs`.

use std::path::{Path, PathBuf};

use super::doc::Document;
use super::fs_tree::FsTree;

/// The open tabs and the directory they live in.
/// Files above this size are refused (the editor is not built for them).
pub const MAX_OPEN_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub tree: FsTree,
    pub tabs: Vec<Document>,
    pub active: usize,
    untitled_seq: usize,
    next_id: u64,
}

impl Workspace {
    /// A workspace rooted at `root`, with no tabs.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let tree = FsTree::new(&root);
        Self {
            root,
            tree,
            tabs: Vec::new(),
            active: 0,
            untitled_seq: 1,
            next_id: 1,
        }
    }

    /// A workspace rooted at the file's parent, with that file open.
    pub fn with_file(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut workspace = Self::new(root);
        workspace.open(path)?;
        Ok(workspace)
    }

    /// Opens `path` in a new tab (or focuses the tab that already has it).
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<usize, String> {
        let path = path.into();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path() == Some(path.as_path()))
        {
            self.active = index;
            return Ok(index);
        }
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.len() > MAX_OPEN_BYTES {
                return Err(format!(
                    "{} is too large ({} MB limit)",
                    path.display(),
                    MAX_OPEN_BYTES / (1024 * 1024)
                ));
            }
        }
        let id = self.take_id();
        let doc =
            Document::open(id, &path).map_err(|error| format!("{}: {error}", path.display()))?;
        // Make sure the file is visible in the sidebar (expand its ancestors).
        if path.starts_with(&self.root) {
            if let Some(parent) = path.parent() {
                self.tree.expand_to(parent);
            }
        }
        self.tabs.push(doc);
        self.active = self.tabs.len() - 1;
        Ok(self.active)
    }

    /// A fresh untitled buffer, focused.
    pub fn new_tab(&mut self) -> usize {
        let id = self.take_id();
        let doc = Document::untitled(id, self.untitled_seq);
        self.untitled_seq += 1;
        self.tabs.push(doc);
        self.active = self.tabs.len() - 1;
        self.active
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Focuses tab `index` (no-op when out of range).
    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn active_doc(&self) -> Option<&Document> {
        self.tabs.get(self.active)
    }

    pub fn active_doc_mut(&mut self) -> Option<&mut Document> {
        self.tabs.get_mut(self.active)
    }

    /// Saves the active tab under `path` (Save As).
    pub fn save_active_as(&mut self, path: impl Into<PathBuf>) -> Result<String, String> {
        let path = canonical_target(path.into());
        let Some(doc) = self.active_doc_mut() else {
            return Err("no document to save".into());
        };
        doc.set_path(path);
        let name = doc.name().to_string();
        doc.save()
            .map(|()| format!("saved {name}"))
            .map_err(|error| format!("save failed: {error}"))
    }

    /// Removes `index` regardless of the dirty flag (the UI confirmed first).
    pub fn force_close(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            self.validate_active();
        }
    }

    /// Keeps `active` in range after the tab list changed.
    pub fn validate_active(&mut self) {
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Saves tab `index` (used by the close-confirmation flow).
    pub fn save_tab(&mut self, index: usize) -> Result<String, String> {
        let Some(doc) = self.tabs.get_mut(index) else {
            return Err("no such tab".into());
        };
        let name = doc.name().to_string();
        doc.save()
            .map(|()| format!("saved {name}"))
            .map_err(|error| format!("save failed: {error}"))
    }

    /// Saves the active tab, returning the status-bar message.
    pub fn save_active(&mut self) -> Result<String, String> {
        let Some(doc) = self.active_doc_mut() else {
            return Err("no document to save".into());
        };
        let name = doc.name().to_string();
        doc.save()
            .map(|()| format!("saved {name}"))
            .map_err(|error| format!("save failed: {error}"))
    }
}

/// Canonical directory + the (possibly not yet existing) file name, so a path
/// chosen through a dialog matches the canonical paths used everywhere else
/// (macOS `/var` vs `/private/var`).
fn canonical_target(path: PathBuf) -> PathBuf {
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(parent) = std::fs::canonicalize(parent) {
            return parent.join(name);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("igui-editor-ws-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn open_edit_save_round_trips() {
        let dir = temp_dir("save");
        let path = dir.join("note.md");
        fs::write(&path, "# hi\n").unwrap();

        let mut workspace = Workspace::with_file(&path).unwrap();
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_doc().unwrap().text(), "# hi\n");
        assert!(!workspace.active_doc().unwrap().is_dirty());

        let doc = workspace.active_doc_mut().unwrap();
        doc.set_caret(doc.text().len());
        doc.insert_text("body\n");
        assert!(doc.is_dirty());
        assert_eq!(workspace.save_active().unwrap(), "saved note.md");
        assert!(!workspace.active_doc().unwrap().is_dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# hi\nbody\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_the_same_file_twice_focuses_the_existing_tab() {
        let dir = temp_dir("dupe");
        let path = dir.join("a.txt");
        fs::write(&path, "a").unwrap();
        let mut workspace = Workspace::with_file(&path).unwrap();
        workspace.new_tab();
        assert_eq!(workspace.tabs.len(), 2);
        let index = workspace.open(&path).unwrap();
        assert_eq!(index, 0);
        assert_eq!(workspace.tabs.len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn force_close_removes_even_a_dirty_tab() {
        let mut workspace = Workspace::new(std::env::temp_dir());
        workspace.new_tab();
        workspace.active_doc_mut().unwrap().insert_text("x");
        workspace.force_close(0);
        assert!(workspace.tabs.is_empty());
        assert_eq!(workspace.active, 0);
    }

    #[test]
    fn untitled_tabs_are_numbered() {
        let mut workspace = Workspace::new(std::env::temp_dir());
        workspace.new_tab();
        workspace.new_tab();
        let names: Vec<&str> = workspace.tabs.iter().map(|tab| tab.name()).collect();
        assert_eq!(names, ["untitled-1", "untitled-2"]);
        assert_eq!(workspace.active, 1);
    }
}
