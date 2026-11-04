# Keys and keymaps

| Key | Action |
|---|---|
| `⌘O` / `⇧⌘O` | open file / open folder (sidebar root) |
| `⌘S` / `⇧⌘S` | save / save as |
| `⌘N` | new buffer (untitled) |
| `⇧⌘N` | new file on disk (quick prompt) |
| `⌃\`` | toggle the built-in terminal |
| `⌘W` | close tab (asks Save / Discard / Cancel when dirty) |
| `⌘1` … `⌘8` / `⌘9` | select tab by position / select the last tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | next / previous tab (`Ctrl+Tab` on macOS; `Ctrl` is literal there) |
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
[design.md §Drag handle](design.md#drag-handle-specification)): one centered
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

On macOS the application menu is the **native system menu bar** (App, File,
Edit, View, Window, Help); the in-app header only carries the simulated window
controls. On other platforms the in-app File/Edit/View menus remain
mouse-operated. Menu shortcut hints in the in-app UI are resolved from the active
keymap profile, so VS Code / JetBrains / Vim show the chords they actually use.

Mouse: click a folder to expand it, a file to open it in a tab, `×` to close a
tab (unsaved tabs open a Save / Discard / Cancel modal), `+` for a new buffer,
drag a tab to reorder it, right-click it for Close / Close Others / Close to the
Right / Copy Path / Reveal in Sidebar (closing several dirty tabs asks once),
right-click a file-tree row for Open / New File / Copy Path / Reveal in Finder,
drag the editor scrollbars, drag in the text to select, wheel-scroll the
preview.
