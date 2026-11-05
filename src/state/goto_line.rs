//! Go to Line prompt state (`⌃G` / `⌘L`).
//!
//! Opening prefills the caret line and selects it so typing replaces the
//! number. Submitting validates `1..=line_count`, moves the caret to the line
//! start, clears the selection, reveals the line and closes the prompt; an
//! invalid entry stays open with an inline error. Text and IME input are
//! filtered to ASCII digits here so the field can never hold anything else.

use super::*;

impl AppState {
    /// Opens the Go to Line prompt.
    pub fn open_goto_line(&mut self) {
        if self.goto_line.open
            || self.new_file.open
            || self.modal.is_some()
            || self.recovery_prompt.is_some()
        {
            return;
        }
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let line = doc.line_col().0 + 1;
        self.settings_open = false;
        self.quick_open.open = false;
        self.lsp.dismiss_transient();
        if self.search.open {
            self.close_project_search();
        }
        self.close_find();
        self.menu_open = None;
        self.tab_menu = None;
        self.tree_menu = None;
        self.goto_line.field.set_text(line.to_string());
        self.goto_line.field.select_all();
        self.goto_line.error = None;
        self.goto_line.preedit = None;
        self.goto_line.open = true;
        self.focus = Focus::Editor;
        self.terminal.focused = false;
        self.dirty = true;
    }

    /// Closes the prompt without moving the caret.
    pub fn close_goto_line(&mut self) {
        self.goto_line.open = false;
        self.goto_line.error = None;
        self.goto_line.preedit = None;
        self.dirty = true;
    }

    /// Jumps to the entered line. Invalid input keeps the prompt open.
    pub fn submit_goto_line(&mut self) {
        let Some(doc) = self.ws.active_doc() else {
            self.close_goto_line();
            return;
        };
        let count = doc.line_count();
        let text = self.goto_line.field.text();
        let parsed = if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
            text.parse::<usize>().ok()
        } else {
            None
        };
        let Some(line) = parsed.filter(|line| (1..=count).contains(line)) else {
            self.goto_line.error = Some(format!("line must be 1..={count}"));
            self.dirty = true;
            return;
        };
        let start = doc.line_start(line - 1);
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_caret(start);
        }
        self.editor.reveal = true;
        self.find.dirty = true;
        self.goto_line.open = false;
        self.goto_line.error = None;
        self.goto_line.preedit = None;
        self.status(format!("line {line}"));
        self.dirty = true;
    }

    /// Inserts the ASCII digits from `text` (text input and IME commits).
    /// Returns whether the field changed.
    pub fn type_goto_line(&mut self, text: &str) -> bool {
        if !self.goto_line.open {
            return false;
        }
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return false;
        }
        self.goto_line.field.insert(&digits);
        self.goto_line.error = None;
        self.goto_line.preedit = None;
        self.dirty = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use igui_core::Modifiers;

    fn state_with(label: &str, text: &str) -> (AppState, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "igui-editor-goto-line-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            Workspace::new(&base),
            Session::default(),
            base.join("session.conf"),
        );
        state.ws.new_tab();
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.insert_text(text);
        }
        (state, base)
    }

    #[test]
    fn opening_prefills_and_selects_the_caret_line() {
        let (mut state, base) = state_with("prefill", "one\ntwo\nthree\nfour");
        if let Some(doc) = state.ws.active_doc_mut() {
            let start = doc.line_start(2);
            doc.set_caret(start);
        }
        state.open_goto_line();
        assert!(state.goto_line.open);
        assert_eq!(state.goto_line.field.text(), "3");
        assert_eq!(state.goto_line.field.selected_text(), Some("3"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn submitting_a_valid_line_moves_the_caret_and_closes() {
        let (mut state, base) = state_with("submit", "one\ntwo\nthree\nfour");
        state.open_goto_line();
        state.goto_line.field.set_text("4");
        state.submit_goto_line();
        let doc = state.ws.active_doc().unwrap();
        assert_eq!(doc.line_col().0, 3);
        assert_eq!(doc.caret(), doc.line_start(3));
        assert!(!doc.has_selection());
        assert!(!state.goto_line.open);
        assert_eq!(state.message(), Some(("line 4", false)));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn invalid_input_keeps_the_prompt_open_with_an_error() {
        let (mut state, base) = state_with("invalid", "one\ntwo\nthree\nfour");
        state.open_goto_line();
        let before = state.ws.active_doc().unwrap().caret();
        for value in ["0", "5", "abc"] {
            state.goto_line.field.set_text(value);
            state.submit_goto_line();
            assert!(state.goto_line.open, "{value} closed the prompt");
            assert_eq!(
                state.goto_line.error.as_deref(),
                Some("line must be 1..=4"),
                "{value}"
            );
            assert_eq!(state.ws.active_doc().unwrap().caret(), before);
        }
        // Editing clears the inline error without jumping yet.
        assert!(state.type_goto_line("2"));
        assert!(state.goto_line.error.is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn escape_closes_without_moving() {
        let (mut state, base) = state_with("escape", "one\ntwo\nthree\nfour");
        if let Some(doc) = state.ws.active_doc_mut() {
            let start = doc.line_start(1);
            doc.set_caret(start);
        }
        state.open_goto_line();
        let before = state.ws.active_doc().unwrap().caret();
        assert!(crate::ui::goto_line::on_key(
            &mut state,
            Key::Escape,
            Modifiers::NONE
        ));
        assert!(!state.goto_line.open);
        assert_eq!(state.ws.active_doc().unwrap().caret(), before);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn text_and_ime_input_keep_only_digits() {
        let (mut state, base) = state_with("digits", "one\ntwo\nthree\nfour");
        state.open_goto_line();
        state.goto_line.field.set_text("");
        assert!(state.type_goto_line("1a2"));
        assert_eq!(state.goto_line.field.text(), "12");
        crate::ui::goto_line::on_text(&mut state, " 3x");
        assert_eq!(state.goto_line.field.text(), "123");
        crate::ui::goto_line::on_ime(&mut state, &igui_core::ImeEvent::Commit("4z".into()));
        assert_eq!(state.goto_line.field.text(), "1234");
        assert_eq!(state.goto_line.preedit, None);
        assert!(state.goto_line.error.is_none());
        let _ = std::fs::remove_dir_all(base);
    }
}
