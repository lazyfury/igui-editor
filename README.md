# igui-editor

A compact, square-cornered **text / Markdown editor** built on the
[`igui`](../igui) engine (Rust). Native window: `winit` + `wgpu`, UI painted
through `igui_ui` / `igui_render` only.

Tabs · undo/redo · find & replace · Markdown split preview (tables included) ·
native file dialogs · recent files · session restore.

This checkout is **outside** the igui repository: it consumes the engine through
path dependencies and never modifies it.

```
Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
```

## Run

```bash
cd igui-editor
cargo run --release                          # restore the last session (or an untitled buffer)
cargo run --release -- src/app.rs            # open a file (workspace = its directory)
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
| wheel / trackpad | vertical scroll, `⇧`+wheel horizontal |

Find bar: query and replacement fields, `Aa` (case) and `W` (whole word)
toggles, `Replace` / `Replace All`, match counter, prev/next/close. All matches
are tinted in the editor, the current one strongly.

Markdown preview (`.md` opens in Split; `⇧⌘M` cycles): headings, paragraphs,
bold / italic / strikethrough, inline code, fenced code blocks with a language
label, nested lists, quotes with an accent rule, rules, images as `▢ alt`,
styled links (no navigation) and **tables with per-column alignment**. The
preview scrolls with the wheel/trackpad or its scrollbar.

Mouse: click a folder to expand it, a file to open it in a tab, `×` to close a
tab (unsaved tabs open a Save / Discard / Cancel modal), `+` for a new buffer,
drag the editor scrollbars, drag in the text to select, wheel-scroll the
preview.

## Theme: "geek", compact, no corners

Not `igui_theme::DefaultTheme`. The app owns its palette and metrics
(`src/theme.rs`):

- **No rounded rectangle anywhere** — every surface is a square fill with 1 px
  rules; scrollbar thumbs and carets are straight bars.
- **One monospace grid** — the host seeds `FontServer` with the first installed
  monospace face (SF Mono / Menlo / JetBrains Mono / Fira Code / DejaVu Sans
  Mono); the whole UI uses it.
- **Compact rhythm** — 26 px menu bar, 22 px rows, 22 px status bar, 19 px
  editor lines, 8 px scrollbars, 4-space indents.
- **Palette** — dark: `#0B0E13` canvas, `#0E1218` panels, `#39E0AE` phosphor
  accent, `#16362F` selection; light: `#F7F9FC` canvas, `#0A8F6F` teal accent.
- Dark and light are separate hand-tuned palettes, not an inversion; switching
  is a value swap (no rebuild, no second code path).

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
    and a fresh app restores the theme, the open tab and the recent list from
    the session file.

No OS capture of any kind is used.

## Layout of the code

```
src/
  model/doc.rs        text buffer, caret/selection, movement, delta undo stack
  model/find.rs       plain-text matcher (case / whole-word)
  model/fs_tree.rs    lazy sidebar tree (dirs first, hidden entries skipped)
  model/markdown.rs   Markdown event stream -> flat block model
  model/session.rs    key=value session file (atomic write, escaping)
  model/workspace.rs  tabs + file IO
  dialogs.rs          `Dialogs` trait + scripted test double
  input.rs            keyboard dispatch: modal -> find -> shortcuts -> tree
  state.rs            shared state, actions, hit rects recorded during paint
  theme.rs            geek palette + compact metrics
  kit.rs              control-node construction and text/line helpers
  ui/menu.rs          menu bar + app-level drop-down overlay
  ui/sidebar.rs       file tree
  ui/tabs.rs          tab strip
  ui/editor.rs        gutter, text, selection, caret, scrollbars, IME preedit
  ui/find_bar.rs      find / replace strip
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

Deferred (M3): syntax highlighting, indent guides, bracket matching, settings
panel (font size / tab width / wrap), large-file measurement cache, sidebar
resize, tab overflow scrolling.

Notes:

- Only macOS has been exercised manually; the Linux (XDG portal) and Windows
  dialog paths are wired but untested.
- The macOS sheet path is asynchronous by design (no nested modal loop, no
  re-entrant event dispatch); the app polls the dialog future once per frame
  while it is open.
- The session restores **saved** tabs; unsaved edits of a tab are not recovered
  after a crash (a recovery buffer is M3+).
- Closing the last tab immediately opens a fresh untitled buffer (the empty
  workspace hint is kept for the no-document case).

Known engine limitation: `igui_core::Key` has no `PageUp` / `PageDown`, so page
scrolling is not bound yet (`Document::page` is implemented and tested, waiting
for a key variant). `⌘↑` / `⌘↓` cover document navigation meanwhile.
