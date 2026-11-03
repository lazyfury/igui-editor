# igui-editor — design

Agreed direction (M1 + M2 implemented). This document records the decisions so
M3 does not re-litigate them.

## Product

One app, two document kinds:

- **code / plain text** — open, edit, save, tabs, find/replace;
- **Markdown** — edit, split preview and preview-only views (per document).

Out of scope: image editing (that app lives in its own repository), wasm builds,
a general plugin system.

## Where it lives

`~/Documents/igui-editor` — an independent Cargo package with an empty
`[workspace]`, depending on the engine by path:

```
igui_core, igui_render, igui_scene, igui_ui, igui_font,
igui_app, igui_winit, igui_backend_wgpu,
igui_headless, igui_backend_recording, igui_profile
```

No changes to `../igui`. If a capability is missing, build it in the app first;
propose an upstream change only after it has proven necessary.

## Layering

```
Input -> SceneTree -> Layout -> Paint -> DrawList -> wgpu
```

- `model/` is pure Rust: no scene, no backend, no window. All editing rules are
  unit-tested.
- `state.rs` is the single shared value (documents, theme, hit rectangles,
  message). Input callbacks mutate it; painters read it.
- `ui/` is one module per shell region; each builds exactly one control node
  and paints itself.
- `app.rs` owns the tree, routes input, and implements `igui_app::AppLogic`.
  The shell is rebuilt only when its structure changes (sidebar toggle).
- `host.rs` installs the platform plugins; `selfcheck.rs` drives the same app
  headlessly.

## Key decisions

1. **The editor surface is custom, not `igui_components::TextArea`.** TextArea
   is a fixed-rows box with one text color, no gutter, no horizontal scroll, no
   per-line painting and no undo. `ui/editor.rs` draws the gutter, current line,
   selection, caret and text itself, using the backend's real `TextMeasurer` so
   caret/selection and glyphs agree. `TextEdit` is *not* reused: the buffer,
   caret math and undo live in `model/doc.rs` (byte offsets, char-aware columns,
   delta undo).
2. **No `igui_theme` / `igui_components` dependency.** The requested look
   (compact, square, monospace) is not the component library's default, and its
   radius constants are baked into components. The app owns a small palette +
   metric module instead, and paints every surface with `igui_ui` primitives.
3. **Tabs, gutter, menu and popup stay app-local.** No new component is promoted
   into the engine during M1.
4. **The drop-down is an app-level overlay** painted after the tree. Geometry
   comes from one function (`ui/menu::popup_layout`) used by both paint and hit
   testing, so the menu never depends on z-order or a second window.
5. **Mouse hit rectangles are recorded during paint** (tabs, menu bar, sidebar
   rows). Pointer events happen after a frame, so this is stable and avoids a
   parallel layout implementation.
6. **Keyboard focus is always the editor.** Clicking the sidebar or tab strip
   makes the engine focus that control on pointer-down; `app.event` restores
   focus right after routing, so typing and shortcuts never stop working.
7. **Verification is programmatic.** `cargo test` for the model and
   `--selfcheck` for the pipeline (recorded `DrawList` + `igui_profile::inspect`
   + semantic text/color assertions). No screenshots or screen recordings.
8. **Native dialogs behind a trait, asynchronously.** `dialogs::Dialogs`
   returns a boxed future per call (open file / open folder / save file).
   `host.rs` implements it with `rfd`'s **async** API, attached to the winit
   window via `SharedWindow`: on macOS that is an `NSOpenPanel`/`NSSavePanel`
   **sheet**, so the winit loop keeps running and the app handler is never
   re-entered (the blocking API would run a nested modal loop inside the frame
   callback). The app starts one dialog at a time (`state.pending_dialog` →
   `PendingDialog`) and polls its future once per frame with `Waker::noop`;
   `needs_frame` keeps frames coming until the sheet answers. `ScriptedDialogs`
   returns ready futures, so the self-check exercises the same state machine
   without an OS window.
9. **Keys are dispatched centrally.** `input.rs` orders:
   modal → menu Esc → find field (if focused) → global shortcuts → editor/tree.
   Text and IME events follow the `Focus` value (`Editor` / `Find` / `Replace`).
   Global commands therefore work in every mode, and a field edit can never leak
   into the document.
10. **Find state is a value, matching is pure.** `model::find` maps `(text,
   matcher)` to byte ranges; `FindState` holds two `igui_ui::TextEdit` fields
   (the engine's pure text/caret state), the match list and the bar geometry.
   The editor paints matches from that list; no second document model.
11. **Markdown is parsed to a flat block list, previewed through cached
   primitives.** `model::markdown` folds `pulldown-cmark`'s event stream into
   `Block { quote, indent, kind }` with styled inline runs (nested lists/quotes
   become indentation, not nesting). Tables are a block kind with a header row
   and flat cell runs plus per-column alignment; the preview measures natural
   column widths, shrinks them to fit, wraps each cell (with left/center/right
   alignment) and rules the rows. `ui::preview` lays blocks out into absolute
   `Prim` (text/fills) once per `(doc id, rev, width, theme mode)` and paints
   them with a scroll offset — no per-frame reflow, no second tree.
12. **Session is a tiny hand-written file.** `theme`, `sidebar`, up to eight
   `recent.N` and `tabs.N` entries, written atomically. No serde. Values escape
   backslash/newline; a corrupt line is ignored rather than failing startup.
13. **Unsaved-close is an app overlay, not a native alert.** It must be
   headless-testable and visually consistent (square, 1px rules, accent Save),
   so it is painted after the tree like the menu and hit-tested from one layout
   function. Enter = Save, Esc = Cancel; Save on an untitled tab routes through
   the Save As dialog and closes after a successful write.

## Theme spec (geek / compact / no corners)

Rules:

- zero corner radius; 1 px rules only; no shadows, no gradients, no blur;
- one monospace family for the entire UI;
- dense rhythm, generous horizontal air inside rows.

Metrics (`theme::Metrics::COMPACT`):

| token | value |
|---|---|
| UI font / small / editor | 12 / 11 / 13 px |
| editor line / sidebar row | 19 / 22 px |
| menu bar / tab strip / status | 26 / 28 / 22 px |
| sidebar width | 232 px |
| scrollbar | 8 px (4 px thumb inset) |
| text padding / gutter padding | 8 / 8 px |
| tab width (displayed) | 4 spaces |

Palette:

| token | dark | light |
|---|---|---|
| canvas `bg` | `#0B0E13` | `#F7F9FC` |
| panel | `#0E1218` | `#EDF1F6` |
| raised (popup / active tab) | `#121821` | `#FFFFFF` |
| border | `#1D2531` | `#CDD7E3` |
| text / dim / faint | `#C7D4E4` / `#8494A8` / `#4E5C71` | `#16202C` / `#5C6C7E` / `#94A2B2` |
| accent | `#39E0AE` | `#0A8F6F` |
| selection | `#16362F` | `#C9EFE4` |
| current line | `#10151D` | `#EFF4F9` |
| gutter | `#0D1117` | `#EEF2F7` |
| hover / active row | `#151C26` / `#19222E` | `#E3EAF2` / `#DCE6F1` |
| error | `#F07178` | `#C0392B` |

Monospace face candidates, first installed wins:
SF Mono, Menlo, JetBrains Mono, Fira Code, DejaVu Sans Mono. CJK falls back via
the font server's normal per-character discovery.

## Milestones

**M1 (done)** — shell, sidebar tree, tabs, full text editing, selection,
undo/redo, clipboard, save, dark/light, IME preedit, self-check.

**M2** — multi-document polish (close-with-confirm via a custom modal), recent
files, find/replace bar, settings persistence (plain key=value file), Markdown
edit/preview split with a self-written block renderer on `pulldown-cmark`'s
event stream, `Save As`.

**M3** — lightweight per-line tokenizer (Rust / JSON / TOML / Markdown) with
dirty-line caching, indent guides, bracket matching, large-file virtualization
(measure cache + visible-only painting already in place), settings panel
(font size, tab width, wrap, theme).

## Non-goals for the editor core

- Rope/CRDT: a `String` with a line-start index is enough while files are
  bounded; revisit if M3's large-file work demands it.
- Syntax trees: M3 needs coloring, not parsing.
- Multi-cursor, split view, VCS integration: not planned for M1–M3.
