//! Tab strip.
//!
//! Tabs are painted from the workspace (no per-tab control nodes): the active
//! tab gets an accent top rule, unsaved buffers carry an accent dot, and the
//! close button is two 1.5 px lines so it never depends on a glyph.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::{AppState, TabsUi};

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "tabs");
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
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => click(&mut state, pos),
                PointerPhase::Move => drag_move(&mut state, pos),
                PointerPhase::Up => finish_drag(&mut state),
                PointerPhase::DoubleClick => {}
            }
            igui_ui::request_paint(tree);
        }
    });
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            state.tabs.scroll += delta.y + delta.x;
            state.tabs.reveal_active = false;
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
    kit::fill(env.ctx, rect, p.panel);
    kit::hline(
        env.ctx,
        rect.left(),
        rect.right(),
        rect.bottom() - 1.0,
        p.border,
    );

    let count = state.ws.tabs.len();
    let plus_w = 28.0;
    let available = (rect.width() - plus_w).max(0.0);
    let tab_w = if count == 0 {
        0.0
    } else {
        (available / count as f32).min(m.tab_w).max(96.0)
    };
    let total = tab_w * count as f32;
    let max_scroll = (total - available).max(0.0);
    let mut scroll = state.tabs.scroll.clamp(0.0, max_scroll);
    // Reveal the active tab only after a switch/new/close, not while the user
    // is scrolling the strip. No scrollbar is drawn; the wheel is the affordance.
    if state.tabs.reveal_active {
        let active_left = state.ws.active as f32 * tab_w;
        if active_left < scroll {
            scroll = active_left;
        } else if active_left + tab_w > scroll + available {
            scroll = active_left + tab_w - available;
        }
        state.tabs.reveal_active = false;
    }
    scroll = scroll.clamp(0.0, max_scroll);
    state.tabs.scroll = scroll;

    let mut hits: Vec<(usize, Rect, Rect)> = Vec::with_capacity(count);
    for index in 0..count {
        let x = rect.left() + index as f32 * tab_w - scroll;
        let tab = Rect::from_min_size(Vec2::new(x, rect.top()), Size::new(tab_w, rect.height()));
        if tab.right() <= rect.left() || tab.left() >= rect.right() - plus_w {
            continue;
        }
        let active = index == state.ws.active;
        let hovered = state.tabs.hover == Some(index);
        if active {
            kit::fill(env.ctx, tab, p.bg);
            kit::hline(env.ctx, tab.left(), tab.right(), tab.top(), p.accent);
        } else if hovered {
            kit::fill(env.ctx, tab, p.hover);
        }
        kit::vline(
            env.ctx,
            tab.right() - 1.0,
            tab.top(),
            tab.bottom() - 1.0,
            p.border,
        );

        // Generous hit target: the painted glyph stays small, but the whole
        // right edge of the tab (24 × tab height) is clickable.
        let close = Rect::from_min_size(
            Vec2::new(tab.right() - 26.0, tab.top()),
            Size::new(24.0, tab.height()),
        );
        let name_reserved = close.left() - tab.left() - 14.0;
        let name = state.ws.tabs[index].name().to_string();
        let name = kit::truncate(measurer, &name, m.font_ui, name_reserved.max(24.0));
        let baseline = kit::baseline(measurer, tab, m.font_ui);
        let mut text_x = tab.left() + 12.0;
        if state.ws.tabs[index].is_dirty() {
            env.ctx
                .fill_circle(Vec2::new(text_x - 3.0, tab.center().y), 1.8, p.accent);
            text_x += 6.0;
        }
        crate::ui::emoji::paint_text(
            env.ctx,
            &state.emoji,
            measurer,
            &name,
            Vec2::new(text_x, baseline),
            m.font_ui,
            igui_core::FontWeight::NORMAL,
            None,
            if active { p.text } else { p.dim },
        );

        if (active || hovered) && count > 0 {
            let color = if hovered { p.text } else { p.faint };
            let c = close.center();
            env.ctx.draw_line(
                Vec2::new(c.x - 3.0, c.y - 3.0),
                Vec2::new(c.x + 3.0, c.y + 3.0),
                1.5,
                color,
            );
            env.ctx.draw_line(
                Vec2::new(c.x - 3.0, c.y + 3.0),
                Vec2::new(c.x + 3.0, c.y - 3.0),
                1.5,
                color,
            );
        }
        hits.push((index, tab, close));
    }

    // `+` button, right-aligned.
    let plus = Rect::from_min_size(
        Vec2::new(rect.right() - plus_w, rect.top()),
        Size::new(plus_w, rect.height()),
    );
    let hovered = state.tabs.hover == Some(usize::MAX);
    if hovered {
        kit::fill(env.ctx, plus, p.hover);
    }
    let c = plus.center();
    env.ctx.draw_line(
        Vec2::new(c.x - 4.0, c.y),
        Vec2::new(c.x + 4.0, c.y),
        1.5,
        p.dim,
    );
    env.ctx.draw_line(
        Vec2::new(c.x, c.y - 4.0),
        Vec2::new(c.x, c.y + 4.0),
        1.5,
        p.dim,
    );

    let drag = state.tab_drag.filter(|drag| drag.active);
    if let Some(drag) = drag {
        let marker_x = hits
            .iter()
            .find(|(index, _, _)| *index == drag.marker)
            .map(|(_, tab, _)| tab.left())
            .or_else(|| hits.last().map(|(_, tab, _)| tab.right()))
            .unwrap_or(rect.left())
            .clamp(rect.left(), rect.right() - plus_w);
        kit::fill(
            env.ctx,
            Rect::from_min_size(
                Vec2::new(marker_x - 1.0, rect.top() + 2.0),
                Size::new(2.0, rect.height() - 4.0),
            ),
            p.accent,
        );
    }

    state.tabs = TabsUi {
        hits,
        hover: state.tabs.hover,
        plus,
        scroll: state.tabs.scroll,
        strip: rect,
        reveal_active: state.tabs.reveal_active,
    };
}

fn click(state: &mut AppState, pos: Vec2) {
    state.tab_menu = None;
    if state.tabs.plus.contains(pos) {
        state.tab_drag = None;
        state.run(crate::state::Action::New);
        return;
    }
    let hits = state.tabs.hits.clone();
    for (index, tab, close) in hits {
        if tab.contains(pos) {
            let closable = index == state.ws.active || state.tabs.hover == Some(index);
            if close.contains(pos) && closable {
                state.tab_drag = None;
                state.request_close_tab(index);
                return;
            }
            state.activate_tab(index);
            state.tab_drag = Some(crate::state::TabDrag {
                from: index,
                start: pos,
                marker: index,
                insertion: index,
                active: false,
            });
            return;
        }
    }
}

fn drag_move(state: &mut AppState, pos: Vec2) {
    let Some(mut drag) = state.tab_drag else {
        return;
    };
    if !drag.active {
        if (pos - drag.start).length() < 4.0 {
            return;
        }
        drag.active = true;
    }
    let count = state.ws.tabs.len();
    let mut marker = count;
    for (index, tab, _) in &state.tabs.hits {
        if pos.x < tab.center().x {
            marker = *index;
            break;
        }
    }
    let insertion = if marker > drag.from {
        marker.saturating_sub(1)
    } else {
        marker
    }
    .min(count.saturating_sub(1));
    if drag.marker != marker || drag.insertion != insertion {
        drag.marker = marker;
        drag.insertion = insertion;
        state.dirty = true;
    }
    state.tab_drag = Some(drag);
}

fn finish_drag(state: &mut AppState) {
    let Some(drag) = state.tab_drag.take() else {
        return;
    };
    if drag.active && drag.from != drag.insertion && state.ws.move_tab(drag.from, drag.insertion) {
        state.after_tabs_changed();
        state.status("reordered tabs");
    }
}

/// Hover target: a tab index, `usize::MAX` for the `+` button.
pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    if state.tabs.plus.contains(pos) {
        return Some(usize::MAX);
    }
    state
        .tabs
        .hits
        .iter()
        .find(|(_, tab, _)| tab.contains(pos))
        .map(|(index, _, _)| *index)
}
