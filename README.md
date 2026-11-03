# igui-editor

A compact, square-cornered **text / Markdown editor** built on the
[`igui`](../igui) engine (Rust). Native window: `winit` + `wgpu`, UI painted
through `igui_ui` / `igui_render` only.

Tabs · undo/redo · find & replace · syntax highlighting · Markdown split preview
(tables included) · Quick Open (`⌘P`) · project search (`⇧⌘F`) · drag & drop ·
settings panel · native file dialogs · recent files · session + window restore ·
autosave & crash recovery · large-file guards.

This checkout is **outside** the igui repository: it consumes the engine through
path dependencies and never modifies it.

```
Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
```

## Run

```bash
cd igui-editor
cargo run --release                          # restore the last session, else the welcome state
cargo run --release -- src/app.rs            # open a file (its directory becomes the folder)
cargo run --release -- src                    # open a folder (no tab: welcome with sidebar)
cargo run --release -- --light README.md     # light palette
```

`⌘O` / `⇧⌘O` / `⇧⌘S` use the **native** file dialogs (macOS `NSOpenPanel` /
`NSSavePanel` sheets, Linux XDG portal, Windows common dialogs). They are
asynchronous: the sheet attaches to the window, the app keeps rendering, and the
result is applied on the first frame after it closes.

## Keys

| Key | Action |
|---|---|
| `⌘O` / `⇧⌘O` | open file / open folder (sidebar root) |
| `⌘S` / `⇧⌘S` | save / save as |
| `⌘N` | new buffer (untitled) |
| `⌘W` | close tab (asks Save / Discard / Cancel when dirty) |
| `⌘Z` / `⇧⌘Z` | undo / redo (typing runs coalesce) |
| `⌘X` / `⌘C` / `⌘V` | cut / copy / paste |
| `⌘A` | select all |
| `⌘P` | Quick Open (fuzzy file picker) |
| `⇧⌘F` | search in folder |
| `⌘,` | settings panel |
| `⌘F` / `⌥⌘F` | find / jump to the replace field |
| `⌘G` / `⇧⌘G` | next / previous match |
| `⌘B` | toggle sidebar |
| `⇧⌘L` | toggle dark / light |
| `⇧⌘M` | cycle Editor → Split Preview → Preview Only |
| `Tab` / `⇧Tab` | indent / outdent (block-aware) |
| `Enter` | newline with the current indentation |
| `⌘←` / `⌘→` | smart home / line end |
| `⌘↑` / `⌘↓` | document start / end |
| `⌥←` / `⌥→` | word left / right |
| `⌥` + click | extend selection |
| double click | select word |
| `Enter` / `⇧Enter` in find | next / previous match |
| `Esc` | close find · cancel modal |
| wheel / trackpad | vertical scroll, `⇧`+wheel horizontal; over the tab strip it scrolls overflowing tabs |
| drop files / folders | files open in tabs; a folder becomes the workspace |

Find bar: query and replacement fields, `Aa` (case) and `W` (whole word)
toggles, `Replace` / `Replace All`, match counter, prev/next/close. All matches
are tinted in the editor, the current one strongly.

Sidebar and split dividers share one **4 px drag handle** (spec:
[DESIGN.md §Drag handle](DESIGN.md#drag-handle-specification)): one centered
1 px line plus a 2 × 28 grip nudged 1 px toward the window centre (sidebar →
right, split → left); both grips share the content-band centre, so they are the
same height even with the find bar open. Line/grip are
`border`/`faint` at rest, `dim`/`text` on hover, `accent` while dragging (the
strip tints). Each seam has exactly one line — neither the sidebar nor the
preview paints its own border. The sidebar handle resizes 180–420 px and
persists. Tabs shrink to 96 px, then scroll horizontally; the active tab is
always kept visible.

Markdown preview (`.md` opens in Split; `⇧⌘M` cycles): headings, paragraphs,
bold / italic / strikethrough, inline code, fenced code blocks with a language
label, nested lists, quotes with an accent rule, rules, images as `▢ alt`,
styled links (no navigation) and **tables with per-column alignment**. The
preview scrolls with the wheel/trackpad or its scrollbar.

Mouse: click a folder to expand it, a file to open it in a tab, `×` to close a
tab (unsaved tabs open a Save / Discard / Cancel modal), `+` for a new buffer,
drag the editor scrollbars, drag in the text to select, wheel-scroll the
preview.

## Syntax highlighting

Built-in providers: **Rust, C, C++, JavaScript, TypeScript, Go, GDScript, JSON,
TOML, Markdown**. Highlighting is line based with carried state (block comments,
raw/template strings, fences), so multi-line constructs color correctly. The
cache re-tokenizes from the first edited line and stops when the carried state
converges, budgeted per frame.

The provider API is public (`igui_editor::syntax`) so a future plugin host can
add languages without touching the editor core:

```rust
use std::sync::Arc;
use igui_editor::syntax::{Highlight, SyntaxProvider, SyntaxRegistry, TokenKind};

struct MyLang;
impl SyntaxProvider for MyLang {
    fn name(&self) -> &'static str { "mylang" }
    fn extensions(&self) -> &'static [&'static str] { &["mylang"] }
    fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
        // encode multi-line constructs in `state`; 0 is the neutral state
        let _ = (line, state);
        (vec![], 0)
    }
}

let mut registry = SyntaxRegistry::with_builtins();
registry.register(Arc::new(MyLang));

// In the app, the host calls the same registration through `EditorApp`:
// app.register_syntax(Arc::new(MyLang));
```

Plugin **loading** is not implemented; the trait, the registry and
`EditorApp::register_syntax` are the extension seam only.

## Settings (`⌘,` or View → Settings…)

| Row | Values |
|---|---|
| Theme | Dark / Light |
| Font Size | 11–18 px |
| Tab Width | 2 / 4 / 8 |
| Line Numbers | on / off |
| Indent Guides | on / off |
| Syntax Highlight | on / off |
| Autosave | on / off |
| Sidebar Width | 180–420 px |
| Reset to Defaults | — |

Changes apply immediately and are stored in the session file. `↑↓` select,
`←→` change, `Enter` toggles, `Esc` closes; the `−`/`+` boxes and rows are
clickable.

## Large files

| Limit | Behavior |
|---|---|
| > 2 MB or > 20k lines | syntax highlighting off |
| > 8 MB | preview disabled (banner); editing continues |
| > 32 MB | opening refused with a status-bar error |
| preview > 512 KB | first 512 KB rendered + "truncated" banner |

## Workspace, welcome and drag & drop

- **No folder open** (first launch, or after closing the folder's tabs): the
  editor shows a welcome panel with actions (Open File / Open Folder / New
  Buffer / Quick Open / Search when a folder is open), and the sidebar shows
  `NO FOLDER OPEN` with the same actions plus a drop hint. Opening a file works
  without a folder; Quick Open and project search ask for one first.
- **Open Folder…** (`⇧⌘O`) or dropping a folder sets the sidebar root and
  persists it in the session. Dropping files opens them in tabs; dropping both
  opens the folder first, then the files. While a drag hovers the window, a
  themed “Drop to open” overlay covers the viewport.
- Closing the last tab returns to the welcome state (no hidden untitled buffer).
- Status bar without a document shows `no document` plus the folder name (or
  `no folder open`).

## Quick Open (`⌘P`)

Fuzzy picker over the workspace: subsequence match on the relative path with
bonuses for the file name (prefix > substring > subsequence) and recent files.
Indexing skips `.git`, `target`, `node_modules`, `dist`, `build`, `.venv`,
hidden entries; capped at 20k files. `↑↓` select, `Enter` opens, `Esc` closes,
typing filters live; the panel shows file count and match preview.

## Search in Folder (`⇧⌘F`)

Background literal search (case/whole-word toggles) over the workspace, reusing
the Quick Open ignore rules. Skips binary files and files over 1 MB; caps at
5k scanned files / 500 hits / 20 per file. Results stream in grouped by line
with the match highlighted; `Enter` or a click opens the file, selects the match
and reveals it. The query is debounced (120 ms) and a new query cancels the old
worker. No replace-in-files yet.

## Split view

`⇧⌘M` cycles Editor → Split Preview → Preview Only. In split mode the divider is
draggable (20–80%, double-click resets to 50%) and uses the same 4 px handle
specification as the sidebar; the preview draws no extra border. The ratio is
remembered per document and the last ratio is persisted.

## Autosave, recovery and window state

- **Autosave** (settings row, default on): dirty files with a path are written
  after 2 s without edits.
- **Crash recovery**: dirty buffers (including untitled ones) are snapshotted
  under `<config>/recovery/` after the same idle delay. On the next launch a
  modal offers **Restore All** or **Discard All**; restored buffers keep their
  original name/path and are marked dirty so the first save writes them back.
- **Window state**: size and position are persisted and restored (skipped while
  maximized/fullscreen).
- Session format is version 2 (`folder`, `split`, `autosave`, `window.*` added);
  v1 files still load (the folder is inferred from the first tab).

## Theme: "geek", compact, no corners

Not `igui_theme::DefaultTheme`. The app owns its palette and metrics
(`src/theme.rs`):

- **No rounded rectangle anywhere** — every surface is a square fill with 1 px
  rules; scrollbar thumbs and carets are straight bars.
- **One monospace grid** — the host seeds `FontServer` with the first installed
  monospace face (SF Mono / Menlo / JetBrains Mono / Fira Code / DejaVu Sans
  Mono); the whole UI uses it.
- **Compact rhythm** — 26 px menu bar, 22 px rows, 22 px status bar, 8 px
  scrollbars; the editor defaults to 13 px text / 19 px lines and scales with the
  font setting (11–18 px).
- **Palette** — dark: `#0B0E13` canvas, `#0E1218` panels, `#39E0AE` phosphor
  accent, `#16362F` selection; light: `#F7F9FC` canvas, `#0A8F6F` teal accent.
- Dark and light are separate hand-tuned palettes, not an inversion; switching
  is a value swap (no rebuild, no second code path).
- Syntax colors (dark): keyword `#C792EA`, type `#7DD3FC`, function `#82E2A6`,
  string `#E6C889`, number `#F78C6C`, comment `#55637A`, constant `#FF9E64`,
  macro `#39E0AE`, punctuation `#8494A8`; the light palette has equivalents.

## Verify (no screenshots)

```bash
cargo test                                  # model: caret, undo, find, markdown, session, FS tree
cargo run -- --selfcheck                    # headless DrawList assertions
```

`--selfcheck` drives the real app (update -> layout -> paint -> record) into
`igui_backend_recording`, with **scripted dialogs** and a **temporary session
file**, then asserts:

1. `igui_profile::inspect` finds no `Error`-level structural problems,
2. the opened file's text, line numbers, menu labels and status facts are in
   the recorded draw list, and the editor emits a clip,
3. typing paints and mutates the model, `⌘S` round-trips through disk,
   `⌘Z` / `⇧⌘Z` undo/redo, the window title tracks the dirty flag,
4. `File > Save` works through the drop-down's real hit rectangles,
5. `⌘F` → typing finds and selects a match (`1 / 1` in the bar), Replace All
   rewrites the document, `Esc` returns typing to the editor,
6. `⇧⌘S` and `⌘O` go through the dialog layer and land on disk / in a tab,
   Markdown opens in Split with the rendered heading, list and code label,
7. the Markdown table renders as cells (header baseline, column order,
   right-aligned column, body below the header), the preview wheel scrolls in
   the right direction, and `⇧⌘M` cycles the pane structure (preview-only has
   no editor caret and no raw pipe text),
8. `⌘W` on a dirty tab shows the modal; `Esc` keeps the tab, `Discard` closes
   it, `Save` writes and closes it,
9. a sidebar click opens a file **and returns keyboard focus**; drag selection
   and wheel scrolling work,
10. `⇧⌘L` repaints with the light palette, `⇧⌘O` retargets the sidebar root,
    and a fresh app restores the theme, the open tab, the recent list and every
    editor setting from the session file.

M3 checks: syntax token colors (keyword/function/string/comment), incremental
re-highlight after an edit, indent guides on/off, caret bracket matching, the
settings panel (font, tab width, gutter, guides, syntax, sidebar width), sidebar
drag-resize, the large-preview truncation banner, the >32 MB open refusal, tab
overflow scrolling, and settings restored from the session.

M4 checks: welcome/no-folder state and empty sidebar, drop overlay, dropping a
file vs a folder, CLI folder argument, Quick Open (filter → open), project
search (debounced worker → result → jump selects the match), split-divider drag
+ double-click reset + persisted ratio, autosave write, recovery snapshot
creation/removal, the startup restore/discard prompt, window geometry
persistence, and folder/split/window restored from the session.

No OS capture of any kind is used.

## Layout of the code

```
src/
  model/doc.rs        text buffer, caret/selection, movement, delta undo stack
  model/find.rs       plain-text matcher (case / whole-word)
  model/fuzzy.rs      Quick Open ranking
  model/workspace_index.rs  ignored-dir file index (Quick Open)
  model/search.rs     background project search (worker + caps)
  model/recovery.rs   crash snapshots (txt + metadata)
  model/fs_tree.rs    lazy sidebar tree (dirs first, hidden entries skipped)
  model/markdown.rs   Markdown event stream -> flat block model
  model/session.rs    key=value session file (atomic write, escaping, v2)
  model/workspace.rs  tabs + file IO
  dialogs.rs          `Dialogs` trait + scripted test double
  drop.rs             shared file-drop target (winit observer -> update)
  input.rs            keyboard dispatch: modal -> find -> shortcuts -> tree
  state.rs            shared state, actions, hit rects recorded during paint
  theme.rs            geek palette + compact metrics
  kit.rs              control-node construction and text/line helpers
  ui/menu.rs          menu bar + app-level drop-down overlay
  ui/sidebar.rs       file tree
  ui/tabs.rs          tab strip
  ui/editor.rs        gutter, text, selection, caret, scrollbars, IME preedit
  ui/find_bar.rs      find / replace strip
  ui/quick_open.rs    Quick Open palette
  ui/project_search.rs  project search panel
  ui/split.rs         draggable editor/preview divider
  ui/recovery.rs      startup recovery prompt
  ui/drop.rs          drop overlay
  ui/preview.rs       rendered Markdown pane (cached primitive layout)
  ui/modal.rs         unsaved-changes confirmation
  ui/status.rs        file / position / facts / messages
  app.rs              shell construction, input routing, igui_app glue
  host.rs             winit + wgpu plugins, native dialogs, monospace face
  selfcheck.rs        --selfcheck
```

## Scope and known gaps

Delivered (M1): shell, sidebar, tabs, editing, selection, undo/redo, clipboard,
save, dark/light, IME preedit, headless self-check.

Delivered (M2): native Open / Open Folder / Save As, find & replace with
case/whole-word and match highlighting, Markdown edit/split/preview with a
block renderer, dirty-close confirmation, recent files, session persistence
(theme, sidebar, open tabs), window title, centralized key dispatch.

Delivered (M3): syntax highlighting (10 languages + public provider API),
incremental highlight cache with per-frame budget, indent guides, bracket
matching, settings panel with persisted values, sidebar drag-resize, tab
overflow scrolling, large-file guards (highlight/preview/open limits), the
library split (`igui_editor::syntax` etc.).

Delivered (M4): welcome/no-folder state, file & folder drag-and-drop, CLI folder
argument, Quick Open, project search (worker + caps), draggable split divider,
autosave, crash recovery prompt, window size/position restore, session v2.

Next candidates: plugin/language-pack loading on top of `SyntaxRegistry`,
replace-in-files, editor/preview scroll sync (needs a block ↔ line mapping),
incremental preview layout for very large documents, tab drag-reorder/context
menu, and soft wrap (still needs a design: it breaks the line/column mapping).

Notes:

- Only macOS has been exercised manually; the Linux (XDG portal) and Windows
  dialog paths are wired but untested.
- The macOS sheet path is asynchronous by design (no nested modal loop, no
  re-entrant event dispatch); the app polls the dialog future once per frame
  while it is open.
- Recovery snapshots are written after the 2 s idle delay: a hard kill within
  that window can still lose the last keystrokes (autosave shares the delay).
- Project search skips binary files, files over 1 MB, and stops at the caps
  above; there is no replace-in-files yet.
- Quick Open builds its index when opened (no filesystem watcher), so files
  created while it is closed appear on the next open.

Known engine limitation: `igui_core::Key` has no `PageUp` / `PageDown`, so page
scrolling is not bound yet (`Document::page` is implemented and tested, waiting
for a key variant). `⌘↑` / `⌘↓` cover document navigation meanwhile.
