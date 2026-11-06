# Minimap — design and delivery notes

Status: **implemented** (session v19, default on). This document records the
investigation of the editor surface, the decided design, and the measured
delivery at the end.

Product priority check: a minimap serves priority 1 (text editing). It is not
new architecture — it reuses the editor's existing scroll row space, the
document's revision and line range, and the `igui_render` fill/clip primitives.
No engine change, no new dependency, no new window, no second document model.

## Goal and non-goals

Goal: a low-contrast thumbnail of the whole document on the right edge of the
editor (left of the vertical scrollbar), with the viewport window drawn on it,
click-to-jump and drag-to-scroll. It must stay correct while typing, under soft
wrap, in split view and on large files, and it must not change the idle frame
or the typing budget.

Non-goals (v1):

- no glyph rendering / tiny-font text (see options below);
- no hover preview, no tooltip, no section markers, no context menu;
- no marks for find hits or diagnostics (cheap follow-up, not part of v1);
- no horizontal panning of the map when a line is wider than the scaled width;
- no resize handle, no per-language or per-project settings;
- no minimap in Preview-only view (there is no editor), for the dedicated docs
  tab, or for the Git diff overlay (neither is the text editor).

## Current surface (investigation)

Facts the design depends on, verified in the tree:

| Fact | Where |
|---|---|
| The editor paints one leaf control and derives `text_rect` as `rect.width() - gutter_w - scrollbar` | `src/ui/editor.rs` (`paint`) |
| `hit()` only rejects points left/above the text area, not to the right of it | `src/ui/editor.rs` |
| Mouse drags are one `Drag` enum (`Select`, `VScroll`, `HScroll`) | `src/state/mod.rs` |
| Scroll offsets live on the document, so tab switches keep them | `src/model/doc.rs` |
| `Document::edit_span()` reports the inclusive line range of the last mutation and is `None` after load/reload | `src/model/doc.rs` |
| Soft wrap is a cached visual-row layout keyed by `(doc, rev, width, font, tab)`; `line_of_row` / `first_row_of_line` map both ways | `src/model/wrap.rs` |
| The tree `DrawList` is cached across unchanged frames; a painter that mutates state sets `dirty` | `src/app.rs`, `igui_ui::paint_cached` |
| A View toggle can persist in the session without a settings-panel row (`sync_scroll`, `soft_wrap` are the precedents) | `src/state/mod.rs`, `src/model/session.rs` |
| Commands have one source of truth: `Action`, `commands.rs`, keymap, menu item, native macOS command | `src/state/`, `src/keymap.rs`, `src/native.rs`, `src/macos/imp.rs` |
| The engine can render a subtree into a `SubViewport`, but that renders what is on screen | `../igui` (viewport model) |

## Options investigated

| Option | How | Verdict |
|---|---|---|
| A. Structural runs (chosen) | Cache each line's non-blank runs (start/end column + token class), then rasterize them into map cells | Chosen: font-independent, no glyph measuring, gaps and separate code blocks stay visible, unit-testable |
| B. Syntax-tinted raster | Each run carries its token class; the raster keeps the dominant class per cell | Delivered: colors follow keywords, strings, comments and numbers instead of one color per line |
| C. Tiny-font text thumbnail | Draw each line at 2–3 px | Rejected: tiny runs are illegible, force text measurement and font fallback for every line, and blow up the `DrawList` |
| D. Render the editor into a scaled sub-viewport | Engine `SubViewport` render target | Rejected: the minimap shows *off-screen* content, so it would have to render the whole document (up to 20k lines) to a texture every edit; cost is proportional to the document, not to the map |

A is the smallest surface that answers "where am I and what shape is this
file"; B adds the syntax-colored reading without changing geometry, state or
input. Grouping a whole line into one bar was tried and rejected: it hid
indentation-spaced tokens and made different code blocks look identical.

## Content model (`src/model/minimap.rs`, pure Rust)

```text
LineMark { start: u16, end: u16, runs: [Run; 6], count: u8 }
Run      { start: u16, end: u16, kind: TokenKind }
```

One mark per logical line, independent of viewport width, font and theme:

- scan the line's chars; `start`/`end` are the first/last non-blank columns
  (trailing whitespace ignored, tabs count as `tab_width` columns);
- whitespace breaks a run, so indentation and spaces inside code stay as
  visible gaps; whitespace *inside* a string/comment token stays in that run;
- each run carries its token class (keyword, string, comment, number,
  punctuation, …), so different blocks inside a line keep different colors;
- busy lines merge the closest neighbouring runs until at most
  `MAX_LINE_RUNS = 6` remain; a minified line folds its tail into the last run;
- a blank line has no runs and contributes no ink;
- `scale_col = max(end).clamp(1, MAX_COLUMNS)` normalizes x; ink beyond
  `MAX_COLUMNS = 100` is clipped to the right edge.

The **raster** is the paint-facing cache: `rows` stacked buckets × `cells`
(one logical pixel each) storing the dominant class for that cell (`0` =
empty). A run maps to cells `start * cells / scale` up to `end * cells /
scale`; styled classes outrank plain ink from another line sharing the same
row. Paint scans each row and emits one rect per same-class run, so a code
line becomes several small blocks with gaps instead of one solid bar.

Buckets and geometry are separate pure helpers so they can be tested without a
window:

```text
content_h    = min(map_h, line_count * MAP_ROW_H)          // short files stay top-aligned
bucket_count = min(MAX_BUCKETS, line_count,
                   ceil(content_h / MAP_ROW_H) max 1)
bucket(line) = line * bucket_count / line_count
line_at(y)   = floor((y - map_top) / content_h * line_count)  // clamped
slider(line_count, first_visible, last_visible, content_h) -> (top, height)
raster(rows, cells) -> class per cell
```

At a 700 px editor the raster is ~350 × 100 cells. A row is normally painted
with 1–4 runs; rows above `MAX_RUNS_PER_ROW = 8` fall back to one dominant bar
so a pathological document cannot grow the draw list without bound.

Rasterizing is `O(line_count × runs)` arithmetic over cached marks (no char
scanning, no text measurement) and runs only when the document revision, the
row count or the cell count changes — i.e. on edits and on resize, never on
scroll or hover.

### Cache and invalidation

`MinimapCache` keeps one document slot (like the highlight and wrap caches):

```text
{ doc: u64, rev: u64, tab: usize, line_count: usize,
  marks: Vec<LineMark>, max_end: u16,
  raster: Vec<u8>, raster_rows: usize, raster_cells: usize,
  available: bool, colored: bool }
```

`sync(&doc, tab, highlight)` cases:

| Situation | Work |
|---|---|
| Same `(doc, rev, tab)` and colors are settled | none |
| Highlighting became complete for the revision | full rebuild to fill token colors |
| Highlighting is behind after an edit | incremental; the changed lines use the tokens that are already available, untouched lines keep their colors |
| Other document / reload (`edit_span() == None`) | rebuild all marks |
| Revision gap larger than one (`doc.rev() != cache.rev + 1`) | rebuild all marks; several mutations landed between two frames, so the single cached `edit_span` cannot describe them |
| Edit with the same line count | rescan `[first..=last]` lines, splice marks |
| Edit with a line-count delta | derive the replaced old span as `new_len - delta`, `Vec::splice` the rescan; fall back to a full rebuild if the arithmetic or bounds look inconsistent |
| Tab width changed | full rebuild |

The splice rule is safe because every `Document` mutation applies one
contiguous replacement (`replace_range`, `replace_ranges`, `replace_edits`,
multi-site inserts all collapse to one range), so `edit_span` covers the entire
change. The revision-gap rule is what keeps that assumption true: `edit_span`
holds only the latest mutation, so it cannot be used when two edits happened
before the painter ran. `HighlightCache` makes the same per-mutation
assumption.

Caps mirror the existing large-file policy: **2 MB / 20 000 lines**. Above a
cap the minimap is not drawn (the persisted toggle is untouched). Memory is at
most ~0.9 MB (20k marks with six runs each, plus the raster) for the active
document only, and the cache is reset on tab switch.

## Layout and paint (`src/ui/minimap.rs`)

One layout function serves paint and hit-testing; the rectangles are recorded
in `MinimapUi` during paint, matching the scrollbar/tab pattern.

```text
+ gutter | text area                    | map | vbar |
|        |                              |     |      |
|        |   editor text (text_rect)    | 100 |  10  |
```

- `Metrics::minimap = 100.0` (logical px, about one pixel per source column)
  is reserved from the editor width;
  `text_rect` shrinks by that amount, everything else (caret, selection,
  hit-test, horizontal scroll, wrap width) follows automatically.
- Auto-hide without changing the toggle when the remaining text area would fall
  below `MIN_TEXT_W = 360.0` (narrow window, split view). In that case
  the reserved width is 0 and the recorded rect is `Rect::ZERO`, so pointers
  fall through to text/scrollbar handling.
- The map rect is `[text_rect.right, rect.right - scrollbar)` with
  `text_rect.height()`; the horizontal scrollbar keeps its own band below.
- Paint order: background fill, current-line tint, guides, selection, find,
  text, tokens, diagnostics, preedit, caret — then the minimap. The map fills
  its own rect with `p.bg` (opaque, so the current-line tint cannot bleed
  through), draws a 1 px `p.border` rule on its left edge, the raster runs, and
  finally the viewport slider.
- Ink: plain runs use `p.text.alpha_mul(0.10 + 0.22 * density)`; styled runs
  use `theme.token_color(kind)` at `0.55 + 0.20 * density`, matching the
  editor's syntax palette. `density` is the share of filled cells in the row.
  Slider: fill `p.text.alpha_mul(0.08)` plus a 1 px `p.dim.alpha_mul(0.60)`
  border, painted only while the document can scroll. Square corners, no
  radius, no shadow — consistent with the theme spec.
- The current-line fill and any other full-width editor chrome must stop at the
  map's left edge; the map repaints its own background over the strip.

Software-wrap interplay: marks are always *logical* lines. The slider and hit
mapping convert lines ↔ visual rows with the existing wrap helpers
(`line_of_row`, `first_row_of_line`), so with wrap on:

- the slider's line span is `line_of_row(first_visible_row) ..
  line_of_row(last_visible_row)` (exact when the layout is cached, otherwise
  the row itself);
- a click resolves to a line and scrolls to `first_row_of_line(line) * line_h`,
  so the clicked line ends up at the top of the viewport (clamped).

## Input

- `pointer_down`: after the scrollbar checks, if the map contains the point,
  start `Drag::Minimap { grab }` where `grab` is the line offset from the
  slider top; apply the jump immediately. Focus goes to the editor, the caret
  and selection are **not** moved, `editor.reveal = false` so the caret does not
  fight the scroll.
- `pointer_move` during `Drag::Minimap`: map the pointer back to a line minus
  `grab`, set `scroll_y` (row-based under wrap), clamp in the next paint.
- `pointer_up`: existing generic `Drag::None`.
- Guards are at the call sites, not inside `hit()`: `pointer_down`, hover
  tracking and double-click ignore points inside the map, while a `Drag::Select`
  that sweeps across the map keeps using `hit()` so the selection still extends
  to the line end (the column math clamps). This avoids turning the map into an
  invisible wall in the middle of a drag.
- Wheel/trackpad over the map keeps scrolling the editor (the whole editor node
  owns the handler; no separate node, no second wheel convention).
- The node cursor stays `Cursor::Text` (one cursor per control in the engine);
  documented as a cosmetic limitation, not worth an engine change.

## Command, state and persistence

Follows the `sync_scroll`/`soft_wrap` precedent:

| Piece | Value |
|---|---|
| `Action::ToggleMinimap`, label `Minimap`, id `view.minimap` | `src/state/mod.rs`, `src/state/commands.rs` |
| Enabled with a document open; `checked` = `session.minimap` | `Action::available` / `Action::checked` |
| View menu item next to Soft Wrap | `state::items(Menu::View, …)` |
| Shortcut `primary+alt+m` (`⌘⌥M`) in the global map; menu hints resolve from the keymap | `src/keymap.rs` |
| Native macOS View item + `NativeCommand::ToggleMinimap` (new tag) | `src/native.rs`, `src/macos/imp.rs`, `src/app.rs` |
| `session.minimap` (default **on**), `SESSION_VERSION` 19, feature doc version note | `src/model/session.rs`, `docs/features.md` |
| UI state: `MinimapUi { rect, slider, first_line, cache }` nested in `EditorUi` | `src/state/mod.rs` |

No settings-panel row: the toggle is a view command, and adding a row would
repeat the 10-site settings tax for one boolean. Toggling marks the frame dirty;
the width change rebuilds the wrap key and re-clamps scrolling on the next
paint.

### Constants

| Constant | Value | Owner |
|---|---|---|
| `Metrics::minimap` (width) | 100 px | `theme.rs` |
| `MAP_ROW_H` (minimum map row) | 2.0 px | `model/minimap.rs` |
| `MAX_COLUMNS` (x normalization cap) | 100 | `model/minimap.rs` |
| `MAX_LINE_RUNS` (runs cached per line) | 6 | `model/minimap.rs` |
| `MAX_BUCKETS` (raster row cap) | 512 | `model/minimap.rs` |
| `MAX_RUNS_PER_ROW` (paint fallback) | 8 | `ui/minimap.rs` |
| `MAX_MINIMAP_BYTES` / `MAX_MINIMAP_LINES` | 2 MB / 20 000 | `model/minimap.rs` |
| `MIN_TEXT_W` (auto-hide) | 360 px | `ui/minimap.rs` |
| Plain ink alpha | 0.10 + 0.22 × density | `ui/minimap.rs` |
| Styled run alpha | 0.55 + 0.20 × density | `ui/minimap.rs` |
| Slider fill / border alpha | 0.08 / 0.60 | `ui/minimap.rs` |

## Performance plan

The map is part of the editor's cached `DrawList`, so idle frames stay at zero;
the only new cost is on dirty frames.

| Path | Expected cost | Guard |
|---|---|---|
| Typing | rescan the changed line + `O(line_count × runs)` raster (~20k lines) + ~1–4 rects per map row | typing bench stays **< 2 ms/frame**; measured map delta +0.09 ms on the 4,760-line fixture |
| Tab switch / open | full run build over ≤2 MB | measured 0.42 ms per 4,800 lines; if it grows, budget the build across frames (highlight-style) |
| Resize | raster rebuild only (runs are width-independent) | report command count |
| Idle | 0 new frames | `idle` must stay 0 frames/0% CPU |

Bench additions: a `minimap build` row (full + incremental) in `src/bench.rs`
and `benches/markdown.rs`; the existing `typing` row now includes the map and
its command count is reported. Budgets are recorded in
[`benchmarks.md`](benchmarks.md) when the numbers land.

## Verification

Unit tests (`src/model/minimap.rs`):

1. run extraction: indentation, trailing whitespace, tabs, blank lines, CJK,
   token-class boundaries, spaces inside strings/comments, the six-run cap;
2. raster: gaps between runs survive, styled cells outrank plain ink,
   empty/short/long documents, short-file top alignment vs compressed fill;
3. incremental parity: for typing, newline insert/delete, multi-line indent,
   undo and replace-all, incremental marks equal a fresh build;
4. color: enabling syntax recolors the raster, colored incremental edits match
   a rebuild;
5. geometry helpers: `line_at` top/bottom/blank-area clamping, slider top and
   height for wrap and non-wrap spans, minimum slider height;
6. caps: `is_active` is false above 2 MB / 20k lines and for no document.

`--selfcheck` (real pipeline, recorded frame):

- with a file open, `editor.minimap` is non-zero, sits left of the scrollbar and
  `text_rect.right == minimap.left`;
- recorded fill commands exist inside the map rect and the slider rect, span the
  strip, and the command count stays under the documented cap;
- a click in the map changes `scroll_y` to the expected line and does **not**
  move the caret; a drag scrolls monotonically and clamps at both ends;
- `ToggleMinimap` (action **and** `⌘⌥M`) removes the map, widens `text_rect`,
  persists `minimap=0`, and a fresh harness restores the hidden state;
- soft wrap on: click/drag map through `first_row_of_line` and the slider stays
  inside the map;
- narrow viewport: the map auto-hides without changing the persisted toggle.

Manual smoke test: launch a real window with a long file, verify the map,
slider, click/drag, resize, theme switch and soft-wrap; note it in the final
response (host/paint change).

## Implementation slices

| Slice | Files | Done when |
|---|---|---|
| S1 model | `src/model/minimap.rs`, `src/model/mod.rs` | unit tests green; bench row measured |
| S2 paint | `src/ui/minimap.rs`, `src/ui/mod.rs`, `src/ui/editor.rs`, `src/theme.rs`, `src/state/mod.rs` | map drawn, rects recorded, auto-hide/caps, current-line chrome trimmed |
| S3 input | `src/ui/editor.rs`, `src/ui/minimap.rs`, `src/state/mod.rs` (`Drag`) | click/drag/hover guards, hit edge case fixed |
| S4 command | `Action`, `commands.rs`, `keymap.rs`, `menu.rs`, `native.rs`, `macos/imp.rs`, `app.rs`, `session.rs` | toggle + shortcut + persistence + native item |
| S5 docs/bench | `docs/features.md`, `docs/keys.md`, `docs/design.md`, `docs/code-map.md`, `docs/benchmarks.md`, `docs/README.md`, `AGENTS.md` | docs truthful, bench numbers recorded, `scripts/check.sh` green |

## Decisions and open questions

Decided:

- structural runs (A) plus token-class colors (B); no one-block-per-line;
- logical lines as content space, visual rows only for slider/scroll mapping;
- default on, fixed 100 px, auto-hide under 360 px of text area, caps at
  2 MB / 20k lines;
- no settings row; View menu command + persisted session key;
- map lives inside the editor control: one layout function, rects recorded
  during paint, no new node and no shell rebuild.

Open (not blocking):

- whether find-match and diagnostic ticks earn their draw commands later;
- whether very wide lines should pan the map to the caret instead of clipping.

## Delivered

Implemented in slices S1–S5:

- `src/model/minimap.rs` — per-line runs, the raster, incremental cache and
  geometry helpers; 16 unit tests covering runs, gaps, the run cap, raster
  classes, caps, short/long geometry, the slider, incremental parity (typing,
  newline, line delete, indent, undo, colored edits and a 400-step random edit
  sequence), a revision gap and tab-width changes.
- `src/ui/minimap.rs` — reserved width, paint, viewport slider, click/drag
  mapping.
- `src/ui/editor.rs` — strip reservation, paint hook, hover/double-click guards,
  `Drag::Minimap`; full-width current-line chrome stops at the strip.
- `src/state/*`, `src/keymap.rs`, `src/native.rs`, `src/macos/imp.rs` — the
  `view.minimap` command, `⌘⌥M`, the native macOS View item and
  `session.minimap`.
- `src/bench.rs`, `benches/markdown.rs` — `minimap build`, `minimap
  incremental`, and Edit-view `typing, map off/on` rows.
- `src/selfcheck.rs` — `check_minimap`: geometry, draw-command cap, click/drag
  (caret untouched), toggle + persistence, auto-hide, soft wrap, no document.

Measured (macOS arm64, bench profile): full build **0.42 ms** on the 4,800-line
Markdown fixture, incremental **0.064 ms**, and Edit-view typing on a 4,760-line
JavaScript fixture (syntax colors active) **0.36 → 0.45 ms/frame** and
**953 → 2026 commands**; idle frames are unchanged (the map lives in the cached
`DrawList`).

Deviations from the plan:

- short documents keep 2 px rows and stay top-aligned; only documents taller
  than the box are compressed to fill it;
- rows above eight runs fall back to one dominant bar, which bounds the draw
  list on pathological files while normal code gets per-run fidelity;
- the Markdown typing benchmark opens Split view, where the strip auto-hides,
  so a dedicated Edit-view `editor_typing(..., minimap)` benchmark was added;
  it uses a JavaScript fixture so highlighting (and therefore colors) is active
  without spawning a language server;
- `MinimapUi` is nested in `EditorUi`, and `MinimapCache::marks()` is a
  test-only accessor.

`scripts/check.sh` is green (401 unit tests + 2 doc tests, selfcheck including
`check_minimap`); the numbers above come from `scripts/bench.sh`.
