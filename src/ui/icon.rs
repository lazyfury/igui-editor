//! Lucide icons, embedded as SVG and stroked by `igui_svg`.
//!
//! The app vendors a small subset of [Lucide](https://lucide.dev) (ISC, see
//! `assets/icons/README.md`) under `assets/icons/lucide`. Each source is
//! embedded with `include_str!`, parsed once into `igui_svg::SvgDocument` and
//! then stroked straight into `igui_render`'s draw list. The engine handles the
//! SVG path grammar, `viewBox` mapping and Lucide's round caps/joins, so the app
//! keeps only the icon table: no texture atlas, no runtime asset path, and no
//! second parser to keep in sync with the engine.
//!
//! Icons stay vector-crisp at any DPI and take the color passed to [`paint`],
//! which resolves `stroke="currentColor"` exactly like a themed icon needs.

use std::sync::OnceLock;

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_svg::SvgDocument;

/// Icon box in a compact panel header or tab strip.
pub(crate) const SMALL: f32 = 13.0;
/// Icon box inside a header icon button (the click target stays larger).
pub(crate) const HEADER: f32 = 16.0;

/// Icons used by the shell. The SVG source is embedded at compile time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Icon {
    Files,
    GitBranch,
    ListTree,
    PanelLeft,
    PanelRight,
    PanelBottom,
    Terminal,
    CircleAlert,
}

impl Icon {
    /// Every icon, in cache order (see [`Icon::index`]).
    pub(crate) const ALL: [Icon; 8] = [
        Icon::Files,
        Icon::GitBranch,
        Icon::ListTree,
        Icon::PanelLeft,
        Icon::PanelRight,
        Icon::PanelBottom,
        Icon::Terminal,
        Icon::CircleAlert,
    ];

    /// Source SVG (embedded, never read from disk at runtime).
    fn svg(self) -> &'static str {
        match self {
            Icon::Files => include_str!("../../assets/icons/lucide/files.svg"),
            Icon::GitBranch => include_str!("../../assets/icons/lucide/git-branch.svg"),
            Icon::ListTree => include_str!("../../assets/icons/lucide/list-tree.svg"),
            Icon::PanelLeft => include_str!("../../assets/icons/lucide/panel-left.svg"),
            Icon::PanelRight => include_str!("../../assets/icons/lucide/panel-right.svg"),
            Icon::PanelBottom => include_str!("../../assets/icons/lucide/panel-bottom.svg"),
            Icon::Terminal => include_str!("../../assets/icons/lucide/terminal.svg"),
            Icon::CircleAlert => include_str!("../../assets/icons/lucide/circle-alert.svg"),
        }
    }

    /// Index into the document cache. Must match [`Icon::ALL`] order.
    const fn index(self) -> usize {
        match self {
            Icon::Files => 0,
            Icon::GitBranch => 1,
            Icon::ListTree => 2,
            Icon::PanelLeft => 3,
            Icon::PanelRight => 4,
            Icon::PanelBottom => 5,
            Icon::Terminal => 6,
            Icon::CircleAlert => 7,
        }
    }
}

/// Paints `icon` centered in `rect`, using `color` for `currentColor`.
///
/// A non-square rect uses its smaller side so the icon keeps its aspect ratio.
pub(crate) fn paint(ctx: &mut PaintContext, icon: Icon, rect: Rect, color: Color) {
    let Some(document) = document(icon) else {
        return;
    };
    let side = rect.size.width.min(rect.size.height);
    if side <= 0.0 {
        return;
    }
    let target = Rect::from_min_size(
        Vec2::new(rect.center().x - side * 0.5, rect.center().y - side * 0.5),
        Size::new(side, side),
    );
    document.draw(ctx, target, color);
}

fn document(icon: Icon) -> Option<&'static SvgDocument> {
    static CACHE: OnceLock<Vec<Option<SvgDocument>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        Icon::ALL
            .iter()
            .map(|icon| SvgDocument::parse(icon.svg()).ok())
            .collect()
    });
    cache[icon.index()].as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use igui_render::DrawCommand;

    fn square(size: f32) -> Rect {
        Rect::from_min_size(Vec2::ZERO, Size::new(size, size))
    }

    #[test]
    fn cache_order_matches_enum_indices() {
        for (index, icon) in Icon::ALL.into_iter().enumerate() {
            assert_eq!(icon.index(), index, "{icon:?}");
            assert!(document(icon).is_some(), "{icon:?} did not parse");
        }
    }

    #[test]
    fn every_lucide_icon_draws_geometry() {
        for icon in Icon::ALL {
            let document = document(icon).expect("icon parses");
            let mut ctx = PaintContext::new();
            document.draw(&mut ctx, square(24.0), Color::WHITE);
            assert!(
                !ctx.into_draw_list().is_empty(),
                "{icon:?} drew no geometry"
            );
        }
    }

    #[test]
    fn paint_records_strokes_in_the_target_box() {
        let mut ctx = PaintContext::new();
        let rect = Rect::from_min_size(Vec2::new(10.0, 20.0), Size::new(16.0, 16.0));
        paint(&mut ctx, Icon::PanelLeft, rect, Color::WHITE);
        let list = ctx.into_draw_list();
        assert!(list
            .commands()
            .iter()
            .any(|command| matches!(command, DrawCommand::Line { .. })));
        for command in list.commands() {
            if let DrawCommand::Line { from, to, .. } = command {
                for point in [from, to] {
                    assert!(
                        (rect.left() - 2.0..=rect.right() + 2.0).contains(&point.x)
                            && (rect.top() - 2.0..=rect.bottom() + 2.0).contains(&point.y),
                        "{point:?} escaped {rect:?}"
                    );
                }
            }
        }
    }
}
