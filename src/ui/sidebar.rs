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
use crate::theme::Theme;

pub const HEADER_H: f32 = 28.0;

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
            if state.sidebar_tab == "git" {
                crate::ui::git::scroll(&mut state, delta.y);
            } else {
                let row_h = state.theme.m.row_h;
                let content = state.ws.tree.rows().len() as f32 * row_h;
                let view = (state.sidebar.rect.height() - HEADER_H).max(0.0);
                let max = (content - view).max(0.0);
                crate::ui::scroll::apply(&mut state.sidebar.scroll, delta.y, max);
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, id, Cursor::Default);
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

    // Header tabs contributed by built-in plugins (Explorer, Git, …).
    let header = Rect::from_min_size(rect.origin, Size::new(rect.width(), HEADER_H));
    let baseline = kit::baseline(measurer, header, m.font_small);
    state.sidebar.tabs.clear();
    let mut tab_x = rect.left() + 8.0;
    let tabs = state.sidebar_tabs().to_vec();
    for tab in tabs {
        let width = kit::width(measurer, tab.label, m.font_small) + 16.0;
        let hit = Rect::from_min_size(
            Vec2::new(tab_x, header.top()),
            Size::new(width, header.height()),
        );
        let active = state.sidebar_tab == tab.id;
        if active {
            kit::hline(
                env.ctx,
                hit.left() + 4.0,
                hit.right() - 4.0,
                hit.bottom() - 2.0,
                p.accent,
            );
        }
        kit::text(
            env.ctx,
            tab.label,
            hit.left() + 8.0,
            baseline,
            m.font_small,
            if active { p.text } else { p.faint },
        );
        state.sidebar.tabs.push((tab.id, hit));
        tab_x += width;
    }
    if state.sidebar_tab == "explorer" && state.ws.folder_open {
        let root_name = state
            .ws
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| state.ws.root.display().to_string());
        let available = (rect.right() - 10.0 - tab_x).max(0.0);
        let root_name = kit::truncate_left(measurer, &root_name, m.font_small, available);
        kit::text_right(
            env.ctx,
            measurer,
            &root_name,
            rect.right() - 10.0,
            baseline,
            m.font_small,
            p.dim,
        );
    }
    kit::hline(
        env.ctx,
        rect.left(),
        rect.right() - 1.0,
        header.bottom() - 1.0,
        p.border,
    );

    if state.sidebar_tab == "git" {
        crate::ui::git::paint_sidebar(state, env, rect);
        return;
    }

    // Empty state: no folder open yet (welcome screen in the editor).
    if !state.ws.folder_open {
        state.sidebar.actions.clear();
        let left = rect.left() + 12.0;
        kit::text(
            env.ctx,
            "NO FOLDER OPEN",
            left,
            header.bottom() + 22.0,
            m.font_small,
            p.faint,
        );
        let mut y = header.bottom() + 44.0;
        for (action, label) in [
            (crate::state::Action::OpenFolder, "Open Folder…"),
            (crate::state::Action::Open, "Open File…"),
            (crate::state::Action::New, "New Buffer"),
        ] {
            let row = Rect::from_min_size(
                Vec2::new(rect.left() + 8.0, y),
                Size::new((rect.width() - 16.0).max(40.0), 26.0),
            );
            kit::text(
                env.ctx,
                label,
                row.left() + 8.0,
                kit::baseline(measurer, row, m.font_ui),
                m.font_ui,
                p.accent,
            );
            kit::text_right(
                env.ctx,
                measurer,
                action.shortcut(),
                row.right() - 10.0,
                kit::baseline(measurer, row, m.font_small),
                m.font_small,
                p.faint,
            );
            state.sidebar.actions.push((action, row));
            y += 28.0;
        }
        let hint = kit::truncate(
            measurer,
            "or drop files / folders here",
            m.font_small,
            rect.width() - 24.0,
        );
        kit::text(env.ctx, &hint, left, y + 16.0, m.font_small, p.faint);
        state.sidebar.scroll = 0.0;
        return;
    }

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
        let git = if row.is_dir {
            None
        } else {
            state.git_entry_for(&row.path)
        };
        let name = kit::truncate(
            measurer,
            &row.name,
            m.font_ui,
            row_rect.right() - name_x - if git.is_some() { 24.0 } else { 8.0 },
        );
        let baseline = kit::baseline(measurer, row_rect, m.font_ui);
        let color: Color = if selected {
            p.text
        } else if row.is_dir {
            p.dim
        } else if let Some(entry) = git {
            git_color(entry, &theme)
        } else {
            p.text
        };
        kit::text(env.ctx, &name, name_x, baseline, m.font_ui, color);
        if let Some(entry) = git {
            kit::text_right(
                env.ctx,
                measurer,
                &entry.kind.code().to_string(),
                row_rect.right() - 8.0,
                baseline,
                m.font_small,
                git_color(entry, &theme),
            );
        }
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
    let tab = state
        .sidebar
        .tabs
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(id, _)| *id);
    if let Some(id) = tab {
        state.set_sidebar_tab(id);
        return;
    }
    if state.sidebar_tab == "git" {
        crate::ui::git::click(state, pos);
        return;
    }
    let view = state.sidebar.rect;
    if !view.contains(pos) || pos.y < view.top() + HEADER_H {
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
    if !state.ws.folder_open {
        return None;
    }
    let view = state.sidebar.rect;
    if !view.contains(pos) || pos.y < view.top() + HEADER_H {
        return None;
    }
    let row_h = state.theme.m.row_h;
    let index = ((pos.y - view.top() - HEADER_H + state.sidebar.scroll) / row_h).floor() as usize;
    (index < state.ws.tree.rows().len()).then_some(index)
}
