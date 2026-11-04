//! Filled file-type icons (Material icon theme artwork), painted as spans.
//!
//! [`crate::ui::icon`] paints Lucide line art: `igui_svg` strokes paths. The
//! file-type icons vendored from
//! [material-icon-theme](https://github.com/material-extensions/vscode-material-icon-theme)
//! (MIT, see `assets/icons/README.md`) are filled silhouettes instead, and the
//! engine's SVG renderer does not fill yet.
//!
//! Rather than write a second path parser, this module still uses `igui_svg`,
//! only as a flattener: before parsing, `fill="` is rewritten to `stroke="` so
//! the engine's parser keeps the artwork's explicit colors and hands back
//! flattened subpaths. Those polygons are filled here with a scanline pass that
//! emits horizontal `FillRect` spans — exact fractional x edges plus four
//! sub-rows per logical pixel give antialiasing without a texture atlas, a
//! bitmap asset or a new dependency.
//!
//! Span lists are pure geometry, so they are cached per icon and icon size; a
//! repaint only copies cached rectangles.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use igui_core::{Color, Rect, Vec2};
use igui_render::PaintContext;
use igui_svg::{ColorSource, SvgDocument};

use crate::model::fileicon::FileKind;

/// The square icon box in an Explorer row (it fills the marker slot).
pub(crate) const SIZE: f32 = 12.0;
/// Vertical sub-rows per logical pixel. Four keeps near-horizontal edges smooth
/// at icon sizes without doubling the span count four times over.
const SUBROWS: f32 = 4.0;
/// Spans narrower than this are scanline noise (touching vertices).
const MIN_SPAN: f32 = 0.02;

/// One cached fill span in icon-local coordinates.
#[derive(Clone, Copy)]
struct Span {
    rect: Rect,
    color: Color,
}

/// Paints `kind` centred in `rect` (the smaller side wins so it stays square).
pub(crate) fn paint(ctx: &mut PaintContext, kind: FileKind, rect: Rect) {
    let side = rect.size.width.min(rect.size.height);
    if side <= 0.0 {
        return;
    }
    let origin = Vec2::new(rect.center().x - side * 0.5, rect.center().y - side * 0.5);
    for span in spans(kind, side).iter() {
        ctx.fill_rect(span.rect.translate(origin), span.color);
    }
}

/// Source SVG for `kind` (embedded; never read from disk at runtime).
fn svg(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Document => include_str!("../../assets/icons/material/document.svg"),
        FileKind::Rust => include_str!("../../assets/icons/material/rust.svg"),
        FileKind::Python => include_str!("../../assets/icons/material/python.svg"),
        FileKind::JavaScript => include_str!("../../assets/icons/material/javascript.svg"),
        FileKind::TypeScript => include_str!("../../assets/icons/material/typescript.svg"),
        FileKind::React => include_str!("../../assets/icons/material/react.svg"),
        FileKind::Json => include_str!("../../assets/icons/material/json.svg"),
        FileKind::Markdown => include_str!("../../assets/icons/material/markdown.svg"),
        FileKind::Html => include_str!("../../assets/icons/material/html.svg"),
        FileKind::Css => include_str!("../../assets/icons/material/css.svg"),
        FileKind::Sass => include_str!("../../assets/icons/material/sass.svg"),
        FileKind::Godot => include_str!("../../assets/icons/material/godot.svg"),
        FileKind::Git => include_str!("../../assets/icons/material/git.svg"),
        FileKind::Console => include_str!("../../assets/icons/material/console.svg"),
        FileKind::Yaml => include_str!("../../assets/icons/material/yaml.svg"),
        FileKind::Toml => include_str!("../../assets/icons/material/toml.svg"),
        FileKind::Xml => include_str!("../../assets/icons/material/xml.svg"),
        FileKind::Image => include_str!("../../assets/icons/material/image.svg"),
        FileKind::Svg => include_str!("../../assets/icons/material/svg.svg"),
        FileKind::Pdf => include_str!("../../assets/icons/material/pdf.svg"),
        FileKind::Archive => include_str!("../../assets/icons/material/zip.svg"),
        FileKind::Audio => include_str!("../../assets/icons/material/audio.svg"),
        FileKind::Video => include_str!("../../assets/icons/material/video.svg"),
        FileKind::Database => include_str!("../../assets/icons/material/database.svg"),
        FileKind::Settings => include_str!("../../assets/icons/material/settings.svg"),
        FileKind::Lock => include_str!("../../assets/icons/material/lock.svg"),
        FileKind::Diff => include_str!("../../assets/icons/material/diff.svg"),
        FileKind::Readme => include_str!("../../assets/icons/material/readme.svg"),
        FileKind::License => include_str!("../../assets/icons/material/license.svg"),
        FileKind::Makefile => include_str!("../../assets/icons/material/makefile.svg"),
        FileKind::Cmake => include_str!("../../assets/icons/material/cmake.svg"),
        FileKind::Docker => include_str!("../../assets/icons/material/docker.svg"),
        FileKind::Vue => include_str!("../../assets/icons/material/vue.svg"),
        FileKind::Go => include_str!("../../assets/icons/material/go.svg"),
        FileKind::C => include_str!("../../assets/icons/material/c.svg"),
        FileKind::Cpp => include_str!("../../assets/icons/material/cpp.svg"),
        FileKind::Java => include_str!("../../assets/icons/material/java.svg"),
        FileKind::Swift => include_str!("../../assets/icons/material/swift.svg"),
        FileKind::Lua => include_str!("../../assets/icons/material/lua.svg"),
        FileKind::Ruby => include_str!("../../assets/icons/material/ruby.svg"),
        FileKind::Php => include_str!("../../assets/icons/material/php.svg"),
        FileKind::Csharp => include_str!("../../assets/icons/material/csharp.svg"),
        FileKind::Font => include_str!("../../assets/icons/material/font.svg"),
    }
}

/// The parsed document, cached for the life of the process.
fn document(kind: FileKind) -> Option<&'static SvgDocument> {
    static CACHE: OnceLock<Vec<Option<SvgDocument>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        FileKind::ALL
            .iter()
            .map(|kind| {
                // Material artwork is fill-only; `igui_svg` reads the paint from
                // `stroke`, so swap the attribute and use it as a flattener.
                let source = svg(*kind).replace("fill=\"", "stroke=\"");
                SvgDocument::parse(&source).ok()
            })
            .collect()
    });
    cache[kind.index()].as_ref()
}

/// Spans cached per icon and quantized box size.
type SpanCache = HashMap<(FileKind, u16), Rc<Vec<Span>>>;

/// Cached spans for one icon at one box size (sizes are quantized to ¼ px).
fn spans(kind: FileKind, side: f32) -> Rc<Vec<Span>> {
    thread_local! {
        static CACHE: RefCell<SpanCache> = RefCell::new(SpanCache::new());
    }
    let key = (kind, (side * SUBROWS).round().max(1.0) as u16);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(spans) = cache.get(&key) {
            return spans.clone();
        }
        // Explorer rows are one size; the cap only guards exotic callers.
        if cache.len() > 4 * FileKind::ALL.len() {
            cache.clear();
        }
        let quantized = key.1 as f32 / SUBROWS;
        let spans = Rc::new(
            document(kind)
                .map(|document| rasterize(document, quantized))
                .unwrap_or_default(),
        );
        cache.insert(key, spans.clone());
        spans
    })
}

/// Rasterizes every filled shape of `document` into horizontal spans.
fn rasterize(document: &SvgDocument, side: f32) -> Vec<Span> {
    let view = document.view_box;
    let scale = if view.size.width > 0.0 && view.size.height > 0.0 {
        (side / view.size.width).min(side / view.size.height)
    } else {
        1.0
    };
    let origin = Vec2::new(
        (side - view.size.width * scale) * 0.5 - view.min.x * scale,
        (side - view.size.height * scale) * 0.5 - view.min.y * scale,
    );

    let mut spans = Vec::new();
    for shape in &document.shapes {
        let color = match shape.stroke.source {
            // Material artwork is explicitly coloured; `none` is the invisible
            // background path some icons carry, `currentColor` never appears.
            ColorSource::None | ColorSource::CurrentColor => continue,
            ColorSource::Color(color) => color,
        };
        let polygons: Vec<Vec<Vec2>> = shape
            .subpaths
            .iter()
            .map(|subpath| {
                subpath
                    .points
                    .iter()
                    .map(|point| Vec2::new(origin.x + point.x * scale, origin.y + point.y * scale))
                    .collect()
            })
            .collect();
        fill(&polygons, color, side, &mut spans);
    }
    spans
}

/// One polygon edge, oriented top-to-bottom with its winding direction.
struct Edge {
    top: f32,
    bottom: f32,
    x: f32,
    slope: f32,
    direction: i32,
}

/// Scanline-fills `polygons` (non-zero winding, the SVG default) into `spans`.
fn fill(polygons: &[Vec<Vec2>], color: Color, side: f32, spans: &mut Vec<Span>) {
    let mut edges = Vec::new();
    for points in polygons {
        if points.len() < 3 {
            continue;
        }
        for index in 0..points.len() {
            let from = points[index];
            let to = points[(index + 1) % points.len()];
            if (from.y - to.y).abs() < f32::EPSILON {
                continue;
            }
            let (top, bottom, direction) = if from.y < to.y {
                (from, to, 1)
            } else {
                (to, from, -1)
            };
            edges.push(Edge {
                top: top.y,
                bottom: bottom.y,
                x: top.x,
                slope: (bottom.x - top.x) / (bottom.y - top.y),
                direction,
            });
        }
    }
    if edges.is_empty() {
        return;
    }

    let rows = (side * SUBROWS).round().max(1.0) as usize;
    let row_h = side / rows as f32;
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for row in 0..rows {
        let y = row as f32 * row_h;
        let sample = y + row_h * 0.5;
        crossings.clear();
        for edge in &edges {
            if sample >= edge.top && sample < edge.bottom {
                crossings.push((edge.x + (sample - edge.top) * edge.slope, edge.direction));
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(Ordering::Equal)
                .then(left.1.cmp(&right.1))
        });

        let mut winding = 0;
        let mut start = 0.0;
        for &(x, direction) in crossings.iter() {
            let before = winding;
            winding += direction;
            match (before == 0, winding == 0) {
                (true, false) => start = x,
                (false, true) => {
                    let (left, right) = (start.max(0.0), x.min(side));
                    if right - left > MIN_SPAN {
                        push(spans, color, left, right, y, row_h);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Appends one span, merging it into the previous one when they stack exactly
/// (vertical edges then cost a single rectangle instead of one per sub-row).
fn push(spans: &mut Vec<Span>, color: Color, left: f32, right: f32, y: f32, height: f32) {
    if let Some(last) = spans.last_mut() {
        if last.color == color
            && (last.rect.bottom() - y).abs() < 1e-3
            && (last.rect.left() - left).abs() < 1e-3
            && (last.rect.right() - right).abs() < 1e-3
        {
            last.rect = Rect::from_min_max(last.rect.min(), Vec2::new(right, y + height));
            return;
        }
    }
    spans.push(Span {
        rect: Rect::from_min_max(Vec2::new(left, y), Vec2::new(right, y + height)),
        color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use igui_core::Size;
    use igui_render::DrawCommand;

    fn square(size: f32) -> Rect {
        Rect::from_min_size(Vec2::ZERO, Size::new(size, size))
    }

    #[test]
    fn cache_order_matches_enum_indices() {
        for (index, kind) in FileKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), index, "{kind:?}");
            assert!(document(kind).is_some(), "{kind:?} did not parse");
        }
    }

    #[test]
    fn every_icon_fills_geometry_inside_its_box() {
        for kind in FileKind::ALL {
            let spans = spans(kind, SIZE);
            assert!(!spans.is_empty(), "{kind:?} drew no geometry");
            assert!(
                spans.len() <= 240,
                "{kind:?} needs {} spans, too many for one row",
                spans.len()
            );
            for span in spans.iter() {
                assert!(
                    span.rect.left() >= -0.01
                        && span.rect.right() <= SIZE + 0.01
                        && span.rect.top() >= -0.01
                        && span.rect.bottom() <= SIZE + 0.01,
                    "{kind:?} span {:?} escaped the icon box",
                    span.rect
                );
            }
        }
    }

    #[test]
    fn paint_emits_fill_rects_with_the_artwork_color() {
        let rust = spans(FileKind::Rust, SIZE);
        let expected = rust[0].color.to_rgba8();
        let mut ctx = PaintContext::new();
        paint(&mut ctx, FileKind::Rust, square(SIZE));
        let list = ctx.into_draw_list();
        let rects = list
            .commands()
            .iter()
            .filter(|command| matches!(command, DrawCommand::FillRect { .. }))
            .count();
        assert_eq!(rects, rust.len());
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCommand::FillRect { paint, .. } if paint.color.to_rgba8() == expected
        )));
    }

    #[test]
    fn spans_are_cached_per_size() {
        let first = spans(FileKind::Markdown, SIZE);
        let second = spans(FileKind::Markdown, SIZE);
        assert!(Rc::ptr_eq(&first, &second));
        assert!(Rc::ptr_eq(&first, &spans(FileKind::Markdown, SIZE + 0.01)));
    }
}
