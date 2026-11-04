# Vendored icons

Both icon sets are compiled into the binary with `include_str!` and parsed by the
engine's `igui_svg` renderer; nothing is fetched at build or run time.

## `lucide/` — line icons (shell chrome)

A small subset of [Lucide](https://lucide.dev) static SVGs (`lucide-static`
1.68.0), cached by `src/ui/icon.rs` and stroked by `igui_svg` in the active
palette color.

Add an icon by copying its SVG here and adding one `Icon` variant plus one
`Icon::svg` arm. Keep the `@license` comment in the SVG.

## `material/` — filled file-type icons (Explorer rows)

A curated subset of the [Material Icon
Theme](https://github.com/material-extensions/vscode-material-icon-theme)
artwork (`v5.40.0`), used for the Explorer's file-type icons. These SVGs are
fill-only, so `src/ui/fileicon.rs` uses `igui_svg` only as a path flattener
(rewriting `fill=` to `stroke=` before parsing) and rasterizes the fills into
horizontal `FillRect` spans. The kind table lives in `src/model/fileicon.rs`.

Add a type by copying its SVG here, adding one `FileKind` variant plus its
`FileKind::ALL`/`index`/`svg` entries, and one mapping entry in
`model::fileicon::kind_for`. Keep the `@license` comment in the SVG. Use artwork
with explicit `fill="#rrggbb"` colors: `currentColor` and gradients are skipped
(the span painter only draws solid fills).

## License

Window chrome icons are ISC; file-type icons are MIT. Both notices are
reproduced below.

### Lucide — ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part
of Feather (MIT). All other copyright (c) for Lucide are held by Lucide
Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.

### Material Icon Theme — MIT License

Copyright (c) 2025 Material Extensions

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
