//! Outline rows for the active document.
//!
//! LSP `textDocument/documentSymbol` results always win. Markdown headings and
//! GDScript declarations are derived by line scanners for files that usually
//! have no running language server, and cached by `(document id, revision)`:
//! a repaint reuses the rows instead of re-scanning the buffer. Rows are shared
//! through `Rc` so a repaint does not clone every row's name.

use std::path::Path;
use std::rc::Rc;

use super::*;
use crate::model::godot::{self, OutlineKind};
use crate::model::lsp::OutlineSymbol;
use crate::model::markdown;

/// Synthetic symbol kind for a Markdown heading row.
pub const MARKDOWN_HEADING_KIND: u32 = 100;

/// Cached native outline rows for one document revision.
#[derive(Clone, Debug, Default)]
pub struct NativeOutline {
    key: Option<(u64, u64)>,
    rows: Rc<Vec<OutlineSymbol>>,
}

impl NativeOutline {
    fn cached(&self, key: (u64, u64)) -> Option<Rc<Vec<OutlineSymbol>>> {
        (self.key == Some(key)).then(|| self.rows.clone())
    }

    fn store(&mut self, key: (u64, u64), rows: Rc<Vec<OutlineSymbol>>) {
        self.key = Some(key);
        self.rows = rows;
    }

    fn clear(&mut self) {
        if self.key.is_some() || !self.rows.is_empty() {
            self.key = None;
            self.rows = Rc::new(Vec::new());
        }
    }
}

impl AppState {
    /// Outline rows for `path`: LSP symbols first, then the native fallbacks
    /// (Markdown headings, GDScript declarations).
    ///
    /// The native rows are cached for the active document revision and only
    /// derived when that document revision has changed.
    pub fn outline_symbols_for(&mut self, path: &Path) -> Rc<Vec<OutlineSymbol>> {
        let lsp = self.lsp.symbols_for(path);
        if !lsp.is_empty() {
            let rows = Rc::new(lsp.to_vec());
            self.outline.native.clear();
            return rows;
        }
        let Some(doc) = self.ws.active_doc() else {
            self.outline.native.clear();
            return Rc::new(Vec::new());
        };
        if doc.path() != Some(path) {
            self.outline.native.clear();
            return Rc::new(Vec::new());
        }
        let key = (doc.id(), doc.rev());
        if let Some(rows) = self.outline.native.cached(key) {
            return rows;
        }
        let rows: Vec<OutlineSymbol> = if markdown::is_markdown_path(path) {
            markdown::headings(doc.text())
                .into_iter()
                .map(|heading| OutlineSymbol {
                    name: heading.text,
                    kind: MARKDOWN_HEADING_KIND,
                    depth: heading.level.saturating_sub(1) as usize,
                    line: heading.line as u32,
                    character: 0,
                })
                .collect()
        } else if is_gdscript(path) {
            godot::outline(doc.text())
                .into_iter()
                .map(|row| OutlineSymbol {
                    name: row.name,
                    kind: godot_kind(row.kind),
                    depth: row.depth,
                    line: row.line,
                    character: 0,
                })
                .collect()
        } else {
            self.outline.native.clear();
            return Rc::new(Vec::new());
        };
        let rows = Rc::new(rows);
        self.outline.native.store(key, rows.clone());
        rows
    }
}

/// Whether `path` is a GDScript source file.
fn is_gdscript(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gd"))
}

/// Maps the native GDScript kinds onto LSP `SymbolKind` numbers so the shared
/// outline renderer gives them the same marker and color as server symbols.
fn godot_kind(kind: OutlineKind) -> u32 {
    match kind {
        OutlineKind::Class => 5,
        OutlineKind::Function => 12,
        OutlineKind::Signal => 24,
        OutlineKind::Constant => 14,
        OutlineKind::Variable => 13,
        OutlineKind::Enum => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::theme::Theme;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-outline-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn open(dir: &Path, name: &str, text: &str) -> (AppState, PathBuf) {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        let mut ws = Workspace::new(dir);
        ws.new_tab();
        let mut state = AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            dir.join("session.conf"),
        );
        state.open_path(path);
        let path = state
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(Path::to_path_buf)
            .expect("the Markdown fixture is open");
        (state, path)
    }

    #[test]
    fn markdown_headings_become_outline_rows_without_an_lsp() {
        let dir = temp("rows");
        let (mut state, path) = open(
            &dir,
            "notes.md",
            "# One\n\nBody\n\n## Two ##\n\n### Three\n",
        );
        let rows = state.outline_symbols_for(&path);
        let summary: Vec<(u32, usize, u32, u32, &str)> = rows
            .iter()
            .map(|row| {
                (
                    row.kind,
                    row.depth,
                    row.line,
                    row.character,
                    row.name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (MARKDOWN_HEADING_KIND, 0, 0, 0, "One"),
                (MARKDOWN_HEADING_KIND, 1, 4, 0, "Two"),
                (MARKDOWN_HEADING_KIND, 2, 6, 0, "Three"),
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn editing_a_heading_invalidates_the_cached_rows() {
        let dir = temp("cache");
        let (mut state, path) = open(&dir, "notes.md", "# One\n\n## Two\n");
        assert_eq!(
            state
                .outline_symbols_for(&path)
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["One", "Two"]
        );
        let before = state.ws.active_doc().unwrap().rev();
        let len = state.ws.active_doc().unwrap().text().len();
        assert!(state
            .ws
            .active_doc_mut()
            .unwrap()
            .replace_range(2, len, "Uno\n\n## Dos\n"));
        let after = state.ws.active_doc().unwrap().rev();
        assert_ne!(before, after);
        assert_eq!(
            state
                .outline_symbols_for(&path)
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Uno", "Dos"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gdscript_declarations_become_outline_rows_without_an_lsp() {
        let dir = temp("godot");
        let (mut state, path) = open(
            &dir,
            "brick.gd",
            "class_name Brick\nextends Node\n\nsignal died\n\
             @export var health = 100\n\nfunc _ready():\n\tvar local = 2\n\
             \nfunc hit(amount):\n\tpass\n",
        );
        let rows = state.outline_symbols_for(&path);
        let summary: Vec<(&str, u32, usize, u32)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.kind, row.depth, row.line))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Brick", 5, 0, 0),
                ("died", 24, 0, 3),
                ("health", 13, 0, 4),
                ("_ready", 12, 0, 6),
                ("hit", 12, 0, 9),
            ]
        );
        // Function locals are not members.
        assert!(rows.iter().all(|row| row.name != "local"));

        // The native rows follow edits through the revision cache.
        let len = state.ws.active_doc().unwrap().text().len();
        assert!(state.ws.active_doc_mut().unwrap().replace_range(
            0,
            len,
            "func changed():\n\tpass\n"
        ));
        let rows = state.outline_symbols_for(&path);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "changed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_markdown_and_headingless_documents_stay_empty() {
        let dir = temp("empty");
        let (mut state, text_path) = open(&dir, "notes.txt", "# Not an outline\n");
        assert!(state.outline_symbols_for(&text_path).is_empty());

        let (mut state, md_path) = open(&dir, "plain.md", "Just a paragraph.\n");
        assert!(state.outline_symbols_for(&md_path).is_empty());

        let (mut state, md_path) = open(&dir, "heads.md", "# Heading\n");
        assert!(!state.outline_symbols_for(&md_path).is_empty());
        assert!(state.outline_symbols_for(&dir.join("other.md")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
