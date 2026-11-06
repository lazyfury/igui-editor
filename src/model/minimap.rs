//! Minimap content: one ink mark per logical line, aggregated into pixel rows.
//!
//! The minimap is a structural thumbnail, not a second text renderer: a line is
//! reduced to its first and last non-blank *column*, and lines are grouped into
//! buckets that share one draw rectangle. That keeps the map independent of the
//! font, the viewport width and the theme, and keeps the paint cost bounded by
//! the map height instead of the document size.
//!
//! Marks are cached per document revision and updated incrementally from
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
/// Number of source columns the map shows before clipping at its right edge.
pub const MAX_COLUMNS: u32 = 120;
/// Upper bound on painted buckets, so a very tall window cannot grow the list.
pub const MAX_BUCKETS: usize = 512;
/// Classes tracked per bucket: `Plain` plus the eight styled non-punctuation
/// kinds. Punctuation folds into `Plain` (it is structural, not an accent).
const KIND_COUNT: usize = 9;

/// Ink extent of one logical line, in display columns (tabs expanded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineMark {
    /// Column of the first non-blank character (0 when the line is blank).
    pub start: u32,
    /// One past the column of the last non-blank character.
    pub end: u32,
    /// Most prominent styled token on the line (`Plain` when none).
    pub kind: TokenKind,
}

impl Default for LineMark {
    fn default() -> Self {
        Self {
            start: 0,
            end: 0,
            kind: TokenKind::Plain,
        }
    }
}

impl LineMark {
    /// Whether the line contains anything worth painting.
    pub fn has_ink(&self) -> bool {
        self.end > self.start
    }
}

/// Lines that share one map row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bucket {
    pub x0: u32,
    pub x1: u32,
    /// Lines in this bucket that contain ink.
    pub inked: u32,
    /// Lines in this bucket (including blank ones).
    pub lines: u32,
    /// Column coverage per [`TokenKind`] class (indexed by [`kind_index`]).
    kinds: [u32; KIND_COUNT],
}

impl Bucket {
    /// Most prominent styled class in this bucket (`Plain` when none).
    pub fn kind(&self) -> TokenKind {
        dominant(&self.kinds)
    }

    /// Share of lines with ink, in `0.0..=1.0`.
    pub fn density(&self) -> f32 {
        if self.lines == 0 {
            0.0
        } else {
            self.inked as f32 / self.lines as f32
        }
    }
}

/// Whether `doc` is small enough to be mapped.
pub fn is_active(doc: &Document) -> bool {
    doc.text().len() <= MAX_MINIMAP_BYTES && doc.line_count() <= MAX_MINIMAP_LINES
}

/// First/last non-blank columns of one line, plus its most prominent styled
/// token class. `tokens` is the line's highlight slice when it is ready.
pub fn line_mark(text: &str, tab: usize, tokens: Option<&[Highlight]>) -> LineMark {
    let tab = tab.max(1) as u32;
    let mut column = 0_u32;
    let mut start = 0_u32;
    let mut end = 0_u32;
    for ch in text.chars() {
        if ch == '\t' {
            column += tab;
        } else if ch.is_whitespace() {
            column += 1;
        } else {
            if end == 0 {
                start = column;
            }
            column += 1;
            end = column;
        }
    }

    let mut weights = [0_u32; KIND_COUNT];
    for token in tokens.unwrap_or_default() {
        if !token.kind.is_styled() {
            continue;
        }
        let weight = token.end.saturating_sub(token.start).max(1) as u32;
        weights[kind_index(token.kind)] = weights[kind_index(token.kind)].saturating_add(weight);
    }
    LineMark {
        start,
        end,
        kind: dominant(&weights),
    }
}

/// Index into the per-class weight table. Punctuation is structural and folds
/// into `Plain`, so it never wins the color choice.
fn kind_index(kind: TokenKind) -> usize {
    match kind {
        TokenKind::Plain | TokenKind::Punctuation => 0,
        TokenKind::Keyword => 1,
        TokenKind::TypeName => 2,
        TokenKind::Function => 3,
        TokenKind::String => 4,
        TokenKind::Number => 5,
        TokenKind::Comment => 6,
        TokenKind::Constant => 7,
        TokenKind::Macro => 8,
    }
}

fn kind_at(index: usize) -> TokenKind {
    match index {
        1 => TokenKind::Keyword,
        2 => TokenKind::TypeName,
        3 => TokenKind::Function,
        4 => TokenKind::String,
        5 => TokenKind::Number,
        6 => TokenKind::Comment,
        7 => TokenKind::Constant,
        8 => TokenKind::Macro,
        _ => TokenKind::Plain,
    }
}

/// Most prominent styled class; `Plain` when every styled weight is zero.
fn dominant(weights: &[u32; KIND_COUNT]) -> TokenKind {
    let mut best = 0usize;
    let mut best_weight = 0u32;
    for (index, &weight) in weights.iter().enumerate().skip(1) {
        if weight > best_weight {
            best = index;
            best_weight = weight;
        }
    }
    if best_weight == 0 {
        TokenKind::Plain
    } else {
        kind_at(best)
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
    max_end: u32,
    buckets: Vec<Bucket>,
    bucket_count: usize,
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
        self.buckets.clear();
        self.bucket_count = 0;
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
        self.buckets.clear();
        self.bucket_count = 0;
    }

    /// Column span the map normalizes against (at least 1, capped at
    /// [`MAX_COLUMNS`]).
    pub fn max_columns(&self) -> u32 {
        self.max_end.clamp(1, MAX_COLUMNS)
    }

    /// Line marks (also the access point for incremental tests).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn marks(&self) -> &[LineMark] {
        &self.marks
    }

    /// Aggregates marks into `bucket_count` rows, reusing the previous result
    /// while the count is unchanged.
    pub fn buckets(&mut self, bucket_count: usize) -> &[Bucket] {
        let bucket_count = bucket_count.clamp(1, MAX_BUCKETS);
        if self.bucket_count == bucket_count {
            return &self.buckets;
        }
        self.buckets.clear();
        self.buckets.resize(bucket_count, Bucket::default());
        let lines = self.line_count.max(1);
        for (index, mark) in self.marks.iter().enumerate() {
            let bucket = (index * bucket_count / lines).min(bucket_count - 1);
            let bucket = &mut self.buckets[bucket];
            bucket.lines += 1;
            if mark.has_ink() {
                if bucket.inked == 0 {
                    bucket.x0 = mark.start;
                    bucket.x1 = mark.end;
                } else {
                    bucket.x0 = bucket.x0.min(mark.start);
                    bucket.x1 = bucket.x1.max(mark.end);
                }
                bucket.inked += 1;
                bucket.kinds[kind_index(mark.kind)] = bucket.kinds[kind_index(mark.kind)]
                    .saturating_add(mark.end.saturating_sub(mark.start).max(1));
            }
        }
        self.bucket_count = bucket_count;
        &self.buckets
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
    for line in first..=last {
        out.push(line_mark(doc.line(line), tab, highlight.tokens(line)));
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

    /// Highlight cache with no provider: marks stay plain.
    fn plain() -> HighlightCache {
        HighlightCache::new()
    }

    fn doc(text: &str) -> Document {
        Document::from_snapshot(7, "map.txt".into(), None, text.into(), 0, 0, LineEnding::Lf)
    }

    fn fresh(text: &str) -> MinimapCache {
        let doc = doc(text);
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        cache
    }

    #[test]
    fn line_marks_track_indentation_and_trailing_space() {
        assert_eq!(
            line_mark("    let value = 1;   ", 4, None),
            LineMark {
                start: 4,
                end: 18,
                kind: TokenKind::Plain,
            }
        );
        assert_eq!(
            line_mark("\t\tfoo", 4, None),
            LineMark {
                start: 8,
                end: 11,
                kind: TokenKind::Plain,
            }
        );
        assert_eq!(line_mark("", 4, None), LineMark::default());
        assert_eq!(line_mark("   ", 4, None), LineMark::default());
        // Wide glyphs count as one display column each.
        assert_eq!(
            line_mark("中文", 4, None),
            LineMark {
                start: 0,
                end: 2,
                kind: TokenKind::Plain,
            }
        );
    }

    #[test]
    fn max_columns_is_capped_for_very_long_lines() {
        let cache = fresh(&format!("{}\nshort\n", "x".repeat(400)));
        assert_eq!(cache.max_columns(), MAX_COLUMNS);
        let short = fresh("abc\ndef\n");
        assert_eq!(short.max_columns(), 3);
    }

    #[test]
    fn buckets_aggregate_extents_and_density() {
        let mut cache = fresh("    a\n  bbb\n\nzzzz");
        let buckets = cache.buckets(4);
        assert_eq!(buckets.len(), 4);
        // One line per bucket: exact extents.
        assert_eq!(
            (
                buckets[0].x0,
                buckets[0].x1,
                buckets[0].inked,
                buckets[0].lines
            ),
            (4, 5, 1, 1)
        );
        assert_eq!(
            (
                buckets[1].x0,
                buckets[1].x1,
                buckets[1].inked,
                buckets[1].lines
            ),
            (2, 5, 1, 1)
        );
        assert_eq!((buckets[2].inked, buckets[2].lines), (0, 1));
        assert_eq!(
            (
                buckets[3].x0,
                buckets[3].x1,
                buckets[3].inked,
                buckets[3].lines
            ),
            (0, 4, 1, 1)
        );

        // Compressing two lines per bucket unions their extents.
        let buckets = cache.buckets(2);
        assert_eq!(
            (
                buckets[0].x0,
                buckets[0].x1,
                buckets[0].inked,
                buckets[0].lines
            ),
            (2, 5, 2, 2)
        );
        assert_eq!((buckets[1].x0, buckets[1].x1), (0, 4));
        assert_eq!(buckets[1].density(), 0.5);

        // Re-requesting the same count reuses the rows.
        assert_eq!(cache.buckets(2).len(), 2);
    }

    #[test]
    fn blank_document_has_no_ink() {
        let mut cache = fresh("");
        let buckets = cache.buckets(8);
        assert!(buckets.iter().all(|bucket| bucket.x1 <= bucket.x0));
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
        let mut doc = doc("\tindented\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &plain());
        assert_eq!(cache.marks()[0].start, 4);
        cache.sync(&doc, 8, &plain());
        assert_eq!(cache.marks()[0].start, 8);
        // No document change needed for a rebuild.
        doc.click(0, 0, false);
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

    fn rust_doc(text: &str) -> Document {
        Document::from_snapshot(
            9,
            "map.rs".into(),
            Some("map.rs".into()),
            text.into(),
            0,
            0,
            LineEnding::Lf,
        )
    }

    /// Highlight cache primed to completion for `doc` (Rust provider).
    fn colored(doc: &Document) -> HighlightCache {
        let registry = SyntaxRegistry::with_builtins();
        let mut highlight = HighlightCache::new();
        assert!(highlight.sync(doc, &registry, &Settings::default(), 100_000, None));
        highlight
    }

    #[test]
    fn line_marks_pick_the_dominant_styled_token() {
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
        assert_eq!(mark.kind, TokenKind::String);
        // Punctuation is structural and never wins.
        let punct = [Highlight {
            start: 0,
            end: 4,
            kind: TokenKind::Punctuation,
        }];
        assert_eq!(line_mark("(){}", 4, Some(&punct)).kind, TokenKind::Plain);
        assert_eq!(line_mark("plain", 4, None).kind, TokenKind::Plain);
    }

    #[test]
    fn token_colors_are_aggregated_into_buckets() {
        let doc = rust_doc("fn main() {\n    let s = \"a long string\";\n}\n");
        let highlight = colored(&doc);
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &highlight);
        assert!(
            cache.marks().iter().any(|mark| mark.kind.is_styled()),
            "no styled line marks"
        );
        assert_eq!(cache.marks()[1].kind, TokenKind::String);
        {
            let buckets = cache.buckets(3);
            assert!(
                buckets.iter().any(|bucket| bucket.kind().is_styled()),
                "no styled buckets"
            );
        }
    }

    #[test]
    fn enabling_syntax_recolors_the_map() {
        let doc = rust_doc("fn main() {}\n");
        let mut cache = MinimapCache::default();
        cache.sync(&doc, 4, &HighlightCache::new());
        assert!(cache
            .marks()
            .iter()
            .all(|mark| mark.kind == TokenKind::Plain));
        cache.sync(&doc, 4, &colored(&doc));
        assert!(cache.marks().iter().any(|mark| mark.kind.is_styled()));
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
            if cache.marks().iter().any(|mark| mark.kind.is_styled()) {
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
    }
}
