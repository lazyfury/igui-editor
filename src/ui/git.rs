//! GIT sidebar tab and read-only diff overlay.
//!
//! The plugin contributes the tab; this module paints the snapshot owned by
//! `GitState` and issues only service calls (refresh/diff).

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::{PaintEnv, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub const ROW_H: f32 = 22.0;

pub fn paint_sidebar(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    let content = Rect::from_min_size(
        Vec2::new(rect.left(), rect.top() + crate::ui::sidebar::HEADER_H),
        Size::new(
            rect.width(),
            (rect.height() - crate::ui::sidebar::HEADER_H).max(0.0),
        ),
    );
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
        Size::new(content.width(), 30.0),
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
        Vec2::new(header.right() - 26.0, header.top() + 5.0),
        Size::new(20.0, 20.0),
    );
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
    let tabs_y = header.bottom() + 2.0;
    let changes_tab = Rect::from_min_size(
        Vec2::new(content.left() + 10.0, tabs_y),
        Size::new(72.0, 20.0),
    );
    let history_tab = Rect::from_min_size(
        Vec2::new(changes_tab.right() + 4.0, tabs_y),
        Size::new(72.0, 20.0),
    );
    for (rect, label, active) in [
        (
            changes_tab,
            "Changes",
            state.git.mode == crate::state::GitMode::Changes,
        ),
        (
            history_tab,
            "History",
            state.git.mode == crate::state::GitMode::History,
        ),
    ] {
        if active {
            kit::hline(
                env.ctx,
                rect.left(),
                rect.right(),
                rect.bottom() - 1.0,
                p.accent,
            );
        }
        kit::text(
            env.ctx,
            label,
            rect.left() + 6.0,
            kit::baseline(measurer, rect, m.font_small),
            m.font_small,
            if active { p.text } else { p.faint },
        );
    }
    state.git.changes_rect = changes_tab;
    state.git.history_rect = history_tab;
    let mut y = tabs_y + 24.0;
    if state.git.mode == crate::state::GitMode::History {
        state.git.history_rows.clear();
        env.ctx.save();
        env.ctx.clip_rect(content);
        for commit in state.git.commits.iter() {
            let row = Rect::from_min_size(
                Vec2::new(content.left(), y - state.git.scroll),
                Size::new(content.width() - 1.0, 34.0),
            );
            if row.bottom() > content.top() && row.top() < content.bottom() {
                kit::text(
                    env.ctx,
                    &commit.short,
                    row.left() + 12.0,
                    row.top() + 14.0,
                    m.font_small,
                    p.accent,
                );
                kit::text(
                    env.ctx,
                    &kit::truncate(measurer, &commit.subject, m.font_ui, row.width() - 92.0),
                    row.left() + 72.0,
                    row.top() + 14.0,
                    m.font_ui,
                    p.text,
                );
                kit::text(
                    env.ctx,
                    &format!("{} · {}", commit.author, commit.date),
                    row.left() + 12.0,
                    row.top() + 27.0,
                    m.font_small,
                    p.faint,
                );
            }
            state.git.history_rows.push((commit.hash.clone(), row));
            y += 36.0;
        }
        if state.git.commits.is_empty() {
            kit::text(
                env.ctx,
                "no commits",
                content.left() + 12.0,
                y + 16.0,
                m.font_small,
                p.faint,
            );
        }
        env.ctx.restore();
        return;
    }

    let mut rows = Vec::new();
    let sections = [
        ("Staged", &status.staged),
        ("Changes", &status.changes),
        ("Conflicts", &status.conflicts),
    ];
    let content_h = content.height() - 34.0;
    let max_scroll = (sections
        .iter()
        .map(|(_, entries)| entries.len() + 1)
        .sum::<usize>() as f32
        * ROW_H
        - content_h)
        .max(0.0);
    state.git.scroll = state.git.scroll.clamp(0.0, max_scroll);
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
            } else if state.sidebar.hover.is_some_and(|index| index == rows.len()) {
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
                row.left() + 28.0,
                kit::baseline(measurer, row, m.font_ui),
                m.font_ui,
                if selected { p.text } else { p.dim },
            );
            let action = Rect::from_min_size(
                Vec2::new(row.right() - 24.0, row.top() + 2.0),
                Size::new(20.0, 18.0),
            );
            if !entry.conflict {
                env.ctx.stroke_rect(action, 1.0, p.border);
                kit::text_center(
                    env.ctx,
                    measurer,
                    if entry.staged { "−" } else { "+" },
                    action,
                    kit::baseline(measurer, action, m.font_small),
                    m.font_small,
                    p.dim,
                );
            }
            rows.push((entry.path.clone(), entry.staged, row, action));
            y += ROW_H;
        }
        y += 4.0;
    }
    env.ctx.restore();
    state.git.rows = rows;

    // Bottom actions: Stage all + Commit.
    let bottom = Rect::from_min_size(
        Vec2::new(content.left() + 8.0, content.bottom() - 30.0),
        Size::new(content.width() - 16.0, 24.0),
    );
    let stage_all = Rect::from_min_size(bottom.origin, Size::new(bottom.width() * 0.48, 24.0));
    let commit = Rect::from_min_size(
        Vec2::new(bottom.right() - bottom.width() * 0.56, bottom.top()),
        Size::new(bottom.width() * 0.56, 24.0),
    );
    env.ctx.stroke_rect(stage_all, 1.0, p.border);
    kit::text_center(
        env.ctx,
        measurer,
        "Stage all",
        stage_all,
        kit::baseline(measurer, stage_all, m.font_small),
        m.font_small,
        p.dim,
    );
    env.ctx.fill_rect(
        commit,
        if status.staged.is_empty() {
            p.panel
        } else {
            p.accent
        },
    );
    kit::text_center(
        env.ctx,
        measurer,
        "Commit",
        commit,
        kit::baseline(measurer, commit, m.font_small),
        m.font_small,
        if status.staged.is_empty() {
            p.faint
        } else {
            p.bg
        },
    );
    state.git.stage_all_rect = stage_all;
    state.git.commit_rect = commit;

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

pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if state.sidebar_tab != "git" {
        return false;
    }
    if state.git.refresh_rect.contains(pos) {
        state.request_git_refresh();
        return true;
    }
    if state.git.changes_rect.contains(pos) {
        state.set_git_mode(crate::state::GitMode::Changes);
        return true;
    }
    if state.git.history_rect.contains(pos) {
        state.set_git_mode(crate::state::GitMode::History);
        return true;
    }
    if state.git.mode == crate::state::GitMode::History {
        let commit = state
            .git
            .history_rows
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(hash, _)| hash.clone());
        if let Some(hash) = commit {
            state.open_git_commit_detail(hash);
        }
        return true;
    }
    if state.git.stage_all_rect.contains(pos) {
        state.git_stage_all();
        return true;
    }
    if state.git.commit_rect.contains(pos) {
        state.open_git_commit();
        return true;
    }
    let row = state
        .git
        .rows
        .iter()
        .find(|(_, _, rect, _)| rect.contains(pos))
        .map(|(path, staged, _, action)| (path.clone(), *staged, *action));
    if let Some((path, staged, action)) = row {
        if action.contains(pos) {
            if staged {
                state.git_unstage(vec![path]);
            } else {
                state.git_stage(vec![path]);
            }
        } else {
            state.open_git_diff(path, staged);
        }
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
    state.git.diff_scroll = state.git.diff_scroll.clamp(0.0, max_scroll);
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

pub fn detail_layout(state: &AppState) -> (Rect, Rect) {
    let viewport = state.viewport.logical_rect();
    let panel = Rect::from_center_size(
        viewport.center(),
        Size::new(
            (viewport.width() - 100.0).max(320.0),
            (viewport.height() - 120.0).max(240.0),
        ),
    );
    let close = Rect::from_min_size(
        Vec2::new(panel.right() - 30.0, panel.top() + 8.0),
        Size::new(22.0, 22.0),
    );
    (panel, close)
}

pub fn paint_commit_detail(
    state: &mut AppState,
    measurer: &dyn TextMeasurer,
    ctx: &mut PaintContext,
) {
    if !state.git.detail_visible {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let (panel, close) = detail_layout(state);
    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.45));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    let Some(detail) = state.git.detail.as_ref() else {
        kit::text(
            ctx,
            if state.git.detail_loading {
                "reading commit…"
            } else {
                "no commit"
            },
            panel.left() + 16.0,
            panel.top() + 32.0,
            m.font_ui,
            p.faint,
        );
        return;
    };
    let header = format!(
        "{}  {}  {}  {}",
        detail.commit.short, detail.commit.author, detail.commit.date, detail.commit.subject
    );
    kit::text(
        ctx,
        &kit::truncate(measurer, &header, m.font_ui, panel.width() - 60.0),
        panel.left() + 16.0,
        panel.top() + 28.0,
        m.font_ui,
        p.text,
    );
    let c = close.center();
    ctx.draw_line(
        Vec2::new(c.x - 4.0, c.y - 4.0),
        Vec2::new(c.x + 4.0, c.y + 4.0),
        1.5,
        p.dim,
    );
    ctx.draw_line(
        Vec2::new(c.x - 4.0, c.y + 4.0),
        Vec2::new(c.x + 4.0, c.y - 4.0),
        1.5,
        p.dim,
    );
    let body = Rect::from_min_size(
        Vec2::new(panel.left() + 16.0, panel.top() + 44.0),
        Size::new(panel.width() - 32.0, panel.height() - 60.0),
    );
    let line_h = (m.font_small + 4.0).max(15.0);
    let lines: Vec<&str> = detail.diff.lines().collect();
    let max_scroll = (lines.len() as f32 * line_h - body.height()).max(0.0);
    state.git.detail_scroll = state.git.detail_scroll.clamp(0.0, max_scroll);
    ctx.save();
    ctx.clip_rect(body);
    let first = (state.git.detail_scroll / line_h).floor() as usize;
    let visible = (body.height() / line_h).ceil() as usize + 1;
    for (index, line) in lines.iter().enumerate().skip(first).take(visible) {
        let y = body.top() + (index as f32 - first as f32) * line_h
            - (state.git.detail_scroll - first as f32 * line_h);
        let color = if line.starts_with('+') {
            p.accent
        } else if line.starts_with('-') {
            p.error
        } else {
            p.dim
        };
        kit::text(
            ctx,
            &kit::truncate(measurer, line, m.font_small, body.width()),
            body.left(),
            y + line_h - 4.0,
            m.font_small,
            color,
        );
    }
    ctx.restore();
}

pub fn click_commit_detail(state: &mut AppState, pos: Vec2) -> bool {
    if !state.git.detail_visible {
        return false;
    }
    let (panel, close) = detail_layout(state);
    if close.contains(pos) || !panel.contains(pos) {
        state.close_git_commit_detail();
    }
    true
}

pub fn wheel_commit_detail(state: &mut AppState, delta: Vec2) -> bool {
    if !state.git.detail_visible {
        return false;
    }
    let line_h = (state.theme.m.font_small + 4.0).max(15.0);
    let lines = state
        .git
        .detail
        .as_ref()
        .map_or(0, |detail| detail.diff.lines().count());
    let (panel, _) = detail_layout(state);
    let max = (lines as f32 * line_h - (panel.height() - 60.0)).max(0.0);
    state.git.detail_scroll = (state.git.detail_scroll - delta.y).clamp(0.0, max);
    state.dirty = true;
    true
}

pub fn commit_layout(state: &AppState) -> (Rect, Rect) {
    let viewport = state.viewport.logical_rect();
    let panel = Rect::from_center_size(
        viewport.center(),
        Size::new(
            (viewport.width() - 220.0).max(320.0),
            (viewport.height() - 300.0).max(170.0),
        ),
    );
    let field = Rect::from_min_size(
        Vec2::new(panel.left() + 18.0, panel.top() + 50.0),
        Size::new(panel.width() - 36.0, 34.0),
    );
    (panel, field)
}

pub fn paint_commit(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.git.commit_open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let (panel, field) = commit_layout(state);
    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.4));
    kit::fill(ctx, panel, p.raised);
    ctx.stroke_rect(panel, 1.0, p.border);
    kit::hline(ctx, panel.left(), panel.right(), panel.top(), p.accent);
    kit::text(
        ctx,
        "Commit staged changes",
        panel.left() + 18.0,
        panel.top() + 28.0,
        m.font_ui,
        p.text,
    );
    kit::fill(ctx, field, p.bg);
    ctx.stroke_rect(field, 1.0, p.accent);
    let inner = Rect::from_min_size(
        Vec2::new(field.left() + 8.0, field.top()),
        Size::new((field.width() - 16.0).max(8.0), field.height()),
    );
    ctx.save();
    ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, field, m.font_ui);
    let text = &state.git.commit_message;
    if let Some((start, end)) = text.selection() {
        let x0 = inner.left() + kit::width(measurer, &text.text()[..start], m.font_ui);
        let x1 = inner.left() + kit::width(measurer, &text.text()[..end], m.font_ui);
        ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(x0, field.top() + 4.0),
                Size::new((x1 - x0).max(1.0), field.height() - 8.0),
            ),
            p.selection,
        );
    }
    kit::text(ctx, text.text(), inner.left(), baseline, m.font_ui, p.text);
    let caret_x = inner.left() + kit::width(measurer, &text.text()[..text.caret()], m.font_ui);
    ctx.fill_rect(
        Rect::from_min_size(
            Vec2::new(caret_x, field.top() + 4.0),
            Size::new(2.0, field.height() - 8.0),
        ),
        p.accent,
    );
    ctx.restore();
    if let Some(error) = &state.git.commit_error {
        kit::text(
            ctx,
            error,
            panel.left() + 18.0,
            field.bottom() + 20.0,
            m.font_small,
            p.error,
        );
    }
    kit::text(
        ctx,
        "Enter commit · Esc cancel",
        panel.left() + 18.0,
        panel.bottom() - 16.0,
        m.font_small,
        p.faint,
    );
}

pub fn commit_click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.git.commit_open {
        return false;
    }
    let (panel, _) = commit_layout(state);
    if !panel.contains(pos) {
        state.close_git_commit();
    }
    true
}

pub fn commit_key(
    state: &mut AppState,
    key: igui_core::Key,
    modifiers: igui_core::Modifiers,
) -> bool {
    if !state.git.commit_open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    let shift = modifiers.shift;
    if accel {
        match key {
            igui_core::Key::Character(ch) => match ch.to_ascii_lowercase() {
                'a' => state.git.commit_message.select_all(),
                'c' => {
                    if let Some(text) = state.git.commit_message.selected_text() {
                        state.set_clipboard(text);
                    }
                }
                'x' => {
                    if let Some(text) = state.git.commit_message.selected_text() {
                        state.set_clipboard(text);
                        state.git.commit_message.backspace();
                    }
                }
                'v' => {
                    if let Some(text) = state.get_clipboard() {
                        state.git.commit_message.insert(&text);
                    }
                }
                _ => return false,
            },
            _ => return false,
        }
        state.git.commit_error = None;
        state.dirty = true;
        return true;
    }
    match key {
        igui_core::Key::Escape => state.close_git_commit(),
        igui_core::Key::Enter => state.submit_git_commit(),
        igui_core::Key::Backspace => state.git.commit_message.backspace(),
        igui_core::Key::Delete => state.git.commit_message.delete(),
        igui_core::Key::ArrowLeft => state.git.commit_message.move_left(shift, modifiers.alt),
        igui_core::Key::ArrowRight => state.git.commit_message.move_right(shift, modifiers.alt),
        igui_core::Key::Home => state.git.commit_message.move_home(shift),
        igui_core::Key::End => state.git.commit_message.move_end(shift),
        igui_core::Key::Space => state.git.commit_message.insert(" "),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn commit_text(state: &mut AppState, text: &str) {
    if state.git.commit_open {
        state.git.commit_message.insert(text);
        state.git.commit_error = None;
        state.dirty = true;
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
    state.git.diff_scroll = (state.git.diff_scroll - delta.y).clamp(0.0, max);
    state.dirty = true;
    true
}
