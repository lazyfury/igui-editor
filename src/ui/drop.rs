//! Drop overlay: shown while files/folders are dragged over the window.

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub fn paint(state: &AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.drop_hover {
        return;
    }
    let p = state.theme.p;
    let viewport = state.viewport.logical_rect();
    ctx.fill_rect(viewport, Color::new(0.0, 0.0, 0.0, 0.35));
    let inset = 14.0;
    let rect = Rect::from_min_size(
        Vec2::new(viewport.left() + inset, viewport.top() + inset),
        Size::new(
            (viewport.width() - inset * 2.0).max(0.0),
            (viewport.height() - inset * 2.0).max(0.0),
        ),
    );
    ctx.stroke_rect(rect, 2.0, p.accent);

    let title = "Drop to open";
    let hint = "files open in tabs · a folder becomes the project";
    let center = viewport.center();
    let title_width = kit::width(measurer, title, 18.0);
    kit::text(
        ctx,
        title,
        center.x - title_width * 0.5,
        center.y - 6.0,
        18.0,
        p.text,
    );
    let hint_width = kit::width(measurer, hint, 12.0);
    kit::text(
        ctx,
        hint,
        center.x - hint_width * 0.5,
        center.y + 20.0,
        12.0,
        p.dim,
    );
}
