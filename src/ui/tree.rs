//! Shared row visuals for tree/list panels.
//!
//! The Explorer and the LSP outline are different data sources but should look
//! like one family: same row height, indentation, hover/selection fills,
//! markers and typography. Both painters go through [`paint_row`] so a change
//! to the tree rhythm happens in one place.

use igui_core::{Color, Rect, Size, Vec2};
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
/// Width reserved for the disclosure chevron or kind glyph (compact layout).
pub const MARKER_W: f32 = 12.0;
/// Fixed width of the disclosure-arrow slot. Every row reserves it, so a file
/// icon never sits to the left of a folder icon.
pub const ARROW_W: f32 = 8.0;
/// Arrow center inside its slot (`indent + ARROW_CX`), unchanged from the
/// chevron-only layout.
pub const ARROW_CX: f32 = 3.0;
/// Gap between the arrow slot and the icon column.
pub const ICON_GAP: f32 = 6.0;
/// Gap between the icon column and the label.
pub const LABEL_GAP: f32 = 6.0;

/// File/folder icon box for a row starting at `indent`.
///
/// The icon column always starts after the fixed arrow slot plus [`ICON_GAP`],
/// so folder icons (which have an arrow) and file icons (which keep the slot
/// empty) line up exactly.
pub fn icon_box(indent: f32, row: Rect) -> Rect {
    let side = crate::ui::fileicon::SIZE;
    Rect::from_min_size(
        Vec2::new(indent + ARROW_W + ICON_GAP, row.center().y - side * 0.5),
        Size::new(side, side),
    )
}

/// Left edge of the label: after the icon column when icons are on, otherwise
/// the original compact marker slot.
pub const fn label_x(indent: f32, icon_slot: bool) -> f32 {
    if icon_slot {
        indent + ARROW_W + ICON_GAP + crate::ui::fileicon::SIZE + LABEL_GAP
    } else {
        indent + MARKER_W
    }
}

/// Everything one row needs; the painter owns the layout.
pub struct Row<'a> {
    pub depth: usize,
    /// Fixed disclosure arrow: `Some(expanded)` for folders, `None` for files.
    /// Files still reserve the slot through [`label_x`].
    pub arrow: Option<bool>,
    /// File/folder icon from the active VS Code icon theme.
    pub icon: Option<(&'a str, &'a str)>,
    /// Reserve the icon column even while this row's SVG is still loading.
    pub icon_slot: bool,
    /// Outline symbol kind (`T`, `ƒ`, `P`, …): the compact marker slot.
    pub glyph: Option<(&'a str, Color)>,
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
    store: &crate::state::EmojiStore,
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
    if let Some(expanded) = row.arrow {
        let cx = indent + ARROW_CX;
        let cy = rect.center().y;
        let color = if row.selected { p.text } else { p.dim };
        if expanded {
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
    if let Some((key, source)) = row.icon {
        crate::ui::fileicon::paint(ctx, key, source, icon_box(indent, rect));
    }
    if let Some((text, color)) = row.glyph {
        let width = kit::width(measurer, text, m.font_small);
        let x = indent + ((MARKER_W - width) * 0.5).max(0.0);
        kit::text(ctx, text, x, baseline, m.font_small, color);
    }

    let name_x = label_x(indent, row.icon_slot || row.icon.is_some());
    let trailing_w = row.trailing.map_or(0.0, |(text, _)| {
        kit::width(measurer, text, m.font_small) + 6.0
    });
    let name_w = (rect.right() - 8.0 - name_x - trailing_w).max(12.0);
    let name = kit::truncate(measurer, row.label, m.font_ui, name_w);
    crate::ui::emoji::paint_text(
        ctx,
        store,
        measurer,
        &name,
        Vec2::new(name_x, baseline),
        m.font_ui,
        igui_core::FontWeight::NORMAL,
        None,
        row.color,
    );

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_column_sits_after_the_fixed_arrow_slot_with_gaps() {
        let row = Rect::from_min_size(Vec2::new(10.0, 20.0), Size::new(300.0, 28.0));
        let indent = 18.0;
        let icon = icon_box(indent, row);

        // The icon column starts after the fixed arrow slot.
        assert!(icon.left() >= indent + ARROW_W + ICON_GAP - 1e-4);
        assert!(icon.left() - (indent + ARROW_CX + 3.5) > 0.0);
        assert!((icon.center().y - row.center().y).abs() < 1e-4);

        // Files and folders share the icon box, and the label follows the
        // icon with the same gap while icons are on.
        let file_icon = icon_box(indent, row);
        assert_eq!(icon, file_icon);
        assert!((label_x(indent, true) - (icon.right() + LABEL_GAP)).abs() < 1e-4);

        // With icons off the tree keeps the compact marker column.
        assert!((label_x(indent, false) - (indent + MARKER_W)).abs() < 1e-4);
        assert!(icon.left() > label_x(indent, false));
    }
}
