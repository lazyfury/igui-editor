//! Shared row visuals for tree/list panels.
//!
//! The Explorer and the LSP outline are different data sources but should look
//! like one family: same row height, indentation, hover/selection fills,
//! markers and typography. Both painters go through [`paint_row`] so a change
//! to the tree rhythm happens in one place.

use igui_core::{Color, Rect, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit;
use crate::theme::{Metrics, Palette};

/// Horizontal padding from the panel edge to the first indent level.
///
/// Derived from the dock tab strip so a depth-0 marker is centred on the tab's
/// icon: rows read as the tab's content, indented the way a child level is.
pub const PAD: f32 = crate::ui::dock::TAB_PAD * 2.0 + crate::ui::icon::SMALL * 0.5 - MARKER_W * 0.5;
/// Indentation added per tree depth.
pub const INDENT: f32 = 12.0;
/// Width reserved for the disclosure chevron or kind glyph.
pub const MARKER_W: f32 = 12.0;

/// The marker painted before a row's label.
pub enum Marker<'a> {
    /// Folder disclosure triangle.
    Chevron { expanded: bool },
    /// No marker: the label starts in the marker column (plain file rows).
    Blank,
    /// Outline symbol kind (`T`, `ƒ`, `P`, …).
    Glyph { text: &'a str, color: Color },
}

/// Everything one row needs; the painter owns the layout.
pub struct Row<'a> {
    pub depth: usize,
    pub marker: Marker<'a>,
    pub label: &'a str,
    pub color: Color,
    pub selected: bool,
    pub hovered: bool,
    /// Right-aligned status text (the Explorer's Git code).
    pub trailing: Option<(&'a str, Color)>,
}

/// Paints one tree row inside `rect` (the row's full-width hit box).
pub fn paint_row(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    rect: Rect,
    m: &Metrics,
    p: &Palette,
    row: &Row<'_>,
) {
    if row.selected {
        kit::fill(ctx, rect, p.active);
        kit::vline(ctx, rect.left(), rect.top(), rect.bottom(), p.accent);
    } else if row.hovered {
        kit::fill(ctx, rect, p.hover);
    }

    let indent = rect.left() + PAD + row.depth as f32 * INDENT;
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    match &row.marker {
        Marker::Chevron { expanded } => {
            let cx = indent + 3.0;
            let cy = rect.center().y;
            let color = if row.selected { p.text } else { p.dim };
            if *expanded {
                ctx.draw_line(
                    Vec2::new(cx - 3.5, cy - 1.5),
                    Vec2::new(cx, cy + 2.0),
                    1.5,
                    color,
                );
                ctx.draw_line(
                    Vec2::new(cx, cy + 2.0),
                    Vec2::new(cx + 3.5, cy - 1.5),
                    1.5,
                    color,
                );
            } else {
                ctx.draw_line(
                    Vec2::new(cx - 1.5, cy - 3.5),
                    Vec2::new(cx + 2.0, cy),
                    1.5,
                    color,
                );
                ctx.draw_line(
                    Vec2::new(cx + 2.0, cy),
                    Vec2::new(cx - 1.5, cy + 3.5),
                    1.5,
                    color,
                );
            }
        }
        Marker::Blank => {}
        Marker::Glyph { text, color } => {
            let width = kit::width(measurer, text, m.font_small);
            let x = indent + ((MARKER_W - width) * 0.5).max(0.0);
            kit::text(ctx, text, x, baseline, m.font_small, *color);
        }
    }

    let name_x = indent + MARKER_W;
    let trailing_w = row.trailing.map_or(0.0, |(text, _)| {
        kit::width(measurer, text, m.font_small) + 6.0
    });
    let name_w = (rect.right() - 8.0 - name_x - trailing_w).max(12.0);
    let name = kit::truncate(measurer, row.label, m.font_ui, name_w);
    kit::text(ctx, &name, name_x, baseline, m.font_ui, row.color);

    if let Some((text, color)) = row.trailing {
        kit::text_right(
            ctx,
            measurer,
            text,
            rect.right() - 8.0,
            baseline,
            m.font_small,
            color,
        );
    }
}
