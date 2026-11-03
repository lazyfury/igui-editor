//! Menu bar plus its drop-down.
//!
//! The bar is a normal control in the shell. The drop-down is painted by the
//! app *after* the tree (so it is above the editor) and hit-tested from the same
//! geometry function — no second window, no z-order. Items are generated from
//! the live state (recent files, checked view mode, disabled commands).

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Key, Modifiers, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::state::{self, Action, AppState, Menu};

/// Drop-down geometry.
pub const POPUP_W: f32 = 260.0;
pub const ITEM_H: f32 = 24.0;
pub const SEP_H: f32 = 7.0;
pub const POPUP_PAD: f32 = 4.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "menubar");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, move |env, rect, _| {
        let mut state = state.borrow_mut();
        paint_bar(&mut state, env, rect);
    });
    id
}

fn paint_bar(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
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

    let mut x = rect.left() + 10.0;
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    state.menubar_hits.clear();
    for menu in Menu::ALL {
        let label = menu.label();
        let width = kit::width(measurer, label, m.font_ui) + 20.0;
        let hit = Rect::from_min_size(Vec2::new(x, rect.top()), Size::new(width, rect.height()));
        state.menubar_hits.push((menu, hit));
        let open = state.menu_open == Some(menu);
        if open {
            kit::fill(env.ctx, hit, p.raised);
            kit::hline(
                env.ctx,
                hit.left(),
                hit.right(),
                hit.bottom() - 2.0,
                p.accent,
            );
        } else if state.menubar_hover == Some(menu) {
            kit::fill(env.ctx, hit, p.hover);
        }
        kit::text_center(
            env.ctx,
            measurer,
            label,
            hit,
            baseline,
            m.font_ui,
            if open { p.text } else { p.dim },
        );
        x += width;
    }

    // Right side: the active file, dim, clamped to the remaining width.
    let right = rect.right() - 10.0;
    let label = match state.ws.active_doc() {
        Some(doc) => {
            let path = doc
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| doc.name().to_string());
            let dirty = if doc.is_dirty() { " •" } else { "" };
            format!("{path}{dirty}")
        }
        None => "no document open".to_string(),
    };
    let label = kit::truncate_left(measurer, &label, m.font_small, rect.width() * 0.5);
    kit::text_right(
        env.ctx,
        measurer,
        &label,
        right,
        baseline,
        m.font_small,
        p.faint,
    );
}

/// Drop-down rectangle and per-item rectangles (separators keep their rect but
/// carry `None`).
pub fn popup_layout(
    state: &AppState,
    measurer: &dyn TextMeasurer,
) -> (Rect, Vec<(Option<Action>, Rect)>) {
    let Some(menu) = state.menu_open else {
        return (Rect::ZERO, Vec::new());
    };
    // Menus open below their bar item; the language selector opens *above*
    // its footer cell (bottom-right).
    let (anchor, above) = if menu == Menu::Language {
        (state.language_hit, true)
    } else {
        (
            state
                .menubar_hits
                .iter()
                .find(|(candidate, _)| *candidate == menu)
                .map(|(_, rect)| *rect)
                .unwrap_or(Rect::ZERO),
            false,
        )
    };
    if anchor == Rect::ZERO {
        return (Rect::ZERO, Vec::new());
    }
    let items = state::items(menu, state);

    let mut widest = POPUP_W;
    for item in items.iter().flatten() {
        let label = kit::width(measurer, &item.label(), 12.0);
        let hint = kit::width(
            measurer,
            &state.keymap.shortcut_label(item).unwrap_or_default(),
            11.0,
        );
        widest = widest.max(label + hint + 64.0);
    }
    let mut height = POPUP_PAD * 2.0;
    for item in &items {
        height += if item.is_some() { ITEM_H } else { SEP_H };
    }
    let mut origin = if above {
        Vec2::new(anchor.right() - widest, anchor.top() - height)
    } else {
        Vec2::new(anchor.left(), anchor.bottom())
    };
    let bounds = state.viewport.logical_rect();
    origin.x = origin.x.clamp(
        bounds.left() + 4.0,
        (bounds.right() - widest - 4.0).max(bounds.left() + 4.0),
    );
    if above {
        origin.y = origin.y.max(bounds.top() + 4.0);
    }
    let rect = Rect::from_min_size(origin, Size::new(widest, height));
    let mut y = origin.y + POPUP_PAD;
    let mut hits = Vec::with_capacity(items.len());
    for item in &items {
        let item_height = if item.is_some() { ITEM_H } else { SEP_H };
        let item_rect = Rect::from_min_size(Vec2::new(origin.x, y), Size::new(widest, item_height));
        hits.push((item.clone(), item_rect));
        y += item_height;
    }
    (rect, hits)
}

/// Paints the drop-down on top of the whole UI.
pub fn paint_popup(
    state: &mut AppState,
    measurer: &dyn TextMeasurer,
    ctx: &mut igui_render::PaintContext,
) {
    if state.menu_open.is_none() {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let (rect, hits) = popup_layout(state, measurer);
    state.menu_rect = rect;
    state.menu_items.clear();

    kit::fill(ctx, rect, p.raised);
    ctx.stroke_rect(rect, 1.0, p.border);

    for (index, (item, item_rect)) in hits.iter().enumerate() {
        match item {
            None => {
                let y = item_rect.center().y;
                kit::hline(ctx, rect.left() + 8.0, rect.right() - 8.0, y, p.border);
            }
            Some(action) => {
                state.menu_items.push((action.clone(), *item_rect));
                let enabled = action.enabled(state);
                let hovered = state.menu_hover == Some(index) && enabled;
                if hovered {
                    kit::fill(ctx, *item_rect, p.hover);
                    kit::vline(
                        ctx,
                        item_rect.left(),
                        item_rect.top(),
                        item_rect.bottom(),
                        p.accent,
                    );
                }
                if action.checked(state) {
                    ctx.fill_rect(
                        Rect::from_min_size(
                            Vec2::new(item_rect.left() + 9.0, item_rect.center().y - 2.0),
                            Size::new(4.0, 4.0),
                        ),
                        p.accent,
                    );
                }
                let baseline = kit::baseline(measurer, *item_rect, m.font_ui);
                kit::text(
                    ctx,
                    &action.label(),
                    item_rect.left() + 20.0,
                    baseline,
                    m.font_ui,
                    if enabled { p.text } else { p.faint },
                );
                let hint = state.keymap.shortcut_label(action).unwrap_or_default();
                if !hint.is_empty() {
                    kit::text_right(
                        ctx,
                        measurer,
                        &hint,
                        item_rect.right() - 12.0,
                        baseline,
                        m.font_small,
                        if enabled { p.dim } else { p.faint },
                    );
                }
            }
        }
    }
}

/// Indices of enabled items in the open menu (separators and disabled
/// commands are skipped, so arrow keys never land on a dead row).
fn enabled_items(state: &AppState) -> Vec<usize> {
    let Some(menu) = state.menu_open else {
        return Vec::new();
    };
    state::items(menu, state)
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            item.as_ref()
                .filter(|action| action.enabled(state))
                .map(|_| index)
        })
        .collect()
}

fn move_selection(state: &mut AppState, forward: bool) {
    let enabled = enabled_items(state);
    let Some(first) = enabled.first().copied() else {
        return;
    };
    let next = match state
        .menu_hover
        .and_then(|current| enabled.iter().position(|index| *index == current))
    {
        Some(position) if forward => enabled[(position + 1) % enabled.len()],
        Some(position) => enabled[(position + enabled.len() - 1) % enabled.len()],
        None if forward => first,
        None => *enabled.last().unwrap_or(&first),
    };
    state.menu_hover = Some(next);
    state.dirty = true;
}

fn switch_menu(state: &mut AppState, delta: isize) {
    let Some(menu) = state.menu_open else {
        return;
    };
    let current = Menu::ALL.iter().position(|item| *item == menu).unwrap_or(0);
    let next = (current as isize + delta).rem_euclid(Menu::ALL.len() as isize) as usize;
    state.menu_open = Some(Menu::ALL[next]);
    state.menu_hover = None;
    move_selection(state, delta > 0);
    state.dirty = true;
}

fn activate(state: &mut AppState) -> bool {
    let enabled = enabled_items(state);
    let Some(index) = state
        .menu_hover
        .filter(|index| enabled.contains(index))
        .or_else(|| enabled.first().copied())
    else {
        return false;
    };
    let Some(menu) = state.menu_open else {
        return false;
    };
    let action = state::items(menu, state)
        .get(index)
        .and_then(|item| item.clone());
    let Some(action) = action else {
        return false;
    };
    state.menu_open = None;
    state.menu_hover = None;
    state.run(action);
    true
}

/// Opens the File menu from the keyboard (`F10`), or closes an open menu.
pub fn toggle_keyboard(state: &mut AppState) {
    if state.menu_open.is_some() {
        state.menu_open = None;
        state.menu_hover = None;
    } else {
        state.menu_open = Some(Menu::File);
        state.menu_hover = None;
        move_selection(state, true);
    }
    state.dirty = true;
}

/// Keyboard handling while a menu is open. Returns `true` when consumed.
/// Modifier chords fall through so global shortcuts keep working.
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if state.menu_open.is_none() {
        return false;
    }
    if modifiers.meta || modifiers.ctrl || modifiers.alt {
        return false;
    }
    match key {
        Key::Escape => {
            state.menu_open = None;
            state.menu_hover = None;
            state.dirty = true;
            true
        }
        Key::ArrowDown => {
            move_selection(state, true);
            true
        }
        Key::ArrowUp => {
            move_selection(state, false);
            true
        }
        Key::ArrowLeft => {
            switch_menu(state, -1);
            true
        }
        Key::ArrowRight => {
            switch_menu(state, 1);
            true
        }
        Key::Home => {
            state.menu_hover = enabled_items(state).first().copied();
            state.dirty = true;
            true
        }
        Key::End => {
            state.menu_hover = enabled_items(state).last().copied();
            state.dirty = true;
            true
        }
        Key::Enter | Key::Space => activate(state),
        // While a menu owns the keyboard, no plain key may edit the document
        // behind it (Tab, Delete, function keys, …).
        _ => true,
    }
}

/// Result of a pointer press while a menu may be open.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Click {
    /// The press was outside the drop-down: the caller closes the menu.
    Outside,
    /// Consumed (padding, separator, or a disabled item).
    Consumed,
    Run(Action),
}

pub fn handle_click(state: &mut AppState, pos: Vec2, measurer: &dyn TextMeasurer) -> Click {
    if state.menu_open.is_none() {
        return Click::Outside;
    }
    let (rect, hits) = popup_layout(state, measurer);
    if !rect.contains(pos) {
        return Click::Outside;
    }
    for (index, (item, item_rect)) in hits.iter().enumerate() {
        if item_rect.contains(pos) {
            match item {
                Some(action) if action.enabled(state) => {
                    state.menu_hover = Some(index);
                    return Click::Run(action.clone());
                }
                _ => return Click::Consumed,
            }
        }
    }
    Click::Consumed
}

/// Toggles the menu whose bar item contains `pos`. Returns whether it did.
pub fn handle_bar_click(state: &mut AppState, pos: Vec2) -> bool {
    let hit = state
        .menubar_hits
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(menu, _)| *menu);
    let Some(menu) = hit else {
        return false;
    };
    state.menu_open = if state.menu_open == Some(menu) {
        None
    } else {
        Some(menu)
    };
    state.menu_hover = None;
    state.dirty = true;
    true
}

/// Which popup item is under `pos`, for hover feedback.
pub fn hover_at(state: &AppState, pos: Vec2, measurer: &dyn TextMeasurer) -> Option<usize> {
    state.menu_open?;
    let (rect, hits) = popup_layout(state, measurer);
    if !rect.contains(pos) {
        return None;
    }
    hits.iter()
        .position(|(item, item_rect)| item.is_some() && item_rect.contains(pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::theme::Theme;

    fn state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-menu-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let mut ws = Workspace::new(&dir);
        ws.new_tab();
        AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            dir.join("session.conf"),
        )
    }

    #[test]
    fn f10_opens_the_file_menu_with_the_first_enabled_item() {
        let mut state = state();
        toggle_keyboard(&mut state);
        assert_eq!(state.menu_open, Some(Menu::File));
        assert_eq!(state.menu_hover, Some(0));
        assert!(on_key(&mut state, Key::Escape, Modifiers::NONE));
        assert!(state.menu_open.is_none());
    }

    #[test]
    fn arrows_skip_separators_and_enter_runs_the_selection() {
        let mut state = state();
        toggle_keyboard(&mut state);
        let tabs = state.ws.tabs.len();
        assert!(on_key(&mut state, Key::ArrowDown, Modifiers::NONE));
        assert_eq!(state.menu_hover, Some(1));
        assert!(on_key(&mut state, Key::ArrowUp, Modifiers::NONE));
        assert_eq!(state.menu_hover, Some(0));
        assert!(on_key(&mut state, Key::Enter, Modifiers::NONE));
        assert!(state.menu_open.is_none());
        assert_eq!(state.ws.tabs.len(), tabs + 1, "Enter must run New");
    }

    #[test]
    fn left_and_right_switch_the_open_menu() {
        let mut state = state();
        toggle_keyboard(&mut state);
        assert!(on_key(&mut state, Key::ArrowRight, Modifiers::NONE));
        assert_eq!(state.menu_open, Some(Menu::Edit));
        assert!(on_key(&mut state, Key::ArrowLeft, Modifiers::NONE));
        assert_eq!(state.menu_open, Some(Menu::File));
    }

    #[test]
    fn plain_keys_do_not_reach_the_document_while_a_menu_is_open() {
        let mut state = state();
        toggle_keyboard(&mut state);
        assert!(on_key(&mut state, Key::Tab, Modifiers::NONE));
        assert_eq!(state.menu_open, Some(Menu::File));
    }

    #[test]
    fn modified_keys_fall_through_to_global_shortcuts() {
        let mut state = state();
        toggle_keyboard(&mut state);
        assert!(!on_key(
            &mut state,
            Key::Character('s'),
            Modifiers {
                meta: true,
                ..Modifiers::NONE
            }
        ));
        assert_eq!(state.menu_open, Some(Menu::File));
    }
}
