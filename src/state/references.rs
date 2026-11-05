//! Symbol references: the `Fn`/`Shift+F12` result list.
//!
//! The language server returns locations; previews are filled once when the
//! response arrives (never per frame) from an open buffer or a bounded disk
//! read. The overlay itself is a plain list: arrows move, Enter opens, Esc
//! closes.

use super::*;

/// Rows kept from one `textDocument/references` response.
const MAX_REFERENCE_ROWS: usize = 500;
/// Files larger than this are not read just to preview a reference line.
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ReferenceRow {
    pub location: model_lsp::Location,
    /// Workspace-relative path and 1-based line.
    pub label: String,
    pub preview: String,
}

#[derive(Clone, Debug, Default)]
pub struct ReferencesUi {
    pub open: bool,
    pub symbol: String,
    pub rows: Vec<ReferenceRow>,
    pub selected: usize,
    pub scroll: usize,
    /// Painted panel rectangle (wheel/hit-testing).
    pub rect: Rect,
}

impl AppState {
    /// `Shift+F12`: ask for every reference to the identifier at the caret.
    pub fn request_lsp_references(&mut self) {
        self.lsp.dismiss_transient();
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let Some(symbol) = word_at(doc, doc.caret()) else {
            self.status("place the caret on a symbol first");
            return;
        };
        if !self.lsp.capabilities().references {
            self.status("LSP: this server has no references support");
            return;
        }
        self.references.open = false;
        self.references.rows.clear();
        if self.lsp.request_references(doc, &symbol) {
            self.status(format!("finding references to {symbol}…"));
            self.dirty = true;
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    /// Builds the overlay from a server response (called once per result).
    pub(crate) fn show_references(&mut self, symbol: String, locations: Vec<model_lsp::Location>) {
        if locations.is_empty() {
            self.status(format!("no references to {symbol}"));
            self.dirty = true;
            return;
        }
        let root = self.ws.root.clone();
        let mut cache: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
        let mut rows = Vec::new();
        for location in locations.into_iter().take(MAX_REFERENCE_ROWS) {
            let display = crate::model::workspace_index::display_path(&root, &location.path);
            let preview = line_preview(&self.ws, &location.path, location.line, &mut cache);
            rows.push(ReferenceRow {
                label: format!("{display}:{}", location.line + 1),
                preview,
                location,
            });
        }
        let count = rows.len();
        self.references = ReferencesUi {
            open: true,
            symbol: symbol.clone(),
            rows,
            selected: 0,
            scroll: 0,
            rect: Rect::ZERO,
        };
        self.status(format!("{count} references to {symbol}"));
        self.dirty = true;
    }

    pub fn close_references(&mut self) {
        if self.references.open {
            self.references.open = false;
            self.references.rows.clear();
            self.dirty = true;
        }
    }

    pub fn move_references(&mut self, delta: isize) {
        if self.references.rows.is_empty() {
            return;
        }
        let last = self.references.rows.len() as isize - 1;
        self.references.selected =
            (self.references.selected as isize + delta).clamp(0, last) as usize;
        self.dirty = true;
    }

    pub fn open_reference(&mut self, index: usize) {
        let Some(row) = self.references.rows.get(index).cloned() else {
            return;
        };
        let label = row.label.clone();
        self.close_references();
        self.open_lsp_location(row.location);
        self.status(format!("reference: {label}"));
    }

    /// Wheel handling for the references list.
    pub fn scroll_references(&mut self, delta_y: f32, visible: usize) {
        let max = self.references.rows.len().saturating_sub(visible.max(1)) as f32;
        self.references.scroll = (self.references.scroll as f32 + delta_y).clamp(0.0, max) as usize;
        self.dirty = true;
    }
}

/// Identifier at `caret` (byte offset), if any.
pub(crate) fn word_at(doc: &Document, caret: usize) -> Option<String> {
    let text = doc.text();
    let caret = caret.min(text.len());
    let mut start = caret;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if !(ch.is_alphanumeric() || ch == '_') {
            break;
        }
        start -= ch.len_utf8();
    }
    let mut end = caret;
    while end < text.len() {
        let ch = text[end..].chars().next()?;
        if !(ch.is_alphanumeric() || ch == '_') {
            break;
        }
        end += ch.len_utf8();
    }
    (start < end).then(|| text[start..end].to_string())
}

/// One-line preview for a reference. Open tabs win over disk, and each file is
/// read at most once per response.
fn line_preview(
    ws: &Workspace,
    path: &Path,
    line: u32,
    cache: &mut HashMap<PathBuf, Option<Vec<String>>>,
) -> String {
    if let Some(doc) = ws.tabs.iter().find(|doc| doc.path() == Some(path)) {
        return doc.line(line as usize).trim().to_string();
    }
    let lines = cache.entry(path.to_path_buf()).or_insert_with(|| {
        let metadata = std::fs::metadata(path).ok()?;
        if metadata.len() > MAX_PREVIEW_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        Some(text.lines().map(str::to_string).collect())
    });
    lines
        .as_ref()
        .and_then(|lines| lines.get(line as usize))
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str, caret: usize) -> Document {
        let mut doc = Document::from_snapshot(
            1,
            "t.gd".into(),
            None,
            text.to_string(),
            caret,
            caret,
            crate::model::doc::LineEnding::Lf,
        );
        doc.set_caret(caret);
        doc
    }

    #[test]
    fn word_at_reads_the_identifier_around_the_caret() {
        let text = "var player := $Player\n";
        assert_eq!(word_at(&doc(text, 0), 0).as_deref(), Some("var"));
        assert_eq!(
            word_at(
                &doc(text, text.find("Player").unwrap() + 2),
                text.find("Player").unwrap() + 2
            )
            .as_deref(),
            Some("Player")
        );
        assert_eq!(word_at(&doc("  ", 1), 1), None);
        assert_eq!(word_at(&doc("x", 1), 1).as_deref(), Some("x"));
    }
}
