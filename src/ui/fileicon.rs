//! Material file icons from a VS Code icon theme, painted as filled spans.
//!
//! [`crate::ui::icon`] paints Lucide line art: `igui_svg` strokes paths. The
//! Material Icon Theme artwork is filled silhouettes instead, and the engine's
//! SVG renderer does not fill yet. This module still uses `igui_svg`, only as a
//! flattener: before parsing, `fill="` is rewritten to `stroke="` so the engine
//! keeps the artwork's explicit colors and returns flattened subpaths. Those
//! polygons are filled here with a scanline pass that emits horizontal
//! `FillRect` spans — fractional x edges plus four sub-rows per logical pixel
//! give antialiasing without a texture atlas, bitmap asset or new dependency.
//!
//! The SVG text comes from the installed extension through
//! `state::fileicons`; paint never opens a file. Span lists are pure geometry,
//! cached per icon path and icon size, so a repaint only copies rectangles.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use igui_core::{Color, Rect, Vec2};
use igui_render::PaintContext;
use igui_svg::{ColorSource, SvgDocument};

/// The square icon box in an Explorer row (it fills the marker slot).
pub(crate) const SIZE: f32 = 12.0;
/// Vertical sub-rows per logical pixel. Four keeps near-horizontal edges smooth
/// at icon sizes without quadrupling the span count.
const SUBROWS: f32 = 4.0;
/// Spans narrower than this are scanline noise (touching vertices).
const MIN_SPAN: f32 = 0.02;
/// Cache caps: a real theme has ~1,300 definitions, but one session only shows
/// a small subset.
const DOC_CAP: usize = 512;
const SPAN_CAP: usize = 512;

type DocCache = HashMap<String, Option<Rc<SvgDocument>>>;
type SpanCache = HashMap<(String, u16), Rc<Vec<Span>>>;

/// One cached fill span in icon-local coordinates.
#[derive(Clone, Copy)]
struct Span {
    rect: Rect,
    color: Color,
}

/// Paints `svg` centred in `rect` (the smaller side wins so it stays square).
///
/// `key` is the resolved icon path; it separates cache entries between themes
/// and extension versions.
pub(crate) fn paint(ctx: &mut PaintContext, key: &str, svg: &str, rect: Rect) {
    let side = rect.size.width.min(rect.size.height);
    if side <= 0.0 {
        return;
    }
    let origin = Vec2::new(rect.center().x - side * 0.5, rect.center().y - side * 0.5);
    for span in spans(key, svg, side).iter() {
        ctx.fill_rect(span.rect.translate(origin), span.color);
    }
}

/// Parsed document, cached per icon path.
fn document(key: &str, svg: &str) -> Option<Rc<SvgDocument>> {
    thread_local! {
        static CACHE: RefCell<DocCache> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(document) = cache.get(key) {
            return document.clone();
        }
        if cache.len() >= DOC_CAP {
            cache.clear();
        }
        // Material artwork is fill-only; `igui_svg` reads the paint from
        // `stroke`, so swap the attribute and use it as a flattener.
        let parsed = SvgDocument::parse(&svg.replace("fill=\"", "stroke=\"")).ok();
        let parsed = parsed.map(Rc::new);
        cache.insert(key.to_string(), parsed.clone());
        parsed
    })
}

/// Cached spans for one icon at one box size (sizes are quantized to ¼ px).
fn spans(key: &str, svg: &str, side: f32) -> Rc<Vec<Span>> {
    thread_local! {
        static CACHE: RefCell<SpanCache> = RefCell::new(HashMap::new());
    }
    let quantized = (side * SUBROWS).round().max(1.0) as u16;
    let cache_key = (key.to_string(), quantized);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(spans) = cache.get(&cache_key) {
            return spans.clone();
        }
        if cache.len() >= SPAN_CAP {
            cache.clear();
        }
        let side = quantized as f32 / SUBROWS;
        let even_odd = svg.contains("fill-rule=\"evenodd\"");
        let spans = Rc::new(
            document(key, svg)
                .map(|document| rasterize(&document, side, even_odd))
                .unwrap_or_default(),
        );
        cache.insert(cache_key, spans.clone());
        spans
    })
}

/// Rasterizes every filled shape of `document` into horizontal spans.
fn rasterize(document: &SvgDocument, side: f32, even_odd: bool) -> Vec<Span> {
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
        fill(&polygons, color, side, even_odd, &mut spans);
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

/// Scanline-fills `polygons` (non-zero winding by default, even-odd when the
/// artwork asks for it) into `spans`.
fn fill(polygons: &[Vec<Vec2>], color: Color, side: f32, even_odd: bool, spans: &mut Vec<Span>) {
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

        if even_odd {
            for pair in crossings.chunks_exact(2) {
                let (left, right) = (pair[0].0, pair[1].0);
                let (left, right) = (left.max(0.0), right.min(side));
                if right - left > MIN_SPAN {
                    push(spans, color, left, right, y, row_h);
                }
            }
            continue;
        }

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

    /// A red square (the exact artwork does not matter; the geometry does).
    const RED_SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#ff0000" d="M0 0h16v16H0z"/></svg>"##;
    /// A red square with an even-odd hole in the middle.
    const RED_RING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill-rule="evenodd" fill="#ff0000" d="M0 0h16v16H0zM4 4h8v8H4z"/></svg>"##;

    fn square(size: f32) -> Rect {
        Rect::from_min_size(Vec2::ZERO, Size::new(size, size))
    }

    #[test]
    fn spans_stay_inside_the_icon_box() {
        let spans = spans("red", RED_SQUARE, SIZE);
        assert!(!spans.is_empty());
        assert!(spans.len() < 100, "{} spans for one square", spans.len());
        for span in spans.iter() {
            assert!(
                span.rect.left() >= -0.01
                    && span.rect.right() <= SIZE + 0.01
                    && span.rect.top() >= -0.01
                    && span.rect.bottom() <= SIZE + 0.01,
                "span {:?} escaped the icon box",
                span.rect
            );
        }
    }

    #[test]
    fn paint_emits_fill_rects_with_the_artwork_color() {
        let mut ctx = PaintContext::new();
        paint(&mut ctx, "red", RED_SQUARE, square(SIZE));
        let list = ctx.into_draw_list();
        let rects = list
            .commands()
            .iter()
            .filter(|command| matches!(command, DrawCommand::FillRect { .. }))
            .count();
        assert_eq!(rects, spans("red", RED_SQUARE, SIZE).len());
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCommand::FillRect { paint, .. }
                if paint.color.to_rgba8() == [255, 0, 0, 255]
        )));
    }

    #[test]
    fn even_odd_artwork_keeps_its_hole() {
        let spans = spans("ring", RED_RING, SIZE);
        assert!(!spans.is_empty());
        let covers = |x: f32, y: f32| {
            spans.iter().any(|span| {
                span.rect.left() <= x
                    && span.rect.right() >= x
                    && span.rect.top() <= y
                    && span.rect.bottom() >= y
            })
        };
        assert!(covers(1.0, 6.0), "the ring's edge is filled");
        assert!(!covers(6.0, 6.0), "the ring's hole is empty");
    }

    #[test]
    fn installed_theme_artwork_rasterizes() {
        let Some(json) = crate::model::icon_theme::discover() else {
            return; // No VS Code extension installed on this host.
        };
        let theme = crate::model::icon_theme::IconTheme::load(&json).expect("theme loads");
        for name in ["main.rs", "index.ts", "README.md", "project.godot"] {
            let icon = theme
                .file_icon(name, false)
                .unwrap_or_else(|| panic!("{name} has no icon"));
            let svg = std::fs::read_to_string(icon).expect("icon file is readable");
            let key = icon.to_string_lossy();
            let spans = spans(&key, &svg, SIZE);
            assert!(!spans.is_empty(), "{name} ({key}) rasterized to nothing");
        }
        let folder = theme
            .folder_icon("src", false, false, false)
            .expect("folder icon");
        let svg = std::fs::read_to_string(folder).expect("folder icon is readable");
        assert!(
            !spans(&folder.to_string_lossy(), &svg, SIZE).is_empty(),
            "the folder icon rasterized to nothing"
        );
    }

    #[test]
    fn spans_are_cached_per_icon_and_size() {
        let first = spans("cache", RED_SQUARE, SIZE);
        let second = spans("cache", RED_SQUARE, SIZE);
        assert!(Rc::ptr_eq(&first, &second));
        assert!(Rc::ptr_eq(&first, &spans("cache", RED_SQUARE, SIZE + 0.01)));
    }
}
