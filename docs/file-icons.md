# File icons (VS Code Material Icon Theme)

The Explorer paints file and folder icons from the **installed** VS Code
[Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme)
extension. No artwork is vendored into this repository: the theme JSON and SVGs
are read at runtime, so updating the extension updates the editor and the repo
stays free of a generated icon table.

This supersedes the removed vendored set (Session 5 reduction item 2.3). The
surface is deliberately smaller than before: no 43-entry `FileKind` table, no
`assets/icons/material` directory, and no per-file match code — VS Code's own
theme data decides which artwork a name gets.

## Discovery

1. `IGUI_ICON_THEME` — a theme JSON file or an extension directory (tests,
   portable installs, a custom fork).
2. The newest `pkief.material-icon-theme-*` in the usual extension directories:
   `~/.vscode/extensions`, `~/.vscode-insiders/extensions`,
   `~/.vscode-oss/extensions`, `~/.cursor/extensions`,
   `~/.windsurf/extensions`.

Version titles are compared numerically (`5.40.0` beats `5.9.0`). The JSON is
looked up at `<extension>/dist/material-icons.json`; a directory override also
accepts `<dir>/material-icons.json`.

Nothing is read until the Explorer is visible with **File Icons** on and a
project is open, so a session that never opens a folder (or turns icons off)
pays nothing.

## Lookup rules (VS Code precedence)

| Row | Rule |
|---|---|
| File name | Exact lowercased match in `fileNames` first (`readme.md`, `Cargo.toml`, `.gitignore`). |
| File extension | Every suffix after a dot, longest first (`foo.d.ts` tries `d.ts`, then `ts`, then the default). Case-insensitive. |
| Unknown file | The theme's `file` default. |
| Folder | Name in `folderNames` / `folderNamesExpanded`, else `folder` / `folderExpanded`. |
| Root folder | `rootFolder` / `rootFolderExpanded` (the Explorer does not show a root row today). |
| Light theme | The theme's `light` maps are checked before the dark maps; files without an override keep their explicit artwork, exactly like VS Code. |

Definitions that only carry a font glyph (`fontCharacter`) are dropped, so a
lookup falls back to the default SVG instead of drawing nothing.

## Rendering

Material artwork is fill-based; `igui_svg` strokes paths. `ui/fileicon` uses the
engine parser as a *flattener*: `fill="` is rewritten to `stroke="` so explicit
colors survive and the parser returns flattened subpaths. Those polygons are
filled with a scanline pass into coloured `FillRect` spans:

- 4 sub-rows per logical pixel give antialiased edges without a texture atlas;
- `fill-rule="evenodd"` artwork keeps its holes (32 of the bundled icons);
- spans are cached per resolved icon path and quantized 12 px box, so frames
  only copy rectangles;
- icons are 12 px, vertically centered in the row.

The disclosure arrow is **independent of the theme and always occupies the same
slot**: an 8 px arrow column at the row indent (chevron centre unchanged), then
a 6 px gap, the 12 px icon column, another 6 px gap, then the label. Files leave
the arrow slot empty but still reserve it, so file and folder icons start on the
same x and labels line up. With **File Icons** off the icon column is not
reserved and rows return to the compact 12 px marker layout; the folder arrow
does not move.

Parsing, transforms and gradients are the engine parser's limits: SVG
components it ignores simply draw less (or nothing). The self-check asserts the
common file kinds resolve and rasterize, and the unit tests cover the scanline
fill (including the even-odd hole).

## Settings and session

**Explorer → File Icons** (default on) toggles the whole surface immediately and
persists as `file_icons=1` in the session (format v20). Toggling off restores the
chevron-only tree; the loaded theme and SVG cache stay in memory, so toggling
back on is instant.

## Files

| Piece | Responsibility |
|---|---|
| `model/icon_theme.rs` | Pure JSON parse, name/extension/folder lookup, light overrides, discovery. |
| `state/fileicons.rs` | Lazy theme load, SVG source cache (`< 256 KB`, ≤ 1024 entries), visible-row preload from the sidebar scroll. |
| `ui/fileicon.rs` | SVG flatten → scanline spans, cache, paint. |
| `ui/tree.rs` / `ui/sidebar.rs` | Marker slot: theme icon when resolved, chevron otherwise. |

## Verification

- `cargo test model::icon_theme` — precedence, compound extensions, light
  overrides, relative paths, newest-version discovery, installed-theme smoke
  test.
- `cargo test ui::fileicon` — geometry stays inside the box, colors, even-odd
  holes, cache reuse, installed artwork rasterizes.
- `--selfcheck check_file_icons` — writes a deterministic two-file project and
  theme, asserts the red/green/blue icon spans for `main.rs`, `notes.md` and the
  `src` folder, then turns **File Icons** off and asserts none remain.
