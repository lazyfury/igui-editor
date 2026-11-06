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
| A. Structural blocks (chosen) | Cache one `(indent, end)` mark per line; aggregate lines into ~2 px buckets and paint one low-alpha rect per bucket | Chosen: font-independent, no glyph measuring, deterministic, ~1 draw command per bucket, unit-testable |
| B. Syntax-tinted buckets | Same as A, but the bucket's dominant token class selects its color | Delivered on top of A: no extra draw commands, one color per bucket |
| C. Tiny-font text thumbnail | Draw each line at 2–3 px | Rejected: tiny runs are illegible, force text measurement and font fallback for every line, and blow up the `DrawList` |
| D. Render the editor into a scaled sub-viewport | Engine `SubViewport` render target | Rejected: the minimap shows *off-screen* content, so it would have to render the whole document (up to 20k lines) to a texture every edit; cost is proportional to the document, not to the map |

A is the smallest surface that answers "where am I and what shape is this
file"; B adds the syntax-colored reading without changing geometry, state or
input.

## Content model (`src/model/minimap.rs`, pure Rust)

```text
LineMark { start: u32, end: u32, kind: TokenKind }
```

One mark per logical line, independent of viewport width, font and theme:

- scan the line's chars; `start` = column of the first non-blank char,
  `end` = one past the column of the last non-blank char (trailing whitespace
  ignored, tabs count as `tab_width` columns);
- `kind` = the styled token class covering the most line bytes (keywords,
  strings, comments, numbers, …); punctuation is structural and folds into
  `Plain`, so a line of braces never wins a color;
- a blank line is `start == end == 0` and contributes no ink;
- `scale_col = max(end).clamp(1, MAX_COLUMNS)` normalizes x; ink beyond
  `MAX_COLUMNS` is clipped to the right edge, so one minified line cannot
  squash every other line into a 1 px sliver.

Buckets and geometry are separate pure helpers so they can be tested without a
window:

```text
content_h    = min(map_h, line_count * MAP_ROW_H)          // short files stay top-aligned
bucket_count = min(MAX_BUCKETS, line_count,
                   ceil(content_h / MAP_ROW_H) max 1)
bucket(line) = line * bucket_count / line_count
line_at(y)   = floor((y - map_top) / content_h * line_count)  // clamped
slider(line_count, first_visible, last_visible, content_h) -> (top, height)
```

Each bucket is `{ x0: min(start), x1: max(end), inked, lines, kinds }`, where
`kinds` accumulates the column coverage of each class and `kind()` returns the
dominant styled class. `x0/x1` are painted with alpha
`INK_MIN + (INK_MAX - INK_MIN) * inked / lines` for plain buckets and
`STYLED_MIN + (STYLED_MAX - STYLED_MIN) * density` for colored ones, which
makes dense code read darker than sparse code. Buckets with `x1 <= x0` (blank
or entirely clipped) are skipped. One rect per bucket: at a 700 px editor that
is at most ~350 commands, capped at `MAX_BUCKETS` on very tall windows.

Aggregation is `O(line_count)` arithmetic over cached marks (no char scanning,
no text measurement) and runs only when the document revision or the bucket
count changed — i.e. on edits and on resize, never on scroll or hover.

### Cache and invalidation

`MinimapCache` keeps one document slot (like the highlight and wrap caches):

```text
{ doc: u64, rev: u64, tab: usize, line_count: usize,
  marks: Vec<LineMark>, max_end: u32,
  buckets: Vec<Bucket>, bucket_count: usize }
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
most ~160 KB (20k marks) for the active document only, and the cache is reset
on tab switch.

## Layout and paint (`src/ui/minimap.rs`)

One layout function serves paint and hit-testing; the rectangles are recorded
in `MinimapUi` during paint, matching the scrollbar/tab pattern.

```text
+ gutter | text area                    | map | vbar |
|        |                              |     |      |
|        |   editor text (text_rect)    | 84  |  10  |
```

- `Metrics::minimap = 84.0` (logical px) is reserved from the editor width;
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
  through), draws a 1 px `p.border` rule on its left edge, the bucket rects, and
  finally the viewport slider.
- Ink: plain buckets use `p.text.alpha_mul(0.10 + 0.22 * density)`; styled
  buckets use `theme.token_color(kind)` at `0.45 + 0.30 * density`, matching the
  editor's syntax palette. Slider: fill `p.text.alpha_mul(0.08)` plus a 1 px
  `p.dim.alpha_mul(0.60)` border, painted only while the document can scroll.
  Square corners, no radius, no shadow — consistent with the theme spec.
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
| `Metrics::minimap` (width) | 84 px | `theme.rs` |
| `MAP_ROW_H` (minimum map row) | 2.0 px | `model/minimap.rs` |
| `MAX_COLUMNS` (x normalization cap) | 120 | `model/minimap.rs` |
| Styled bucket alpha | 0.45 + 0.30 × density | `ui/minimap.rs` |
| `MAX_BUCKETS` (draw-command cap) | 512 | `model/minimap.rs` |
| `MAX_MINIMAP_BYTES` / `MAX_MINIMAP_LINES` | 2 MB / 20 000 | `model/minimap.rs` |
| `MIN_TEXT_W` (auto-hide) | 360 px | `ui/minimap.rs` |
| Ink alpha | 0.10 + 0.22 × density | `ui/minimap.rs` |
| Slider fill / border alpha | 0.05 / 0.45 | `ui/minimap.rs` |

## Performance plan

The map is part of the editor's cached `DrawList`, so idle frames stay at zero;
the only new cost is on dirty frames.

| Path | Expected cost | Guard |
|---|---|---|
| Typing | scan one changed line + `O(line_count)` aggregation (~20k simple ops) + ≤512 rect commands | typing bench must stay **< 2 ms/frame**; flag any >20% regression against the current 1.1 ms |
| Tab switch / open | full mark build over ≤2 MB | measure; if it shows up, budget the build across frames (highlight-style) as a follow-up |
| Resize | re-aggregation only (marks are width-independent) | report command count |
| Idle | 0 new frames | `idle` must stay 0 frames/0% CPU |

Bench additions: a `minimap build` row (full + incremental) in `src/bench.rs`
and `benches/markdown.rs`; the existing `typing` row now includes the map and
its command count is reported. Budgets are recorded in
[`benchmarks.md`](benchmarks.md) when the numbers land.

## Verification

Unit tests (`src/model/minimap.rs`):

1. mark extraction: leading indentation, trailing whitespace, tabs, blank
   lines, CJK (char columns), column clipping at `MAX_COLUMNS`;
2. bucket aggregation: min/max extents, density, empty/short/long documents,
   short-file top alignment vs compressed fill, bucket cap;
3. incremental parity: for typing, newline insert/delete, multi-line indent,
   undo and replace-all, incremental marks equal a fresh build;
4. color: dominant-style selection (punctuation never wins), bucket color
   aggregation, enabling syntax recolors the map, colored incremental edits
   match a rebuild;
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

- structural blocks (A) plus dominant-style bucket colors (B);
- logical lines as content space, visual rows only for slider/scroll mapping;
- default on, fixed 84 px, auto-hide under 360 px of text area, caps at
  2 MB / 20k lines;
- no settings row; View menu command + persisted session key;
- map lives inside the editor control: one layout function, rects recorded
  during paint, no new node and no shell rebuild.

Open (not blocking):

- whether find-match and diagnostic ticks earn their draw commands later;
- whether very wide lines should pan the map to the caret instead of clipping.

## Delivered

Implemented in slices S1–S5:

- `src/model/minimap.rs` — line marks, incremental cache, bucket and geometry
  helpers; 16 unit tests covering marks, dominant token color, bucket
  aggregation, caps, short/long geometry, the slider, incremental parity
  (typing, newline, line delete, indent, undo, colored edits and a 400-step
  random edit sequence), a revision gap and tab-width changes.
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

Measured (macOS arm64, bench profile): full build **0.116 ms** on the 4,800-line
Markdown fixture, incremental **0.014 ms**, and Edit-view typing on a 4,760-line
JavaScript fixture (syntax colors active) **0.361 → 0.369 ms/frame** and
**953 → 1310 commands**; idle frames are unchanged (the map lives in the cached
`DrawList`).

Deviations from the plan:

- short documents keep 2 px rows and stay top-aligned; only documents taller
  than the box are compressed to fill it;
- bucket colors are the bucket's dominant styled class (one color per row, no
  per-glyph spans), which keeps the draw-command budget flat;
- the Markdown typing benchmark opens Split view, where the strip auto-hides,
  so a dedicated Edit-view `editor_typing(..., minimap)` benchmark was added;
  it uses a JavaScript fixture so highlighting (and therefore colors) is active
  without spawning a language server;
- `MinimapUi` is nested in `EditorUi`, and `MinimapCache::marks()` is a
  test-only accessor.

`scripts/check.sh` is green (401 unit tests + 2 doc tests, selfcheck including
`check_minimap`); the numbers above come from `scripts/bench.sh`.
