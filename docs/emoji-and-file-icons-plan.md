# Plan — color emoji and a VS Code file-icon theme

Status: **complete** (Session 6). Product priority ([`../AGENTS.md`](../AGENTS.md)):
color emoji completes text/Markdown rendering (priority 1); file icons are
Explorer polish requested explicitly by the user, and are re-introduced as a
*runtime* VS Code icon theme instead of the vendored SVG table removed in the
Session 5 [reduction plan](reduction-plan.md#phase-2--scope-reduction-priority-driven).

## Why this shape

- **Emoji:** the engine's font stack is outline-only (`ab_glyph`); Apple Color
  Emoji is an `sbix` bitmap font, so the engine correctly draws the advance but
  no ink. `../igui` must not be modified, so the app extracts the color bitmap
  itself (`ttf-parser`) and paints it as a `DrawImage` at the same advance the
  text layout already reserves. No dependency is added for shaping or GPU work;
  `ttf-parser`, `memmap2` and `png` are already in the lock file via the engine
  or are pure-Rust decoders.
- **Icons:** VS Code's Material Icon Theme is a JSON description
  (`dist/material-icons.json`) plus SVG files in the installed extension. The
  app reads that description at runtime (no vendored artwork, no build step, no
  network), resolves a file/folder name with VS Code's precedence and rasterizes
  the SVG through `igui_svg` (which strokes rather than fills) into cached spans.
  The `assets/icons` directory stays Lucide-only.
- Both features are opt-out where it matters: emoji has no setting (documents
  with emoji should render), file icons have a settings-panel row so a user who
  dislikes the theme can turn it off.

## Constraints and layering

- `model/emoji.rs` and `model/icon_theme.rs` are pure Rust: detection, bitmap
  decode and theme-JSON lookup are unit-testable without a window or GPU.
- `state/` owns IO and caches: font/JSON discovery, SVG source reads, texture
  ids, visible-row preloading.
- `ui/` only paints cached data: the sprite store is read during paint, the
  scanline rasterizer is pure geometry, and no file is opened from a painter.
- No engine change. No OS capture. Every behavior change gets a unit test or a
  `--selfcheck` assertion.
- `scripts/check.sh` green before every commit; `scripts/bench.sh` before/after
  the editor-paint change (typing budget < 2 ms/frame).

## Phase 0 — baseline

The working tree already contained a completed, green batch (overlay pointer
precedence, IME carets, composed glyph spans, grouped/searchable settings panel,
highlight catch-up, window minimum). Commit it first so the new work is
reviewable on its own.

- [x] `scripts/check.sh`: 438 unit + 2 doc tests, `--selfcheck` green.
- [x] One baseline commit for the pending batch.

## Phase 1 — emoji model (`src/model/emoji.rs`)

1. Emoji classification per code point:
   - default emoji presentation (`1F000..=1FAFF`, regional indicators, …);
   - text-default symbols (`2600..=27BF`, `2B00..=2BFF`, …) only with `FE0F`;
   - skip zero-width/component code points (`FE0F`, `200D`, `20E3`,
     skin-tone `1F3FB..=1F3FF`, hair components) so a modifier does not paint a
     second swatch.
2. Font discovery: `IGUI_EMOJI_FONT` override, then platform candidates
   (Apple Color Emoji `ttc`, Noto Color Emoji, Segoe UI Emoji). Memory-map the
   file (183 MB on macOS) — never read it into RAM.
3. Bitmap extraction: `ttf_parser::Face::glyph_raster_image` (`sbix`/`CBDT`),
   decode `png` data (RGBA/RGB/gray/indexed) and `BitmapPremulBgra32` into
   straight-alpha RGBA8 for `RenderBackend::register_texture`.
4. Tests: classification matrix; synthetic PNG decode; macOS real-font
   extraction for `😀` at a device ppem; missing-font returns `None`.

Acceptance: `cargo test model::emoji`; no window needed; the real-font test
skips cleanly on hosts without a color emoji font.

## Phase 2 — emoji painting (editor)

1. `state/emoji.rs` (or a small `EmojiStore` next to the app state): sprite
   cache keyed by `(char, ppem)`, texture-id allocator, upload closure installed
   by `EditorApp::init` from `SharedBackend`, device scale from
   `SharedWindowState`.
2. `ui/emoji.rs`: split a text run at emoji code points, paint an image centered
   in that code point's measured advance (baseline-anchored), and keep the
   advance identical to the text layout so caret, selection, hit-testing and
   soft wrap stay correct. Non-emoji text still paints in one run.
3. Editor integration in `ui/editor.rs::draw_text_mixed` (the one place editor
   code paint resolves families).
4. `--selfcheck`: a document containing emoji paints at least one `DrawImage`
   inside the text area and no `DrawText` containing the emoji code point.
   Skips (with a printed note) when no emoji font is installed.
5. `scripts/bench.sh` before/after: typing budget unchanged.

Acceptance: emoji visible in Edit and Split views; caret/selection/column math
unchanged (existing wrap/caret tests stay green); no file or GPU work grows per
frame after warm-up.

## Phase 3 — emoji in preview and terminal

1. Preview: the text primitive path uses the same splitting helper (fonts are
   measured before, so layout is already correct).
2. Terminal: a cell whose contents are an emoji base paints the sprite fitted to
   the (wide) cell; the grid, caret and selection stay cell-based.
3. `--selfcheck`: rendered Markdown containing emoji and a terminal row with
   emoji each emit image commands in their panes.
4. Docs: `docs/emoji.md` (discovery, metrics, cache, limits, why the engine is
   untouched); link from `features.md` and the index.

Acceptance: same as Phase 2 for both panes; no new per-frame IO.

## Phase 4 — icon theme model (`src/model/icon_theme.rs`)

1. Parse a VS Code icon theme JSON: `iconDefinitions` (`iconPath` relative to
   the JSON), `fileNames`, `fileExtensions`, `folderNames`, `folderNamesExpanded`
   and the default `file`/`folder`/`folderExpanded`/`rootFolder` ids, with the
   `light` override maps. Tolerant of unknown/font-based definitions.
2. Lookup follows VS Code: exact lowercased file name, then compound-extension
   suffixes longest-first (`d.ts` before `ts`), then `file`; folders by
   lowercased name, then `folder`/`folderExpanded`; light theme applies the
   `light` maps first.
3. Discovery: `IGUI_ICON_THEME` (JSON path) override, then the installed
   Material Icon Theme in `~/.vscode`, `~/.vscode-insiders`, `~/.vscode-oss`,
   `~/.cursor`, `~/.windsurf` extension directories (newest version wins).
4. Tests: fixture JSON covering every precedence rule, light overrides, missing
   definitions, relative `iconPath` resolution.

Acceptance: `cargo test model::icon_theme`; no UI dependency.

## Phase 5 — icons in the Explorer

1. `state/fileicons.rs`: load theme lazily on the first Explorer frame (setting
   on), resolve the visible row range from the sidebar scroll (same math as
   paint), read and cache only those SVG sources. Paint never opens a file.
2. `ui/fileicon.rs`: resurrect the deleted scanline rasterizer, keyed by
   resolved SVG path + quantized size, with `fill-rule="evenodd"` support. It
   rewrites `fill=` to `stroke=` for `igui_svg` (stroke-only engine) and fills
   the flattened subpaths with coloured `FillRect` spans.
3. `ui/tree.rs` / `ui/sidebar.rs`: file rows show their icon; directory rows
   show the folder/folder-open icon instead of the chevron when the theme is
   active (chevrons remain when it is not).
4. Settings: `Row::FileIcons` in the Explorer group (default on), session key
   `file_icons`, `SESSION_VERSION` 20 + migration note; `Reset` restores it.
5. `--selfcheck`: with `IGUI_ICON_THEME` pointed at a fixture theme, Explorer
   rows paint icon geometry (and the expected folder icon); turning the setting
   off removes it. Docs: `docs/file-icons.md`, features/settings table, design
   note, AGENTS routing.

Acceptance: icons appear for the open project, no startup cost when the setting
is off, no per-frame file IO, `scripts/check.sh` green, release size within the
12 MB growth guard.

## Phase 6 — verification and delivery

- `scripts/check.sh` (fmt, clippy, unit + doc tests, selfcheck).
- `scripts/check.sh --release` (shipped binary + size guard).
- `scripts/bench.sh` and record before/after numbers (typing, preview, minimap).
- Update this plan's progress log, `docs/README.md`, `AGENTS.md` routing and
  `docs/features.md`; note the deliberate reversal of reduction item 2.3 and why
  (runtime theme, no vendored assets).
- Commit one logical change per phase with imperative subjects; push
  `origin/master` through the configured proxy.

## Risks / decisions

- **ZWJ sequences and flags** are painted one image per base code point (engine
  layout measures per code point, so aligning to a single shaped glyph would
  desync carets). Family/flag sequences therefore show their components side by
  side; this is a documented limit, not a layout bug.
- **Light theme**: the Material theme only overrides some artwork under `light`;
  the rest keeps its explicit colour, exactly as VS Code does.
- **SVG coverage**: transforms, gradients and text in an SVG are ignored by the
  engine's parser. Icons that fail to parse draw nothing rather than a broken
  glyph; the Explorer still shows the label, and the selfcheck asserts that the
  installed theme parses for the common file kinds.
- **Memory**: the emoji font is mmap'd and glyph PNGs are cached as GPU
  textures with an LRU-ish cap; no 183 MB read is ever copied into the heap.

## Progress log

| Phase | Status | Notes |
|---|---|---|
| 0 Baseline | done | `d41f1ca`; 438 unit + 2 doc tests, selfcheck green. |
| 1 Emoji model | done | `model/emoji.rs`; classification, mmap font discovery, sbix/CBDT PNG + premul BGRA decode, 7 tests. |
| 2 Emoji editor paint | done | Sprite store + texture upload, editor split-at-emoji paint, selfcheck image assertion. |
| 3 Emoji preview/terminal | done | Preview, terminal, Explorer/outline/tab labels, docs/emoji.md, selfcheck image assertions. |
| 4 Icon theme model | done | `model/icon_theme.rs`; VS Code JSON parse, precedence (name > compound ext > default), light overrides, discovery + newest-version pick, 8 tests. |
| 5 Explorer icons | done | Runtime theme store, visible-row preload, scanline rasterizer (even-odd), tree markers, File Icons setting + session v20, selfcheck fixture. |
| 6 Verification/delivery | done | check + release gate green, bench recorded, manual window smoke, docs and push. |

## Results

- **Color emoji**: system face discovered via mmap (Apple Color Emoji on this
  host), one image per base code point in the editor, split preview, terminal,
  Explorer/outline labels and tab labels. 7 model tests + 3 paint tests.
- **File icons**: runtime Material Icon Theme discovery and VS Code lookup
  precedence, lazy visible-row SVG preload, scanline rasterizer with even-odd
  support, chevron fallback, **Explorer → File Icons** setting, session v20.
  8 model tests + 5 rasterizer tests.
- **Gate**: `scripts/check.sh --release` green — 464 unit + 2 doc tests,
  `--selfcheck` (now includes emoji paint and the file-icon fixture), release
  binary **5.29 MB** (12 MB growth guard). Manual launch of the release binary
  with `README.md` ran for several seconds without a panic.
- **Bench** (`scripts/bench.sh`, same fixture): typing map off 0.046 → 0.056
  ms/frame, map on 0.172 → 0.183, 16 ms frame 0.703 → 0.771; markdown parse
  1.45 ms, layout 2.10 ms, minimap 0.418/0.062. All inside the documented
  budgets; idle frames are still zero.
- **Commits**: plan → emoji model → editor paint → preview/terminal/labels →
  icon-theme model → Explorer icons → verification/docs.
