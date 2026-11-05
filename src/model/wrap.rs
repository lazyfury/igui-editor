//! Soft-wrap layout: display rows for logical document lines.
//!
//! The document stays line/column based; this module only maps a logical line
//! to one or more visual rows. It is rebuilt when the document, width, font or
//! tab width changes and is deliberately capped: soft wrap is a comfort feature,
//! not a large-file renderer.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use igui_ui::TextMeasurer;

use super::doc::Document;
use crate::fonts;

/// Soft wrap is disabled above these limits (status bar explains the fallback).
pub const MAX_WRAP_LINES: usize = 20_000;
pub const MAX_WRAP_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WrapKey {
    doc: u64,
    rev: u64,
    width: u32,
    font: u32,
    tab: u8,
    /// Hash of the fixed-pitch family (the UI family is implied for wide runs).
    family: u64,
}

impl WrapKey {
    pub fn new(
        doc: u64,
        rev: u64,
        width: f32,
        font: f32,
        tab: usize,
        family: Option<&str>,
    ) -> Self {
        let mut hasher = DefaultHasher::new();
        family.hash(&mut hasher);
        Self {
            doc,
            rev,
            width: width.round().to_bits(),
            font: font.round().to_bits(),
            tab: tab.min(u8::MAX as usize) as u8,
            family: hasher.finish(),
        }
    }
}

/// One visual row: a character span inside a logical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    /// True for the first visual row of its logical line.
    pub first: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WrapLayout {
    key: Option<WrapKey>,
    rows: Vec<Row>,
    /// `(first row, one-past-last row)` per logical line.
    line_rows: Vec<(usize, usize)>,
}

impl WrapLayout {
    pub fn key(&self) -> Option<WrapKey> {
        self.key
    }

    pub fn matches(&self, doc: u64, rev: u64) -> bool {
        self.key.is_some_and(|key| key.doc == doc && key.rev == rev)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn line_rows(&self, line: usize) -> &[Row] {
        let Some(&(start, end)) = self.line_rows.get(line) else {
            return &[];
        };
        &self.rows[start..end]
    }

    /// Visual row containing `(line, column)`; boundary columns stay on the
    /// earlier row so a caret at the wrap edge is drawn at the right edge.
    pub fn row_of(&self, line: usize, column: usize) -> Option<usize> {
        let (start, end) = *self.line_rows.get(line)?;
        let rows = &self.rows[start..end];
        for (offset, row) in rows.iter().enumerate() {
            if column <= row.end_col {
                return Some(start + offset);
            }
        }
        end.checked_sub(1)
    }

    pub fn line_of_row(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(|row| row.line)
    }

    /// First visual row of `line` (clamped to the document).
    pub fn first_row_of_line(&self, line: usize) -> usize {
        self.line_rows
            .get(line.min(self.line_rows.len().saturating_sub(1)))
            .map_or(0, |(start, _)| *start)
    }

    pub fn build(
        doc: &Document,
        width: f32,
        font: f32,
        tab: usize,
        family: Option<&str>,
        measurer: &dyn TextMeasurer,
    ) -> Self {
        let width = width.max(16.0);
        let mut rows = Vec::new();
        let mut line_rows = Vec::with_capacity(doc.line_count());
        for line in 0..doc.line_count() {
            let text = doc.line(line);
            let start_index = rows.len();
            let mut start_col = 0;
            let mut start_byte = 0;
            let mut col = 0;
            let mut byte = 0;
            let mut used = 0.0_f32;
            let mut has_content = false;
            // (end column, end byte, width after the break) for the last
            // whitespace seen after actual content. Breaking there keeps words
            // intact instead of cutting them at the viewport edge.
            let mut last_break: Option<(usize, usize, f32)> = None;

            for ch in text.chars() {
                let advance = if ch == '\t' {
                    fonts::advance(measurer, ' ', font, family) * tab as f32
                } else {
                    fonts::advance(measurer, ch, font, family)
                };
                let whitespace = ch.is_whitespace();
                let len = ch.len_utf8();
                if col > start_col && used + advance > width {
                    let (break_col, break_byte, break_used) = if whitespace {
                        // Keep the trailing space in the previous row: a row
                        // starting with the separator would waste a line.
                        (col + 1, byte + len, used + advance)
                    } else if let Some(breakpoint) = last_break {
                        breakpoint
                    } else {
                        // A single token wider than the viewport: fall back to
                        // a character boundary.
                        (col, byte, used)
                    };
                    rows.push(Row {
                        line,
                        start_col,
                        end_col: break_col,
                        start_byte,
                        end_byte: break_byte,
                        first: start_col == 0,
                    });
                    let carried = used - break_used;
                    start_col = break_col;
                    start_byte = break_byte;
                    used = carried;
                    has_content = carried > 0.0;
                    last_break = None;
                    if whitespace && break_col == col + 1 {
                        // The overflowing whitespace belongs to the row we
                        // just emitted, so do not process it again.
                        col += 1;
                        byte += len;
                        continue;
                    }
                }
                used += advance;
                col += 1;
                byte += len;
                if whitespace {
                    if has_content && used <= width {
                        last_break = Some((col, byte, used));
                    }
                } else {
                    has_content = true;
                }
            }
            rows.push(Row {
                line,
                start_col,
                end_col: col,
                start_byte,
                end_byte: byte,
                first: start_col == 0,
            });
            line_rows.push((start_index, rows.len()));
        }
        Self {
            key: Some(WrapKey::new(doc.id(), doc.rev(), width, font, tab, family)),
            rows,
            line_rows,
        }
    }
}

pub fn expand_tabs(line: &str, tab: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + tab * 2);
    for ch in line.chars() {
        if ch == '\t' {
            out.extend(std::iter::repeat_n(' ', tab));
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn width_of(
    measurer: &dyn TextMeasurer,
    text: &str,
    font: f32,
    tab: usize,
    family: Option<&str>,
) -> f32 {
    let display = expand_tabs(text, tab);
    fonts::measure_run_weighted(
        measurer,
        &display,
        font,
        igui_core::FontWeight::NORMAL,
        family,
    )
}

/// Character column whose boundary is closest to `x` within `text`.
pub fn column_at_x(
    measurer: &dyn TextMeasurer,
    text: &str,
    x: f32,
    font: f32,
    tab: usize,
    family: Option<&str>,
) -> usize {
    if x <= 0.0 {
        return 0;
    }
    let mut used = 0.0;
    let mut column = 0;
    for ch in text.chars() {
        let advance = if ch == '\t' {
            fonts::advance(measurer, ' ', font, family) * tab as f32
        } else {
            fonts::advance(measurer, ch, font, family)
        };
        if x < used + advance * 0.5 {
            return column;
        }
        used += advance;
        column += 1;
    }
    column
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// Every character is exactly 10 px wide, so wrap widths are predictable.
    struct Fixed;

    impl TextMeasurer for Fixed {
        fn advance(&self, _ch: char, _font_size: f32) -> f32 {
            10.0
        }

        fn line_height(&self, _font_size: f32) -> f32 {
            20.0
        }
    }

    fn doc(text: &str) -> Document {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "igui-editor-wrap-{}-{}",
            std::process::id(),
            text.len()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        fs::write(&path, text).unwrap();
        let doc = Document::open(1, &path).unwrap();
        let _ = fs::remove_dir_all(dir);
        doc
    }

    #[test]
    fn wraps_at_spaces_without_starting_a_row_with_a_separator() {
        let doc = doc("alpha beta gamma");
        let wrap = WrapLayout::build(&doc, 50.0, 13.0, 4, None, &Fixed);
        let rows = wrap.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(&doc.text()[rows[0].start_byte..rows[0].end_byte], "alpha ");
        assert_eq!(&doc.text()[rows[1].start_byte..rows[1].end_byte], "beta ");
        assert_eq!(&doc.text()[rows[2].start_byte..rows[2].end_byte], "gamma");
        assert!(rows[1].start_col > 0);
    }

    #[test]
    fn long_words_fall_back_to_character_boundaries() {
        let doc = doc("abcdefghij");
        let wrap = WrapLayout::build(&doc, 50.0, 13.0, 4, None, &Fixed);
        let rows = wrap.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(&doc.text()[rows[0].start_byte..rows[0].end_byte], "abcde");
        assert_eq!(&doc.text()[rows[1].start_byte..rows[1].end_byte], "fghij");
    }

    #[test]
    fn leading_indentation_stays_with_the_first_word() {
        let doc = doc("    alpha beta");
        let wrap = WrapLayout::build(&doc, 100.0, 13.0, 4, None, &Fixed);
        let rows = wrap.rows();
        assert!(rows[0].start_col == 0);
        assert_eq!(
            &doc.text()[rows[0].start_byte..rows[0].end_byte],
            "    alpha "
        );
        assert_eq!(&doc.text()[rows[1].start_byte..rows[1].end_byte], "beta");
    }

    #[test]
    fn long_lines_split_into_rows_with_line_mapping() {
        let doc = doc(&format!("{}\nshort\n", "abcdefghij".repeat(10)));
        let measurer = igui_ui::ApproxTextMeasurer;
        let wrap = WrapLayout::build(&doc, 80.0, 13.0, 4, None, &measurer);
        assert!(wrap.len() > doc.line_count());
        assert_eq!(wrap.line_of_row(0), Some(0));
        assert!(wrap.line_rows(0).len() > 1);
        assert_eq!(wrap.line_rows(1).len(), 1);
        assert_eq!(wrap.first_row_of_line(1), wrap.line_rows(0).len());
        // Column at the end of the first row stays on that row.
        let first = wrap.rows()[0];
        assert_eq!(wrap.row_of(0, first.end_col), Some(0));
        assert_eq!(wrap.row_of(0, first.end_col + 1), Some(1));
    }
}
