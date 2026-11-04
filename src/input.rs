//! Central keyboard dispatch: overlays first, then global shortcuts, then the
//! editor/tree.
//!
//! The editor used to own every key callback. Overlays (find/replace, the
//! confirmation modal, native dialog requests) make that impossible: a key must
//! reach the focused field without also reaching the document. The order is:
//!
//! 1. **Modal** — swallows everything except its own shortcuts.
//! 2. **Find/replace fields** — while a field has focus, editing keys edit that
//!    field; app commands the field does not use (`⌘S`, `⌘F`, …) still run.
//! 3. **Global shortcuts** — work regardless of focus (`⌘S`, `⌘O`, `⇧⌘M`, …).
//! 4. **The tree** — the editor control handles plain editing keys.
//!
//! Text and IME events follow the same focus split (see `app::event`).

use igui_core::{EventResult, Key, Modifiers};

use crate::keymap::KeymapId;
use crate::state::{AppState, Focus, Modal, ModalButton};

/// App-wide commands. `Some` means the key was consumed.
///
/// Dispatch is intentionally delegated to [`crate::keymap`] only: there is no
/// second hard-coded shortcut table here, so profile overrides and menu hints
/// always describe the same behavior.
pub fn shortcut(state: &mut AppState, key: Key, modifiers: Modifiers) -> Option<EventResult> {
    if state.focus == Focus::Editor
        && state.settings.keymap == KeymapId::Vim
        && !modifiers.meta
        && !modifiers.alt
        && (!modifiers.ctrl || matches!(key, Key::Character('r')))
        && crate::vim::handle_key(state, key, modifiers)
    {
        return Some(EventResult::Handled);
    }
    state
        .keymap_key(key, modifiers)
        .then_some(EventResult::Handled)
}

/// Keys while a find/replace field has focus.
pub fn find_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> EventResult {
    if !matches!(state.focus, Focus::Find | Focus::Replace) {
        return EventResult::Ignored;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    let shift = modifiers.shift;

    match key {
        Key::Escape => {
            state.close_find();
            return EventResult::Handled;
        }
        Key::Enter => {
            state.find_step(if shift { -1 } else { 1 });
            return EventResult::Handled;
        }
        Key::Tab => {
            state.focus = match state.focus {
                Focus::Find => Focus::Replace,
                _ => Focus::Find,
            };
            if let Some(field) = state.find_field_mut() {
                field.select_all();
            }
            state.dirty = true;
            return EventResult::Handled;
        }
        Key::Character(ch) if accel => {
            match ch.to_ascii_lowercase() {
                'a' => {
                    if let Some(field) = state.find_field_mut() {
                        field.select_all();
                    }
                }
                'c' => field_copy(state),
                'x' => {
                    field_copy(state);
                    if let Some(field) = state.find_field_mut() {
                        field.backspace();
                    }
                }
                'v' => field_paste(state),
                // Everything else is an app command (`⌘S`, `⌘F`, …).
                _ => return EventResult::Ignored,
            }
            after_edit(state);
            return EventResult::Handled;
        }
        _ if accel => return EventResult::Ignored,
        Key::Backspace => {
            if let Some(field) = state.find_field_mut() {
                field.backspace();
            }
        }
        Key::Delete => {
            if let Some(field) = state.find_field_mut() {
                field.delete();
            }
        }
        Key::ArrowLeft => {
            if let Some(field) = state.find_field_mut() {
                field.move_left(shift, modifiers.alt);
            }
        }
        Key::ArrowRight => {
            if let Some(field) = state.find_field_mut() {
                field.move_right(shift, modifiers.alt);
            }
        }
        Key::Home => {
            if let Some(field) = state.find_field_mut() {
                field.move_home(shift);
            }
        }
        Key::End => {
            if let Some(field) = state.find_field_mut() {
                field.move_end(shift);
            }
        }
        Key::Space => {
            if let Some(field) = state.find_field_mut() {
                field.insert(" ");
            }
        }
        _ => return EventResult::Ignored,
    }
    after_edit(state);
    EventResult::Handled
}

/// Keys while the confirmation modal is open: Enter saves, Esc cancels.
pub fn modal_key(state: &mut AppState, key: Key) -> EventResult {
    match key {
        Key::Escape => state.resolve_modal(ModalButton::Cancel),
        Key::Enter => {
            let confirm = match state.modal {
                Some(Modal::ReplaceInFiles { .. }) => ModalButton::Apply,
                Some(Modal::CloseFolder { .. }) => ModalButton::Discard,
                Some(Modal::SwitchProject { .. }) => ModalButton::Discard,
                Some(Modal::DeletePath { .. }) => ModalButton::Cancel,
                Some(Modal::CloseTabs { .. }) => ModalButton::Discard,
                Some(Modal::ExternalChange { .. }) => ModalButton::Reload,
                Some(Modal::Quit { .. }) => ModalButton::Cancel,
                _ => ModalButton::Save,
            };
            state.resolve_modal(confirm);
        }
        _ => {}
    }
    EventResult::Handled
}

/// Inserts committed text into the focused find/replace field.
pub fn find_text(state: &mut AppState, text: &str) {
    if let Some(field) = state.find_field_mut() {
        field.insert(text);
    }
    after_edit(state);
}

/// Handles an IME event for the focused find/replace field.
pub fn find_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.find.preedit = Some(text.clone());
            state.dirty = true;
        }
        igui_core::ImeEvent::Commit(text) => {
            state.find.preedit = None;
            if let Some(field) = state.find_field_mut() {
                field.insert(text);
            }
            after_edit(state);
        }
        igui_core::ImeEvent::Disabled => {
            state.find.preedit = None;
            state.dirty = true;
        }
        igui_core::ImeEvent::Enabled => {}
    }
}

fn after_edit(state: &mut AppState) {
    state.find.preedit = None;
    state.find.dirty = true;
    state.refresh_find();
    state.find_select_current();
    state.dirty = true;
}

fn field_copy(state: &AppState) {
    let field = match state.focus {
        Focus::Find => Some(&state.find.query),
        Focus::Replace => Some(&state.find.replace),
        Focus::Editor | Focus::Explorer | Focus::Terminal => None,
    };
    let text = field.and_then(|field| field.selected_text().map(str::to_string));
    if let Some(text) = text {
        state.set_clipboard(&text);
    }
}

fn field_paste(state: &mut AppState) {
    if let Some(text) = state.get_clipboard() {
        if let Some(field) = state.find_field_mut() {
            field.insert(&text);
        }
    }
}
