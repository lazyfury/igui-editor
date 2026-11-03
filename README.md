# igui-editor

A compact, square-cornered **text / Markdown editor** built on the
[`igui`](https://github.com/lazyfury/igui) engine (Rust). Native window:
`winit` + `wgpu`, UI painted through `igui_ui` / `igui_render` only.

Tabs · undo/redo · find & replace · syntax highlighting (JSON language packs) ·
native LSP (Rust / `rust-analyzer`, Godot / GDScript) · outline panel · Markdown split preview (tables included) ·
Quick Open (`⌘P`) · project search + replace (`⇧⌘F`) · local Git
status/diff/stage/commit · built-in terminal · scroll sync · tab drag/context menu ·
soft wrap · external-change detection · Finder/Dock file opening · Dock Recent ·
quick file creation · drag & drop · settings panel · native file dialogs ·
recent files · session + window restore · autosave & crash recovery ·
large-file guards.

This repository is **independent from the igui engine**: it consumes the
engine through path dependencies and never modifies it. The two checkouts must
be siblings:

```text
workspace/
├── igui/
└── igui-editor/
```

```
Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
```

## Requirements

- Rust **1.85+** with Cargo.
- A sibling [`igui`](https://github.com/lazyfury/igui) checkout, because
  `Cargo.toml` uses `../igui/...` path dependencies.
- `rust-analyzer` for Rust LSP features (optional but recommended; install with
  `rustup component add rust-analyzer`).
- Godot 4 for GDScript LSP features (optional; the default macOS path is
  `/Applications/Godot.app/Contents/MacOS/Godot`; override with
  `IGUI_EDITOR_GODOT`). A `project.godot` file marks the project root.
- macOS is the primary, manually tested platform. Linux/Windows host paths are
  wired, but not manually verified in this repository.

## Build from source

```bash
git clone https://github.com/lazyfury/igui-editor.git
git clone https://github.com/lazyfury/igui.git
cd igui-editor

cargo run --release                          # restore the last session, else the welcome state
cargo run --release -- src/app.rs            # open a file (its directory becomes the folder)
cargo run --release -- src                    # open a folder (no tab: welcome with sidebar)
cargo run --release -- --light README.md     # light palette
cargo run --release -- /path/to/godot_project # open a Godot project folder
```

A self-check runs the same UI headlessly (no screenshots required):

```bash
cargo run --release -- --selfcheck
```

## Package the macOS app

```bash
packaging/package-macos.sh
open dist/igui-editor.app
```

The script builds a release binary, turns `packaging/icon.png` into a full
`.icns`, assembles `dist/igui-editor.app` (Info.plist, icon, binary), and
ad-hoc signs it, and writes a versioned `.zip` next to it. See
[`packaging/README.md`](packaging/README.md) for icon-only
regeneration, bundle IDs, version/build overrides, and Developer ID/notarization
notes. `dist/` is git-ignored.

`⌘O` / `⇧⌘O` / `⇧⌘S` use the **native** file dialogs (macOS `NSOpenPanel` /
`NSSavePanel` sheets, Linux XDG portal, Windows common dialogs). They are
asynchronous: the sheet attaches to the window, the app keeps rendering, and the
result is applied on the first frame after it closes.

## Git panel

The file tree marks modified/untracked files with a colored Git status letter
(modified orange, added green, deleted/conflict red, renamed purple, untracked
teal), and the status bar shows `GIT <branch> ↑ahead ↓behind`. The sidebar has
`EXPLORER / GIT` tabs. GIT shows the repository branch,
ahead/behind, and staged/changes/conflicts sections from `git status
--porcelain=v2 -z`. Click a file for a read-only unified diff overlay; `Esc` or
the overlay ✕ closes it. `+` / `−` stage or unstage a row, `Stage all` stages the
worktree, and `Commit` opens a one-line message prompt. `Changes / History`
switches to the last 50 commits; clicking one opens a read-only commit detail
with author/date/subject, changed files and patch. Diff and commit details open
as read-only tabs in the main editor (never persisted, never editable; the status
bar shows `READ-ONLY`). Diff lines are colored (`+` accent, `-` error, hunk
headers faint); change views append a full current-file preview after the diff. Refreshes happen on
open/save/tab/folder changes, commit and via the panel button. Discard and
network operations are intentionally not exposed.

Git is a **built-in plugin** (`services/git.rs`) over a native worker
(`model/git.rs`), not a JS or dynamic-library extension. See
[`DESIGN-services.md`](DESIGN-services.md).

The selectable **VS Code / JetBrains / Vim** keymaps are implemented. The
profile lives in **Settings → Keymap** and persists in the session; VS Code and
JetBrains share the command layer, while Vim has a real Normal/Visual/Insert
state machine for the common motions and operators. See
[`DESIGN-keymaps.md`](DESIGN-keymaps.md) for the design and boundaries.

## Language intelligence (LSP)

The editor talks to language servers over native JSON-RPC — no WebView, no JS
bridge. Two built-in servers are registered:

- **Rust** — opening a `.rs` file inside a directory with a `Cargo.toml` lazily
  starts `rust-analyzer` from `PATH` (or the path in
  `IGUI_EDITOR_RUST_ANALYZER`) over stdio.
- **Godot GDScript** — opening a `.gd` file inside a directory with a
  `project.godot` lazily starts the Godot editor's GDScript language server
  over loopback TCP. On macOS the default executable is
  `/Applications/Godot.app/Contents/MacOS/Godot`; set `IGUI_EDITOR_GODOT` to
  use a different install. Godot runs headless (`--headless --editor --path .`)
  and is stopped when the editor releases the project.

Install the servers separately; there is no server for loose files or
non-project buffers.

- Diagnostics are painted as severity-colored underlines and a full-height
  gutter severity rail. The footer chip is colored by the worst severity and
  shows `LSP 1E 2W` (or `LSP ERR` if the server cannot start). Click the chip
  to open **PROBLEMS**: every diagnostic with `E`/`W`, line:column and the full
  message; click a row to jump, `Esc` or an outside click closes it. Hovering
  an underlined diagnostic shows its message immediately; moving onto normal
  code resumes the normal LSP hover card.
- Semantic tokens refine the syntax pass: variables, parameters, fields,
  functions/methods, types, macros, keywords and literals get server-aware
  colors. Tokens the server does not classify keep their syntax color.
- Packaged apps launched from Finder also search `~/.cargo/bin`,
  `/opt/homebrew/bin`, `/opt/homebrew/opt/rustup/bin`, `/usr/local/bin` and
  rustup toolchains; if the server still cannot start, the OUTLINE panel shows
  the error detail.
- `F1` shows server hover, `F12` jumps to definition (macOS top-row media keys
  may need `Fn`; `⌘⇧I` / `⌘⇧D` are also available), and `⌘I` (`Ctrl+I` on
  Linux/Windows) opens completion. Typing an identifier also activates
  completion after a 140 ms debounce. Completion results are filtered by the
  identifier prefix before the UI cap, so Godot global methods such as `print`
  are found even though the server returns thousands of candidates. The
  selected item is resolved with `completionItem/resolve` and shows a
  hover-style documentation preview; `OPEN GODOT DOCS` (or `OPEN DOCS` for
  local methods) opens the documentation tab.
- When a hover is a Godot native class or member, the card shows **OPEN GODOT
  DOCS**; clicking it opens a **read-only editor tab** backed by Godot's
  `textDocument/nativeSymbol` response. The tab uses the normal file-tab strip,
  title, scrolling and close behavior, and renders BBCode references as
  clickable class/member links. Member pages include a `Class: @GlobalScope`
  style breadcrumb linking back to the owning class. Ctrl/Cmd-clicking any
  native symbol (class, method, virtual method, global function or property)
  opens the same tab; the completion preview offers the same docs action. `_ready` and other local completion
  items can open their resolved documentation as a plain embedded document.
- Moving the mouse over code for about 350 ms opens the same hover card. The
  card stays while the pointer is over it, and gets a short grace period when
  the pointer leaves the editor.
  Holding Control and clicking an identifier asks LSP for its definition; Cmd
  +click does the same on macOS. `Ctrl+Space` also works where the OS does
  not reserve it for input-method switching. Completion uses `↑`/`↓`,
  `Enter`/`Tab` and `Esc`; the same commands are in **Edit**.
- Buffers are synchronized full-text (debounced after edits); positions use
  LSP UTF-16 columns. Diagnostics/hover/completion are transient UI state and
  are not persisted.

Both servers are declarative [`LanguageServerSpec`](src/services/mod.rs) values;
Rust uses stdio and Godot uses TCP. Data-only language packs under
`<config>/languages/*.json` can contribute one too; the generic client, owned
spec shape and capability ids are ready for a future external extension host.
Native plugins are still compile-time (no dylib ABI).

```json
{
  "name": "lua",
  "extensions": ["lua"],
  "lsp": {
    "id": "lua-ls",
    "language": "lua",
    "extensions": ["lua"],
    "project_markers": ["stylua.toml"],
    "command": "lua-language-server",
    "args": ["--stdio"]
  }
}
```

### Godot project

Open a project folder (or any `.gd` file inside it); `.gd` tabs use the GDScript
syntax provider immediately, and opening one starts Godot's headless language
server for the nearest `project.godot`. For example:

```bash
cargo run --release -- "/path/to/godot_project/new_script.gd"
```

`IGUI_EDITOR_GODOT` can point at another Godot executable. Godot's server sends
diagnostics, hover, completion, definition and document symbols; semantic
tokens are not provided by Godot, so the syntax palette remains in charge of
token colors.

## Outline (`⌘⌥O`, View → Outline)

The right sidebar shows LSP document symbols (`textDocument/documentSymbol`) for
the active document. Rust files inside a Cargo project get functions, methods,
structs/enums, fields, variables and nested items from `rust-analyzer`; GDScript
files get functions, classes, signals, constants and variables from Godot's
server. The list updates after edits. Clicking a row moves the caret to the
symbol and reveals it in the editor. The left edge is a shared drag handle: drag it to resize
(160–420 px), double-click resets to 224 px. The panel is persisted in the
session and can be hidden from View → Outline or `⌘⌥O`. Symbols are transient
LSP state, not saved to disk.

## Terminal (`⌃\``, View → Terminal)

Native PTY (`portable-pty`) + `vt100` screen, no WebView or JS. The panel sits
below the editor with a draggable top handle (120–600 px, default 220 px,
double-click resets). It starts
`$SHELL`, supports ANSI/256-color cells, cursor, resize, plain text, arrows,
Ctrl letters, Alt prefixes, application cursor mode (vim/less) and IME candidate
placement at the terminal cursor. The terminal requests frames while a PTY is
alive, so full-screen programs repaint without extra input. `Ctrl+\`` closes it
and kills the child. Focus is visible through the caret: the active editor caret is accent and
2 px, the inactive caret dims, and the terminal header text/cursor accent only
while it owns keyboard focus. Editor and terminal tops have no accent rule.
Open state and height persist in the session (v9); the shell process and screen
contents do not. Multiple sessions are a follow-up.

## macOS integration

- The `.app` registers text/Markdown document types (`txt`, `md`, `markdown`,
  `mdown`, `mkd`) with Editor role and `Alternate` rank. Finder double-click,
  `open -a`, “Open With” and cold launch all feed the same native request queue.
- **Dock Recent**: right-click the Dock icon for `New File…`, an `Open Recent`
  submenu built from the last 8 session paths, and `Clear Menu`. Missing files
  are hidden/disabled; opening/saving also notes the file in the system Recent
  Items via `NSDocumentController`.
- **Quit guard**: `⌘Q` with dirty tabs returns `NSTerminateLater` and shows the
  in-app **Discard & Quit / Cancel** confirmation; a clean app quits directly.
- The native bridge never replaces winit's `NSApplicationDelegate`; it adds
  `application:openURLs:`, `applicationDockMenu:` and
  `applicationShouldTerminate:` to the existing delegate class and keeps
  `setDelegate:` hooked so cold-open events are not lost.

## Keys

| Key | Action |
|---|---|
| `⌘O` / `⇧⌘O` | open file / open folder (sidebar root) |
| `⌘S` / `⇧⌘S` | save / save as |
| `⌘N` | new buffer (untitled) |
| `⇧⌘N` | new file on disk (quick prompt) |
| `⌃\`` | toggle the built-in terminal |
| `⌘W` | close tab (asks Save / Discard / Cancel when dirty) |
| `⌘Z` / `⇧⌘Z` | undo / redo (typing runs coalesce) |
| `⌘X` / `⌘C` / `⌘V` | cut / copy / paste (empty selection: cut/copy the current line) |
| `⌘⇧K` (VS Code) | delete the current line |
| `⌘/` | toggle line comment (when the language has a marker) |
| `⌘D` / `⌘⌫` (JetBrains) | duplicate / delete the current line |
| `⌥↑` / `⌥↓` | move the current line up / down (VS Code profile) |
| `⌘A` | select all |
| `⌘P` | Quick Open (fuzzy file picker) |
| `⇧⌘F` | search in folder |
| `⌘⌥O` | toggle the right outline panel |
| `F1` / `F12` | LSP hover / go to definition (or `⌘⇧I` / `⌘⇧D`) |
| `⌘I` / `Ctrl+I` | LSP completion (also `Ctrl+Space` when available) |
| `⌘,` | settings panel |
| `⌘F` / `⌥⌘F` | find / jump to the replace field |
| `⌘G` / `⇧⌘G` | next / previous match |
| `⌘B` | toggle sidebar |
| `⇧⌘L` | toggle dark / light |
| `⇧⌘M` | cycle Editor → Split Preview → Preview Only |
| `⇧⌘Y` | toggle editor/preview scroll sync |
| `⌥Z` | toggle soft wrap |
| `Tab` / `⇧Tab` | indent / outdent (block-aware) |
| `Enter` | newline with the current indentation |
| `⌘←` / `⌘→` | smart home / line end |
| `⌘↑` / `⌘↓` | document start / end |
| `⌥←` / `⌥→` | word left / right |
| `⌥` + click | extend selection |
| double click | select word |
| triple click | select the current line |
| `Enter` / `⇧Enter` in find | next / previous match |
| `Esc` | close find · cancel modal |
| wheel / trackpad | vertical scroll, `⇧`+wheel horizontal; over the tab strip it scrolls overflowing tabs (no tab scrollbar) |
| drag a tab | reorder it (order persists across restarts) |
| right-click a tab | Close / Close Others / Close to the Right / Copy Path / Reveal in Sidebar |
| drop files / folders | files open in tabs; a folder becomes the workspace |

Editing follows VS Code / JetBrains conventions: `⌘X` / `⌘C` with an empty
selection operate on the current line, double-click selects a word, and
triple-click selects the line (including its newline). Put the caret at the end
of a line and press `⌘X` to remove that line; no special “line selection” click
mode is required.

Keymap profiles:

- **VS Code** — default; `⌘⇧K` delete line, `⌥↑/↓` move line,
  `⇧⌥↓` duplicate line, empty-selection line cut/copy.
- **JetBrains** — `⌘D` duplicate line, `⌘⌫` delete line on macOS;
  `Ctrl+D` / `Ctrl+Y` on Linux/Windows; `⌘⇧↑/↓` move line.
- **Vim** — starts in Normal mode. Supports `i a I A o O`, `h j k l`,
  `w b`, `0 $`, `gg G`, `x`, `dd`, `yy`, `p`, `u`, `Ctrl+r`, and simple
  Visual (`v`/`V`, motions, `d`/`y`). Ex commands, text objects and registers
  are not implemented.

Overrides can be added in `<config>/keymaps.json` (read at startup), using
command ids such as `edit.deleteLine`, `edit.duplicateLine` or `file.save`.
The selected built-in profile supplies the base bindings; null removes one.

Find bar: query and replacement fields, `Aa` (case) and `W` (whole word)
toggles, `Replace` / `Replace All`, match counter, prev/next/close. All matches
are tinted in the editor, the current one strongly.

Sidebar and split dividers share one **4 px drag handle** (spec:
[DESIGN.md §Drag handle](DESIGN.md#drag-handle-specification)): one centered
1 px line plus a 2 × 28 grip nudged 1 px toward the window centre (sidebar →
right, split and outline → left); the grips share the content-band centre, so
they are the same height even with the find bar open. Line/grip are
`border`/`faint` at rest, `dim`/`text` on hover, `accent` while dragging (the
strip tints). Each seam has exactly one line — neither the sidebar nor the
preview or outline paints its own border. The sidebar handle resizes 180–420 px,
the outline handle 160–420 px, and both widths persist. Tabs shrink to 96 px, then scroll horizontally with the wheel; the
active tab is revealed on a switch/new/close, while manual scrolling is free.
No tab scrollbar is drawn.

Markdown preview (`.md` opens in Split; `⇧⌘M` cycles): headings, paragraphs,
bold / italic / strikethrough, inline code, fenced code blocks with a language
label, nested lists, quotes with an accent rule, rules, images as `▢ alt`,
styled links (no navigation) and **tables with per-column alignment**. The
preview scrolls with the wheel/trackpad or its scrollbar.

Mouse: click a folder to expand it, a file to open it in a tab, `×` to close a
tab (unsaved tabs open a Save / Discard / Cancel modal), `+` for a new buffer,
drag a tab to reorder it, right-click it for Close / Close Others / Close to the
Right / Copy Path / Reveal in Sidebar (closing several dirty tabs asks once),
right-click a file-tree row for Open / New File / Copy Path / Reveal in Finder,
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

## Language packs (footer language type, `<config>/languages/*.json`)

Built-ins always win a name clash; packs are data only (no code loading).
Drop a JSON file like this into the config's `languages/` directory and restart:

```json
{
  "name": "lua",
  "extensions": ["lua"],
  "line_comment": ["--"],
  "block_comment": ["--[[", "]]"],
  "keywords": ["function", "local", "end", "return"],
  "constants": ["nil", "true", "false"],
  "strings": [{ "quote": "\"", "escapes": true }]
}
```

The optional fields mirror the C-family tokenizer: `types`, `constants`,
`strings`, `nested_block`, `triple`, `template`, `template_escapes`,
`preprocessor`, `annotations`, `toml_tables`, `case_types`. An optional `lsp`
object contributes a language server (see **Language intelligence**). Limits:
64 packs, 64 KB each; invalid packs are skipped (reported on stderr + status).

Per-document override: click the language type in the **bottom-right footer**
(`LUA`, `RUST`, `TEXT`, …) → Auto (by extension) or any registered provider.
The popup opens above the status bar. Path overrides are persisted in the
session; untitled buffers keep theirs for the session only.

## Replace in files

The `⇧⌘F` panel has a replace row. `Replace all in files` plans the change and
asks for confirmation, then rewrites files atomically:

- re-matches each file's **current** text (never stale offsets);
- skips files open in a tab, changed since the search (length/mtime), binaries
  and files over 1 MB;
- caps at 200 files per run; failures are listed, the rest still applies;
- there is no cross-file undo — commit or back up first.

## Scroll sync (`⇧⌘Y`, View menu)

In Split view the editor and rendered preview stay in step. Markdown blocks
carry their source line ranges, so the preview publishes `(source line → y)`
anchors; scrolling either side moves the other to the corresponding block. One
side wins per frame (the one that changed), which prevents feedback loops; the
setting is persisted in the session. Preview-only and Edit modes ignore it.

## Settings (`⌘,` or View → Settings…)

| Row | Values |
|---|---|
| Theme | Dark / Light |
| Font Size | 11–18 px |
| Tab Width | 2 / 4 / 8 |
| Line Numbers | on / off |
| Indent Guides | on / off |
| Syntax Highlight | on / off |
| Keymap | VS Code / JetBrains / Vim |
| Autosave | on / off |
| Sidebar Width | 180–420 px |
| Reset to Defaults | — |

Changes apply immediately and are stored in the session file. `↑↓` select,
`←→` change, `Enter` toggles, `Esc` closes; the `−`/`+` boxes and rows are
clickable.

## Large files

| Limit | Behavior |
|---|---|
| > 2 MB or > 20k lines | syntax highlighting and soft wrap off |
| > 8 MB | preview disabled (banner); editing continues |
| > 32 MB | opening refused with a status-bar error |
| preview > 512 KB | first 512 KB rendered + "truncated" banner |

## Workspace, welcome and drag & drop

- **No folder open** (first launch, **File → Close Folder**, or after closing
  the folder's tabs): the editor shows a welcome panel with actions (Open File / Open Folder / New
  Buffer / Quick Open / Search when a folder is open), and the sidebar shows
  `NO FOLDER OPEN` with the same actions plus a drop hint. Opening a file works
  without a folder; Quick Open and project search ask for one first.
- **Open Folder…** (`⇧⌘O`) or dropping a folder sets the sidebar root and
  persists it in the session. **File → Close Folder** clears the folder, all
  open tabs and the persisted root (asking first if a tab is dirty). Dropping files
  opens them in tabs; dropping both opens the folder first, then the files.
  While a drag hovers the window, a themed “Drop to open” overlay covers the
  viewport.
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
worker. The replace row and **Replace all in files** flow are described under
[Replace in files](#replace-in-files).

## Soft wrap (`⌥Z`, View menu)

Off by default and persisted in the session. Long logical lines are split into
display rows inside the editor; only the editor changes — preview and split
layout are untouched. The status-bar `Ln/Col` stays logical, `↑`/`↓` and clicks
use visual rows, and the gutter numbers only appear on a line's first row.
Horizontal scrolling is suppressed while wrapped. Soft wrap is disabled above
2 MB / 20k lines (the toggle reports why). The last horizontal scroll offset is
restored when wrap is turned off.

## External file changes

Every second the app compares open files' length + mtime. A clean tab reloads
automatically; a dirty tab asks **Keep** or **Reload** before replacing the
buffer. Deleted files are reported in the status bar and the buffer is kept. No
filesystem-watcher dependency; the poll is metadata-only.

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
- Session format is version 9 (`outline_width`; v8 added `outline`, v7 added
  `keymap`, v6 added `terminal_open`, `terminal_height`; v5 added
  `sidebar_tab`, v4 `soft_wrap`); older files still load (v1 infers the folder
  from the first tab).

## Theme: "geek", compact, no corners

Not `igui_theme::DefaultTheme`. The app owns its palette and metrics
(`src/theme.rs`):

- **No rounded rectangle anywhere** — every surface is a square fill with 1 px
  rules; scrollbar thumbs and carets are straight bars.
- **One text grid** — on macOS the host defaults to **PingFang SC** so Chinese
  renders with real glyphs instead of the engine's box-shaped symbol fallback.
  Other hosts prefer a monospace face (JetBrains Mono / Fira Code / DejaVu Sans
  Mono) and use the font server's per-character fallback.
- **Compact rhythm** — 14 px UI text / 12 px secondary text, 26 px menu bar,
  22 px rows, 22 px status bar, 8 px scrollbars; the editor defaults to 14 px
  text / 19 px lines and scales with the font setting (11–18 px).
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

M5 checks: a temporary JSON language pack highlights a `.lua` fixture; the
footer language type (bottom-right) opens a popup above the status bar, and
lua → rust → Auto changes token colors and the footer label; overrides survive a
session restore; File → Close Folder clears the tabs and the persisted folder;
replace-in-files (cancel is a no-op, apply rewrites a non-open file, an
open tab is skipped); and scroll sync moves the preview from the editor and the
editor back from the preview (including the disable toggle).

M7 checks: native request queue opens a file and triggers New File; quick-create
creates a real file and refuses to overwrite; `application:openURLs:` is verified
against cold and running `open -a` launches; Dock menu/quit methods are installed
on the existing winit delegate.

LSP checks: a scripted stdio language server is launched through the real
client; diagnostics appear under the changed line and in the footer, `F1` paints
a hover card, `⌘I`/`Ctrl+I` opens a completion list and inserts the selected
item, and `F12` jumps to the returned definition.

M6 checks: tab drag-reorder and the right-click Close menu, soft-wrap layout
(long-line split, wrapped click / Up / Down, `⌥Z`), clean/dirty external reload
with the Keep/Reload modal, and the session soft-wrap setting persisted.

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
  model/git.rs        git status/diff/stage/commit worker + porcelain parser
  model/pty.rs        portable-pty PTY session
  model/fuzzy.rs      Quick Open ranking
  model/workspace_index.rs  ignored-dir file index (Quick Open)
  model/search.rs     background project search (worker + caps)
  keymap.rs           chord parser, profiles, user override resolver
  vim.rs              Normal/Insert/Visual key handling (Vim profile)
  syntax/pack.rs      declarative JSON language packs
  model/lsp.rs        stdio/TCP JSON-RPC client, diagnostics, hover/completion,
                      native Godot symbols
  model/bbcode.rs     Godot BBCode -> text spans + clickable references
  model/docs.rs       native symbol -> docs view text + link ranges
  services/lsp.rs     built-in Rust language-server spec (rust-analyzer)
  services/godot.rs   built-in Godot GDScript language-server spec (TCP)
  model/recovery.rs   crash snapshots (txt + metadata)
  model/replace.rs    replace-in-files planning/apply (conflict + open skip)
  model/fs_tree.rs    lazy sidebar tree (dirs first, hidden entries skipped)
  model/markdown.rs   Markdown event stream -> flat block model
  model/session.rs    key=value session file (atomic write, escaping, v5)
  model/workspace.rs  tabs + file IO (open/save/move/close folder)
  model/wrap.rs       soft-wrap display rows + visual row/column mapping
  dialogs.rs          `Dialogs` trait + scripted test double
  drop.rs             shared file-drop target (winit observer -> update)
  native.rs           Finder/Dock/quit request queue (main-thread)
  macos/              Apple Event open, Dock menu, recent + quit guard
  services/           native service registry + built-in plugin descriptors
  ui/git.rs           GIT sidebar tab + diff overlay + commit prompt
  ui/lsp.rs           hover card + completion list overlays
  ui/problems.rs      severity-colored PROBLEMS popup and diagnostic hover
  ui/docs_view.rs     dedicated read-only docs tab view (wrap + link hit-test)
  ui/outline.rs       right document-symbol outline
  ui/tree_menu.rs     file-tree context menu
  ui/settings.rs      settings + Keymap row
  ui/terminal.rs      vt100 cell renderer + terminal key encoding
  input.rs            keyboard dispatch: modal -> find -> shortcuts -> tree
  state.rs            shared state, actions, hit rects recorded during paint
  theme.rs            geek palette + compact metrics
  kit.rs              control-node construction and text/line helpers
  ui/menu.rs          menu bar + app-level drop-down overlay
  ui/sidebar.rs       file tree
  ui/tabs.rs          tab strip (drag reorder, close targets)
  ui/tab_menu.rs      right-click tab menu (close variants / path / reveal)
  ui/editor.rs        gutter, text, selection, caret, scrollbars, IME preedit
  ui/find_bar.rs      find / replace strip
  ui/quick_open.rs    Quick Open palette
  ui/new_file.rs      quick-create-file prompt
  ui/project_search.rs  project search panel
  ui/split.rs         draggable editor/preview divider
  ui/recovery.rs      startup recovery prompt
  ui/drop.rs          drop overlay
  ui/preview.rs       rendered Markdown pane (cached primitive layout)
  ui/modal.rs         close-dirty / close-many / external / replace modals
  ui/status.rs        file / position / facts / messages
  app.rs              shell construction, input routing, igui_app glue
  host.rs             winit + wgpu plugins, native dialogs, CJK-capable face
  selfcheck.rs        --selfcheck
packaging/
  icon.png            1254×1254 app-icon source
  build-icon.sh       sips/iconutil -> AppIcon.icns
  package-macos.sh    release build + .app bundle + ad-hoc signature
  Info.plist.in       bundle metadata template
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

Delivered (M5): declarative JSON language packs + per-document language
override (persisted by path), replace-in-files with a confirmation modal,
conflict/open-tab skipping and atomic writes, editor/preview scroll sync via
Markdown source-line anchors, session v3.

Delivered (M6): tab drag-reorder + right-click context menu, soft wrap with
visual-row caret/click/navigation mapping and a 2 MB / 20k-line cap, external
file-change detection (clean auto-reload, dirty Keep/Reload), session v4.

Delivered (M7): macOS Finder/`open -a` file opening, Dock Recent/New File/Clear,
quick file creation (`⇧⌘N`), system recent items, and a native dirty-quit guard.

Delivered (M12): generic LSP stdio/TCP client + Rust `rust-analyzer` and Godot
GDScript plugins (diagnostics, hover, completion, definition and symbols), plus
data-only language-pack language-server specs.

Next candidates: native macOS top menu bar (Window/Services/Help), window-close
guard + Dock reopen, code-plugin loading (dylib/wasm), per-hit replace and
cross-file undo, incremental preview layout for very large documents, and
word-boundary wrapping.

Notes:

- Native macOS top menu, a close-window dirty guard and Dock reopen are not
  implemented yet; closing the window still follows winit's event-loop exit.
- Only macOS has been exercised manually; the Linux (XDG portal) and Windows
  dialog paths are wired but untested.
- The macOS sheet path is asynchronous by design (no nested modal loop, no
  re-entrant event dispatch); the app polls the dialog future once per frame
  while it is open.
- Recovery snapshots are written after the 2 s idle delay: a hard kill within
  that window can still lose the last keystrokes (autosave shares the delay).
- Project search skips binary files, files over 1 MB, and stops at the caps
  above.
- Quick Open builds its index when opened (no filesystem watcher), so files
  created while it is closed appear on the next open. Open files are polled for
  external changes, but the sidebar/search index is not automatically refreshed.
- Soft wrap breaks at character boundaries, not words, and uses a full-layout
  cache; it is intentionally disabled for very large files.

Known engine limitation: `igui_core::Key` has no `PageUp` / `PageDown`, so page
scrolling is not bound yet (`Document::page` is implemented and tested, waiting
for a key variant). `⌘↑` / `⌘↓` cover document navigation meanwhile.
