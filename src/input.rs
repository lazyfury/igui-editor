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

use crate::state::{Action, AppState, Focus, Modal, ModalButton};

/// App-wide commands. `Some` means the key was consumed.
pub fn shortcut(state: &mut AppState, key: Key, modifiers: Modifiers) -> Option<EventResult> {
    // Option/Alt+Z is the soft-wrap toggle (not a Cmd/Ctrl accelerator).
    if modifiers.alt && !modifiers.meta && !modifiers.ctrl {
        if state.focus == Focus::Editor
            && matches!(key, Key::Character(ch) if ch.eq_ignore_ascii_case(&'z'))
        {
            state.run(Action::ToggleSoftWrap);
            return Some(EventResult::Handled);
        }
        return None;
    }
    if !(modifiers.meta || modifiers.ctrl) {
        return None;
    }
    let shift = modifiers.shift;
    let action = match key {
        Key::Character(ch) => match ch.to_ascii_lowercase() {
            'n' => {
                if shift {
                    Action::NewFile
                } else {
                    Action::New
                }
            }
            'o' => {
                if shift {
                    Action::OpenFolder
                } else {
                    Action::Open
                }
            }
            's' => {
                if shift {
                    Action::SaveAs
                } else {
                    Action::Save
                }
            }
            'w' => Action::CloseTab,
            'z' => {
                if shift {
                    Action::Redo
                } else {
                    Action::Undo
                }
            }
            'x' => Action::Cut,
            'c' => Action::Copy,
            'v' => Action::Paste,
            'a' => Action::SelectAll,
            ',' => {
                state.toggle_settings();
                return Some(EventResult::Handled);
            }
            'p' => {
                state.open_quick_open();
                return Some(EventResult::Handled);
            }
            'f' => {
                if shift {
                    state.open_project_search();
                    return Some(EventResult::Handled);
                }
                let replace = modifiers.alt;
                if state.find.open {
                    state.focus = if replace { Focus::Replace } else { Focus::Find };
                    state.dirty = true;
                } else {
                    state.open_find(replace);
                }
                return Some(EventResult::Handled);
            }
            'b' => Action::ToggleSidebar,
            'l' => Action::ToggleTheme,
            'm' => Action::SetView(state.view_of_active().next()),
            'y' => Action::ToggleScrollSync,
            '`' => Action::ToggleTerminal,
            'g' => {
                if state.find.open {
                    state.find_step(if shift { -1 } else { 1 });
                } else {
                    state.open_find(false);
                }
                return Some(EventResult::Handled);
            }
            _ => return None,
        },
        Key::ArrowLeft | Key::ArrowRight => {
            let Some(doc) = state.ws.active_doc_mut() else {
                return Some(EventResult::Handled);
            };
            if matches!(key, Key::ArrowLeft) {
                doc.move_home(shift);
            } else {
                doc.move_end(shift);
            }
            state.editor.reveal = true;
            state.dirty = true;
            return Some(EventResult::Handled);
        }
        Key::ArrowUp | Key::ArrowDown => {
            let Some(doc) = state.ws.active_doc_mut() else {
                return Some(EventResult::Handled);
            };
            if matches!(key, Key::ArrowUp) {
                doc.move_doc_start(shift);
            } else {
                doc.move_doc_end(shift);
            }
            state.editor.reveal = true;
            state.dirty = true;
            return Some(EventResult::Handled);
        }
        _ => return None,
    };
    state.run(action);
    Some(EventResult::Handled)
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
        Focus::Editor | Focus::Terminal => None,
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
