# Feature guide

Everything below is implemented in this checkout. For architecture and
verification, see [design.md](design.md) and [verification.md](verification.md).

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
[`services.md`](services.md).

The selectable **VS Code / JetBrains / Vim** keymaps are implemented. The
profile lives in **Settings → Keymap** and persists in the session; VS Code and
JetBrains share the command layer, while Vim has a real Normal/Visual/Insert
state machine for the common motions and operators. See
[`keymaps.md`](keymaps.md) for the design and boundaries.

## Language intelligence (LSP)

The editor talks to language servers over native JSON-RPC — no WebView, no JS
bridge. Six built-in servers are registered:

- **Rust** — `rust-analyzer` over stdio, rooted at the nearest `Cargo.toml`
  (workspace root when `Cargo.lock`/`rust-toolchain.toml` exists).
- **Go** — `gopls` over stdio, rooted at `go.work` or the nearest `go.mod`.
- **TypeScript / JavaScript** — `typescript-language-server --stdio`, rooted at
  a workspace marker (`pnpm-workspace.yaml`, `lerna.json`, `nx.json`, …) or the
  nearest `package.json`/`tsconfig.json`; `.tsx`/`.jsx` are sent as
  `typescriptreact`/`javascriptreact`.
- **C / C++** — `clangd` over stdio, rooted at `compile_commands.json` /
  `CMakeLists.txt` / `.clangd` / the repository root; `.h` is `c`, other
  headers and sources are `cpp`.
- **Python** — `pyright-langserver --stdio`, rooted at `pyrightconfig.json`, a
  Python project marker or the file's folder (loose files work).
- **Godot GDScript** — the Godot editor's GDScript server over loopback TCP.
  On macOS the default executable is
  `/Applications/Godot.app/Contents/MacOS/Godot`; set `IGUI_EDITOR_GODOT` to
  use a different install. Godot runs headless (`--headless --editor --path .`)
  and is stopped when the editor releases the project.

Each spec declares `IGUI_EDITOR_*` overrides (`IGUI_EDITOR_GOPLS`,
`IGUI_EDITOR_TS_SERVER`, `IGUI_EDITOR_CLANGD`, `IGUI_EDITOR_PYRIGHT`,
`IGUI_EDITOR_RUST_ANALYZER`, `IGUI_EDITOR_GODOT`). Repository-local
`node_modules/.bin` shims are **not** executed by default; set
`IGUI_EDITOR_TRUST_WORKSPACE=1` only for projects you trust. PATH and the
user/global toolchains remain available either way. Servers are installed
separately; the editor never installs them itself. When an executable is
missing, a copy-only prompt names the tool and shows the install command
(e.g. `go install golang.org/x/tools/gopls@latest`) with **COPY COMMAND**,
**RETRY** and **CLOSE**; nothing is executed automatically.

**Roots and monorepos.** Workspace-level markers win over package markers, so
`go.work`, `pnpm-workspace.yaml`, `Cargo.lock` and `rust-toolchain.toml` keep a
repository on one server instead of one per package. Markers are searched above
the open folder too, so opening a single file inside a monorepo still finds the
workspace root. Servers without markers fall back to the open folder (or the
file's folder) only where the spec allows loose files.

**Lifecycle.** One server runs per `(language, project root)`; up to four are
kept (least-recently-active evicted after 10 idle minutes), so switching tabs
between Rust, TypeScript and Python does not restart them. The footer `LSP`
chip has a small on/off dot: stopping kills every child and clears
diagnostics/semantic tokens/outline. The same switch is the
**Language Intelligence** settings row, persisted in the session.

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
- Hover cards keep fenced code blocks as code: the fence language (or the
  active document language when the fence is unlabeled) drives the same syntax
  provider as the editor, so keywords, types, functions, strings and comments
  stay colored. A leading plain-text declaration such as `func Node._ready()`
  or `fn main()` is highlighted the same way. Inline code (Markdown backticks,
  or raw BBCode `[code]…[/code]` from an older server) gets a code background
  and is tokenized with the active language, which covers Godot's `[code]`
  comment snippets as well as `[method …]`/`[member …]` references converted
  by Godot's LSP. Raw `[codeblock lang=…]`, `[gdscript]` and `[codeblocks]`
  wrappers are also accepted as a fallback.
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

All servers are declarative [`LanguageServerSpec`](../src/services/mod.rs) values;
only Godot uses TCP, the rest use stdio. Data-only language packs under
`<config>/languages/*.json` can contribute one too (including extension →
language-id maps and an install hint); the generic client, owned spec shape and
capability ids are ready for a future external extension host. Native plugins
are still compile-time (no dylib ABI).

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

## Header panel groups

The right side of the header is a compact row of icon buttons, grouped by the
edge each panel is docked to: **left** (Explorer, Git), **right** (Outline) and
**bottom** (Terminal). Hairlines separate the groups and the active panel gets
an accent underline, so the current dock layout is readable without opening a
menu. Clicking a button toggles that panel; clicking Explorer or Git while the
left dock shows the other tab switches tabs instead of closing the dock. The
active file label sits to their left and truncates from the left when space runs
out. The generic toggles remain in the View menu, and every panel is
bindable through `keymaps.json` (`view.explorer`, `view.git`, `view.outline`,
`view.terminal`).

Icons are Lucide SVGs vendored under `assets/icons/lucide` (ISC). They are
compiled in, parsed once by the engine's `igui_svg` renderer and stroked by
`src/ui/icon.rs` (including Lucide's round caps/joins), so they scale with DPI,
follow the active palette and need no texture atlas or runtime asset files. The Explorer/Git tabs, the Outline header
and the Terminal header use the same icon set.

## Outline (`⌘⌥O`, View → Outline)

The right sidebar shows LSP document symbols (`textDocument/documentSymbol`) for
the active document. Rust files inside a Cargo project get functions, methods,
structs/enums, fields, variables and nested items from `rust-analyzer`; GDScript
files get functions, classes, signals, constants and variables from Godot's
server. The list updates after edits. Rows use the Explorer's tree rhythm
(28 px rows, 14 px labels, shared marker/hover/selection painting), and the
nearest preceding symbol to the caret is highlighted. Clicking a row moves the
caret to the symbol and reveals it in the editor. The left edge is a shared drag
handle: drag it to resize (160–420 px), double-click resets to 224 px. The panel
is persisted in the session and can be hidden from View → Outline or `⌘⌥O`.
Symbols are transient LSP state, not saved to disk.

## Terminal (`⌃\``, View → Terminal)

Native PTY (`portable-pty`) + `vt100` screen, no WebView or JS. The panel sits
below the editor with a draggable top handle (120–600 px, default 220 px,
double-click resets). It starts a **login** `$SHELL` (`-l`), so
`/etc/zprofile` and `~/.zprofile` run just as they do in Terminal.app, and
supports ANSI/256-color cells, cursor, resize, plain text, arrows, Ctrl letters,
Alt prefixes, application cursor mode (vim/less) and IME candidate placement at
the terminal cursor. The wheel scrolls the vt100 scrollback (up to 4,000 rows);
typing or clicking the live screen returns to the prompt. Drag across cells to
select text, `⌘C` / `Ctrl+Shift+C` copies the selection, `⌘V` / `Ctrl+Shift+V`
pastes and `⌘A` / `Ctrl+Shift+A` selects the visible screen. Plain `Ctrl+C` and
`Ctrl+A` still go to the shell. Typing, wheel scrolling or switching tabs clears
the selection.
The terminal requests frames while a PTY is alive, so full-screen programs repaint
without extra input.

A macOS app launched from Finder inherits no `LANG`/`LC_*`. The PTY child would
then run in the `C` locale, where zsh prints multibyte prompt text as
`\M-^V`-style meta escapes (for example a Chinese folder name). The terminal
spawns the shell with a UTF-8 fallback (`en_US.UTF-8` on macOS, `C.UTF-8`
elsewhere) when the effective locale is missing or not UTF-8, and removes an
invalid `LC_ALL` override; a valid user locale is left untouched. This also keeps
OSC title sequences out of the cell grid.

The packaged app also augments the process `PATH` before starting services:
inherited entries stay first, then Homebrew, MacPorts, standard system
directories and `~/.cargo/bin`, `~/.local/bin`, `~/bin` are appended when they
exist. This makes `cargo`, `rust-analyzer` and Homebrew tools discoverable from
the terminal, LSP and Git even when the app was launched from Finder.

`src/fonts.rs` is the project's font policy: the editor and terminal pick a
**fixed-pitch** family at runtime (`Menlo` / `SF Mono` / `.SF NS Mono` / `Monaco` /
`JetBrains Mono` / … — each candidate is accepted only if its glyph advances
agree, and the first hit wins), while chrome and overlays keep the UI family
(PingFang SC on macOS, the system default elsewhere). Code text draws narrow runs
in the fixed-pitch face and wide (CJK) glyphs in the UI family at the **same
em**; the editor measures each run with its own family so the caret, selection
and syntax overlays stay aligned. Every terminal cell is
painted on `body.left + col * cell_w`; narrow cells use the fixed-pitch family
and wide (CJK) glyphs use the UI family centered in their two columns, so
`ls -l` columns, colored prompt blocks and the caret stay aligned without mixing
regional fallback faces. This relies on the engine's
per-draw font family (`DrawCommand::DrawText::family`,
`PaintContext::draw_text_in`, `TextMeasurer::advance_in`). The terminal grid size
is always reconciled against the parser's screen, so closing and reopening the
panel re-sizes the PTY instead of drawing a stale 80×24 screen. `Ctrl+\`` closes it
and kills the child. Focus is visible through the caret: the active editor caret is accent and
2 px, the inactive caret dims, and the terminal header text/cursor accent only
while it owns keyboard focus. Editor and terminal tops have no accent rule.
Open state and height persist in the session (v10); the shell process and screen
contents do not. Multiple sessions are a follow-up.

## macOS integration

- **Native menu bar**: App (About/Settings/Services/Hide/Quit), File, Edit,
  View, Window (Minimize/Zoom) and Help are real `NSMenu`s. Their key
  equivalents dispatch through the native request queue into the same
  `Action` commands as the keyboard. The in-app header menu is hidden on macOS.
- **Custom window chrome**: the macOS window is borderless/full-size-content
  with the native title bar and traffic lights hidden. The header paints
  simulated red/yellow/green controls; red uses the existing dirty-quit guard,
  yellow minimizes and green toggles maximize/restore. Pressing anywhere else
  on the header starts a custom window drag; double-clicking it toggles
  maximize/restore. Quit requests are deferred one run-loop turn, because
  AppKit's terminate notification re-enters winit's event handler.
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

## Syntax highlighting

Built-in providers: **Rust, C, C++, JavaScript, TypeScript, Go, GDScript,
Python, JSON, TOML, Markdown**. Highlighting is line based with carried state (block comments,
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
- skips files open in a tab, changed since the search (length/mtime), binaries,
  files over 1 MB and files with more than 10,000 matches (reported, not partially
  rewritten);
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
| Language Intelligence | on / off (start/stop all language servers) |
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

## Find and replace (`⌘F` / `⌥⌘F`)

The find bar matches the active buffer with case/whole-word toggles, tints every
match and steps with `⌘G` / `⇧⌘G`. The match list is capped at 10,000 and the
counter shows a trailing `+` when the cap is reached. **Replace** changes the
current match; **Replace All** applies every non-overlapping match as one
undoable batch (one undo step, one line-index rebuild).

## Soft wrap (`⌥Z`, View menu)

Off by default and persisted in the session. Long logical lines are split into
display rows inside the editor; only the editor changes — preview and split
layout are untouched. Rows prefer whitespace boundaries, keep trailing
separators on the previous row and preserve leading indentation; a token wider
than the viewport falls back to character boundaries. The status-bar `Ln/Col`
stays logical, `↑`/`↓` and clicks use visual rows, and the gutter numbers only
appear on a line's first row. Horizontal scrolling is suppressed while wrapped.
Soft wrap is disabled above 2 MB / 20k lines (the toggle reports why). The last
horizontal scroll offset is restored when wrap is turned off.

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
  after 2 s without edits. Saves/autosaves use temp + fsync + rename (Unix) or
  remove + rename (Windows), preserve the original permissions and restore the
  file's LF/CRLF style.
- **Crash recovery**: dirty buffers (including untitled ones) are snapshotted
  under `<config>/recovery/` after the same idle delay. On the next launch a
  modal offers **Restore All** or **Discard All**; restored buffers keep their
  original name/path and are marked dirty so the first save writes them back.
- **Window state**: size, position and the maximized flag are persisted and
  restored (fullscreen is not persisted).
- Session format is version 12 (`window.max` stores maximize state; v10 added
  the current-tab/window geometry set, v9 `outline_width`, v8 `outline`, v7
  `keymap`, v6 `terminal_open`, `terminal_height`, v5 `sidebar_tab`, v4
  `soft_wrap`); older files still load
  (v1 infers the folder from the first tab, and sessions without `active` fall
  back to the last opened tab).

## Theme: "geek", compact, no corners

Not `igui_theme::DefaultTheme`. The app owns its palette and metrics
(`src/theme.rs`):

- **No rounded rectangle anywhere** — every surface is a square fill with 1 px
  rules; scrollbar thumbs and carets are straight bars.
- **Monospace code, CJK-safe chrome** — the editor and terminal draw narrow
  runs in the first installed fixed-pitch face (`Menlo` on macOS), while
  chrome/overlays and wide CJK runs use the UI family: **PingFang SC** on macOS
  so Chinese renders with real glyphs instead of the engine's box-shaped symbol
  fallback, the system default elsewhere. Other hosts prefer JetBrains Mono /
  Fira Code / DejaVu Sans Mono.
- **Compact rhythm** — 14 px UI text / 12 px secondary text, 32 px menu bar,
  28 px rows, 28 px tab strip, 24 px status bar, 10 px scrollbars; the editor
  font comes from the font setting (11–18 px, default 13).
- **Sizes are em pixels** — `FontConfig::em_size` makes every family treat the
  requested size as its em (CSS semantics), so `14` is a 14 px em for PingFang,
  Menlo and the terminal's mono face alike. That matches VS Code's
  `"editor.fontSize": 14`; without the flag ab_glyph measured ascent+descent and
  the same number produced a 10 px em on PingFang and a 12 px em on Menlo.
- **Palette** — dark: `#0B0E13` canvas, `#0E1218` panels, `#39E0AE` phosphor
  accent, `#16362F` selection; light: `#F7F9FC` canvas, `#0A8F6F` teal accent.
- Dark and light are separate hand-tuned palettes, not an inversion; switching
  is a value swap (no rebuild, no second code path).
- Syntax colors (dark): keyword `#C792EA`, type `#7DD3FC`, function `#82E2A6`,
  string `#E6C889`, number `#F78C6C`, comment `#55637A`, constant `#FF9E64`,
  macro `#39E0AE`, punctuation `#8494A8`; the light palette has equivalents.
