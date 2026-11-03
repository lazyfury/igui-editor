//! Small, deliberately non-fake Vim layer.
//!
//! The keymap profile `vim` enables this module. Insert mode delegates to the
//! normal text path; Normal/Visual interpret keys here. The first slice covers
//! the motions and edits most people use (`h j k l`, `w b`, `0 $`, `gg G`,
//! `i a I A o O`, `x`, `dd`, `yy`, `p`, `u`, `Ctrl+r`, `v`, `V`).
//! Ex commands, text objects, registers and macros remain future work.

use igui_core::{Key, Modifiers};

use crate::state::{AppState, Focus};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VimMode {
    #[default]
    Insert,
    Normal,
    Visual,
    VisualLine,
}

impl VimMode {
    pub const fn is_insert(self) -> bool {
        matches!(self, VimMode::Insert)
    }

    pub const fn label(self) -> &'static str {
        match self {
            VimMode::Insert => "INSERT",
            VimMode::Normal => "NORMAL",
            VimMode::Visual | VimMode::VisualLine => "VISUAL",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VimState {
    pub mode: VimMode,
    pending: Option<char>,
    count: usize,
    /// The key that entered Insert also produces a TextInput event; consume
    /// that one character so `i` does not type an `i`.
    pub suppress_text: bool,
}

impl VimState {
    fn enter_insert(&mut self) {
        self.mode = VimMode::Insert;
        self.count = 0;
        self.suppress_text = true;
    }

    pub fn reset_for_keymap(&mut self, vim: bool) {
        self.mode = if vim {
            VimMode::Normal
        } else {
            VimMode::Insert
        };
        self.pending = None;
        self.count = 0;
    }
}

/// Handles a key while Vim owns the editor. Returns true when consumed.
pub fn handle_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if state.focus != Focus::Editor || state.settings.keymap != crate::keymap::KeymapId::Vim {
        return false;
    }
    if state.ws.active_doc().is_none() {
        return false;
    }

    if state.vim.mode.is_insert() {
        if key == Key::Escape {
            state.vim.mode = VimMode::Normal;
            state.vim.pending = None;
            state.vim.count = 0;
            state.dirty = true;
            return true;
        }
        return false;
    }

    if modifiers.ctrl && !modifiers.meta && matches!(key, Key::Character('r')) {
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.redo();
        }
        state.find.dirty = true;
        state.dirty = true;
        return true;
    }

    let upper = shifts_uppercase(key, modifiers);
    let character = match key {
        Key::Character(ch) => Some(ch),
        _ => None,
    };
    let lower = character.map(|ch| ch.to_ascii_lowercase());

    // Counts are only consumed for digits before a motion/operator.
    if let Some(ch) = character {
        if ch.is_ascii_digit() && !(ch == '0' && state.vim.count == 0) {
            state.vim.count = state.vim.count.saturating_mul(10) + (ch as u8 - b'0') as usize;
            state.dirty = true;
            return true;
        }
    }

    if state.vim.mode == VimMode::Normal {
        if key == Key::Escape {
            state.vim.pending = None;
            state.vim.count = 0;
            if let Some(doc) = state.ws.active_doc_mut() {
                let caret = doc.caret();
                doc.set_caret(caret);
            }
            state.dirty = true;
            return true;
        }
        if key == Key::Space {
            move_n(state, Key::ArrowRight, false);
            return true;
        }
        if key == Key::Backspace {
            move_n(state, Key::ArrowLeft, false);
            return true;
        }
        if key == Key::Delete {
            if let Some(doc) = state.ws.active_doc_mut() {
                doc.delete_forward();
            }
            state.find.dirty = true;
            state.dirty = true;
            return true;
        }
        if key == Key::Enter {
            move_n(state, Key::ArrowDown, false);
            return true;
        }
        if let Some(pending) = state.vim.pending.take() {
            if pending == 'd' {
                if lower == Some('d') {
                    let count = state.vim.count.max(1);
                    state.vim.count = 0;
                    let mut yanked = String::new();
                    if let Some(doc) = state.ws.active_doc_mut() {
                        for _ in 0..count {
                            if let Some(text) = doc.cut_current_line() {
                                yanked.push_str(&text);
                            }
                        }
                    }
                    if !yanked.is_empty() {
                        state.set_clipboard(&yanked);
                    }
                    state.find.dirty = true;
                    state.dirty = true;
                    return true;
                }
                if lower == Some('w') {
                    if let Some(doc) = state.ws.active_doc_mut() {
                        doc.delete_word_forward();
                    }
                    state.vim.count = 0;
                    state.dirty = true;
                    return true;
                }
                state.vim.count = 0;
                return true;
            }
            if pending == 'y' {
                if lower == Some('y') {
                    let count = state.vim.count.max(1);
                    state.vim.count = 0;
                    if let Some(doc) = state.ws.active_doc() {
                        if let Some(text) = doc.current_line_text() {
                            state.set_clipboard(&text);
                        }
                    }
                    let _ = count;
                    state.dirty = true;
                    return true;
                }
                state.vim.count = 0;
                return true;
            }
            if pending == 'g' {
                if lower == Some('g') {
                    let count = state.vim.count.max(1);
                    state.vim.count = 0;
                    if let Some(doc) = state.ws.active_doc_mut() {
                        let _ = count;
                        doc.move_doc_start(false);
                    }
                    state.editor.reveal = true;
                    state.dirty = true;
                    return true;
                }
                state.vim.count = 0;
                return true;
            }
        }

        match lower {
            Some('i') if !upper => {
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('a') if !upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.move_right(false, false);
                }
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('i') if upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    let line = doc.line_of(doc.caret());
                    let start = doc.line_start(line);
                    doc.set_caret(start);
                }
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('a') if upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.move_end(false);
                }
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('o') if !upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.move_end(false);
                    doc.insert_newline();
                }
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('o') if upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    let line = doc.line_of(doc.caret());
                    let start = doc.line_start(line);
                    doc.set_caret(start);
                    doc.insert_newline();
                }
                state.vim.enter_insert();
                state.dirty = true;
                true
            }
            Some('h') => move_n(state, Key::ArrowLeft, false),
            Some('l') => move_n(state, Key::ArrowRight, false),
            Some('j') => move_n(state, Key::ArrowDown, false),
            Some('k') => move_n(state, Key::ArrowUp, false),
            Some('w') => move_n(state, Key::ArrowRight, true),
            Some('b') => move_n(state, Key::ArrowLeft, true),
            Some('0') => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    let line = doc.line_of(doc.caret());
                    let start = doc.line_start(line);
                    doc.set_caret(start);
                }
                state.vim.count = 0;
                state.dirty = true;
                true
            }
            Some('$') => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.move_end(false);
                }
                state.vim.count = 0;
                state.dirty = true;
                true
            }
            Some('g') if upper => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.move_doc_end(false);
                }
                state.vim.count = 0;
                state.editor.reveal = true;
                state.dirty = true;
                true
            }
            Some('g') => {
                state.vim.pending = Some('g');
                state.dirty = true;
                true
            }
            Some('x') => {
                let count = state.vim.count.max(1);
                if let Some(doc) = state.ws.active_doc_mut() {
                    for _ in 0..count {
                        doc.delete_forward();
                    }
                }
                state.vim.count = 0;
                state.find.dirty = true;
                state.dirty = true;
                true
            }
            Some('d') => {
                state.vim.pending = Some('d');
                state.dirty = true;
                true
            }
            Some('y') => {
                state.vim.pending = Some('y');
                state.dirty = true;
                true
            }
            Some('p') => {
                if let Some(text) = state.get_clipboard() {
                    if let Some(doc) = state.ws.active_doc_mut() {
                        doc.insert_text(&text);
                    }
                }
                state.vim.count = 0;
                state.find.dirty = true;
                state.dirty = true;
                true
            }
            Some('u') => {
                let count = state.vim.count.max(1);
                if let Some(doc) = state.ws.active_doc_mut() {
                    for _ in 0..count {
                        doc.undo();
                    }
                }
                state.vim.count = 0;
                state.find.dirty = true;
                state.dirty = true;
                true
            }
            Some('v') if upper => {
                state.vim.mode = VimMode::VisualLine;
                state.vim.count = 0;
                if let Some(doc) = state.ws.active_doc_mut() {
                    let caret = doc.caret();
                    doc.select_line_at(caret);
                }
                state.dirty = true;
                true
            }
            Some('v') => {
                state.vim.mode = VimMode::Visual;
                state.vim.count = 0;
                if let Some(doc) = state.ws.active_doc_mut() {
                    let caret = doc.caret();
                    doc.set_caret(caret);
                }
                state.dirty = true;
                true
            }
            _ => {
                state.vim.count = 0;
                false
            }
        }
    } else {
        // Visual / VisualLine.
        if key == Key::Escape {
            state.vim.mode = VimMode::Normal;
            state.vim.count = 0;
            if let Some(doc) = state.ws.active_doc_mut() {
                let caret = doc.caret();
                doc.set_caret(caret);
            }
            state.dirty = true;
            return true;
        }
        match lower {
            Some('h') => visual_move(state, Key::ArrowLeft),
            Some('l') => visual_move(state, Key::ArrowRight),
            Some('j') => visual_move(state, Key::ArrowDown),
            Some('k') => visual_move(state, Key::ArrowUp),
            Some('w') => visual_move_word(state, true),
            Some('b') => visual_move_word(state, false),
            Some('d') | Some('x') => {
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.delete_selection();
                }
                state.vim.mode = VimMode::Normal;
                state.find.dirty = true;
                state.dirty = true;
                true
            }
            Some('y') => {
                state.vim.mode = VimMode::Normal;
                if let Some(text) = state.selected_text() {
                    state.set_clipboard(&text);
                }
                state.dirty = true;
                true
            }
            _ => {
                state.vim.count = 0;
                false
            }
        }
    }
}

fn shifts_uppercase(key: Key, modifiers: Modifiers) -> bool {
    modifiers.shift || matches!(key, Key::Character(ch) if ch.is_ascii_uppercase())
}

fn move_n(state: &mut AppState, key: Key, word: bool) -> bool {
    let count = state.vim.count.max(1);
    state.vim.count = 0;
    if let Some(doc) = state.ws.active_doc_mut() {
        for _ in 0..count {
            match key {
                Key::ArrowLeft => doc.move_left(false, word),
                Key::ArrowRight => doc.move_right(false, word),
                Key::ArrowUp => doc.move_up(false),
                Key::ArrowDown => doc.move_down(false),
                _ => {}
            }
        }
    }
    state.editor.reveal = true;
    state.dirty = true;
    true
}

fn visual_move(state: &mut AppState, key: Key) -> bool {
    if let Some(doc) = state.ws.active_doc_mut() {
        match key {
            Key::ArrowLeft => doc.move_left(true, false),
            Key::ArrowRight => doc.move_right(true, false),
            Key::ArrowUp => doc.move_up(true),
            Key::ArrowDown => doc.move_down(true),
            _ => {}
        }
    }
    state.dirty = true;
    true
}

fn visual_move_word(state: &mut AppState, forward: bool) -> bool {
    if let Some(doc) = state.ws.active_doc_mut() {
        if forward {
            doc.move_right(true, true);
        } else {
            doc.move_left(true, true);
        }
    }
    state.dirty = true;
    true
}
