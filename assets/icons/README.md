# Vendored icons

Icons are compiled into the binary with `include_str!` and parsed by the engine's
`igui_svg` renderer; nothing is fetched at build or run time.

## `lucide/` — line icons (shell chrome)

A small subset of [Lucide](https://lucide.dev) static SVGs (`lucide-static`
1.68.0), cached by `src/ui/icon.rs` and stroked by `igui_svg` in the active
palette color.

Add an icon by copying its SVG here and adding one `Icon` variant plus one
`Icon::svg` arm. Keep the `@license` comment in the SVG.

## License

Icons are ISC. The notice is reproduced below.

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
