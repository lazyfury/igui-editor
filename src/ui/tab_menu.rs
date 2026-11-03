//! Right-click menu for a tab: close variants, copy path, reveal.
//!
//! Same overlay pattern as the menu bar: painted after the tree, hit-tested
//! from one geometry function, dismissed by clicking outside or `Esc`.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit;
use crate::state::{AppState, TabMenuAction, TabMenuUi};

const WIDTH: f32 = 220.0;
const ITEM_H: f32 = 24.0;
const PAD: f32 = 4.0;

/// Opens the context menu for `tab` at the pointer position.
pub fn open(state: &mut AppState, tab: usize, at: Vec2) {
    state.tab_menu = Some(TabMenuUi {
        tab,
        at,
        rect: Rect::ZERO,
        hits: Vec::new(),
        hover: None,
    });
    state.menu_open = None;
    state.menu_hover = None;
    state.dirty = true;
}

fn enabled(state: &AppState, tab: usize, action: TabMenuAction) -> bool {
    let Some(doc) = state.ws.tabs.get(tab) else {
        return false;
    };
    match action {
        TabMenuAction::Close => true,
        TabMenuAction::CloseOthers => state.ws.tabs.len() > 1,
        TabMenuAction::CloseRight => tab + 1 < state.ws.tabs.len(),
        TabMenuAction::CopyPath => doc.path().is_some(),
        TabMenuAction::RevealInSidebar => {
            state.ws.folder_open
                && doc
                    .path()
                    .is_some_and(|path| path.starts_with(&state.ws.root))
        }
    }
}

/// Menu rectangle plus `(action, rect, enabled)` rows.
pub fn layout(state: &AppState) -> (Rect, Vec<(TabMenuAction, Rect, bool)>) {
    let Some(menu) = &state.tab_menu else {
        return (Rect::ZERO, Vec::new());
    };
    let viewport = state.viewport.logical_rect();
    let height = ITEM_H * TabMenuAction::ALL.len() as f32 + PAD * 2.0;
    let x = if menu.at.x + WIDTH + 4.0 > viewport.right() {
        menu.at.x - WIDTH
    } else {
        menu.at.x
    };
    let y = if menu.at.y + height + 4.0 > viewport.bottom() {
        menu.at.y - height
    } else {
        menu.at.y
    };
    let rect = Rect::from_min_size(
        Vec2::new(
            x.clamp(
                viewport.left() + 4.0,
                (viewport.right() - WIDTH - 4.0).max(viewport.left() + 4.0),
            ),
            y.clamp(
                viewport.top() + 4.0,
                (viewport.bottom() - height - 4.0).max(viewport.top() + 4.0),
            ),
        ),
        Size::new(WIDTH, height),
    );
    let mut hits = Vec::with_capacity(TabMenuAction::ALL.len());
    for (index, action) in TabMenuAction::ALL.iter().enumerate() {
        let item = Rect::from_min_size(
            Vec2::new(rect.left(), rect.top() + PAD + index as f32 * ITEM_H),
            Size::new(WIDTH, ITEM_H),
        );
        hits.push((*action, item, enabled(state, menu.tab, *action)));
    }
    (rect, hits)
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let Some(menu) = state.tab_menu.clone() else {
        return;
    };
    if menu.tab >= state.ws.tabs.len() {
        state.tab_menu = None;
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let (rect, hits) = layout(state);
    kit::fill(ctx, rect, p.raised);
    ctx.stroke_rect(rect, 1.0, p.border);

    if let Some(menu) = state.tab_menu.as_mut() {
        menu.rect = rect;
        menu.hits = hits
            .iter()
            .map(|(action, rect, _)| (*action, *rect))
            .collect();
    }
    for (index, (action, item, usable)) in hits.iter().enumerate() {
        let hovered = state.tab_menu.as_ref().and_then(|menu| menu.hover) == Some(index) && *usable;
        if hovered {
            kit::fill(ctx, *item, p.hover);
            kit::vline(ctx, item.left(), item.top(), item.bottom(), p.accent);
        }
        kit::text(
            ctx,
            action.label(),
            item.left() + 14.0,
            kit::baseline(measurer, *item, m.font_ui),
            m.font_ui,
            if *usable { p.text } else { p.faint },
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Outside,
    Consumed,
    Run(TabMenuAction),
}

pub fn click(state: &mut AppState, pos: Vec2) -> Click {
    if state.tab_menu.is_none() {
        return Click::Outside;
    }
    let (rect, hits) = layout(state);
    if !rect.contains(pos) {
        return Click::Outside;
    }
    for (action, item, usable) in hits {
        if item.contains(pos) {
            return if usable {
                Click::Run(action)
            } else {
                Click::Consumed
            };
        }
    }
    // Padding or the border.
    Click::Consumed
}

pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    state.tab_menu.as_ref()?;
    let (rect, hits) = layout(state);
    if !rect.contains(pos) {
        return None;
    }
    hits.iter().position(|(_, item, _)| item.contains(pos))
}
