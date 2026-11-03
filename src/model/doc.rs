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

use std::path::{Path, PathBuf};

/// Indentation inserted by Tab (and used for auto-indent on Enter).
pub const INDENT: &str = "    ";

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
    /// Bumped by every text mutation (edit / undo / redo); caches keyed by it
    /// (find matches, Markdown layout, highlighting) know when to rebuild.
    rev: u64,
    /// Line range touched by the last mutation, for incremental highlighting.
    edit_span: Option<(usize, usize)>,
    undo: UndoStack,
    /// Per-document scroll (kept across tab switches).
    pub scroll_y: f32,
    pub scroll_x: f32,
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
            rev: 0,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
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
            rev: 1,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
        };
        doc.caret = floor_boundary(&doc.text, caret);
        doc.anchor = floor_boundary(&doc.text, anchor);
        doc.rebuild_lines();
        doc
    }

    /// Replaces this buffer's content with a recovery snapshot (used when the
    /// same file is already open as a clean tab).
    pub fn restore_snapshot(&mut self, text: String, caret: usize, anchor: usize) {
        self.text = text;
        self.caret = floor_boundary(&self.text, caret);
        self.anchor = floor_boundary(&self.text, anchor);
        self.dirty = true;
        self.rev += 1;
        self.edit_span = None;
        self.undo = UndoStack::default();
        self.rebuild_lines();
    }

    /// Reads a file as UTF-8, normalizing `\r\n` / `\r` to `\n`.
    pub fn open(id: u64, path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let raw = std::fs::read_to_string(&path)?;
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
            rev: 0,
            edit_span: None,
            undo: UndoStack::default(),
            scroll_y: 0.0,
            scroll_x: 0.0,
        };
        doc.rebuild_lines();
        Ok(doc)
    }

    pub fn id(&self) -> u64 {
        self.id
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
        if start > end || end > self.text.len() {
            return false;
        }
        self.replace(start, end, text, Kind::Other);
        true
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
        self.text = normalize(&raw);
        self.caret = floor_boundary(&self.text, self.caret.min(self.text.len()));
        self.anchor = floor_boundary(&self.text, self.anchor.min(self.text.len()));
        self.dirty = false;
        self.rev += 1;
        self.edit_span = None;
        self.undo = UndoStack::default();
        self.rebuild_lines();
        Ok(())
    }

    /// Writes the buffer back to its file. Untitled buffers have no path.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "untitled buffer has no path (Save As is not implemented yet)",
            ));
        };
        std::fs::write(&path, &self.text)?;
        self.dirty = false;
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
            let previous = prev_boundary(&self.text, start);
            let ch = self.text[previous..start].chars().next().unwrap_or(' ');
            if !is_word_char(ch) {
                break;
            }
            start = previous;
        }
        let mut end = byte;
        while end < self.text.len() {
            let ch = self.text[end..].chars().next().unwrap_or(' ');
            if !is_word_char(ch) {
                break;
            }
            end = next_boundary(&self.text, end);
        }
        self.anchor = start;
        self.caret = end.max(start);
    }

    // -- edits ---------------------------------------------------------------

    pub fn insert_text(&mut self, insert: &str) {
        let insert = sanitize(insert);
        if insert.is_empty() {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let single = insert.chars().count() == 1 && !insert.contains('\n');
        let kind = if single { Kind::Type } else { Kind::Other };
        self.replace(start, end, &insert, kind);
    }

    /// Enter: insert a newline plus the current line's leading whitespace.
    pub fn insert_newline(&mut self) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let line = self.line_of(start);
        let indent: String = self
            .line(line)
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let insert = format!("\n{indent}");
        self.replace(start, end, &insert, Kind::Other);
    }

    pub fn backspace(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.replace(start, end, "", Kind::Other);
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = prev_boundary(&self.text, self.caret);
        self.replace(start, self.caret, "", Kind::Backspace);
    }

    pub fn delete_forward(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.replace(start, end, "", Kind::Other);
            return;
        }
        if self.caret >= self.text.len() {
            return;
        }
        let end = next_boundary(&self.text, self.caret);
        self.replace(self.caret, end, "", Kind::Delete);
    }

    /// Deletes the selection (no-op without one).
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.replace(start, end, "", Kind::Other);
        true
    }

    /// Tab: indent every selected line by [`INDENT`], or insert one unit.
    pub fn indent(&mut self) {
        let Some((start, end)) = self.selection() else {
            let (start, end) = (self.caret, self.caret);
            self.replace(start, end, INDENT, Kind::Other);
            return;
        };
        let first = self.line_of(start);
        let last = self.selection_last_line(start, end);
        let starts: Vec<usize> = (first..=last).map(|i| self.line_start(i)).collect();
        let block_start = starts[0];
        let block_end = self.line_end(last);
        let block = self.text[block_start..block_end].to_string();
        let mut out = String::with_capacity(block.len() + INDENT.len() * starts.len());
        for (index, line) in block.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(INDENT);
            out.push_str(line);
        }
        let deltas: Vec<isize> = starts.iter().map(|_| INDENT.len() as isize).collect();
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
    pub fn outdent(&mut self) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let first = self.line_of(start);
        let last = self.selection_last_line(start, end);
        let starts: Vec<usize> = (first..=last).map(|i| self.line_start(i)).collect();
        let block_start = starts[0];
        let block_end = self.line_end(last);
        let block = self.text[block_start..block_end].to_string();
        let mut out = String::with_capacity(block.len());
        let mut deltas: Vec<isize> = Vec::with_capacity(starts.len());
        for (index, line) in block.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let cut = leading_indent(line);
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

    pub fn undo(&mut self) -> bool {
        if self.undo.cursor == 0 {
            return false;
        }
        self.undo.cursor -= 1;
        let edit = self.undo.edits[self.undo.cursor].clone();
        let first_line = self.line_of(edit.start);
        let end = edit.start + edit.inserted.len();
        self.text.replace_range(edit.start..end, &edit.removed);
        let last_line = self.line_of((edit.start + edit.removed.len()).min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = edit.before.caret.min(self.text.len());
        self.anchor = edit.before.anchor.min(self.text.len());
        self.rebuild_lines();
        self.dirty = true;
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
        let last_line = self.line_of((edit.start + edit.inserted.len()).min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = edit.after.caret.min(self.text.len());
        self.anchor = edit.after.anchor.min(self.text.len());
        self.rebuild_lines();
        self.dirty = true;
        self.rev += 1;
        self.undo.cursor += 1;
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
            prev_boundary(&self.text, self.caret)
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
            next_boundary(&self.text, self.caret)
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
        self.text.replace_range(start..end, insert);
        let after_end = start + insert.len();
        let last_line = self.line_of(after_end.min(self.text.len()));
        self.edit_span = Some((first_line, last_line));
        self.caret = floor_boundary(&self.text, after.caret);
        self.anchor = floor_boundary(&self.text, after.anchor);
        self.rebuild_lines();
        self.rev += 1;
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

/// Number of bytes of leading indentation (one tab, or up to four spaces).
fn leading_indent(line: &str) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(4)
        .take_while(|byte| *byte == b' ')
        .count()
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

fn prev_word_boundary(text: &str, byte: usize) -> usize {
    let mut byte = floor_boundary(text, byte);
    while byte > 0 {
        let previous = prev_boundary(text, byte);
        let ch = text[previous..byte].chars().next().unwrap_or(' ');
        if is_word_char(ch) {
            break;
        }
        byte = previous;
    }
    while byte > 0 {
        let previous = prev_boundary(text, byte);
        let ch = text[previous..byte].chars().next().unwrap_or(' ');
        if !is_word_char(ch) {
            break;
        }
        byte = previous;
    }
    byte
}

fn next_word_boundary(text: &str, byte: usize) -> usize {
    let mut byte = floor_boundary(text, byte);
    while byte < text.len() {
        let ch = text[byte..].chars().next().unwrap_or(' ');
        if is_word_char(ch) {
            break;
        }
        byte = next_boundary(text, byte);
    }
    while byte < text.len() {
        let ch = text[byte..].chars().next().unwrap_or(' ');
        if !is_word_char(ch) {
            break;
        }
        byte = next_boundary(text, byte);
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
        doc.insert_newline();
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
        doc.indent();
        assert_eq!(doc.text(), "    a\n    b\n    c");
        assert!(doc.undo());
        assert_eq!(doc.text(), "a\nb\nc");
    }

    #[test]
    fn outdent_removes_up_to_four_spaces() {
        let mut doc = doc("      a\n  b");
        doc.select_all();
        doc.outdent();
        assert_eq!(doc.text(), "  a\nb");
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
    fn page_up_and_down_clamp() {
        let mut doc = doc("1\n2\n3\n4\n5");
        doc.set_caret(doc.byte_at(4, 0));
        doc.page(-3, false);
        assert_eq!(doc.line_col().0, 1);
        doc.page(10, false);
        assert_eq!(doc.line_col().0, 4);
    }
}
