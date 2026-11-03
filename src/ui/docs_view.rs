//! Dedicated read-only view for Godot documentation tabs.
//!
//! This is deliberately not the text editor: it has no caret, selection or
//! editing callbacks. It only wraps the generated documentation text, paints
//! linked byte ranges, and turns a click on a link into another docs target.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Cursor, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::model::wrap::{self, WrapKey, WrapLayout};
use crate::state::{AppState, Focus};

const PAD: f32 = 16.0;
const SCROLLBAR: f32 = 8.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "docs-view");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::focusable(tree, id);
    kit::set_cursor(tree, id, Cursor::Default);
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
            if phase != PointerPhase::Down {
                return;
            }
            let mut state = state.borrow_mut();
            state.focus = Focus::Editor;
            let measurer = igui_ui::text_measurer(tree);
            if let Some(link) = link_at(&state, pos, measurer.as_ref()) {
                match link {
                    crate::model::bbcode::DocLink::Native { class, symbol } => {
                        state.open_native_docs_at(class, symbol);
                    }
                    crate::model::bbcode::DocLink::Url(url) => state.open_external_url(&url),
                }
            }
            igui_ui::request_paint(tree);
        }
    });
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let max = state.docs_view.scroll_max;
            crate::ui::scroll::apply(&mut state.docs_view.scroll, delta.y, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    id
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    state.docs_view.rect = rect;
    kit::fill(env.ctx, rect, p.bg);
    let Some(doc) = state.ws.active_doc() else {
        return;
    };
    if !doc.is_docs_view() {
        return;
    }
    let font = state.editor_metrics().font;
    let line_h = state.editor_metrics().line_h;
    let tab = state.editor_metrics().tab_columns();
    let body = Rect::from_min_size(
        Vec2::new(rect.left() + PAD, rect.top() + PAD),
        Size::new(
            (rect.width() - PAD * 2.0 - SCROLLBAR).max(20.0),
            (rect.height() - PAD * 2.0).max(20.0),
        ),
    );
    state.docs_view.body = body;
    let key = WrapKey::new(doc.id(), doc.rev(), body.width(), font, tab);
    if state.docs_view.wrap.key() != Some(key) {
        state.docs_view.wrap = WrapLayout::build(doc, body.width(), font, tab, measurer);
    }
    let content_h = state.docs_view.wrap.len() as f32 * line_h;
    state.docs_view.scroll_max = (content_h - body.height()).max(0.0);
    crate::ui::scroll::clamp(&mut state.docs_view.scroll, state.docs_view.scroll_max);
    if state.docs_view.wrap.is_empty() {
        return;
    }
    let first = (state.docs_view.scroll / line_h).floor().max(0.0) as usize;
    let visible = (body.height() / line_h).ceil().max(1.0) as usize + 1;
    let links = state.docs.links.get(&doc.id()).cloned().unwrap_or_default();
    env.ctx.save();
    env.ctx.clip_rect(body);
    for row_index in first..state.docs_view.wrap.len().min(first + visible) {
        let row = state.docs_view.wrap.rows()[row_index];
        let y = body.top() + row_index as f32 * line_h - state.docs_view.scroll;
        let line = doc.line(row.line);
        let line_start = doc.line_start(row.line);
        let segment = &line[row.start_byte..row.end_byte];
        let drawn = wrap::expand_tabs(segment, tab);
        let baseline = y + (line_h - font) * 0.5 + measurer.ascent(font);
        let x = body.left();
        let color = if row.first && is_heading(segment) {
            p.accent
        } else {
            p.text
        };
        kit::text(env.ctx, &drawn, x, baseline, font, color);
        for (start, end, _) in &links {
            let from = (*start).max(line_start + row.start_byte);
            let to = (*end).min(line_start + row.end_byte);
            if from >= to {
                continue;
            }
            let from_rel = from - line_start;
            let to_rel = to - line_start;
            let prefix = wrap::expand_tabs(&line[row.start_byte..from_rel], tab);
            let part = wrap::expand_tabs(&line[from_rel..to_rel], tab);
            let px =
                x + measurer.measure_run_weighted(&prefix, font, igui_core::FontWeight::NORMAL);
            kit::text(env.ctx, &part, px, baseline, font, p.accent);
            kit::hline(
                env.ctx,
                px,
                px + measurer.measure_run_weighted(&part, font, igui_core::FontWeight::NORMAL),
                y + line_h - 2.0,
                p.accent,
            );
        }
    }
    env.ctx.restore();
    if state.docs_view.scroll_max > 0.0 {
        let track = Rect::from_min_size(
            Vec2::new(rect.right() - SCROLLBAR, body.top()),
            Size::new(SCROLLBAR, body.height()),
        );
        let thumb_h = (body.height() * body.height() / content_h)
            .min(body.height())
            .max(8.0);
        let travel = body.height() - thumb_h;
        let y = track.top()
            + (state.docs_view.scroll / state.docs_view.scroll_max.max(f32::EPSILON)) * travel;
        kit::fill(
            env.ctx,
            Rect::from_min_size(
                Vec2::new(track.left() + 2.0, y),
                Size::new(SCROLLBAR - 4.0, thumb_h),
            ),
            p.border,
        );
    }
    let _ = m;
}

fn is_heading(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == "GODOT DOCS"
        || trimmed == "=========="
        || (trimmed
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
            && trimmed.ends_with(')'))
}

fn link_at(
    state: &AppState,
    pos: Vec2,
    measurer: &dyn TextMeasurer,
) -> Option<crate::model::bbcode::DocLink> {
    let doc = state.ws.active_doc()?;
    if !doc.is_docs_view() || !state.docs_view.body.contains(pos) {
        return None;
    }
    let metrics = state.editor_metrics();
    let line_h = metrics.line_h;
    let index = ((pos.y - state.docs_view.body.top() + state.docs_view.scroll) / line_h)
        .floor()
        .max(0.0) as usize;
    let row = *state.docs_view.wrap.row(index)?;
    let line = doc.line(row.line);
    let x = pos.x - state.docs_view.body.left();
    let column = wrap::column_at_x(
        measurer,
        &line[row.start_byte..row.end_byte],
        x,
        metrics.font,
        metrics.tab_columns(),
    );
    let byte = doc.byte_at(row.line, row.start_col + column);
    state.docs_link_at(byte)
}
