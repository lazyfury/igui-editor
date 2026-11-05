//! Read-only image asset tab: a fit-to-pane preview of a decoded texture.
//!
//! No file IO and no texture upload happen here; `state::images` loads the
//! asset during `update`, and this module only reads the cached handle and
//! computes the fitted rectangle. Kept separate from `ui::docs_view` so the
//! documentation tab keeps its wrap/link behaviour.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Cursor, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv};

use crate::kit::{self, RectExt};
use crate::state::{AppState, ImageStatus};
use crate::theme::Theme;

const PAD: f32 = 24.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "image-view");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::focusable(tree, id);
    kit::set_cursor(tree, id, Cursor::Default);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let state = state.borrow();
            paint(&state, env, rect);
        }
    });
    id
}

fn paint(state: &AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let p = theme.p;
    let m = theme.m;
    env.ctx.fill_rect(rect, p.bg);
    let Some(doc) = state.ws.active_doc() else {
        message(
            env,
            rect,
            &theme,
            "Image",
            "Open an image asset from the Explorer",
        );
        return;
    };
    if !doc.is_image_view() {
        return;
    }
    if !state.settings.image_preview {
        message(
            env,
            rect,
            &theme,
            doc.name(),
            "Image preview is off — turn it on in Settings → Image Preview",
        );
        return;
    }
    let Some(path) = doc.path() else {
        message(env, rect, &theme, doc.name(), "This tab has no image path");
        return;
    };
    let status = state.images.status(path);
    let Some(status) = status else {
        message(env, rect, &theme, doc.name(), "Loading image…");
        return;
    };
    match status {
        ImageStatus::Failed(detail) => message(env, rect, &theme, doc.name(), detail),
        ImageStatus::Ready(image) => {
            let avail = Rect::from_min_size(
                Vec2::new(rect.left() + PAD, rect.top() + PAD),
                Size::new(
                    (rect.width() - PAD * 2.0).max(1.0),
                    (rect.height() - PAD * 2.0).max(1.0),
                ),
            );
            let scale = (avail.width() / image.width as f32)
                .min(avail.height() / image.height as f32)
                .max(0.001);
            let destination = Rect::from_center_size(
                avail.center(),
                Size::new(image.width as f32 * scale, image.height as f32 * scale),
            );
            // A flat halo behind the image keeps transparent PNGs readable in
            // both themes without a per-image checkerboard.
            env.ctx.fill_rect(destination, p.raised);
            env.ctx.draw_image(
                image.texture,
                destination,
                None,
                igui_render::Paint::default(),
            );
            env.ctx.stroke_rect(destination, 1.0, p.border);
            let label = format!("{} × {}", image.width, image.height);
            kit::text(
                env.ctx,
                &label,
                rect.left() + PAD,
                rect.bottom() - 8.0,
                m.font_small,
                p.faint,
            );
        }
    }
}

fn message(env: &mut PaintEnv<'_>, rect: Rect, theme: &Theme, title: &str, detail: &str) {
    let p = theme.p;
    let m = theme.m;
    let measurer = env.measurer;
    let line_h = m.font_ui * 1.6;
    let max_w = (rect.width() - PAD * 2.0).max(20.0);
    let title = kit::truncate(measurer, title, m.font_ui, max_w);
    let detail = kit::truncate(measurer, detail, m.font_small, max_w);
    kit::text_center(
        env.ctx,
        measurer,
        &title,
        rect,
        rect.center().y - line_h * 0.35,
        m.font_ui,
        p.text,
    );
    kit::text_center(
        env.ctx,
        measurer,
        &detail,
        rect,
        rect.center().y + line_h * 0.65,
        m.font_small,
        p.faint,
    );
}
