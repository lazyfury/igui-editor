# Color emoji

The editor, Markdown preview and terminal paint system color emoji. The engine
does not: `igui_font` rasterizes outlines with `ab_glyph`, while the platform
emoji faces are bitmap fonts (`sbix` on macOS, `CBDT` on Linux). The app bridges
that gap without modifying `../igui`.

## Where it applies

| Surface | Behavior |
|---|---|
| Editor text | One image per emoji base code point, at the advance text layout reserved. |
| Markdown preview | Same, including headings, lists, tables and inline code. |
| Terminal | An emoji cell paints a sprite fitted to its cell(s); the grid, caret and selection stay cell-based. |
| Explorer/outline rows, tab labels | Filenames with emoji paint the sprite; truncation still measures the reserved advance. |
| Menus, status bar, prompts | Text pass only; a color face has no outline, so emoji in chrome text has no ink. |

## Font discovery

The first readable candidate wins:

1. `IGUI_EMOJI_FONT` (a font-file path; tests, portable installs and unusual
   hosts).
2. macOS: `/System/Library/Fonts/Apple Color Emoji.ttc` (or the
   `Supplemental` copy).
3. Linux: the common `NotoColorEmoji.ttf` locations.
4. Windows: `C:\Windows\Fonts\seguiemj.ttf`.

Discovery happens once per process (`model::emoji::system_font`) and the file is
memory-mapped, never read into the heap: Apple Color Emoji is ~183 MB, and only
the glyph pages actually painted are touched.

## What is painted

`model::emoji::classify` follows the Unicode emoji data closely enough for text
rendering:

- **default emoji presentation** (`😀`, `🚀`, `🇨`) paints without a selector;
- **text-default symbols** (`❤`, `☀`, `⚠`) paint only when followed by the emoji
  variation selector `U+FE0F`;
- **components** (`U+200D`, `U+FE0F`, keycap `U+20E3`, skin tones
  `U+1F3FB..=U+1F3FF`, hair components, tag characters) never paint their own
  image.

One image per **base code point** is intentional. The text layout measures per
code point (caret, wrap and hit-testing all agree on that), so shaping a joined
sequence into a single glyph would desync the paint from the caret. Consequences:

- `👍🏽` paints one thumb (the modifier advances but paints nothing);
- `👨‍👩‍👧` paints three faces side by side;
- regional-indicator pairs render as two letter boxes rather than a flag.

These are documented limits, not layout errors.

## Metrics and caching

- The strike is chosen for `font_size × device_scale` pixels per em, so a
  Retina window gets a crisp bitmap and moving to another monitor re-rasterizes.
- The sprite is drawn in the em box measured for that code point, baseline
  anchored (`top = baseline − 0.82 × side`), and horizontally centered in its
  advance. Using the measured advance keeps every caret/selection/wrap invariant
  intact.
- Decoded glyphs cache as RGBA8 textures keyed by `(char, ppem)`; the cache is
  capped at 1024 entries. The host installs the upload closure in
  `EditorApp::init`; headless runs record `DrawImage` without a texture.
- A face without a bitmap for a code point (JPEG or outline-only glyph) caches
  `None`, so it is not re-probed every frame.

## Failure behavior

No emoji font, an unreadable file or an undecodable glyph simply paints nothing
for that code point; the advance stays correct and the rest of the line is
unchanged. `state.emoji.available()` drives the self-check skip message and can
be used for status text.

## Verification

- `model::emoji` unit tests: classification matrix, modifier/joiner handling,
  PNG and premultiplied-BGRA decoding, and (on macOS) a real `😀` bitmap from
  Apple Color Emoji.
- `ui::emoji` tests: plain text stays one run, emoji split the run without
  changing total advance, components stay in the text pass.
- `--selfcheck`: a Markdown document in Split view paints emoji images in both
  panes and no emoji code point leaks into a text run; the PTY check prints an
  emoji and asserts a terminal sprite. Both skip when no face is installed.

Only PNG and `BitmapPremulBgra32`/`BitmapGray8` payloads are decoded; JPEG/TIFF
`sbix` data and `COLR` vector fonts are not (the latter would need a paint-graph
renderer, which the engine does not have).
