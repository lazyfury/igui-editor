//! The document model: text buffer, caret/selection and a delta undo stack.
//!
//! Pure Rust, no `igui_*` dependency, so every rule here is unit-testable
//! without a window. The view reads it and routes edits through these methods;
//! the view never owns text state.
//!
//! Conventions:
//!
//! - Offsets are **byte** offsets on `char` boundaries (same as `String`).
//! - A selection exists iff `caret != anchor`; the ordered range is
//!   `(min, max)`.
//! - Undo records a *delta* (`start`, `removed`, `inserted`), not a snapshot.
//!   Consecutive single-character typing / backspace / delete coalesce into one
//!   entry.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use super::{fsutil, grapheme};

/// How a document indents a new block level.
///
/// Detection is per file so an existing Godot script keeps its tabs (mixing
/// tabs and spaces is what GDScript rejects). The setting only supplies the
/// default for files with no indentation yet and the target of
/// [`Document::format_indentation`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndentStyle {
    Tabs,
    Spaces(u8),
}

impl IndentStyle {
    /// One indentation unit (a tab or `width` spaces).
    pub fn unit(self) -> String {
        match self {
            IndentStyle::Tabs => "\t".into(),
            IndentStyle::Spaces(width) => " ".repeat(width.max(1) as usize),
        }
    }

    /// Indentation actually used by `text`, or `None` when no line is indented.
    ///
    /// Tabs win ties; the space width is the gcd of the leading-space counts,
    /// which recovers 2/4/8 even when every line is indented two or three
    /// levels deep.
    pub fn detect(text: &str) -> Option<Self> {
        let mut tabs = 0usize;
        let mut spaces = 0usize;
        let mut width = 0usize;
        for line in text.lines() {
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            if ws_len == 0 || line[ws_len..].trim().is_empty() {
                continue;
            }
            let ws = &line[..ws_len];
            if ws.starts_with('\t') {
                tabs += 1;
            } else {
                spaces += 1;
                width = gcd(width, ws_len);
            }
        }
        if tabs == 0 && spaces == 0 {
            return None;
        }
        if tabs >= spaces {
            return Some(IndentStyle::Tabs);
        }
        let width = if (2..=8).contains(&width) { width } else { 4 };
        Some(IndentStyle::Spaces(width as u8))
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Caret plus the other end of the selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cursor {
    pub caret: usize,
    pub anchor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Type,
    Backspace,
    Delete,
    Other,
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    kind: Kind,
    before: Cursor,
    after: Cursor,
}

#[derive(Clone, Debug, Default)]
struct UndoStack {
    edits: Vec<Edit>,
    /// Number of applied edits (everything above is redo history).
    cursor: usize,
}

impl UndoStack {
    fn push(&mut self, edit: Edit) {
        self.edits.truncate(self.cursor);
        // Coalesce a run of like edits into the previous entry.
        if let Some(last) = self.edits.last_mut() {
            let coalesced = match edit.kind {
                Kind::Type => {
                    last.kind == Kind::Type
                        && last.inserted.len() == last.after.caret - last.start
                        && edit.start == last.start + last.inserted.len()
                        && !edit.inserted.contains('\n')
                        && !last.inserted.contains('\n')
                        && edit.removed.is_empty()
                }
                Kind::Backspace => {
                    last.kind == Kind::Backspace
                        && edit.start + edit.removed.len() == last.start
                        && !edit.removed.contains('\n')
                        && !last.removed.contains('\n')
                }
                Kind::Delete => {
                    last.kind == Kind::Delete
                        && edit.start == last.start
                        && !edit.removed.contains('\n')
                        && !last.removed.contains('\n')
                }
                Kind::Other => false,
            };
            if coalesced {
                match edit.kind {
                    Kind::Type => {
                        last.inserted.push_str(&edit.inserted);
                    }
                    Kind::Backspace => {
                        last.start = edit.start;
                        last.removed.insert_str(0, &edit.removed);
                    }
                    Kind::Delete => {
                        last.removed.push_str(&edit.removed);
                    }
                    Kind::Other => unreachable!(),
                }
                last.after = edit.after;
                self.cursor = self.edits.len();
                return;
            }
        }
        self.edits.push(edit);
        self.cursor = self.edits.len();
    }
}

/// Synthetic read-only views shown as normal editor tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Git,
    Docs,
}

/// Line ending used by the file on disk.
///
/// The in-memory buffer is always LF; the original style is restored on save so
/// editing a CRLF file does not rewrite every line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn detect(text: &str) -> Self {
        let bytes = text.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                return if index > 0 && bytes[index - 1] == b'\r' {
                    Self::Crlf
                } else {
                    Self::Lf
                };
            }
        }
        Self::Lf
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }

    pub fn from_name(value: &str) -> Self {
        if value.eq_ignore_ascii_case("crlf") {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    fn render(self, text: &str) -> Cow<'_, str> {
        match self {
            Self::Lf => Cow::Borrowed(text),
            Self::Crlf if text.contains('\n') => Cow::Owned(text.replace('\n', "\r\n")),
            Self::Crlf => Cow::Borrowed(text),
        }
    }
}

/// One open file (or an untitled buffer).
#[derive(Clone, Debug)]
pub struct Document {
    /// Stable per-workspace identity (UI state such as the view mode is keyed
    /// by it, so it survives renames and tab reordering).
    id: u64,
    path: Option<PathBuf>,
    name: String,
    text: String,
    caret: usize,
    anchor: usize,
    /// Byte offset of every line start; `line_count() == line_starts.len()`.
    line_starts: Vec<usize>,
    dirty: bool,
    /// Undo cursor at the last successful save. `None` means the buffer has
    /// never been saved (or a recovered snapshot replaced the buffer).
    saved_cursor: Option<usize>,
    /// Line ending used when writing the file back to disk.
    eol: LineEnding,
    /// Bumped by every text mutation (edit / undo / redo); caches keyed by it
    /// (find matches, Markdown layout, highlighting) know when to rebuild.
    rev: u64,
    /// Line range touched by the last mutation, for incremental highlighting.
    edit_span: Option<(usize, usize)>,
    undo: UndoStack,
    /// Per-document scroll (kept across tab switches).
    pub scroll_y: f32,
    pub scroll_x: f32,
    /// Git diff/commit views are read-only and are never persisted/saved.
    read_only: bool,
    view_key: Option<String>,
    view_kind: Option<ViewKind>,
    preview_start: Option<usize>,
    /// Indentation detected from the file, if it has any indented line.
    indent: Option<IndentStyle>,
}

impl Document {
    /// An empty, unsaved buffer named `untitled-<seq>`.
    pub fn untitled(id: u64, seq: usize) -> Self {
        Self {
            id,
            path: None,
            name: format!("untitled-{seq}"),
            text: String::new(),
            caret: 0,
            anchor: 0,
            line_starts: vec![0],
            dirty: false,
            saved_cursor: Some(0),
            eol: LineEnding::Lf,
            rev: 0,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            read_only: false,
            view_key: None,
            view_kind: None,
            preview_start: None,
            indent: None,
        }
    }

    /// Builds a dirty buffer from a recovery snapshot (no disk read).
    pub fn from_snapshot(
        id: u64,
        name: String,
        path: Option<PathBuf>,
        text: String,
        caret: usize,
        anchor: usize,
        eol: LineEnding,
    ) -> Self {
        let mut doc = Self {
            id,
            path,
            name,
            text,
            caret: 0,
            anchor: 0,
            line_starts: Vec::new(),
            dirty: true,
            saved_cursor: None,
            eol,
            rev: 1,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            read_only: false,
            view_key: None,
            view_kind: None,
            preview_start: None,
            indent: None,
        };
        doc.caret = floor_boundary(&doc.text, caret);
        doc.anchor = floor_boundary(&doc.text, anchor);
        doc.indent = IndentStyle::detect(&doc.text);
        doc.rebuild_lines();
        doc
    }

    /// Replaces this buffer's content with a recovery snapshot (used when the
    /// same file is already open as a clean tab).
    pub fn restore_snapshot(&mut self, text: String, caret: usize, anchor: usize, eol: LineEnding) {
        self.text = text;
        self.caret = floor_boundary(&self.text, caret);
        self.anchor = floor_boundary(&self.text, anchor);
        self.eol = eol;
        self.dirty = true;
        self.saved_cursor = None;
        self.rev += 1;
        self.edit_span = None;
        self.undo = UndoStack::default();
        self.indent = IndentStyle::detect(&self.text);
        self.rebuild_lines();
    }

    /// Reads a file as UTF-8, normalizing `\r\n` / `\r` to `\n`.
    pub fn open(id: u64, path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let raw = std::fs::read_to_string(&path)?;
        let eol = LineEnding::detect(&raw);
        let text = normalize(&raw);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let mut doc = Self {
            id,
            path: Some(path),
            name,
            text,
            caret: 0,
            anchor: 0,
            line_starts: Vec::new(),
            dirty: false,
            saved_cursor: Some(0),
            eol,
            rev: 0,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            read_only: false,
            view_key: None,
            view_kind: None,
            preview_start: None,
            indent: None,
        };
        doc.indent = IndentStyle::detect(&doc.text);
        doc.rebuild_lines();
        Ok(doc)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn view_key(&self) -> Option<&str> {
        self.view_key.as_deref()
    }

    pub fn is_git_view(&self) -> bool {
        self.view_kind == Some(ViewKind::Git)
    }

    pub fn is_docs_view(&self) -> bool {
        self.view_kind == Some(ViewKind::Docs)
    }

    pub fn view_kind(&self) -> Option<ViewKind> {
        self.view_kind
    }

    pub fn preview_start(&self) -> Option<usize> {
        self.preview_start
    }

    pub fn set_git_preview_start(&mut self, start: Option<usize>) {
        self.preview_start = start;
    }

    /// A synthetic, read-only document used for Git diff/commit views.
    pub fn view(
        id: u64,
        name: impl Into<String>,
        text: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        let mut doc = Self::untitled(id, 1);
        doc.name = name.into();
        doc.text = normalize(&text.into());
        doc.caret = 0;
        doc.anchor = 0;
        doc.dirty = false;
        doc.read_only = true;
        doc.view_key = Some(key.into());
        doc.view_kind = Some(ViewKind::Git);
        doc.rebuild_lines();
        doc
    }

    /// A synthetic read-only editor tab for embedded Godot documentation.
    pub fn docs_view(
        id: u64,
        name: impl Into<String>,
        text: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        let mut doc = Self::untitled(id, 1);
        doc.name = name.into();
        doc.text = normalize(&text.into());
        doc.caret = 0;
        doc.anchor = 0;
        doc.dirty = false;
        doc.read_only = true;
        doc.view_key = Some(key.into());
        doc.view_kind = Some(ViewKind::Docs);
        doc.rebuild_lines();
        doc
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn restore_view(&mut self, text: impl Into<String>) {
        self.text = normalize(&text.into());
        self.caret = 0;
        self.anchor = 0;
        self.dirty = false;
        self.saved_cursor = Some(0);
        self.rev += 1;
        self.edit_span = None;
        self.undo = UndoStack::default();
        self.indent = IndentStyle::detect(&self.text);
        self.rebuild_lines();
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// First/last line touched by the latest mutation (`None` after a fresh
    /// load). Consumers use it to start incremental work.
    pub fn edit_span(&self) -> Option<(usize, usize)> {
        self.edit_span
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_ending(&self) -> LineEnding {
        self.eol
    }

    /// Indentation detected from the file content, if any.
    pub fn detected_indent(&self) -> Option<IndentStyle> {
        self.indent
    }

    /// Records the document's indentation after an explicit conversion.
    pub fn set_indent_style(&mut self, style: IndentStyle) {
        self.indent = Some(style);
    }

    /// Text of the current line before the caret (for block-indent rules).
    pub fn line_before_caret(&self) -> &str {
        let line = self.line_of(self.caret);
        &self.text[self.line_start(line)..self.caret.min(self.line_end(line))]
    }

    /// Points the buffer at `path` (Save As) and renames it.
    pub fn set_path(&mut self, path: PathBuf) {
        self.name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        self.path = Some(path);
    }

    /// Replaces `[start, end)` with `text` as one undo step (find/replace).
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) -> bool {
        if self.read_only || start > end || end > self.text.len() {
            return false;
        }
        self.replace(start, end, text, Kind::Other);
        true
    }

    /// Replaces several non-overlapping ranges with the same text as one
    /// undoable edit. Ranges must be sorted, in bounds and on char boundaries.
    pub fn replace_ranges(&mut self, ranges: &[(usize, usize)], text: &str) -> usize {
        if self.read_only || ranges.is_empty() {
            return 0;
        }
        let mut last_end = 0;
        for &(start, end) in ranges {
            if start < last_end
                || start > end
                || end > self.text.len()
                || !self.text.is_char_boundary(start)
                || !self.text.is_char_boundary(end)
            {
                return 0;
            }
            last_end = end;
        }
        let start = ranges[0].0;
        let end = ranges[ranges.len() - 1].1;
        let mut replacement = String::with_capacity(end - start + text.len());
        let mut cursor = start;
        for &(from, to) in ranges {
            replacement.push_str(&self.text[cursor..from]);
            replacement.push_str(text);
            cursor = to;
        }
        replacement.push_str(&self.text[cursor..end]);
        let after = Cursor {
            caret: ranges[0].0 + text.len(),
            anchor: ranges[0].0 + text.len(),
        };
        self.replace_with(start, end, &replacement, Kind::Other, after);
        ranges.len()
    }

    /// Reloads the buffer from disk after an external change, dropping undo
    /// history and preserving the caret/scroll where still valid.
    pub fn reload(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "untitled buffer has no disk file",
            ));
        };
        let raw = std::fs::read_to_string(path)?;
        self.eol = LineEnding::detect(&raw);
        self.text = normalize(&raw);
        self.caret = floor_boundary(&self.text, self.caret.min(self.text.len()));
        self.anchor = floor_boundary(&self.text, self.anchor.min(self.text.len()));
        self.dirty = false;
        self.saved_cursor = Some(0);
        self.rev += 1;
        self.edit_span = None;
        self.undo = UndoStack::default();
        self.rebuild_lines();
        Ok(())
    }

    /// Writes the buffer back to its file. Untitled buffers have no path.
    pub fn save(&mut self) -> std::io::Result<()> {
        if self.read_only {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "read-only Git view cannot be saved",
            ));
        }
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "untitled buffer has no path; use Save As",
            ));
        };
        let permissions = std::fs::metadata(&path)
            .ok()
            .map(|metadata| metadata.permissions());
        let rendered = self.eol.render(&self.text);
        fsutil::atomic_write(&path, rendered.as_bytes(), permissions)?;
        self.dirty = false;
        self.saved_cursor = Some(self.undo.cursor);
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Line `index` without its trailing newline.
    pub fn line(&self, index: usize) -> &str {
        let start = self.line_start(index);
        let end = self.line_end(index);
        &self.text[start..end]
    }

    pub fn line_start(&self, index: usize) -> usize {
        let index = index.min(self.line_count().saturating_sub(1));
        self.line_starts[index]
    }

    pub fn line_end(&self, index: usize) -> usize {
        let index = index.min(self.line_count().saturating_sub(1));
        if index + 1 < self.line_starts.len() {
            self.line_starts[index + 1].saturating_sub(1)
        } else {
            self.text.len()
        }
    }

    /// 0-based line index of a byte offset.
    pub fn line_of(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        self.line_starts.partition_point(|&start| start <= byte) - 1
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor).then(|| {
            let (a, b) = (self.caret, self.anchor);
            (a.min(b), a.max(b))
        })
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(start, end)| &self.text[start..end])
    }

    pub fn set_caret(&mut self, byte: usize) {
        self.caret = floor_boundary(&self.text, byte);
        self.anchor = self.caret;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn extend_to(&mut self, byte: usize) {
        self.caret = floor_boundary(&self.text, byte);
    }

    /// 0-based line and 0-based character column of the caret.
    pub fn line_col(&self) -> (usize, usize) {
        let line = self.line_of(self.caret);
        let column = self.text[self.line_start(line)..self.caret].chars().count();
        (line, column)
    }

    /// Byte offset of `column` characters into `line` (clamped to line end).
    pub fn byte_at(&self, line: usize, column: usize) -> usize {
        let start = self.line_start(line);
        let end = self.line_end(line);
        let mut byte = start;
        for _ in 0..column {
            if byte >= end {
                break;
            }
            byte = next_boundary(&self.text, byte);
        }
        byte.min(end)
    }

    pub fn click(&mut self, line: usize, column: usize, extend: bool) {
        let byte = self.byte_at(line, column);
        if extend {
            self.extend_to(byte);
        } else {
            self.set_caret(byte);
        }
    }

    pub fn select_word_at(&mut self, byte: usize) {
        let byte = floor_boundary(&self.text, byte);
        let mut start = byte;
        while start > 0 {
            let previous = grapheme::prev_boundary(&self.text, start);
            if !is_word_grapheme(&self.text[previous..start]) {
                break;
            }
            start = previous;
        }
        let mut end = byte;
        while end < self.text.len() {
            let next = grapheme::next_boundary(&self.text, end);
            if !is_word_grapheme(&self.text[end..next]) {
                break;
            }
            end = next;
        }
        self.anchor = start;
        self.caret = end.max(start);
    }

    /// Duplicates the caret line below and keeps the caret on the original.
    pub fn duplicate_line(&mut self) {
        if self.read_only || self.text.is_empty() {
            return;
        }
        let line = self.line_of(self.caret);
        let start = self.line_start(line);
        let end = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.text.len()
        };
        let block = self.text[start..end].to_string();
        let insertion = if block.ends_with('\n') {
            block
        } else {
            format!("\n{block}")
        };
        let caret = self.caret;
        if self.replace_range(end, end, &insertion) {
            self.set_caret(caret);
        }
    }

    /// Swaps the caret line with the previous line (Alt+Up).
    pub fn move_line_up(&mut self) {
        if self.read_only {
            return;
        }
        let line = self.line_of(self.caret);
        if line == 0 {
            return;
        }
        let start = self.line_start(line);
        let end = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.text.len()
        };
        let previous = self.line_start(line - 1);
        let current = self.text[start..end].to_string();
        let above = self.text[previous..start].to_string();
        let replacement = if current.ends_with('\n') {
            format!("{current}{above}")
        } else {
            format!("{current}\n{}", above.trim_end_matches('\n'))
        };
        let column = self.caret.saturating_sub(start);
        if self.replace_range(previous, end, &replacement) {
            self.set_caret(previous + column.min(self.text.len().saturating_sub(previous)));
        }
    }

    /// Swaps the caret line with the next line (Alt+Down).
    pub fn move_line_down(&mut self) {
        if self.read_only {
            return;
        }
        let line = self.line_of(self.caret);
        if line + 1 >= self.line_count() {
            return;
        }
        let start = self.line_start(line);
        let middle = self.line_start(line + 1);
        let end = if line + 2 < self.line_count() {
            self.line_start(line + 2)
        } else {
            self.text.len()
        };
        let current = self.text[start..middle].to_string();
        let below = self.text[middle..end].to_string();
        let replacement = if below.ends_with('\n') {
            format!("{below}{current}")
        } else {
            format!("{below}\n{}", current.trim_end_matches('\n'))
        };
        let column = self.caret.saturating_sub(start);
        let new_start = start + below.len();
        if self.replace_range(start, end, &replacement) {
            self.set_caret(new_start + column.min(self.text.len().saturating_sub(new_start)));
        }
    }

    /// Deletes the word before the caret (Ctrl/Alt+Backspace).
    pub fn delete_word_backward(&mut self) {
        if self.read_only {
            return;
        }
        if self.delete_selection() {
            return;
        }
        let mut start = self.caret;
        while start > 0 {
            let previous = prev_boundary(&self.text, start);
            let ch = self.text[previous..start].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                start = previous;
            } else {
                break;
            }
        }
        let word_start = start;
        while start > 0 {
            let previous = prev_boundary(&self.text, start);
            let ch = self.text[previous..start].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                start = previous;
            } else {
                break;
            }
        }
        if start == self.caret {
            start = word_start.min(self.caret);
            if start == self.caret && start > 0 {
                start = prev_boundary(&self.text, start);
            }
        }
        if start < self.caret && self.replace_range(start, self.caret, "") {
            self.set_caret(start);
        }
    }

    /// Deletes the word after the caret (Ctrl/Alt+Delete).
    pub fn delete_word_forward(&mut self) {
        if self.read_only {
            return;
        }
        if self.delete_selection() {
            return;
        }
        let mut end = self.caret;
        while end < self.text.len() {
            let ch = self.text[end..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                end = next_boundary(&self.text, end);
            } else {
                break;
            }
        }
        while end < self.text.len() {
            let ch = self.text[end..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                end = next_boundary(&self.text, end);
            } else {
                break;
            }
        }
        if end == self.caret && end < self.text.len() {
            end = next_boundary(&self.text, end);
        }
        if end > self.caret && self.replace_range(self.caret, end, "") {
            self.set_caret(self.caret);
        }
    }

    /// Toggles `marker` on every line touched by the selection (or the caret
    /// line). Indentation is preserved; the command is a no-op in read-only
    /// views or when the block is empty.
    pub fn toggle_line_comment(&mut self, marker: &str) {
        if self.read_only || self.text.is_empty() || marker.is_empty() {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let first = self.line_of(start);
        let last = self.selection_last_line(start, end);
        let block_start = self.line_start(first);
        let block_end = self.line_end(last);
        let block = self.text[block_start..block_end].to_string();
        let lines: Vec<&str> = block.split('\n').collect();
        let commented = lines.iter().all(|line| {
            let trimmed = line.trim_start();
            trimmed.is_empty() || trimmed.starts_with(marker)
        });
        let mut out = String::with_capacity(block.len() + marker.len() * lines.len() + 2);
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let indent = line.len() - line.trim_start().len();
            if commented {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix(marker) {
                    out.push_str(&line[..indent]);
                    out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                } else {
                    out.push_str(line);
                }
            } else if line.trim().is_empty() {
                out.push_str(line);
            } else {
                out.push_str(&line[..indent]);
                out.push_str(marker);
                out.push(' ');
                out.push_str(&line[indent..]);
            }
        }
        let caret = self.caret;
        let column = caret.saturating_sub(block_start);
        if self.replace_range(block_start, block_end, &out) {
            self.set_caret((block_start + column).min(self.text.len()));
        }
    }

    /// The current line including its trailing newline, for copy/cut with no
    /// selection. The last line has no trailing newline.
    pub fn current_line_text(&self) -> Option<String> {
        let (start, end) = self.clip_line_bounds()?;
        Some(self.text[start..end].to_string())
    }

    /// VS Code / JetBrains style empty-selection cut: remove the current line
    /// (including its newline) and return the removed text. The newline before
    /// the last line is removed too, so cutting the final line does not leave
    /// an empty row behind.
    pub fn cut_current_line(&mut self) -> Option<String> {
        if self.read_only || self.text.is_empty() {
            return None;
        }
        let (start, end) = self.clip_line_bounds()?;
        let text = self.text[start..end].to_string();
        let replace_start = if end == self.text.len() && start > 0 {
            prev_boundary(&self.text, start)
        } else {
            start
        };
        self.replace(replace_start, end, "", Kind::Other);
        let caret = replace_start.min(self.text.len());
        self.set_caret(caret);
        Some(text)
    }

    fn clip_line_bounds(&self) -> Option<(usize, usize)> {
        if self.text.is_empty() {
            return None;
        }
        let line = self.line_of(self.caret);
        let start = self.line_start(line);
        let end = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.text.len()
        };
        Some((start, end))
    }

    /// Selects the whole logical line containing `byte` (triple-click). The
    /// selection includes the trailing newline when there is one.
    pub fn select_line_at(&mut self, byte: usize) {
        let line = self.line_of(byte);
        self.anchor = self.line_start(line);
        self.caret = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.line_end(line)
        };
    }

    // -- edits ---------------------------------------------------------------

    pub fn insert_text(&mut self, insert: &str) {
        if self.read_only {
            return;
        }
        let insert = sanitize(insert);
        if insert.is_empty() {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let single = insert.chars().count() == 1 && !insert.contains('\n');
        let kind = if single { Kind::Type } else { Kind::Other };
        self.replace(start, end, &insert, kind);
        if self.indent.is_none() && insert.contains('\n') {
            self.indent = IndentStyle::detect(&self.text);
        }
    }

    /// Enter: insert a newline plus the current line's leading whitespace.
    ///
    /// `extra` adds one more level (a GDScript block opener such as a line
    /// ending in `:`); it is ignored for a caret inside the indentation itself.
    pub fn insert_newline(&mut self, extra: bool, style: IndentStyle) {
        if self.read_only {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let line = self.line_of(start);
        let indent: String = self
            .line(line)
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let extra = if extra && start >= self.line_start(line) + indent.len() {
            style.unit()
        } else {
            String::new()
        };
        let insert = format!("\n{indent}{extra}");
        self.replace(start, end, &insert, Kind::Other);
        if self.indent.is_none() {
            self.indent = IndentStyle::detect(&self.text);
        }
    }

    pub fn backspace(&mut self) {
        if self.read_only {
            return;
        }
        if let Some((start, end)) = self.selection() {
            self.replace(start, end, "", Kind::Other);
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = grapheme::prev_boundary(&self.text, self.caret);
        self.replace(start, self.caret, "", Kind::Backspace);
    }

    pub fn delete_forward(&mut self) {
        if self.read_only {
            return;
        }
        if let Some((start, end)) = self.selection() {
            self.replace(start, end, "", Kind::Other);
            return;
        }
        if self.caret >= self.text.len() {
            return;
        }
        let end = grapheme::next_boundary(&self.text, self.caret);
        self.replace(self.caret, end, "", Kind::Delete);
    }

    /// Deletes the selection (no-op without one).
    pub fn delete_selection(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.replace(start, end, "", Kind::Other);
        true
    }

    /// Tab: indent every selected line by `style`, or insert one unit.
    pub fn indent(&mut self, style: IndentStyle) {
        if self.read_only {
            return;
        }
        let unit = style.unit();
        let Some((start, end)) = self.selection() else {
            let (start, end) = (self.caret, self.caret);
            self.replace(start, end, &unit, Kind::Other);
            return;
        };
        let first = self.line_of(start);
        let last = self.selection_last_line(start, end);
        let starts: Vec<usize> = (first..=last).map(|i| self.line_start(i)).collect();
        let block_start = starts[0];
        let block_end = self.line_end(last);
        let block = self.text[block_start..block_end].to_string();
        let mut out = String::with_capacity(block.len() + unit.len() * starts.len());
        for (index, line) in block.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&unit);
            out.push_str(line);
        }
        let deltas: Vec<isize> = starts.iter().map(|_| unit.len() as isize).collect();
        let caret = remap(self.caret, &starts, &deltas);
        let anchor = remap(self.anchor, &starts, &deltas);
        self.replace_with(
            block_start,
            block_end,
            &out,
            Kind::Other,
            Cursor { caret, anchor },
        );
    }

    /// Shift+Tab: remove one indentation unit from every selected line.
    pub fn outdent(&mut self, style: IndentStyle) {
        if self.read_only {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let first = self.line_of(start);
        let last = self.selection_last_line(start, end);
        let starts: Vec<usize> = (first..=last).map(|i| self.line_start(i)).collect();
        let block_start = starts[0];
        let block_end = self.line_end(last);
        let block = self.text[block_start..block_end].to_string();
        let width = match style {
            IndentStyle::Tabs => 1,
            IndentStyle::Spaces(width) => width.max(1) as usize,
        };
        let mut out = String::with_capacity(block.len());
        let mut deltas: Vec<isize> = Vec::with_capacity(starts.len());
        for (index, line) in block.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let cut = leading_indent(line, width);
            deltas.push(-(cut as isize));
            out.push_str(&line[cut..]);
        }
        if deltas.iter().all(|delta| *delta == 0) {
            return;
        }
        let caret = remap(self.caret, &starts, &deltas);
        let anchor = remap(self.anchor, &starts, &deltas);
        self.replace_with(
            block_start,
            block_end,
            &out,
            Kind::Other,
            Cursor { caret, anchor },
        );
    }

    /// Normalizes the leading indentation of every code line to `target`.
    ///
    /// Lines inside a GDScript triple-quoted string are left untouched (their
    /// leading whitespace is string content). This is the manual `Format
    /// Document` path: it only converts the indentation unit, which is exactly
    /// what fixes a tab/spaces mix without touching code.
    pub fn format_indentation(
        &mut self,
        target: IndentStyle,
        source_tab_width: usize,
        gdscript: bool,
    ) -> bool {
        if self.read_only || self.text.is_empty() {
            self.indent = Some(target);
            return false;
        }
        let tab_width = source_tab_width.max(1);
        let width = match target {
            IndentStyle::Tabs => tab_width,
            IndentStyle::Spaces(width) => width.max(1) as usize,
        };
        let mut out = String::with_capacity(self.text.len());
        let mut multiline: Option<&'static str> = None;
        for (index, line) in self.text.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            if let Some(delimiter) = multiline {
                // Inside a triple-quoted string: leading whitespace is content.
                out.push_str(line);
                if line.contains(delimiter) {
                    multiline = None;
                }
                continue;
            }
            let opens = if gdscript {
                gdscript_multiline_opens(line)
            } else {
                None
            };
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let columns = indentation_columns(&line[..ws_len], tab_width);
            let converted = match target {
                IndentStyle::Tabs => format!(
                    "{}{}",
                    "\t".repeat(columns / width),
                    " ".repeat(columns % width)
                ),
                IndentStyle::Spaces(_) => " ".repeat(columns),
            };
            out.push_str(&converted);
            out.push_str(&line[ws_len..]);
            if let Some(delimiter) = opens {
                multiline = Some(delimiter);
            }
        }
        if out == self.text {
            self.indent = Some(target);
            return false;
        }
        // Keep the caret on the same line at the same content column.
        let (caret_line, caret_col) = self.line_col();
        let old_lead = leading_indent(self.line(caret_line), width);
        let new_lines: Vec<&str> = out.split('\n').collect();
        let new_line = new_lines.get(caret_line).copied().unwrap_or_default();
        let new_lead = leading_indent(new_line, width);
        let new_col = caret_col
            .saturating_sub(old_lead)
            .saturating_add(new_lead)
            .min(new_line.len());
        let line_start = new_lines
            .iter()
            .take(caret_line)
            .map(|line| line.len() + 1)
            .sum::<usize>();
        let caret = (line_start + new_col).min(out.len());
        let anchor = if self.anchor == self.caret {
            caret
        } else {
            self.anchor.min(out.len())
        };
        self.replace_with(
            0,
            self.text.len(),
            &out,
            Kind::Other,
            Cursor { caret, anchor },
        );
        self.indent = Some(target);
        true
    }

    pub fn undo(&mut self) -> bool {
        if self.undo.cursor == 0 {
            return false;
        }
        self.undo.cursor -= 1;
        let edit = self.undo.edits[self.undo.cursor].clone();
        let first_line = self.line_of(edit.start);
        let end = edit.start + edit.inserted.len();
        self.text.replace_range(edit.start..end, &edit.removed);
        self.update_line_starts(edit.start, edit.inserted.len(), &edit.removed);
        let last_line = self.line_of((edit.start + edit.removed.len()).min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = edit.before.caret.min(self.text.len());
        self.anchor = edit.before.anchor.min(self.text.len());
        self.dirty = self.saved_cursor != Some(self.undo.cursor);
        self.rev += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.undo.cursor >= self.undo.edits.len() {
            return false;
        }
        let edit = self.undo.edits[self.undo.cursor].clone();
        let first_line = self.line_of(edit.start);
        let end = edit.start + edit.removed.len();
        self.text.replace_range(edit.start..end, &edit.inserted);
        self.update_line_starts(edit.start, edit.removed.len(), &edit.inserted);
        let last_line = self.line_of((edit.start + edit.inserted.len()).min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = edit.after.caret.min(self.text.len());
        self.anchor = edit.after.anchor.min(self.text.len());
        self.undo.cursor += 1;
        self.dirty = self.saved_cursor != Some(self.undo.cursor);
        self.rev += 1;
        true
    }

    pub fn can_undo(&self) -> bool {
        self.undo.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.undo.cursor < self.undo.edits.len()
    }

    // -- movement ------------------------------------------------------------

    pub fn move_left(&mut self, select: bool, word: bool) {
        if !select {
            if let Some((start, _)) = self.selection() {
                self.set_caret(start);
                return;
            }
        }
        let target = if word {
            prev_word_boundary(&self.text, self.caret)
        } else {
            grapheme::prev_boundary(&self.text, self.caret)
        };
        self.move_to(target, select);
    }

    pub fn move_right(&mut self, select: bool, word: bool) {
        if !select {
            if let Some((_, end)) = self.selection() {
                self.set_caret(end);
                return;
            }
        }
        let target = if word {
            next_word_boundary(&self.text, self.caret)
        } else {
            grapheme::next_boundary(&self.text, self.caret)
        };
        self.move_to(target, select);
    }

    pub fn move_up(&mut self, select: bool) {
        let (line, column) = self.line_col();
        if line == 0 {
            self.move_to(0, select);
            return;
        }
        let target = self.byte_at(line - 1, column);
        self.move_to(target, select);
    }

    pub fn move_down(&mut self, select: bool) {
        let (line, column) = self.line_col();
        if line + 1 >= self.line_count() {
            self.move_to(self.text.len(), select);
            return;
        }
        let target = self.byte_at(line + 1, column);
        self.move_to(target, select);
    }

    /// Smart Home: first non-whitespace, then the line start.
    pub fn move_home(&mut self, select: bool) {
        let line = self.line_of(self.caret);
        let start = self.line_start(line);
        let first = start
            + self.text[start..self.line_end(line)]
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .map(char::len_utf8)
                .sum::<usize>();
        let target = if self.caret == first { start } else { first };
        self.move_to(target, select);
    }

    pub fn move_end(&mut self, select: bool) {
        let line = self.line_of(self.caret);
        self.move_to(self.line_end(line), select);
    }

    pub fn move_doc_start(&mut self, select: bool) {
        self.move_to(0, select);
    }

    pub fn move_doc_end(&mut self, select: bool) {
        self.move_to(self.text.len(), select);
    }

    #[allow(dead_code)] // Key has no PageUp/PageDown yet
    pub fn page(&mut self, lines: isize, select: bool) {
        let (line, column) = self.line_col();
        let target_line = if lines < 0 {
            line.saturating_sub(lines.unsigned_abs())
        } else {
            (line + lines as usize).min(self.line_count().saturating_sub(1))
        };
        let target = self.byte_at(target_line, column);
        self.move_to(target, select);
    }

    fn move_to(&mut self, byte: usize, select: bool) {
        self.caret = floor_boundary(&self.text, byte);
        if !select {
            self.anchor = self.caret;
        }
    }

    // -- internals -----------------------------------------------------------

    /// Last line touched by a selection: a selection ending exactly at a line
    /// start does not include that line (standard editor behaviour).
    fn selection_last_line(&self, start: usize, end: usize) -> usize {
        let last = self.line_of(end);
        if end > start && self.line_start(last) == end && last > 0 {
            last - 1
        } else {
            last
        }
    }

    fn replace(&mut self, start: usize, end: usize, insert: &str, kind: Kind) {
        let after = Cursor {
            caret: start + insert.len(),
            anchor: start + insert.len(),
        };
        self.replace_with(start, end, insert, kind, after);
    }

    fn replace_with(&mut self, start: usize, end: usize, insert: &str, kind: Kind, after: Cursor) {
        let (start, end) = (start.min(self.text.len()), end.min(self.text.len()));
        let removed = self.text[start..end].to_string();
        if removed.is_empty() && insert.is_empty() {
            return;
        }
        let before = Cursor {
            caret: self.caret,
            anchor: self.anchor,
        };
        let first_line = self.line_of(start);
        let removed_len = end - start;
        self.text.replace_range(start..end, insert);
        self.update_line_starts(start, removed_len, insert);
        let after_end = start + insert.len();
        let last_line = self.line_of(after_end.min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = floor_boundary(&self.text, after.caret);
        self.anchor = floor_boundary(&self.text, after.anchor);
        self.rev += 1;
        // A new edit truncates the redo history at `undo.cursor`; if the save
        // point lived in that truncated branch it can no longer be reached.
        if self
            .saved_cursor
            .is_some_and(|saved| saved > self.undo.cursor)
        {
            self.saved_cursor = None;
        }
        self.undo.push(Edit {
            start,
            removed,
            inserted: insert.to_string(),
            kind,
            before,
            after: Cursor {
                caret: self.caret,
                anchor: self.anchor,
            },
        });
        self.dirty = true;
    }

    /// Updates the line-start index after replacing `removed_len` bytes at
    /// `start` with `insert`. This is the incremental counterpart to
    /// [`Document::rebuild_lines`] used by every edit/undo/redo.
    fn update_line_starts(&mut self, start: usize, removed_len: usize, insert: &str) {
        let end = start + removed_len;
        let delta = insert.len() as isize - removed_len as isize;
        let prefix_end = self
            .line_starts
            .partition_point(|&line_start| line_start <= start);
        // An insertion at `start` keeps the line that already begins there.
        // A replacement removes old starts inside `(start, end]`, including a
        // line that begins exactly at `end`, because the replacement may merge
        // it with the preceding text.
        let suffix_start = if removed_len == 0 {
            prefix_end
        } else {
            self.line_starts
                .partition_point(|&line_start| line_start <= end)
        };

        let new_starts = insert.bytes().filter(|byte| *byte == b'\n').count();
        let mut starts = Vec::with_capacity(
            prefix_end + new_starts + self.line_starts.len().saturating_sub(suffix_start),
        );
        starts.extend_from_slice(&self.line_starts[..prefix_end]);
        for (index, byte) in insert.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(start + index + 1);
            }
        }
        for &line_start in &self.line_starts[suffix_start..] {
            starts.push((line_start as isize + delta) as usize);
        }
        self.line_starts = starts;
    }

    fn rebuild_lines(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(index + 1);
            }
        }
    }
}

/// Position after applying per-line `deltas` to lines `first..=last`.
///
/// A position inside line `i` shifts by the total delta of lines `first..=i`;
/// a position before the block is untouched; after the block it shifts by all
/// deltas. Used by block indent / outdent so one key press is one undo step.
fn remap(pos: usize, starts: &[usize], deltas: &[isize]) -> usize {
    if starts.is_empty() || pos < starts[0] {
        return pos;
    }
    let mut total = 0isize;
    for (index, delta) in deltas.iter().enumerate() {
        total += delta;
        let next = starts.get(index + 1).copied().unwrap_or(usize::MAX);
        if pos < next {
            let line_start = starts[index] as isize + delta;
            let mapped = pos as isize + total;
            return mapped.max(line_start).max(0) as usize;
        }
    }
    (pos as isize + total).max(0) as usize
}

fn normalize(raw: &str) -> String {
    if !raw.contains('\r') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Drops `\r` and control characters other than `\n` / `\t`.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

/// Number of bytes of one indentation unit at the start of `line`.
fn leading_indent(line: &str, width: usize) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(width)
        .take_while(|byte| *byte == b' ')
        .count()
}

/// Visual columns of leading whitespace (tabs use the configured width).
fn indentation_columns(whitespace: &str, tab_width: usize) -> usize {
    whitespace
        .bytes()
        .map(|byte| if byte == b'\t' { tab_width } else { 1 })
        .sum()
}

/// Delimiter of a GDScript triple-quoted string opened on `line`.
fn gdscript_multiline_opens(line: &str) -> Option<&'static str> {
    let code = line.split('#').next().unwrap_or(line);
    let triple = code.find("\"\"\"");
    let single = code.find("'''");
    let delimiter = match (triple, single) {
        (Some(a), Some(b)) if a < b => "\"\"\"",
        (Some(_), Some(_)) => "'''",
        (Some(_), None) => "\"\"\"",
        (None, Some(_)) => "'''",
        (None, None) => return None,
    };
    // An even count means the string opens and closes on this line.
    (code.matches(delimiter).count() % 2 == 1).then_some(delimiter)
}

fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn prev_boundary(text: &str, byte: usize) -> usize {
    let byte = floor_boundary(text, byte);
    if byte == 0 {
        return 0;
    }
    let mut previous = byte - 1;
    while !text.is_char_boundary(previous) {
        previous -= 1;
    }
    previous
}

fn next_boundary(text: &str, byte: usize) -> usize {
    let byte = floor_boundary(text, byte);
    if byte >= text.len() {
        return text.len();
    }
    byte + text[byte..].chars().next().map_or(0, char::len_utf8)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_word_grapheme(grapheme: &str) -> bool {
    grapheme.chars().any(is_word_char)
}

fn prev_word_boundary(text: &str, byte: usize) -> usize {
    let mut byte = floor_boundary(text, byte);
    while byte > 0 {
        let previous = grapheme::prev_boundary(text, byte);
        if is_word_grapheme(&text[previous..byte]) {
            break;
        }
        byte = previous;
    }
    while byte > 0 {
        let previous = grapheme::prev_boundary(text, byte);
        if !is_word_grapheme(&text[previous..byte]) {
            break;
        }
        byte = previous;
    }
    byte
}

fn next_word_boundary(text: &str, byte: usize) -> usize {
    let mut byte = floor_boundary(text, byte);
    while byte < text.len() {
        let next = grapheme::next_boundary(text, byte);
        if is_word_grapheme(&text[byte..next]) {
            break;
        }
        byte = next;
    }
    while byte < text.len() {
        let next = grapheme::next_boundary(text, byte);
        if !is_word_grapheme(&text[byte..next]) {
            break;
        }
        byte = next;
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        let mut doc = Document::untitled(1, 0);
        doc.insert_text(text);
        doc.set_caret(0);
        doc.dirty = false;
        doc.undo = UndoStack::default();
        doc
    }

    #[test]
    fn typing_coalesces_into_one_undo_step() {
        let mut doc = Document::untitled(1, 0);
        for ch in "hello".chars() {
            doc.insert_text(&ch.to_string());
        }
        assert_eq!(doc.text(), "hello");
        assert!(doc.undo());
        assert_eq!(doc.text(), "");
        assert_eq!(doc.caret(), 0);
        assert!(doc.redo());
        assert_eq!(doc.text(), "hello");
    }

    #[test]
    fn newline_and_indent_are_undo_steps_of_their_own() {
        let mut doc = doc("    fn main() {");
        doc.set_caret(doc.text().len());
        doc.insert_newline(false, IndentStyle::Spaces(4));
        assert_eq!(doc.text(), "    fn main() {\n    ");
        doc.insert_text("x");
        assert_eq!(doc.text(), "    fn main() {\n    x");
        doc.undo();
        assert_eq!(doc.text(), "    fn main() {\n    ");
    }

    #[test]
    fn block_indent_is_one_edit() {
        let mut doc = doc("a\nb\nc");
        doc.anchor = 0;
        doc.caret = doc.text().len();
        doc.indent(IndentStyle::Spaces(4));
        assert_eq!(doc.text(), "    a\n    b\n    c");
        assert!(doc.undo());
        assert_eq!(doc.text(), "a\nb\nc");
    }

    #[test]
    fn outdent_removes_up_to_four_spaces() {
        let mut doc = doc("      a\n  b");
        doc.select_all();
        doc.outdent(IndentStyle::Spaces(4));
        assert_eq!(doc.text(), "  a\nb");
    }

    #[test]
    fn tab_documents_indent_with_tabs() {
        let mut doc = doc("\tfunc _ready():\n\t\tpass\n");
        assert_eq!(doc.detected_indent(), Some(IndentStyle::Tabs));
        doc.select_all();
        doc.indent(IndentStyle::Tabs);
        assert_eq!(doc.text(), "\t\tfunc _ready():\n\t\t\tpass\n");
        doc.outdent(IndentStyle::Tabs);
        assert_eq!(doc.text(), "\tfunc _ready():\n\t\tpass\n");
    }

    #[test]
    fn newline_after_a_block_opener_adds_one_level() {
        let mut doc = doc("\tfunc _ready():");
        doc.set_caret(doc.text().len());
        doc.insert_newline(true, IndentStyle::Tabs);
        assert_eq!(doc.text(), "\tfunc _ready():\n\t\t");
        doc.insert_text("pass");
        assert_eq!(doc.line_before_caret(), "\t\tpass");
    }

    #[test]
    fn indent_detection_recovers_tabs_and_space_widths() {
        assert_eq!(
            IndentStyle::detect("\tfunc f():\n\t\tpass\n"),
            Some(IndentStyle::Tabs)
        );
        assert_eq!(
            IndentStyle::detect("func f():\n    pass\n"),
            Some(IndentStyle::Spaces(4))
        );
        assert_eq!(
            IndentStyle::detect("a\n  b\n    c\n"),
            Some(IndentStyle::Spaces(2))
        );
        assert_eq!(
            IndentStyle::detect("a\n        b\n"),
            Some(IndentStyle::Spaces(8))
        );
        assert_eq!(IndentStyle::detect("no indentation\n"), None);
    }

    #[test]
    fn format_converts_indentation_without_touching_code() {
        let mut tabbed = doc("func f():\n\tif true:\n\t\tpass\n");
        assert!(tabbed.format_indentation(IndentStyle::Spaces(4), 4, true));
        assert_eq!(tabbed.text(), "func f():\n    if true:\n        pass\n");
        assert_eq!(tabbed.detected_indent(), Some(IndentStyle::Spaces(4)));
        assert!(tabbed.undo());
        assert_eq!(tabbed.text(), "func f():\n\tif true:\n\t\tpass\n");

        let mut spaced = doc("func f():\n    pass\n");
        assert!(spaced.format_indentation(IndentStyle::Tabs, 4, true));
        assert_eq!(spaced.text(), "func f():\n\tpass\n");
    }

    #[test]
    fn format_leaves_multiline_string_content_alone() {
        let mut doc = doc("func f():\n\tvar text = \"\"\"\n    keep me\n\t\"\"\"\n\tpass\n");
        assert!(doc.format_indentation(IndentStyle::Spaces(4), 4, true));
        assert_eq!(
            doc.text(),
            "func f():\n    var text = \"\"\"\n    keep me\n\t\"\"\"\n    pass\n"
        );
    }

    #[test]
    fn selection_movement_collapses_then_moves() {
        let mut doc = doc("hello world");
        doc.set_caret(0);
        doc.extend_to(5);
        doc.move_left(false, false);
        assert_eq!(doc.caret(), 0);
        assert!(!doc.has_selection());
        doc.move_right(false, true);
        assert_eq!(doc.caret(), 5); // "hello"
    }

    #[test]
    fn smart_home_toggles_between_indent_and_line_start() {
        let mut doc = doc("    abc");
        doc.set_caret(6);
        doc.move_home(false);
        assert_eq!(doc.caret(), 4);
        doc.move_home(false);
        assert_eq!(doc.caret(), 0);
    }

    #[test]
    fn up_down_keeps_the_character_column() {
        let mut doc = doc("hello\nhi\nworld");
        doc.set_caret(0);
        doc.move_right(false, false);
        doc.move_right(false, false);
        doc.move_down(false);
        assert_eq!(doc.caret(), 8); // end of "hi" (clamped)
        doc.move_down(false);
        assert_eq!(doc.caret(), 11); // "wo|rld" keeps column 2
    }

    #[test]
    fn vertical_movement_clamps_longer_lines() {
        let mut doc = doc("ab\nhello");
        doc.set_caret(1);
        doc.move_down(false);
        assert_eq!(doc.caret(), 4); // a|b -> hel|lo at column 1
        doc.move_up(false);
        assert_eq!(doc.caret(), 1);
    }

    #[test]
    fn backspace_runs_coalesce_and_respect_char_boundaries() {
        let mut doc = doc("你好a");
        doc.set_caret(doc.text().len());
        doc.backspace();
        assert_eq!(doc.text(), "你好");
        doc.backspace();
        assert_eq!(doc.text(), "你");
        doc.undo();
        assert_eq!(doc.text(), "你好a");
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut doc = doc("hello world");
        doc.anchor = 6;
        doc.caret = 11;
        doc.insert_text("there");
        assert_eq!(doc.text(), "hello there");
    }

    #[test]
    fn normalizes_crlf_on_load_rules() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn line_and_column_round_trip() {
        let mut doc = doc("ab\n你好x");
        assert_eq!(doc.line_count(), 2);
        let end = doc.text().len();
        doc.set_caret(end);
        assert_eq!(doc.line_col(), (1, 3)); // caret ends after "x"
        let byte = doc.byte_at(1, 2); // after 你好 (6 bytes)
        assert_eq!(&doc.text()[..byte], "ab\n你好");
    }

    #[test]
    fn select_line_includes_the_newline_except_at_eof() {
        let mut doc = doc("one\ntwo");
        doc.select_line_at(1);
        assert_eq!(doc.selected_text(), Some("one\n"));
        doc.select_line_at(doc.byte_at(1, 1));
        assert_eq!(doc.selected_text(), Some("two"));
    }

    #[test]
    fn cut_current_line_includes_newline_and_undoes() {
        let mut doc = doc("one\ntwo\nthree");
        doc.set_caret(doc.byte_at(1, 1));
        assert_eq!(doc.cut_current_line().as_deref(), Some("two\n"));
        assert_eq!(doc.text(), "one\nthree");
        assert_eq!(doc.line_col(), (1, 0));
        doc.undo();
        assert_eq!(doc.text(), "one\ntwo\nthree");
    }

    #[test]
    fn cut_last_line_removes_the_preceding_newline() {
        let mut empty_last = doc("one\n");
        let mut doc = doc("one\ntwo");
        doc.set_caret(doc.byte_at(1, 1));
        assert_eq!(doc.cut_current_line().as_deref(), Some("two"));
        assert_eq!(doc.text(), "one");
        empty_last.set_caret(empty_last.text().len());
        assert_eq!(empty_last.cut_current_line().as_deref(), Some(""));
        assert_eq!(empty_last.text(), "one");
    }

    #[test]
    fn duplicate_and_move_line_keep_the_caret_on_the_line() {
        let mut first = doc("one\ntwo\nthree");
        first.set_caret(first.byte_at(1, 1));
        first.duplicate_line();
        assert_eq!(first.text(), "one\ntwo\ntwo\nthree");
        assert_eq!(first.line_col(), (1, 1));
        first.move_line_down();
        assert_eq!(first.text(), "one\ntwo\ntwo\nthree");
        first.move_line_up();
        assert_eq!(first.text(), "one\ntwo\ntwo\nthree");

        let mut doc = doc("one\ntwo\nthree");
        doc.set_caret(doc.byte_at(1, 1));
        doc.move_line_down();
        assert_eq!(doc.text(), "one\nthree\ntwo");
        assert_eq!(doc.line_col().0, 2);
        doc.move_line_up();
        assert_eq!(doc.text(), "one\ntwo\nthree");
        doc.move_line_up();
        assert_eq!(doc.text(), "two\none\nthree");
        assert_eq!(doc.line_col().0, 0);
    }

    #[test]
    fn toggle_line_comment_preserves_indent_and_uncomments() {
        let mut doc = doc("one\n    two\nthree");
        doc.select_line_at(doc.byte_at(1, 0));
        doc.toggle_line_comment("//");
        assert_eq!(doc.text(), "one\n    // two\nthree");
        doc.select_line_at(doc.byte_at(1, 4));
        doc.toggle_line_comment("//");
        assert_eq!(doc.text(), "one\n    two\nthree");
    }

    #[test]
    fn word_delete_handles_spaces_and_punctuation() {
        let mut doc = doc("hello, world");
        doc.set_caret(doc.text().len());
        doc.delete_word_backward();
        assert_eq!(doc.text(), "hello, ");
        doc.delete_word_backward();
        assert_eq!(doc.text(), "");
        doc.set_caret(0);
        doc.insert_text("hello, world");
        doc.set_caret(0);
        doc.delete_word_forward();
        assert_eq!(doc.text(), " world");
        doc.delete_word_forward();
        assert_eq!(doc.text(), "");
    }

    #[test]
    fn current_line_text_matches_the_caret_line() {
        let mut doc = doc("one\ntwo");
        doc.set_caret(0);
        assert_eq!(doc.current_line_text().as_deref(), Some("one\n"));
        doc.set_caret(doc.byte_at(1, 0));
        assert_eq!(doc.current_line_text().as_deref(), Some("two"));
    }

    #[test]
    fn batched_replace_is_one_undo_step() {
        let mut doc = doc("hello hello hello");
        let ranges = [(0, 5), (6, 11), (12, 17)];
        assert_eq!(doc.replace_ranges(&ranges, "bye"), 3);
        assert_eq!(doc.text(), "bye bye bye");
        doc.undo();
        assert_eq!(doc.text(), "hello hello hello");
        doc.redo();
        assert_eq!(doc.text(), "bye bye bye");
    }

    #[test]
    fn batched_replace_rejects_overlapping_ranges() {
        let mut doc = doc("hello hello");
        assert_eq!(doc.replace_ranges(&[(0, 5), (4, 9)], "bye"), 0);
        assert_eq!(doc.text(), "hello hello");
    }

    #[test]
    fn backspace_and_delete_remove_whole_graphemes() {
        let mut first = doc("e\u{301}x");
        first.set_caret(first.text().len());
        first.backspace();
        assert_eq!(first.text(), "e\u{301}");
        first.backspace();
        assert_eq!(first.text(), "");

        let mut other = doc("x👍🏽y");
        other.set_caret(1);
        other.delete_forward();
        assert_eq!(other.text(), "xy");
    }

    #[test]
    fn horizontal_movement_uses_grapheme_boundaries() {
        let mut doc = doc("a👍🏽b");
        doc.set_caret(1);
        doc.move_right(false, false);
        assert_eq!(doc.caret(), 1 + "👍🏽".len());
        doc.move_left(false, false);
        assert_eq!(doc.caret(), 1);
    }

    #[test]
    fn word_movement_keeps_graphemes_intact() {
        let mut doc = doc("cafe\u{301} noir 👍🏽ok");
        doc.set_caret(0);
        doc.move_right(false, true);
        assert_eq!(doc.caret(), "cafe\u{301}".len());
        doc.move_right(false, true);
        assert_eq!(doc.caret(), "cafe\u{301} noir".len());
        doc.move_right(false, true);
        assert_eq!(doc.caret(), doc.text().len());
        doc.move_left(false, true);
        assert_eq!(doc.caret(), "cafe\u{301} noir 👍🏽".len());
    }

    #[test]
    fn double_click_selection_keeps_combining_marks() {
        let mut doc = doc("cafe\u{301} noir");
        doc.select_word_at(doc.text().find('n').unwrap());
        assert_eq!(doc.selected_text(), Some("noir"));
        doc.select_word_at(doc.text().find('c').unwrap());
        assert_eq!(doc.selected_text(), Some("cafe\u{301}"));
    }

    #[test]
    fn save_preserves_crlf_line_endings() {
        let dir = std::env::temp_dir().join(format!("igui-editor-crlf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crlf.txt");
        std::fs::write(&path, b"one\r\ntwo\r\n").unwrap();

        let mut doc = Document::open(1, &path).unwrap();
        assert_eq!(doc.line_ending(), LineEnding::Crlf);
        assert_eq!(doc.text(), "one\ntwo\n");
        doc.set_caret(doc.text().len());
        doc.insert_text("three\n");
        doc.save().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw, "one\r\ntwo\r\nthree\r\n");
        assert!(!raw.replace("\r\n", "").contains('\n'));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undo_to_the_save_point_clears_dirty() {
        let path =
            std::env::temp_dir().join(format!("igui-editor-save-point-{}.txt", std::process::id()));
        std::fs::write(&path, "base").unwrap();
        let mut doc = Document::open(1, &path).unwrap();
        doc.set_caret(doc.text().len());
        doc.insert_text("x");
        assert!(doc.is_dirty());
        doc.undo();
        assert!(!doc.is_dirty(), "undo to the opened content is clean");
        doc.redo();
        assert!(doc.is_dirty(), "redo leaves the save point");
        doc.undo();
        assert!(!doc.is_dirty());

        doc.insert_text("x");
        doc.save().unwrap();
        assert!(!doc.is_dirty());
        doc.undo();
        assert!(doc.is_dirty(), "undoing past the save point is dirty");
        doc.redo();
        assert!(!doc.is_dirty(), "redo back to the save point is clean");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn divergent_edit_invalidates_the_save_point() {
        let path = std::env::temp_dir().join(format!(
            "igui-editor-save-branch-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "base").unwrap();
        let mut doc = Document::open(1, &path).unwrap();
        doc.set_caret(doc.text().len());
        doc.insert_text("x");
        doc.save().unwrap();
        doc.undo();
        doc.insert_text("y");
        assert!(doc.is_dirty());
        doc.undo();
        assert!(
            doc.is_dirty(),
            "discarding the saved branch must not make the old base clean"
        );
        let _ = std::fs::remove_file(path);
    }

    fn assert_line_index(doc: &Document) {
        let mut expected = vec![0];
        for (index, byte) in doc.text().bytes().enumerate() {
            if byte == b'\n' {
                expected.push(index + 1);
            }
        }
        assert_eq!(doc.line_starts, expected);
    }

    #[test]
    fn incremental_line_index_matches_a_full_rebuild() {
        let mut doc = doc("one\ntwo\nthree\n");
        assert_line_index(&doc);

        doc.set_caret(doc.byte_at(1, 1));
        doc.insert_text("X");
        assert_eq!(doc.line(1), "tXwo");
        assert_line_index(&doc);

        doc.insert_text("\nnew");
        assert_eq!(doc.line_count(), 5);
        assert_eq!(doc.line(1), "tX");
        assert_eq!(doc.line(2), "newwo");
        assert_line_index(&doc);

        doc.set_caret(doc.byte_at(0, 3));
        doc.insert_text("\n");
        assert_line_index(&doc);

        doc.set_caret(doc.byte_at(2, 0));
        doc.backspace();
        assert_line_index(&doc);

        doc.undo();
        doc.undo();
        doc.redo();
        assert_line_index(&doc);
    }

    #[test]
    fn line_index_handles_insertion_at_a_line_start() {
        let mut doc = doc("a\nb");
        let line_start = doc.line_start(1);
        doc.set_caret(line_start);
        doc.insert_text("x");
        assert_eq!(doc.text(), "a\nxb");
        assert_eq!(doc.line_count(), 2);
        assert_line_index(&doc);
    }

    #[test]
    fn page_up_and_down_clamp() {
        let mut doc = doc("1\n2\n3\n4\n5");
        doc.set_caret(doc.byte_at(4, 0));
        doc.page(-3, false);
        assert_eq!(doc.line_col().0, 1);
        doc.page(10, false);
        assert_eq!(doc.line_col().0, 4);
    }
}
