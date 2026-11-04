//! Dock header chrome shared by the left, right and bottom docks.
//!
//! One geometry function lays out a dock's tabs; paint records the rectangles
//! in `AppState::dock_hits` / `dock_close`, and hit-testing reads them, so a tab
//! strip never grows a second layout (see `docs/design.md` §“Geometry is
//! shared”). Panels come from the [`crate::state::Panel`] table, so a new tab
//! needs no new paint code.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Cursor, NodeId, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_scene::SceneTree;
use igui_ui::{Container, PointerPhase, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::state::{AppState, DockId, Focus, Panel};
use crate::theme::Theme;
use crate::ui::icon;

/// Height of every dock header (and therefore of the tab strip).
pub const HEADER_H: f32 = 28.0;
/// Left/right padding inside a tab.
const TAB_PAD: f32 = 8.0;
/// Gap between a tab's icon and its label.
const ICON_GAP: f32 = 5.0;
/// Gap between a tab's label and its badge.
const BADGE_GAP: f32 = 6.0;
/// Width of the header's close-button hit target.
pub const CLOSE_W: f32 = 26.0;

/// Tab rectangles for `dock`, left to right. The header reserves room for the
/// close button on the right.
pub fn tab_layout(
    state: &AppState,
    measurer: &dyn TextMeasurer,
    rect: Rect,
    dock: DockId,
) -> Vec<(Panel, Rect)> {
    let font = state.theme.m.font_small;
    let mut x = rect.left() + TAB_PAD;
    let mut out = Vec::with_capacity(Panel::tabs_in(dock).len());
    for &panel in Panel::tabs_in(dock) {
        let mut width =
            kit::width(measurer, panel.label(), font) + TAB_PAD * 2.0 + icon::SMALL + ICON_GAP;
        if let Some(badge) = panel.badge(state) {
            width += BADGE_GAP + kit::width(measurer, &badge, font);
        }
        out.push((
            panel,
            Rect::from_min_size(Vec2::new(x, rect.top()), Size::new(width, rect.height())),
        ));
        x += width;
    }
    out
}

/// Close-button hit rectangle at the header's right edge.
pub fn close_rect(rect: Rect) -> Rect {
    Rect::from_min_size(
        Vec2::new(rect.right() - CLOSE_W, rect.top()),
        Size::new(CLOSE_W - 6.0, rect.height()),
    )
}

/// Header background: panel fill plus the one hairline that separates it from
/// the dock body.
pub fn paint_background(ctx: &mut PaintContext, theme: &Theme, rect: Rect) {
    kit::fill(ctx, rect, theme.p.panel);
    kit::hline(
        ctx,
        rect.left(),
        rect.right(),
        rect.bottom() - 1.0,
        theme.p.border,
    );
}

/// Paints the tab strip. The caller has already painted the background.
pub fn paint_tabs(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    state: &AppState,
    dock: DockId,
    tabs: &[(Panel, Rect)],
) {
    let m = state.theme.m;
    let p = state.theme.p;
    let active = state.dock_active(dock);
    for (panel, rect) in tabs {
        let is_active = active == Some(*panel);
        if is_active {
            kit::hline(
                ctx,
                rect.left() + 4.0,
                rect.right() - 4.0,
                rect.bottom() - 2.0,
                p.accent,
            );
        } else if state.dock_hover == Some(*panel) {
            kit::fill(ctx, *rect, p.hover);
        }
        let color = if is_active {
            active_color(state, *panel, &p)
        } else if state.dock_hover == Some(*panel) {
            p.text
        } else {
            p.dim
        };
        let icon_rect = Rect::from_min_size(
            Vec2::new(rect.left() + TAB_PAD, rect.center().y - icon::SMALL * 0.5),
            Size::new(icon::SMALL, icon::SMALL),
        );
        icon::paint(ctx, panel.icon(), icon_rect, color);
        let baseline = kit::baseline(measurer, *rect, m.font_small);
        kit::text(
            ctx,
            panel.label(),
            icon_rect.right() + ICON_GAP,
            baseline,
            m.font_small,
            color,
        );
        if let Some(badge) = panel.badge(state) {
            kit::text_right(
                ctx,
                measurer,
                &badge,
                rect.right() - TAB_PAD,
                baseline,
                m.font_small,
                badge_color(state, *panel, &p),
            );
        }
    }
}

/// Active tab color. The terminal tab follows keyboard focus, so the header
/// still shows which pane owns the caret.
fn active_color(state: &AppState, panel: Panel, p: &crate::theme::Palette) -> Color {
    if panel == Panel::Terminal && state.focus != Focus::Terminal {
        return p.faint;
    }
    p.accent
}

/// Badge color: the PROBLEMS count reads like the footer chip (errors first,
/// then warnings), every other badge stays faint.
fn badge_color(state: &AppState, panel: Panel, p: &crate::theme::Palette) -> Color {
    match panel {
        Panel::Problems if state.problems.errors > 0 => p.error,
        Panel::Problems if state.problems.warnings > 0 => p.syntax.number,
        _ => p.faint,
    }
}

/// The header's close button. Hides the dock; the dock keeps its tabs and
/// services, so reopening restores the same state.
pub fn paint_close(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    state: &AppState,
    dock: DockId,
    rect: Rect,
) {
    let hovered = state.dock_close_hover == Some(dock);
    if hovered {
        kit::fill(ctx, rect, state.theme.p.hover);
    }
    let color = if hovered {
        state.theme.p.text
    } else {
        state.theme.p.faint
    };
    kit::text_center(
        ctx,
        measurer,
        "×",
        rect,
        rect.center().y + 5.0,
        state.theme.m.font_ui,
        color,
    );
}

/// Dock tab under `pos` (geometry recorded by the last paint).
pub fn tab_at(state: &AppState, dock: DockId, pos: Vec2) -> Option<Panel> {
    state.dock_hits[dock.index()]
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(panel, _)| *panel)
}

/// Any open dock's tab under `pos`, in dock order.
pub fn tab_at_any(state: &AppState, pos: Vec2) -> Option<Panel> {
    DockId::ALL
        .into_iter()
        .filter(|dock| state.dock_open(*dock))
        .find_map(|dock| tab_at(state, dock, pos))
}

/// Open dock whose close button is under `pos`.
pub fn close_at(state: &AppState, pos: Vec2) -> Option<DockId> {
    DockId::ALL.into_iter().find(|dock| {
        let rect = state.dock_close[dock.index()];
        state.dock_open(*dock) && rect.width() > 0.0 && rect.contains(pos)
    })
}

fn click(state: &mut AppState, dock: DockId, pos: Vec2) {
    if state.dock_close[dock.index()].contains(pos) {
        state.dock_hide(dock);
        return;
    }
    if let Some(panel) = tab_at(state, dock, pos) {
        // A tab press only selects; the strip's × closes the dock. (The header
        // icon buttons keep the open/close toggle.)
        if state.dock_active(dock) != Some(panel) {
            state.dock_show(panel);
        }
    }
}

/// Header strip for the left or right dock (tab click + close only).
pub fn build_tabs(
    tree: &mut SceneTree,
    parent: NodeId,
    state: Rc<RefCell<AppState>>,
    dock: DockId,
) -> NodeId {
    let id = kit::node(tree, parent, "dock-header");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            let theme = state.theme;
            paint_background(env.ctx, &theme, rect);
            let tabs = tab_layout(&state, env.measurer, rect, dock);
            paint_tabs(env.ctx, env.measurer, &state, dock, &tabs);
            let close = close_rect(rect);
            paint_close(env.ctx, env.measurer, &state, dock, close);
            state.dock_hits[dock.index()] = tabs;
            state.dock_close[dock.index()] = close;
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            if phase != PointerPhase::Down {
                return;
            }
            let mut state = state.borrow_mut();
            click(&mut state, dock, pos);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, id, Cursor::Default);
    id
}

/// Bottom dock header: the dock tabs, plus the terminal's shell-instance tabs
/// next to them while the Terminal tab is visible.
pub fn build_bottom(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "bottom-dock-header");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            let theme = state.theme;
            paint_background(env.ctx, &theme, rect);
            let tabs = tab_layout(&state, env.measurer, rect, DockId::Bottom);
            paint_tabs(env.ctx, env.measurer, &state, DockId::Bottom, &tabs);
            state.terminal.header_rect = rect;
            if state.bottom_tab == Panel::Terminal {
                let x = tabs
                    .last()
                    .map(|(_, tab)| tab.right() + 12.0)
                    .unwrap_or(rect.left() + 8.0);
                crate::ui::terminal::paint_session_tabs(env.ctx, &mut state, rect, x);
            }
            let close = close_rect(rect);
            paint_close(env.ctx, env.measurer, &state, DockId::Bottom, close);
            state.dock_hits[DockId::Bottom.index()] = tabs;
            state.dock_close[DockId::Bottom.index()] = close;
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            if phase != PointerPhase::Down {
                return;
            }
            let mut state = state.borrow_mut();
            if state.dock_close[DockId::Bottom.index()].contains(pos) {
                state.dock_hide(DockId::Bottom);
            } else if let Some(panel) = tab_at(&state, DockId::Bottom, pos) {
                state.toggle_panel(panel);
            } else if state.bottom_tab == Panel::Terminal
                && crate::ui::terminal::session_tab_click(&mut state, pos)
            {
                // Handled by the terminal's own tabs.
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, id, Cursor::Default);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;

    struct Fake;

    impl TextMeasurer for Fake {
        fn advance(&self, ch: char, font_size: f32) -> f32 {
            if ch == ' ' {
                font_size * 0.25
            } else {
                font_size * 0.6
            }
        }

        fn line_height(&self, font_size: f32) -> f32 {
            font_size * 1.4
        }
    }

    fn state() -> AppState {
        let mut ws = Workspace::new(std::env::temp_dir());
        ws.new_tab();
        AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            std::env::temp_dir().join("igui-editor-dock-ui-test.conf"),
        )
    }

    #[test]
    fn problems_leads_the_bottom_dock_and_shows_counts() {
        assert_eq!(
            Panel::tabs_in(DockId::Bottom),
            [Panel::Problems, Panel::Terminal]
        );
        let mut state = state();
        assert_eq!(Panel::Problems.badge(&state), None);
        state.problems.errors = 2;
        state.problems.warnings = 1;
        assert_eq!(Panel::Problems.badge(&state).as_deref(), Some("2E 1W"));
        state.problems.errors = 0;
        assert_eq!(Panel::Problems.badge(&state).as_deref(), Some("0E 1W"));
    }

    #[test]
    fn tab_rects_are_ordered_and_inside_the_header() {
        let state = state();
        let measurer = Fake;
        let rect = Rect::from_min_size(Vec2::ZERO, Size::new(400.0, HEADER_H));
        for dock in DockId::ALL {
            let tabs = tab_layout(&state, &measurer, rect, dock);
            assert_eq!(tabs.len(), Panel::tabs_in(dock).len());
            for (panel, tab) in &tabs {
                assert_eq!(tab.top(), rect.top());
                assert_eq!(tab.height(), rect.height());
                assert!(tab.right() <= close_rect(rect).left() + 0.5, "{panel:?}");
            }
            for pair in tabs.windows(2) {
                assert!(pair[0].1.right() <= pair[1].1.left() + 0.5);
            }
        }
    }
}
