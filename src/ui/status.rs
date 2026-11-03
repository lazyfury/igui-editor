//! Status bar: file, cursor position, editor facts and the last message.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv};

use crate::kit::{self, RectExt};
use crate::state::{AppState, Menu};

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "status");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, move |env, rect, _| {
        let mut state = state.borrow_mut();
        paint(&mut state, env, rect);
    });
    id
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    kit::fill(env.ctx, rect, p.panel);
    kit::hline(env.ctx, rect.left(), rect.right(), rect.top(), p.border);

    let baseline = kit::baseline(measurer, rect, m.font_small);

    // Right-aligned facts first, so the left side knows exactly how much room
    // it has (a message must never run under them).
    let mut right = rect.right() - 10.0;
    let kind = if crate::state::ViewMode::for_path(state.ws.active_doc().and_then(|doc| doc.path()))
        == crate::state::ViewMode::Edit
    {
        "TXT"
    } else {
        "MD"
    };
    let view = match state.view_of_active() {
        crate::state::ViewMode::Edit => "EDIT",
        crate::state::ViewMode::Split => "SPLIT",
        crate::state::ViewMode::Preview => "PREVIEW",
    };
    let mode = format!("{kind} · {view}");
    let language = state
        .language_name()
        .map(|name| name.to_uppercase())
        .unwrap_or_else(|| "TEXT".into());
    let size = state
        .ws
        .active_doc()
        .map(|doc| format_size(doc.text().len()))
        .unwrap_or_default();
    let spaces = format!("Spaces: {}", state.settings.tab_width);

    // Language selector: rightmost footer cell, above the popup anchor.
    if state.ws.active_doc().is_some() {
        let width = kit::width(measurer, &language, m.font_small);
        let hit = Rect::from_min_size(
            Vec2::new(right - width - 22.0, rect.top()),
            Size::new(width + 22.0, rect.height()),
        );
        let open = state.menu_open == Some(Menu::Language);
        if state.language_hover || open {
            kit::fill(env.ctx, hit, if open { p.raised } else { p.hover });
        }
        let color = if state.language_hover || open {
            p.accent
        } else {
            p.dim
        };
        kit::text(
            env.ctx,
            &language,
            hit.left() + 8.0,
            baseline,
            m.font_small,
            color,
        );
        // A small down chevron: the label opens the language popup.
        let c = Vec2::new(hit.right() - 9.0, rect.center().y + 1.0);
        env.ctx.draw_line(
            Vec2::new(c.x - 3.0, c.y - 1.5),
            Vec2::new(c.x, c.y + 1.5),
            1.5,
            color,
        );
        env.ctx.draw_line(
            Vec2::new(c.x, c.y + 1.5),
            Vec2::new(c.x + 3.0, c.y - 1.5),
            1.5,
            color,
        );
        if open {
            kit::hline(
                env.ctx,
                hit.left(),
                hit.right(),
                hit.bottom() - 2.0,
                p.accent,
            );
        }
        state.language_hit = hit;
        right = hit.left() - 16.0;
    } else {
        state.language_hit = Rect::ZERO;
        let width = kit::width(measurer, &language, m.font_small);
        kit::text(
            env.ctx,
            &language,
            right - width,
            baseline,
            m.font_small,
            p.faint,
        );
        right -= width + 16.0;
    }

    let facts = [
        theme.name(),
        mode.as_str(),
        size.as_str(),
        "UTF-8",
        spaces.as_str(),
    ];
    for fact in facts {
        let width = kit::width(measurer, fact, m.font_small);
        kit::text(
            env.ctx,
            fact,
            right - width,
            baseline,
            m.font_small,
            p.faint,
        );
        right -= width + 16.0;
    }
    if let Some(doc) = state.ws.active_doc() {
        let (line, column) = doc.line_col();
        let position = format!("Ln {}, Col {}", line + 1, column + 1);
        let width = kit::width(measurer, &position, m.font_small);
        kit::text(
            env.ctx,
            &position,
            right - width,
            baseline,
            m.font_small,
            p.text,
        );
        right -= width + 16.0;
    }

    // Left side: dirty mark, file name, transient message, then the directory.
    // Every part is truncated against the remaining room before the facts, so
    // nothing can overlap.
    let mut left = rect.left() + 10.0;
    let doc = state.ws.active_doc();
    if let Some(doc) = doc {
        if doc.is_dirty() {
            env.ctx
                .fill_circle(Vec2::new(left + 2.0, rect.center().y), 2.0, p.accent);
            left += 10.0;
        }
        let available = (right - left - 24.0).max(64.0);
        let name_width = (rect.width() * 0.32).max(72.0).min(available);
        let name = kit::truncate(measurer, doc.name(), m.font_small, name_width);
        kit::text(env.ctx, &name, left, baseline, m.font_small, p.dim);
        left += kit::width(measurer, &name, m.font_small) + 12.0;
    } else {
        let text = "no document";
        kit::text(env.ctx, text, left, baseline, m.font_small, p.faint);
        left += kit::width(measurer, text, m.font_small) + 14.0;
        let folder = state
            .ws
            .folder()
            .and_then(|folder| folder.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "no folder open".into());
        let folder = kit::truncate(
            measurer,
            &folder,
            m.font_small,
            (right - left - 8.0).max(0.0),
        );
        kit::text(env.ctx, &folder, left, baseline, m.font_small, p.faint);
    }

    // A transient message takes priority over the directory hint.
    if let Some((message, error)) = state.message() {
        let available = (right - left - 12.0).max(0.0);
        if available > 16.0 {
            let text = kit::truncate(measurer, message, m.font_small, available);
            kit::text(
                env.ctx,
                &text,
                left,
                baseline,
                m.font_small,
                if error { p.error } else { p.accent },
            );
            left += kit::width(measurer, &text, m.font_small) + 12.0;
        }
    }

    // Directory hint only when it still fits completely.
    if let Some(doc) = doc {
        if let Some(path) = doc.path() {
            let available = (right - left - 8.0).max(0.0);
            if available > 32.0 {
                let parent = path
                    .parent()
                    .map(|parent| parent.display().to_string())
                    .unwrap_or_default();
                let parent = kit::truncate(measurer, &parent, m.font_small, available);
                kit::text(env.ctx, &parent, left, baseline, m.font_small, p.faint);
            }
        }
    }
}

fn format_size(bytes: usize) -> String {
    const KB: f32 = 1024.0;
    const MB: f32 = KB * 1024.0;
    let size = bytes as f32;
    if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{bytes} B")
    }
}
