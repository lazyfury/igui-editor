//! Outline rows for the active document.
//!
//! LSP `textDocument/documentSymbol` results always win. Markdown files usually
//! have no language server, so their headings are derived by a line scanner and
//! cached by `(document id, revision)`: a repaint reuses the rows instead of
//! re-scanning the buffer.

use std::path::Path;

use super::*;
use crate::model::lsp::OutlineSymbol;
use crate::model::markdown;

/// Synthetic symbol kind for a Markdown heading row.
pub const MARKDOWN_HEADING_KIND: u32 = 100;

/// Cached Markdown heading rows for one document revision.
#[derive(Clone, Debug, Default)]
pub struct MarkdownOutline {
    key: Option<(u64, u64)>,
    rows: Vec<OutlineSymbol>,
}

impl MarkdownOutline {
    fn cached(&self, key: (u64, u64)) -> Option<&[OutlineSymbol]> {
        (self.key == Some(key)).then_some(self.rows.as_slice())
    }

    fn store(&mut self, key: (u64, u64), rows: Vec<OutlineSymbol>) {
        self.key = Some(key);
        self.rows = rows;
    }

    fn clear(&mut self) {
        if self.key.is_some() || !self.rows.is_empty() {
            self.key = None;
            self.rows.clear();
        }
    }
}

impl AppState {
    /// Outline rows for `path`: LSP symbols first, then Markdown headings.
    ///
    /// The Markdown rows are cached for the active document revision and only
    /// derived when that document revision has changed.
    pub fn outline_symbols_for(&mut self, path: &Path) -> Vec<OutlineSymbol> {
        let lsp = self.lsp.symbols_for(path);
        if !lsp.is_empty() {
            let rows = lsp.to_vec();
            self.outline.markdown.clear();
            return rows;
        }
        if !markdown::is_markdown_path(path) {
            self.outline.markdown.clear();
            return Vec::new();
        }
        let Some(doc) = self.ws.active_doc() else {
            self.outline.markdown.clear();
            return Vec::new();
        };
        if doc.path() != Some(path) {
            self.outline.markdown.clear();
            return Vec::new();
        }
        let key = (doc.id(), doc.rev());
        if let Some(rows) = self.outline.markdown.cached(key) {
            return rows.to_vec();
        }
        let headings = markdown::headings(doc.text());
        let rows: Vec<OutlineSymbol> = headings
            .into_iter()
            .map(|heading| OutlineSymbol {
                name: heading.text,
                kind: MARKDOWN_HEADING_KIND,
                depth: heading.level.saturating_sub(1) as usize,
                line: heading.line as u32,
                character: 0,
            })
            .collect();
        self.outline.markdown.store(key, rows.clone());
        rows
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
