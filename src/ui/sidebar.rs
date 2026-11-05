//! Sidebar: the left dock's tab strip plus the workspace file tree.
//!
//! The header is the shared dock strip (`ui::dock`); this module owns the body:
//! one custom control that paints the rows and reads a directory lazily
//! (`FsTree`). Clicking a folder toggles it, clicking a file opens it in a tab.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Cursor, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::{AppState, DockId};
use crate::theme::Theme;

/// The 4 px divider that resizes the sidebar (drag, column-resize cursor).
pub fn build_divider(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "sidebar-divider");
    kit::set_container(tree, id, Container::Leaf);
    kit::set_cursor(tree, id, crate::ui::handle::cursor());
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, interact| {
            let state = state.borrow();
            crate::ui::handle::paint(
                env,
                rect,
                &state.theme,
                interact.hovered,
                state.sidebar_drag.is_some(),
                crate::ui::handle::Axis::Vertical,
                crate::ui::handle::GripSide::Right,
                state.content_center_y,
            );
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => {
                    state.sidebar_drag = Some((pos.x, state.settings.sidebar_width));
                }
                PointerPhase::Move => {
                    if let Some((start_x, start_width)) = state.sidebar_drag {
                        let width = crate::settings::clamp_sidebar(start_width + (pos.x - start_x));
                        if (width - state.settings.sidebar_width).abs() > 0.5 {
                            state.settings.sidebar_width = width;
                            if let Some(sidebar) = state.sidebar_node {
                                kit::flex(tree, sidebar, 0.0, Some(width), 0.0);
                                kit::min_size(tree, sidebar, width, 0.0);
                                igui_ui::mark_dirty(tree, sidebar);
                            }
                            state.dirty = true;
                        }
                    }
                }
                PointerPhase::Up => {
                    if state.sidebar_drag.take().is_some() {
                        state.persist_settings();
                    }
                }
                PointerPhase::DoubleClick => {}
            }
            igui_ui::request_paint(tree);
        }
    });
    id
}

/// The dock: shared tab strip on top, file tree body below.
pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "sidebar");
    kit::set_container(tree, id, kit::tight_column());

    let header = crate::ui::dock::build_tabs(tree, id, state.clone(), DockId::Left);
    kit::flex(tree, header, 0.0, Some(crate::ui::dock::HEADER_H), 0.0);
    kit::min_size(tree, header, 0.0, crate::ui::dock::HEADER_H);

    let body = kit::node(tree, id, "sidebar-body");
    kit::set_container(tree, body, Container::Leaf);
    kit::clip(tree, body, true);
    kit::flex(tree, body, 1.0, None, 1.0);
    kit::draw(tree, body, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });

    kit::on_pointer(tree, body, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            if phase == PointerPhase::Down {
                let mut state = state.borrow_mut();
                state.focus = crate::state::Focus::Explorer;
                click(&mut state, pos);
            }
            igui_ui::request_paint(tree);
        }
    });

    kit::on_scroll(tree, body, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            if state.sidebar_tab == "git" {
                crate::ui::git::scroll(&mut state, delta.y);
            } else {
                let row_h = state.theme.m.row_h;
                let content = state.ws.tree.rows().len() as f32 * row_h;
                let view = state.sidebar.rect.height().max(0.0);
                let max = (content - view).max(0.0);
                crate::ui::scroll::apply(&mut state.sidebar.scroll, delta.y, max);
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, body, Cursor::Default);
    id
}

fn git_color(entry: &crate::model::git::GitEntry, theme: &Theme) -> Color {
    if entry.conflict {
        return theme.p.error;
    }
    let color = match entry.kind {
        crate::model::git::GitKind::Added => theme.p.syntax.function,
        crate::model::git::GitKind::Deleted => theme.p.error,
        crate::model::git::GitKind::Renamed => theme.p.syntax.keyword,
        crate::model::git::GitKind::Untracked => theme.p.accent,
        crate::model::git::GitKind::Conflicted => theme.p.error,
        crate::model::git::GitKind::Modified => theme.p.syntax.number,
    };
    if entry.staged {
        color
    } else {
        color.alpha_mul(0.82)
    }
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    state.sidebar.rect = rect;

    // No right border here: the resize handle's single line is the separator.
    kit::fill(env.ctx, rect, p.panel);

    if state.sidebar_tab == "git" {
        crate::ui::git::paint_sidebar(state, env, rect);
        return;
    }

    // Empty state: no project open yet (welcome screen in the editor).
    if !state.ws.folder_open {
        state.sidebar.actions.clear();
        let left = rect.left() + 12.0;
        kit::text(
            env.ctx,
            "NO PROJECT OPEN",
            left,
            rect.top() + 22.0,
            m.font_small,
            p.faint,
        );
        let mut y = rect.top() + 44.0;
        for (action, label) in [
            (crate::state::Action::OpenFolder, "Open Project…"),
            (crate::state::Action::Open, "Open File…"),
            (crate::state::Action::New, "New Buffer"),
        ] {
            let row = Rect::from_min_size(
                Vec2::new(rect.left() + 8.0, y),
                Size::new((rect.width() - 16.0).max(40.0), 28.0),
            );
            kit::text(
                env.ctx,
                label,
                row.left() + 8.0,
                kit::baseline(measurer, row, m.font_ui),
                m.font_ui,
                p.accent,
            );
            let hint = state.keymap.shortcut_label(&action).unwrap_or_default();
            kit::text_right(
                env.ctx,
                measurer,
                &hint,
                row.right() - 10.0,
                kit::baseline(measurer, row, m.font_small),
                m.font_small,
                p.faint,
            );
            state.sidebar.actions.push((action, row));
            y += 30.0;
        }
        let hint = kit::truncate(
            measurer,
            "or drop files or a project here",
            m.font_small,
            rect.width() - 24.0,
        );
        kit::text(env.ctx, &hint, left, y + 16.0, m.font_small, p.faint);
        state.sidebar.scroll = 0.0;
        return;
    }

    // Rows, scrolled and clipped to the body.
    let rows = state.ws.tree.rows();
    let view = Rect::from_min_size(
        Vec2::new(rect.left(), rect.top()),
        Size::new(rect.width() - 1.0, rect.height().max(0.0)),
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
        let marker = if row.is_dir {
            crate::ui::tree::Marker::Chevron {
                expanded: row.expanded,
            }
        } else {
            crate::ui::tree::Marker::Blank
        };
        let git = if row.is_dir {
            None
        } else {
            state.git_entry_for(&row.path)
        };
        let code = git.map(|entry| entry.kind.code().to_string());
        let trailing = code
            .as_deref()
            .zip(git)
            .map(|(text, entry)| (text, git_color(entry, &theme)));
        let color: Color = if selected {
            p.text
        } else if row.is_dir || row.dim {
            // Directories, hidden dot entries and ignored tool files are all
            // secondary: the row stays clickable, it just does not compete.
            p.dim
        } else if let Some(entry) = git {
            git_color(entry, &theme)
        } else {
            p.text
        };
        crate::ui::tree::paint_row(
            env.ctx,
            measurer,
            row_rect,
            &m,
            &p,
            &crate::ui::tree::Row {
                depth: row.depth,
                marker,
                label: &row.name,
                color,
                selected,
                hovered,
                trailing,
            },
        );
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
    if state.sidebar_tab == "git" {
        crate::ui::git::click(state, pos);
        return;
    }
    let view = state.sidebar.rect;
    if !view.contains(pos) {
        return;
    }
    if !state.ws.folder_open {
        let action = state
            .sidebar
            .actions
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(action, _)| action.clone());
        if let Some(action) = action {
            state.run(action);
        }
        return;
    }
    let row_h = state.theme.m.row_h;
    let index = ((pos.y - view.top() + state.sidebar.scroll) / row_h).floor() as usize;
    let Some(row) = state.ws.tree.rows().get(index).cloned() else {
        return;
    };
    if row.is_dir {
        state.sidebar.selected = Some(row.path.clone());
        state.ws.tree.toggle(&row.path);
        state.status(if row.expanded {
            format!("collapsed {}", row.name)
        } else {
            format!("expanded {}", row.name)
        });
    } else {
        let path = row.path.clone();
        state.open_path(path);
        state.focus = crate::state::Focus::Explorer;
    }
    state.dirty = true;
}

/// Row under `pos`, for hover feedback.
pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    if !state.ws.folder_open || state.sidebar_tab == "git" {
        return None;
    }
    let view = state.sidebar.rect;
    if !view.contains(pos) {
        return None;
    }
    let row_h = state.theme.m.row_h;
    let index = ((pos.y - view.top() + state.sidebar.scroll) / row_h).floor() as usize;
    (index < state.ws.tree.rows().len()).then_some(index)
}
