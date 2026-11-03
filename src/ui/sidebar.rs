//! Sidebar: the workspace file tree.
//!
//! One custom control paints the header and rows; expanding a directory reads
//! it lazily (`FsTree`). Clicking a folder toggles it, clicking a file opens it
//! in a tab.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Cursor, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub const HEADER_H: f32 = 28.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "sidebar");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });

    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            if phase == PointerPhase::Down {
                let mut state = state.borrow_mut();
                click(&mut state, pos);
            }
            igui_ui::request_paint(tree);
        }
    });

    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let row_h = state.theme.m.row_h;
            let content = state.ws.tree.rows().len() as f32 * row_h;
            let view = (state.sidebar.rect.height() - HEADER_H).max(0.0);
            let max = (content - view).max(0.0);
            state.sidebar.scroll = (state.sidebar.scroll - delta.y).clamp(0.0, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, id, Cursor::Default);
    id
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    state.sidebar.rect = rect;

    kit::fill(env.ctx, rect, p.panel);
    kit::vline(
        env.ctx,
        rect.right() - 1.0,
        rect.top(),
        rect.bottom(),
        p.border,
    );

    // Header: "EXPLORER" + the root directory, dim.
    let header = Rect::from_min_size(rect.origin, Size::new(rect.width(), HEADER_H));
    let baseline = kit::baseline(measurer, header, m.font_small);
    kit::text(
        env.ctx,
        "EXPLORER",
        rect.left() + 12.0,
        baseline,
        m.font_small,
        p.faint,
    );
    let root_name = state
        .ws
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| state.ws.root.display().to_string());
    let root_name = kit::truncate_left(measurer, &root_name, m.font_small, rect.width() - 96.0);
    kit::text_right(
        env.ctx,
        measurer,
        &root_name,
        rect.right() - 10.0,
        baseline,
        m.font_small,
        p.dim,
    );
    kit::hline(
        env.ctx,
        rect.left(),
        rect.right() - 1.0,
        header.bottom() - 1.0,
        p.border,
    );

    // Rows, scrolled and clipped to the area under the header.
    let rows = state.ws.tree.rows();
    let view = Rect::from_min_size(
        Vec2::new(rect.left(), header.bottom()),
        Size::new(rect.width() - 1.0, (rect.height() - HEADER_H).max(0.0)),
    );
    let row_h = m.row_h;
    let content_h = rows.len() as f32 * row_h;
    let max_scroll = (content_h - view.height()).max(0.0);
    state.sidebar.scroll = state.sidebar.scroll.clamp(0.0, max_scroll);

    env.ctx.save();
    env.ctx.clip_rect(view);
    if let Some(error) = &state.ws.tree.error {
        kit::text(
            env.ctx,
            &kit::truncate(measurer, error, m.font_small, view.width() - 20.0),
            view.left() + 12.0,
            view.top() + 18.0,
            m.font_small,
            p.error,
        );
    }
    let first = (state.sidebar.scroll / row_h).floor() as usize;
    let last = (first + (view.height() / row_h).ceil() as usize + 1).min(rows.len());
    for (index, row) in rows.iter().enumerate().take(last).skip(first) {
        let y = view.top() + index as f32 * row_h - state.sidebar.scroll;
        let row_rect =
            Rect::from_min_size(Vec2::new(view.left(), y), Size::new(view.width(), row_h));
        let selected = state
            .sidebar
            .selected
            .as_deref()
            .is_some_and(|path| path == row.path);
        let hovered = state.sidebar.hover == Some(index);
        if selected {
            kit::fill(env.ctx, row_rect, p.active);
            kit::vline(env.ctx, row_rect.left(), y, y + row_h, p.accent);
        } else if hovered {
            kit::fill(env.ctx, row_rect, p.hover);
        }

        let indent = row_rect.left() + 10.0 + row.depth as f32 * 12.0;
        if row.is_dir {
            // Chevron: two 1.5 px lines, square and font-independent.
            let cx = indent + 3.0;
            let cy = row_rect.center().y;
            let color = if selected { p.text } else { p.dim };
            if row.expanded {
                env.ctx.draw_line(
                    Vec2::new(cx - 3.5, cy - 1.5),
                    Vec2::new(cx, cy + 2.0),
                    1.5,
                    color,
                );
                env.ctx.draw_line(
                    Vec2::new(cx, cy + 2.0),
                    Vec2::new(cx + 3.5, cy - 1.5),
                    1.5,
                    color,
                );
            } else {
                env.ctx.draw_line(
                    Vec2::new(cx - 1.5, cy - 3.5),
                    Vec2::new(cx + 2.0, cy),
                    1.5,
                    color,
                );
                env.ctx.draw_line(
                    Vec2::new(cx + 2.0, cy),
                    Vec2::new(cx - 1.5, cy + 3.5),
                    1.5,
                    color,
                );
            }
        } else {
            // A 3x3 dot marks files; the dot turns accent for the active tab.
            let active = state
                .ws
                .active_doc()
                .and_then(|doc| doc.path())
                .is_some_and(|path| path == row.path);
            let color = if active { p.accent } else { p.faint };
            env.ctx.fill_rect(
                Rect::from_min_size(
                    Vec2::new(indent + 1.0, row_rect.center().y - 1.5),
                    Size::new(3.0, 3.0),
                ),
                color,
            );
        }

        let name_x = indent + 12.0;
        let name = kit::truncate(
            measurer,
            &row.name,
            m.font_ui,
            row_rect.right() - name_x - 8.0,
        );
        let baseline = kit::baseline(measurer, row_rect, m.font_ui);
        let color: Color = if selected {
            p.text
        } else if row.is_dir {
            p.dim
        } else {
            p.text
        };
        kit::text(env.ctx, &name, name_x, baseline, m.font_ui, color);
    }
    env.ctx.restore();

    // A bare-bones scrollbar (no drag in M1): square 3 px thumb.
    if max_scroll > 0.0 {
        let track = Rect::from_min_size(
            Vec2::new(rect.right() - 4.0, view.top()),
            Size::new(3.0, view.height()),
        );
        let thumb_h = (view.height() * view.height() / content_h)
            .min(view.height())
            .max(1.0);
        let thumb_y =
            track.top() + (state.sidebar.scroll / max_scroll) * (track.height() - thumb_h);
        kit::fill(
            env.ctx,
            Rect::from_min_size(Vec2::new(track.left(), thumb_y), Size::new(3.0, thumb_h)),
            p.faint,
        );
    }
}

fn click(state: &mut AppState, pos: Vec2) {
    let view = state.sidebar.rect;
    if !view.contains(pos) || pos.y < view.top() + HEADER_H {
        return;
    }
    let row_h = state.theme.m.row_h;
    let index = ((pos.y - view.top() - HEADER_H + state.sidebar.scroll) / row_h).floor() as usize;
    let rows = state.ws.tree.rows();
    let Some(row) = rows.get(index) else {
        return;
    };
    if row.is_dir {
        state.ws.tree.toggle(&row.path);
        state.status(if row.expanded {
            format!("collapsed {}", row.name)
        } else {
            format!("expanded {}", row.name)
        });
    } else {
        let path = row.path.clone();
        state.open_path(path);
    }
    state.dirty = true;
}

/// Row under `pos`, for hover feedback.
pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    let view = state.sidebar.rect;
    if !view.contains(pos) || pos.y < view.top() + HEADER_H {
        return None;
    }
    let row_h = state.theme.m.row_h;
    let index = ((pos.y - view.top() - HEADER_H + state.sidebar.scroll) / row_h).floor() as usize;
    (index < state.ws.tree.rows().len()).then_some(index)
}
