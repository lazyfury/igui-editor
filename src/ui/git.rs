//! GIT sidebar tab and read-only diff overlay.
//!
//! The plugin contributes the tab; this module paints the snapshot owned by
//! `GitState` and issues only service calls (refresh/diff).

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::{PaintEnv, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub const ROW_H: f32 = 28.0;

pub fn paint_sidebar(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    // The dock header sits above this body, so the list owns the whole rect.
    let content = rect;
    state.git.rows.clear();
    state.git.refresh_rect = Rect::ZERO;

    if !state.git.status.is_repo() {
        let message = state
            .git
            .error
            .as_deref()
            .or(state.git.status.error.as_deref())
            .unwrap_or("no git repository");
        kit::text(
            env.ctx,
            &kit::truncate(measurer, message, m.font_small, content.width() - 24.0),
            content.left() + 12.0,
            content.top() + 24.0,
            m.font_small,
            p.faint,
        );
        return;
    }

    let status = &state.git.status;
    let header = Rect::from_min_size(
        Vec2::new(content.left(), content.top()),
        Size::new(content.width(), 34.0),
    );
    let branch = if status.branch.is_empty() {
        "(no branch)"
    } else {
        &status.branch
    };
    kit::text(
        env.ctx,
        &kit::truncate(measurer, branch, m.font_ui, header.width() - 90.0),
        header.left() + 12.0,
        kit::baseline(measurer, header, m.font_ui),
        m.font_ui,
        p.text,
    );
    let counts = format!("↑{} ↓{}", status.ahead, status.behind);
    kit::text_right(
        env.ctx,
        measurer,
        &counts,
        header.right() - 34.0,
        kit::baseline(measurer, header, m.font_small),
        m.font_small,
        p.faint,
    );
    let refresh = Rect::from_min_size(
        Vec2::new(header.right() - 30.0, header.top() + 6.0),
        Size::new(22.0, 22.0),
    );
    if state.git.hover == Some(refresh) {
        kit::fill(env.ctx, refresh, p.hover);
    }
    env.ctx.stroke_rect(refresh, 1.0, p.border);
    kit::text_center(
        env.ctx,
        measurer,
        "↻",
        refresh,
        kit::baseline(measurer, refresh, m.font_small),
        m.font_small,
        if state.git.loading { p.accent } else { p.dim },
    );
    state.git.refresh_rect = refresh;
    let mut y = header.bottom() + 10.0;

    let mut rows = Vec::new();
    let sections = [
        ("Staged", &status.staged),
        ("Changes", &status.changes),
        ("Conflicts", &status.conflicts),
    ];
    let content_h = content.height() - 40.0;
    let max_scroll = (sections
        .iter()
        .map(|(_, entries)| entries.len() + 1)
        .sum::<usize>() as f32
        * ROW_H
        - content_h)
        .max(0.0);
    state.git.scroll_max = max_scroll;
    crate::ui::scroll::clamp(&mut state.git.scroll, max_scroll);
    env.ctx.save();
    env.ctx.clip_rect(content);
    for (title, entries) in sections {
        if entries.is_empty() {
            continue;
        }
        let section = Rect::from_min_size(
            Vec2::new(content.left(), y - state.git.scroll),
            Size::new(content.width(), 20.0),
        );
        if section.bottom() > content.top() && section.top() < content.bottom() {
            kit::text(
                env.ctx,
                &format!("{title} ({})", entries.len()),
                section.left() + 12.0,
                kit::baseline(measurer, section, m.font_small),
                m.font_small,
                p.faint,
            );
        }
        y += 20.0;
        for entry in entries {
            let row = Rect::from_min_size(
                Vec2::new(content.left(), y - state.git.scroll),
                Size::new(content.width() - 1.0, ROW_H),
            );
            let selected = state.git.selected.as_deref() == Some(entry.path.as_path());
            if selected {
                kit::fill(env.ctx, row, p.active);
                kit::vline(env.ctx, row.left(), row.top(), row.bottom(), p.accent);
            } else if state.git.hover == Some(row) {
                kit::fill(env.ctx, row, p.hover);
            }
            kit::text(
                env.ctx,
                &entry.kind.code().to_string(),
                row.left() + 12.0,
                kit::baseline(measurer, row, m.font_small),
                m.font_small,
                if entry.conflict { p.error } else { p.accent },
            );
            let name = entry
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.path.display().to_string());
            let name = kit::truncate(measurer, &name, m.font_ui, row.width() - 46.0);
            kit::text(
                env.ctx,
                &name,
                row.left() + 30.0,
                kit::baseline(measurer, row, m.font_ui),
                m.font_ui,
                if selected { p.text } else { p.dim },
            );
            rows.push((entry.path.clone(), entry.staged, row));
            y += ROW_H;
        }
        y += 4.0;
    }
    env.ctx.restore();
    state.git.rows = rows;

    if state.git.loading {
        kit::text(
            env.ctx,
            "reading git…",
            content.left() + 12.0,
            content.bottom() - 10.0,
            m.font_small,
            p.faint,
        );
    }
}

pub fn scroll(state: &mut AppState, delta_y: f32) {
    crate::ui::scroll::apply(&mut state.git.scroll, delta_y, state.git.scroll_max);
    state.dirty = true;
}

pub fn hover_rect(state: &AppState, pos: Vec2) -> Option<Rect> {
    if state.sidebar_tab != "git" || !state.sidebar.rect.contains(pos) {
        return None;
    }
    if state.git.refresh_rect.width() > 0.0 && state.git.refresh_rect.contains(pos) {
        return Some(state.git.refresh_rect);
    }
    state
        .git
        .rows
        .iter()
        .find(|(_, _, row)| row.contains(pos))
        .map(|(_, _, row)| *row)
}

pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if state.sidebar_tab != "git" {
        return false;
    }
    if state.git.refresh_rect.contains(pos) {
        state.request_git_refresh();
        return true;
    }
    let row = state
        .git
        .rows
        .iter()
        .find(|(_, _, rect)| rect.contains(pos))
        .map(|(path, staged, _)| (path.clone(), *staged));
    if let Some((path, staged)) = row {
        state.open_git_diff(path, staged);
        return true;
    }
    false
}

pub fn diff_layout(state: &AppState) -> (Rect, Rect) {
    let viewport = state.viewport.logical_rect();
    let panel = Rect::from_center_size(
        viewport.center(),
        Size::new(
            (viewport.width() - 80.0).max(320.0),
            (viewport.height() - 100.0).max(220.0),
        ),
    );
    let close = Rect::from_min_size(
        Vec2::new(panel.right() - 30.0, panel.top() + 8.0),
        Size::new(22.0, 22.0),
    );
    (panel, close)
}

pub fn paint_diff(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.git.diff_visible {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let (panel, close) = diff_layout(state);
    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.45));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    let header = Rect::from_min_size(
        Vec2::new(panel.left() + 14.0, panel.top() + 8.0),
        Size::new(panel.width() - 56.0, 24.0),
    );
    let title = state
        .git
        .diff
        .as_ref()
        .map(|diff| diff.path.display().to_string())
        .or_else(|| {
            state
                .git
                .selected
                .as_ref()
                .map(|path| path.display().to_string())
        })
        .unwrap_or_else(|| "diff".into());
    kit::text(
        ctx,
        &kit::truncate(measurer, &title, m.font_ui, header.width()),
        header.left(),
        kit::baseline(measurer, header, m.font_ui),
        m.font_ui,
        p.text,
    );
    let staged = state.git.diff.as_ref().is_some_and(|diff| diff.staged);
    kit::text_right(
        ctx,
        measurer,
        if staged { "STAGED" } else { "WORKTREE" },
        close.left() - 8.0,
        kit::baseline(measurer, header, m.font_small),
        m.font_small,
        p.faint,
    );
    // Close X.
    let c = close.center();
    for (a, b) in [
        (
            Vec2::new(c.x - 4.0, c.y - 4.0),
            Vec2::new(c.x + 4.0, c.y + 4.0),
        ),
        (
            Vec2::new(c.x - 4.0, c.y + 4.0),
            Vec2::new(c.x + 4.0, c.y - 4.0),
        ),
    ] {
        ctx.draw_line(a, b, 1.5, p.dim);
    }
    let body = Rect::from_min_size(
        Vec2::new(panel.left() + 14.0, panel.top() + 42.0),
        Size::new(panel.width() - 28.0, panel.height() - 58.0),
    );
    let diff = state.git.diff.as_ref();
    if state.git.diff_loading {
        kit::text(
            ctx,
            "reading diff…",
            body.left(),
            body.top() + 18.0,
            m.font_ui,
            p.faint,
        );
        return;
    }
    let Some(diff) = diff else {
        kit::text(
            ctx,
            "no diff",
            body.left(),
            body.top() + 18.0,
            m.font_ui,
            p.faint,
        );
        return;
    };
    if diff.binary {
        kit::text(
            ctx,
            "binary file",
            body.left(),
            body.top() + 18.0,
            m.font_ui,
            p.faint,
        );
        return;
    }
    let line_h = (m.font_ui + 4.0).max(16.0);
    let lines: Vec<&str> = diff.text.lines().collect();
    let max_scroll = (lines.len() as f32 * line_h - body.height()).max(0.0);
    crate::ui::scroll::clamp(&mut state.git.diff_scroll, max_scroll);
    ctx.save();
    ctx.clip_rect(body);
    let first = (state.git.diff_scroll / line_h).floor() as usize;
    let visible = (body.height() / line_h).ceil() as usize + 1;
    for (index, line) in lines.iter().enumerate().skip(first).take(visible) {
        let y = body.top() + (index as f32 - first as f32) * line_h
            - (state.git.diff_scroll - first as f32 * line_h);
        let color = if line.starts_with('+') {
            p.accent
        } else if line.starts_with('-') {
            p.error
        } else if line.starts_with("@@") {
            p.faint
        } else {
            p.dim
        };
        let text = kit::truncate(measurer, line, m.font_small, body.width());
        kit::text(
            ctx,
            &text,
            body.left(),
            y + line_h - 4.0,
            m.font_small,
            color,
        );
    }
    ctx.restore();
    if diff.truncated {
        kit::text(
            ctx,
            "diff truncated",
            body.left(),
            body.bottom() - 4.0,
            m.font_small,
            p.faint,
        );
    }
}

pub fn click_diff(state: &mut AppState, pos: Vec2) -> bool {
    if !state.git.diff_visible {
        return false;
    }
    let (panel, close) = diff_layout(state);
    if close.contains(pos) || !panel.contains(pos) {
        state.close_git_diff();
    }
    true
}

pub fn wheel_diff(state: &mut AppState, delta: Vec2) -> bool {
    if !state.git.diff_visible {
        return false;
    }
    let line_h = (state.theme.m.font_ui + 4.0).max(16.0);
    let lines = state
        .git
        .diff
        .as_ref()
        .map_or(0, |diff| diff.text.lines().count());
    let (panel, _) = diff_layout(state);
    let max = (lines as f32 * line_h - (panel.height() - 58.0)).max(0.0);
    crate::ui::scroll::apply(&mut state.git.diff_scroll, delta.y, max);
    state.dirty = true;
    true
}
