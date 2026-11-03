//! Draggable divider between the editor and the Markdown preview.
//!
//! Same affordance as the sidebar handle: one centered line plus a light grip,
//! accent while dragging. Dragging updates the ratio (20–80%) live without a
//! rebuild; double-click resets to 50%.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect};
use igui_scene::SceneTree;
use igui_ui::{Container, Control, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::AppState;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "split-divider");
    kit::set_container(tree, id, Container::Leaf);
    kit::set_cursor(tree, id, crate::ui::handle::cursor());
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, interact| {
            let mut state = state.borrow_mut();
            let dragging = state.split_dragging;
            crate::ui::handle::paint(
                env,
                rect,
                &state.theme,
                interact.hovered,
                dragging,
                crate::ui::handle::GripSide::Left,
                state.content_center_y,
            );
            state.split_divider = rect;
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => state.split_dragging = true,
                PointerPhase::Move => {
                    if state.split_dragging {
                        if let (Some(editor), Some(preview)) =
                            (state.split_editor, state.split_preview)
                        {
                            let editor_rect = rect_of(tree, editor);
                            let preview_rect = rect_of(tree, preview);
                            let total = editor_rect.width()
                                + preview_rect.width()
                                + state.split_divider.width().max(4.0);
                            if total > 1.0 {
                                let ratio = ((pos.x - editor_rect.left()) / total).clamp(0.2, 0.8);
                                state.set_split_ratio(ratio);
                                let ratio = state.split_of_active();
                                state.built_split = ratio;
                                kit::flex_percent(tree, editor, ratio);
                                igui_ui::mark_dirty(tree, editor);
                            }
                        }
                    }
                }
                PointerPhase::Up => state.split_dragging = false,
                PointerPhase::DoubleClick => {
                    state.set_split_ratio(0.5);
                    if let Some(editor) = state.split_editor {
                        kit::flex_percent(tree, editor, 0.5);
                        igui_ui::mark_dirty(tree, editor);
                    }
                }
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    id
}

fn rect_of(tree: &SceneTree, id: NodeId) -> Rect {
    tree.data::<Control>(id)
        .map(|control| control.data.rect)
        .unwrap_or(Rect::ZERO)
}
