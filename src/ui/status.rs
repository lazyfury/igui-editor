//! Status bar: file, cursor position, editor facts and the last message.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv};

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "status");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, move |env, rect, _| {
        let state = state.borrow();
        paint(&state, env, rect);
    });
    id
}

fn paint(state: &AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    kit::fill(env.ctx, rect, p.panel);
    kit::hline(env.ctx, rect.left(), rect.right(), rect.top(), p.border);

    let baseline = kit::baseline(measurer, rect, m.font_small);
    let mut left = rect.left() + 10.0;

    // File + dirty mark.
    if let Some(doc) = state.ws.active_doc() {
        if doc.is_dirty() {
            env.ctx
                .fill_circle(Vec2::new(left + 2.0, rect.center().y), 2.0, p.accent);
            left += 10.0;
        }
        let name = doc.name().to_string();
        kit::text(env.ctx, &name, left, baseline, m.font_small, p.dim);
        left += kit::width(measurer, &name, m.font_small) + 14.0;
        if let Some(path) = doc.path() {
            let parent = path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default();
            let parent = kit::truncate(
                measurer,
                &parent,
                m.font_small,
                (rect.width() * 0.35).max(80.0),
            );
            kit::text(env.ctx, &parent, left, baseline, m.font_small, p.faint);
        }
    } else {
        kit::text(
            env.ctx,
            "no document",
            left,
            baseline,
            m.font_small,
            p.faint,
        );
    }

    // Right-aligned facts, outermost first.
    let mut right = rect.right() - 10.0;
    let facts = [theme.name(), "UTF-8", "LF", "Spaces: 4"];
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

    // Transient message sits left of the facts, drawn in the accent (or error).
    if let Some((message, error)) = state.message() {
        let available = (right - left - 16.0).max(0.0);
        let message = kit::truncate(measurer, message, m.font_small, available);
        if !message.is_empty() {
            kit::text(
                env.ctx,
                &message,
                left,
                baseline,
                m.font_small,
                if error { p.error } else { p.accent },
            );
        }
    }
}
