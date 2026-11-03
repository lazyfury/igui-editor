# Verification

```bash
scripts/check.sh                            # fmt + clippy + tests + self-check
scripts/check.sh --release                  # also builds the shipped binary
```

Or individually:

```bash
cargo test                                  # model: caret, undo, find, markdown, session, FS tree
cargo run -- --selfcheck                    # headless DrawList assertions
cargo run --release --features selfcheck -- --selfcheck  # same, optimized
scripts/bench.sh                            # Markdown pipeline performance
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
    and a fresh app restores the theme, the open tabs and current tab, the
    recent list and every editor setting from the session file.

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

No OS capture of any kind is used. Performance methodology and budgets are in
[benchmarks.md](benchmarks.md).
