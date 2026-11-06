//! Minimap content: per-line ink runs aggregated into a small pixel raster.
//!
//! The minimap is a structural thumbnail, not a second text renderer: each
//! logical line is reduced to a handful of non-blank **runs** (start/end column
//! plus token class), so indentation gaps and separate code blocks stay
//! visible instead of collapsing into one bar. Changing the viewport width or
//! the map height only re-rasterizes the cached runs; the document is never
//! re-measured for a resize.
//!
//! Runs are cached per document revision and updated incrementally from
//! [`Document::edit_span`], the same signal the highlight cache uses. Every
//! document mutation applies one contiguous replacement, so the span always
//! describes the whole edit; a revision gap larger than one (several mutations
//! between two paints) falls back to a full rebuild because the span then
//! describes only the last one.

use super::doc::Document;
use super::highlight::HighlightCache;
use crate::syntax::{Highlight, TokenKind};

/// Files above this size are not mapped (same policy as highlighting/wrap).
pub const MAX_MINIMAP_BYTES: usize = 2 * 1024 * 1024;
/// Files with more lines are not mapped.
pub const MAX_MINIMAP_LINES: usize = 20_000;
/// Height of one map row at full resolution: shorter documents stay top-aligned.
pub const MAP_ROW_H: f32 = 2.0;
/// Source columns the map shows before clipping at its right edge. One column
/// is about one logical pixel at the fixed strip width.
pub const MAX_COLUMNS: u32 = 100;
/// Upper bound on raster rows, so a very tall window cannot grow the map.
pub const MAX_BUCKETS: usize = 512;
/// Runs kept per line; busier lines merge their closest neighbours.
pub const MAX_LINE_RUNS: usize = 6;
/// Raw runs buffered while scanning a line (a pathological minified line folds
/// everything beyond this into its last run).
const MAX_RAW_RUNS: usize = 64;

/// One contiguous non-blank span of a logical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// First column of the run.
    pub start: u16,
    /// One past the last column of the run.
    pub end: u16,
    /// Token class painted for the run.
    pub kind: TokenKind,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            start: 0,
            end: 0,
            kind: TokenKind::Plain,
        }
    }
}

/// Ink of one logical line: overall extent plus up to [`MAX_LINE_RUNS`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineMark {
    /// Column of the first non-blank character (0 when the line is blank).
    pub start: u16,
    /// One past the column of the last non-blank character.
    pub end: u16,
    runs: [Run; MAX_LINE_RUNS],
    count: u8,
}

impl Default for LineMark {
    fn default() -> Self {
        Self {
            start: 0,
            end: 0,
            runs: [Run::default(); MAX_LINE_RUNS],
            count: 0,
        }
    }
}

impl LineMark {
    /// The non-blank runs, in column order.
    pub fn runs(&self) -> &[Run] {
        &self.runs[..self.count as usize]
    }
}

/// Whether `doc` is small enough to be mapped.
pub fn is_active(doc: &Document) -> bool {
    doc.text().len() <= MAX_MINIMAP_BYTES && doc.line_count() <= MAX_MINIMAP_LINES
}

/// Reduces one line to its ink runs.
///
/// `tokens` is the line's highlight slice when it is ready; without it every
/// run is plain. Tabs count as `tab_width` columns and whitespace breaks runs,
/// which is what keeps spaces and indentation visible on the map.
#[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
pub fn line_mark(text: &str, tab: usize, tokens: Option<&[Highlight]>) -> LineMark {
    let mut scratch = Vec::new();
    line_mark_into(text, tab, tokens, &mut scratch)
}

fn line_mark_into(
    text: &str,
    tab: usize,
    tokens: Option<&[Highlight]>,
    runs: &mut Vec<Run>,
) -> LineMark {
    runs.clear();
    let tab = tab.max(1) as u16;
    let tokens = tokens.unwrap_or_default();
    let mut column = 0_u16;
    let mut token_index = 0_usize;
    let mut byte = 0_usize;
    let mut current: Option<Run> = None;

    for ch in text.chars() {
        while token_index < tokens.len() && tokens[token_index].end <= byte {
            token_index += 1;
        }
        let kind = tokens
            .get(token_index)
            .filter(|token| token.start <= byte && byte < token.end)
            .map_or(TokenKind::Plain, |token| token.kind);
        let width = if ch == '\t' { tab } else { 1 };
        // Whitespace inside a styled token (a comment or string) is part of
        // the block; whitespace between plain tokens is a visible gap.
        if ch.is_whitespace() && kind == TokenKind::Plain {
            if let Some(run) = current.take() {
                push_run(runs, run);
            }
            column = column.saturating_add(width);
        } else {
            match &mut current {
                Some(run) if run.kind == kind => run.end = column.saturating_add(width),
                Some(_) => {
                    if let Some(run) = current.replace(Run {
                        start: column,
                        end: column.saturating_add(width),
                        kind,
                    }) {
                        push_run(runs, run);
                    }
                }
                None => {
                    current = Some(Run {
                        start: column,
                        end: column.saturating_add(width),
                        kind,
                    });
                }
            }
            column = column.saturating_add(width);
        }
        byte += ch.len_utf8();
    }
    if let Some(run) = current {
        push_run(runs, run);
    }
    cap_runs(runs);

    let start = runs.first().map_or(0, |run| run.start);
    let end = runs.last().map_or(0, |run| run.end);
    let mut mark = LineMark {
        start,
        end,
        ..LineMark::default()
    };
    mark.count = runs.len() as u8;
    mark.runs[..runs.len()].copy_from_slice(runs);
    mark
}

/// Appends a run, merging directly adjacent runs of the same class.
fn push_run(runs: &mut Vec<Run>, run: Run) {
    if run.end <= run.start {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.kind == run.kind && run.start <= last.end {
            last.end = last.end.max(run.end);
            return;
        }
    }
    if runs.len() >= MAX_RAW_RUNS {
        // A minified line can alternate classes on every character; folding
        // the tail into the last run keeps memory and paint cost bounded.
        if let Some(last) = runs.last_mut() {
            last.end = last.end.max(run.end);
        }
        return;
    }
    runs.push(run);
}

/// Merges the closest runs until at most [`MAX_LINE_RUNS`] remain.
fn cap_runs(runs: &mut Vec<Run>) {
    while runs.len() > MAX_LINE_RUNS {
        let mut best = 0usize;
        let mut best_gap = u16::MAX;
        for index in 0..runs.len() - 1 {
            let gap = runs[index + 1].start.saturating_sub(runs[index].end);
            if gap < best_gap {
                best_gap = gap;
                best = index;
            }
        }
        let left = runs[best];
        let right = runs[best + 1];
        // Keep the color of the longer half so a keyword or string does not
        // vanish behind the space it borders.
        let kind = if left.end - left.start >= right.end - right.start {
            left.kind
        } else {
            right.kind
        };
        runs[best] = Run {
            start: left.start,
            end: right.end,
            kind,
        };
        runs.remove(best + 1);
    }
}

/// Raster cell code for `kind`; `0` is reserved for an empty cell.
pub fn code_of(kind: TokenKind) -> u8 {
    match kind {
        TokenKind::Plain => 1,
        TokenKind::Keyword => 2,
        TokenKind::TypeName => 3,
        TokenKind::Function => 4,
        TokenKind::String => 5,
        TokenKind::Number => 6,
        TokenKind::Comment => 7,
        TokenKind::Constant => 8,
        TokenKind::Macro => 9,
        TokenKind::Punctuation => 10,
    }
}

/// Token class for a non-zero raster cell code.
pub fn kind_of(code: u8) -> TokenKind {
    match code {
        2 => TokenKind::Keyword,
        3 => TokenKind::TypeName,
        4 => TokenKind::Function,
        5 => TokenKind::String,
        6 => TokenKind::Number,
        7 => TokenKind::Comment,
        8 => TokenKind::Constant,
        9 => TokenKind::Macro,
        10 => TokenKind::Punctuation,
        _ => TokenKind::Plain,
    }
}

/// Per-document mark cache. One slot only: the minimap always shows the active
/// document, so switching tabs drops the old marks instead of keeping a second
/// copy of every buffer.
#[derive(Clone, Debug, Default)]
pub struct MinimapCache {
    doc: Option<u64>,
    rev: u64,
    tab: usize,
    line_count: usize,
    marks: Vec<LineMark>,
    max_end: u16,
    raster: Vec<u8>,
    raster_rows: usize,
    raster_cells: usize,
    /// Whether a provider was active when the marks were built.
    available: bool,
    /// Whether the highlight cache was complete, so every line has its color.
    colored: bool,
}

impl MinimapCache {
    /// Drops everything (no document / above the size caps).
    pub fn reset(&mut self) {
        self.doc = None;
        self.rev = 0;
        self.tab = 0;
        self.line_count = 0;
        self.marks.clear();
        self.max_end = 0;
        self.raster.clear();
        self.raster_rows = 0;
        self.raster_cells = 0;
        self.available = false;
        self.colored = false;
    }

    /// True when nothing is cached for the current document.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn is_empty(&self) -> bool {
        self.doc.is_none() || self.marks.is_empty()
    }

    /// Brings the cache up to date with `doc`, the tab width it is shown at and
    /// the active highlight cache.
    ///
    /// Token colors are filled only once highlighting is complete for the
    /// revision; a cache built during the budgeted catch-up is marked
    /// uncolored so the completion frame rebuilds it. When highlighting is
    /// still behind after an edit, incremental rescans use the tokens that are
    /// available (the edit is always at the head of the resume point) and keep
    /// the colors of untouched lines.
    ///
    /// Returns whether the incremental path ran (for tests and diagnostics).
    pub fn sync(&mut self, doc: &Document, tab: usize, highlight: &HighlightCache) -> bool {
        if !is_active(doc) {
            self.reset();
            return false;
        }
        let available = highlight.is_active();
        let ready = available && !highlight.is_behind();
        let id = doc.id();
        let rev = doc.rev();
        let same_doc = self.doc == Some(id) && self.tab == tab;
        if same_doc && self.rev == rev && self.available == available && (self.colored || !ready) {
            return false;
        }

        let incremental = same_doc
            && self.available == available
            && self.marks.len() == self.line_count
            && self.rev + 1 == rev
            && doc.edit_span().is_some()
            && !(ready && !self.colored);
        if incremental {
            if let Some((first, last)) = doc.edit_span() {
                let new_count = doc.line_count();
                let new_len = last.saturating_sub(first) + 1;
                let delta = new_count as isize - self.line_count as isize;
                let old_len = new_len as isize - delta;
                if first < self.line_count
                    && old_len >= 0
                    && first + old_len as usize <= self.line_count
                    && last < new_count
                {
                    let fresh = scan(doc, first, last, tab, highlight);
                    self.marks.splice(first..first + old_len as usize, fresh);
                    self.recompute();
                    self.line_count = new_count;
                    self.rev = rev;
                    return true;
                }
            }
        }
        self.rebuild(doc, tab, highlight);
        false
    }

    fn rebuild(&mut self, doc: &Document, tab: usize, highlight: &HighlightCache) {
        let line_count = doc.line_count();
        self.marks = scan(doc, 0, line_count.saturating_sub(1), tab, highlight);
        self.line_count = line_count;
        self.doc = Some(doc.id());
        self.rev = doc.rev();
        self.tab = tab;
        self.available = highlight.is_active();
        self.colored = self.available && !highlight.is_behind();
        self.recompute();
    }

    fn recompute(&mut self) {
        self.max_end = self.marks.iter().map(|mark| mark.end).max().unwrap_or(0);
        // The raster depends on the runs, so it must be rebuilt on any edit.
        self.raster.clear();
        self.raster_rows = 0;
        self.raster_cells = 0;
    }

    /// Column span the map normalizes against (at least 1, capped at
    /// [`MAX_COLUMNS`]).
    pub fn max_columns(&self) -> u32 {
        u32::from(self.max_end).clamp(1, MAX_COLUMNS)
    }

    /// Line marks (also the access point for incremental tests).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn marks(&self) -> &[LineMark] {
        &self.marks
    }

    /// Builds the map raster: `rows` stacked buckets of `cells` columns.
    ///
    /// Each cell stores the dominant token class covering it (`0` = empty), so
    /// paint can emit one rectangle per run and keep horizontal gaps. The
    /// raster is reused until the document, the row count or the cell count
    /// changes.
    pub fn raster(&mut self, rows: usize, cells: usize) -> &[u8] {
        let rows = rows.clamp(1, MAX_BUCKETS);
        let cells = cells.max(1);
        if self.raster_rows == rows
            && self.raster_cells == cells
            && self.raster.len() == rows * cells
        {
            return &self.raster;
        }
        self.raster.clear();
        self.raster.resize(rows * cells, 0);
        let lines = self.line_count.max(1);
        let scale = self.max_columns() as usize;
        for (index, mark) in self.marks.iter().enumerate() {
            let row = (index * rows / lines).min(rows - 1);
            let base = row * cells;
            for run in mark.runs() {
                let x0 = (run.start as usize * cells / scale).min(cells - 1);
                let x1 = (run.end as usize * cells / scale).max(x0 + 1).min(cells);
                let code = code_of(run.kind);
                for cell in &mut self.raster[base + x0..base + x1] {
                    // A styled class outranks plain ink from another line.
                    if *cell == 0 || (*cell == 1 && code != 1) {
                        *cell = code;
                    }
                }
            }
        }
        self.raster_rows = rows;
        self.raster_cells = cells;
        &self.raster
    }
}

fn scan(
    doc: &Document,
    first: usize,
    last: usize,
    tab: usize,
    highlight: &HighlightCache,
) -> Vec<LineMark> {
    let first = first.min(doc.line_count().saturating_sub(1));
    let last = last.min(doc.line_count().saturating_sub(1));
    if doc.line_count() == 0 || last < first {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(last - first + 1);
    let mut scratch = Vec::new();
    for line in first..=last {
        out.push(line_mark_into(
            doc.line(line),
            tab,
            highlight.tokens(line),
            &mut scratch,
        ));
    }
    out
}

// -- geometry ----------------------------------------------------------------
// Pure scalar math (no `Rect`), so paint and hit-testing share it and the
// model stays free of engine types.

/// Height of the mapped content inside a `map_h` box.
///
/// Short documents use [`MAP_ROW_H`] per line and stay top-aligned; documents
/// taller than the box are compressed to fill it.
pub fn content_height(line_count: usize, map_h: f32) -> f32 {
    if line_count == 0 || map_h <= 0.0 {
        return 0.0;
    }
    (line_count as f32 * MAP_ROW_H).min(map_h)
}

/// Number of map rows for `line_count` lines in a `map_h` box.
pub fn bucket_count(line_count: usize, map_h: f32) -> usize {
    if line_count == 0 {
        return 1;
    }
    let fit = (content_height(line_count, map_h) / MAP_ROW_H)
        .ceil()
        .max(1.0) as usize;
    fit.min(line_count).clamp(1, MAX_BUCKETS)
}

/// Document line under `offset` pixels from the top of the mapped content.
pub fn line_at(offset: f32, line_count: usize, content_h: f32) -> usize {
    if line_count == 0 {
        return 0;
    }
    if content_h <= 0.0 {
        return 0;
    }
    let ratio = (offset / content_h).clamp(0.0, 1.0);
    ((ratio * line_count as f32).floor() as usize).min(line_count - 1)
}

/// Viewport slider `(top, height)` in content pixels for the inclusive visible
/// line span `first..=last`.
pub fn slider(line_count: usize, first: usize, last: usize, content_h: f32) -> (f32, f32) {
    if line_count == 0 || content_h <= 0.0 {
        return (0.0, 0.0);
    }
    let total = line_count as f32;
    let first = first.min(line_count - 1);
    let last = last.min(line_count - 1).max(first);
    let top = (first as f32 / total) * content_h;
    let span = (last - first + 1) as f32;
    let height = ((span / total) * content_h)
        .max(MAP_ROW_H.min(content_h))
        .min(content_h - top)
        .max(1.0);
    (top, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::doc::{Document, IndentStyle, LineEnding};
    use crate::settings::Settings;
    use crate::syntax::SyntaxRegistry;

    fn doc(text: &str) -> Document {
        Document::from_snapshot(7, "map.txt".into(), None, text.into(), 0, 0, LineEnding::Lf)
    }

    fn rust_doc(text: &str) -> Document {
        Document::from_snapshot(
            9,
            "map.js".into(),
            Some("map.js".into()),
            text.into(),
            0,
            0,
            LineEnding::Lf,
        )
    }

    /// Highlight cache with no provider: marks stay plain.
    fn plain() -> HighlightCache {
        HighlightCache::new()
    }

    /// Highlight cache primed to completion for `doc` (Rust provider).
    fn colored(doc: &Document) -> HighlightCache {
        let registry = SyntaxRegistry::with_builtins();
        let mut highlight = HighlightCache::new();
        assert!(highlight.sync(doc, &registry, &Settings::default(), 100_000, None));
        highlight
    }

    fn fresh(text: &str) -> MinimapCache {
        let doc = doc(text);
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        cache
    }

    #[test]
    fn line_marks_track_indentation_and_trailing_space() {
        let mark = line_mark("    let value = 1;   ", 4, None);
        assert_eq!((mark.start, mark.end), (4, 18));
        assert_eq!(mark.runs().len(), 4);
        assert!(mark.runs().iter().all(|run| run.kind == TokenKind::Plain));
        assert_eq!((mark.runs()[0].start, mark.runs()[0].end), (4, 7));

        let mark = line_mark("\t\tfoo", 4, None);
        assert_eq!((mark.start, mark.end), (8, 11));
        assert_eq!(line_mark("", 4, None), LineMark::default());
        assert_eq!(line_mark("   ", 4, None), LineMark::default());
        // Wide glyphs count as one display column each.
        assert_eq!(
            (
                line_mark("中文", 4, None).start,
                line_mark("中文", 4, None).end
            ),
            (0, 2)
        );
    }

    #[test]
    fn line_runs_keep_token_colors_and_spaces() {
        let tokens = [
            Highlight {
                start: 0,
                end: 3,
                kind: TokenKind::Keyword,
            },
            Highlight {
                start: 8,
                end: 21,
                kind: TokenKind::String,
            },
            Highlight {
                start: 21,
                end: 22,
                kind: TokenKind::Punctuation,
            },
        ];
        let mark = line_mark("let x = \"long string\";", 4, Some(&tokens));
        let runs = mark.runs();
        // `let`, `x`, `=`, string, `;` — spaces stay as gaps between runs.
        assert_eq!(runs.len(), 5);
        assert_eq!(
            (runs[0].start, runs[0].end, runs[0].kind),
            (0, 3, TokenKind::Keyword)
        );
        assert_eq!((runs[1].start, runs[1].end), (4, 5));
        assert_eq!((runs[2].start, runs[2].end), (6, 7));
        assert_eq!(
            (runs[3].start, runs[3].end, runs[3].kind),
            (8, 21, TokenKind::String)
        );
        assert_eq!(
            (runs[4].start, runs[4].end, runs[4].kind),
            (21, 22, TokenKind::Punctuation)
        );
    }

    #[test]
    fn busy_lines_merge_to_the_run_cap() {
        let text = "abcdefghij";
        let tokens: Vec<Highlight> = text
            .char_indices()
            .enumerate()
            .map(|(index, (start, ch))| Highlight {
                start,
                end: start + ch.len_utf8(),
                kind: if index % 2 == 0 {
                    TokenKind::Keyword
                } else {
                    TokenKind::String
                },
            })
            .collect();
        let mark = line_mark(text, 4, Some(&tokens));
        assert_eq!(mark.runs().len(), MAX_LINE_RUNS);
        assert_eq!((mark.start, mark.end), (0, 10));
        assert_eq!(mark.runs().last().unwrap().end, 10);
    }

    #[test]
    fn max_columns_is_capped_for_very_long_lines() {
        let cache = fresh(&format!("{}\nshort\n", "x".repeat(400)));
        assert_eq!(cache.max_columns(), MAX_COLUMNS);
        let short = fresh("abc\ndef\n");
        assert_eq!(short.max_columns(), 3);
    }

    #[test]
    fn raster_keeps_gaps_and_token_classes() {
        let doc = rust_doc("  const value = \"text\";\n");
        let highlight = colored(&doc);
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &highlight);
        let cells = 64;
        let raster = cache.raster(1, cells);
        // Leading indentation is empty, then code starts.
        assert!(raster[..5].iter().all(|&code| code == 0));
        assert!(raster.iter().any(|&code| code != 0));
        assert!(raster.iter().any(|&code| kind_of(code).is_styled()));
        // The space after `const` (and around `=`) leaves an internal gap:
        // this is what a single unioned bar used to hide.
        let first = raster.iter().position(|&code| code != 0).unwrap();
        let last = raster.iter().rposition(|&code| code != 0).unwrap();
        assert!(raster[first..=last].contains(&0));
    }

    #[test]
    fn blank_document_has_no_ink() {
        let mut cache = fresh("");
        let raster = cache.raster(8, 32);
        assert!(raster.iter().all(|&code| code == 0));
        assert_eq!(cache.max_columns(), 1);
    }

    #[test]
    fn geometry_maps_short_and_long_documents() {
        // 10 lines, 100 px box: 2 px rows at the top, 20 px of content.
        assert_eq!(content_height(10, 100.0), 20.0);
        assert_eq!(bucket_count(10, 100.0), 10);
        assert_eq!(line_at(0.0, 10, 20.0), 0);
        assert_eq!(line_at(10.0, 10, 20.0), 5);
        assert_eq!(line_at(19.9, 10, 20.0), 9);
        // Clicking the empty area below the content clamps to the last line.
        assert_eq!(line_at(80.0, 10, 20.0), 9);

        // 4000 lines in a 700 px box: compressed to the full height.
        assert_eq!(content_height(4000, 700.0), 700.0);
        assert_eq!(bucket_count(4000, 700.0), 350);
        assert_eq!(bucket_count(100_000, 10_000.0), MAX_BUCKETS);
        assert_eq!(line_at(350.0, 4000, 700.0), 2000);
        assert_eq!(line_at(700.0, 4000, 700.0), 3999);
    }

    #[test]
    fn slider_covers_the_visible_line_span() {
        let (top, height) = slider(1000, 100, 199, 500.0);
        assert!((top - 50.0).abs() < 0.01);
        assert!((height - 50.0).abs() < 0.01);
        // At the end the slider cannot spill past the content.
        let (top, height) = slider(1000, 900, 999, 500.0);
        assert!((top - 450.0).abs() < 0.01);
        assert!((top + height - 500.0).abs() < 0.01);
        // A single line still gets a visible slider.
        let (_, height) = slider(1000, 500, 500, 500.0);
        assert!(height >= 1.0);
    }

    #[test]
    fn caps_disable_the_map() {
        let doc = doc("one\ntwo\n");
        assert!(is_active(&doc));
        let big = doc_of_lines(MAX_MINIMAP_LINES + 1);
        assert!(!is_active(&big));
    }

    fn doc_of_lines(lines: usize) -> Document {
        let mut text = String::new();
        for index in 0..lines {
            text.push_str(&format!("line {index}\n"));
        }
        doc(&text)
    }

    /// Applies a mutation and checks that incremental marks match a rebuild.
    fn assert_incremental(
        cache: &mut MinimapCache,
        doc: &mut Document,
        label: &str,
        mutate: impl FnOnce(&mut Document),
    ) {
        let before = doc.line_count();
        mutate(doc);
        cache.sync(doc, 4, &plain());
        let mut fresh = MinimapCache::default();
        fresh.sync(doc, 4, &plain());
        assert_eq!(
            cache.marks(),
            fresh.marks(),
            "{label} (lines {before} -> {})",
            doc.line_count()
        );
        assert_eq!(cache.max_columns(), fresh.max_columns(), "{label}");
    }

    #[test]
    fn incremental_updates_match_a_full_rebuild() {
        let mut doc = doc("fn main() {\n    let a = 1;\n}\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        let baseline = cache.marks().to_vec();

        // Typing inside a line.
        doc.click(1, 4, false);
        assert_incremental(&mut cache, &mut doc, "typing", |doc| doc.insert_text("x"));

        // Newline (line count grows, then shrinks again).
        assert_incremental(&mut cache, &mut doc, "newline", |doc| {
            doc.insert_newline(false, IndentStyle::Spaces(4));
        });
        doc.click(0, 0, false);
        assert_incremental(&mut cache, &mut doc, "delete line", |doc| {
            doc.cut_current_line();
        });

        // Multi-line indent (one contiguous replacement).
        doc.select_all();
        assert_incremental(&mut cache, &mut doc, "indent", |doc| {
            doc.indent(IndentStyle::Spaces(4));
        });

        // Undo restores the previous text and must converge to the baseline.
        doc.undo();
        cache.sync(&doc, 4, &plain());
        let mut fresh = MinimapCache::default();
        fresh.sync(&doc, 4, &plain());
        assert_eq!(cache.marks(), fresh.marks(), "undo");
        assert_eq!(cache.marks().len(), baseline.len());
    }

    #[test]
    fn revision_gap_forces_a_full_rebuild() {
        let mut doc = doc("a\nb\nc\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        // Two mutations before the next sync: `edit_span` only describes the
        // second one, so the cache must rebuild.
        doc.click(0, 0, false);
        doc.insert_text("x");
        doc.insert_text("y");
        cache.sync(&doc, 4, &plain());
        assert_eq!(cache.marks()[0], line_mark(doc.line(0), 4, None));
    }

    #[test]
    fn tab_width_change_rebuilds() {
        let doc = doc("\tindented\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        assert_eq!(cache.marks()[0].start, 4);
        cache.sync(&doc, 8, &plain());
        assert_eq!(cache.marks()[0].start, 8);
    }

    #[test]
    fn random_edits_converge_to_a_rebuild() {
        let mut doc = doc("alpha\nbeta\ngamma\ndelta\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        let mut expected_incremental = 0usize;
        let mut seed = 0x1234_5678_u64;
        for step in 0..400 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let line = ((seed >> 24) as usize) % doc.line_count();
            match (seed >> 33) % 7 {
                0 => {
                    doc.click(line, 0, false);
                    doc.insert_text("word ");
                }
                1 => {
                    doc.click(line, 0, false);
                    doc.insert_text("\n");
                }
                2 => {
                    doc.click(line, 0, false);
                    doc.backspace();
                }
                3 => {
                    doc.click(line, 0, false);
                    doc.delete_forward();
                }
                4 => {
                    doc.undo();
                }
                5 => {
                    doc.click(line, 0, false);
                    doc.cut_current_line();
                }
                _ => {
                    let count = ((seed >> 16) as usize) % 6;
                    doc.click(line, 0, false);
                    doc.insert_text(&"x".repeat(count));
                }
            }
            // Count the fast path itself so the test cannot silently fall back
            // to full rebuilds.
            if cache.sync(&doc, 4, &plain()) {
                expected_incremental += 1;
            }
            let mut fresh = MinimapCache::default();
            fresh.sync(&doc, 4, &plain());
            assert_eq!(
                cache.marks(),
                fresh.marks(),
                "step {step} diverged (incremental runs: {expected_incremental})"
            );
            assert_eq!(cache.max_columns(), fresh.max_columns(), "step {step}");
        }
        assert!(
            expected_incremental > 100,
            "the incremental path ran only {expected_incremental} times"
        );
    }

    #[test]
    fn enabling_syntax_recolors_the_raster() {
        let doc = rust_doc("fn main() {}\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        assert!(cache
            .raster(1, 32)
            .iter()
            .all(|&code| !kind_of(code).is_styled()));
        cache.sync(&doc, 4, &colored(&doc));
        assert!(cache
            .raster(1, 32)
            .iter()
            .any(|&code| kind_of(code).is_styled()));
    }

    #[test]
    fn colored_incremental_edits_match_a_rebuild() {
        let mut doc = rust_doc("fn main() {\n    let s = \"text\";\n}\n");
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        let mut highlight = HighlightCache::new();
        assert!(highlight.sync(&doc, &registry, &settings, 100_000, None));
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &highlight);
        let mut colored_steps = 0;
        for step in 0..40 {
            doc.click(1, 4, false);
            doc.insert_text(if step % 3 == 0 { "\"x\"" } else { "y" });
            assert!(highlight.sync(&doc, &registry, &settings, 100_000, None));
            cache.sync(&doc, 4, &highlight);
            let mut fresh = MinimapCache::default();
            fresh.sync(&doc, 4, &highlight);
            assert_eq!(cache.marks(), fresh.marks(), "step {step}");
            assert_eq!(cache.max_columns(), fresh.max_columns(), "step {step}");
            if cache
                .raster(8, 32)
                .iter()
                .any(|&code| kind_of(code).is_styled())
            {
                colored_steps += 1;
            }
        }
        assert!(
            colored_steps > 10,
            "colors disappeared during incremental edits ({colored_steps})"
        );
    }

    #[test]
    fn reset_clears_the_slot() {
        let doc = doc("one\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        assert!(!cache.is_empty());
        cache.reset();
        assert!(cache.is_empty());
        assert!(cache.marks().is_empty());
        assert!(cache.raster(4, 16).iter().all(|&code| code == 0));
    }
}
