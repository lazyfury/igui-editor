//! Command dispatch and clipboard/session helpers.

use super::*;

impl AppState {
    // -- actions ---------------------------------------------------------

    /// Runs `action` and reports the outcome in the status bar.
    pub fn run(&mut self, action: Action) {
        self.tab_menu = None;
        // Clipboard and history belong to the focused surface. This keeps
        // Cmd/Ctrl+C/V/A/Z from silently editing the document while the
        // terminal owns input.
        if self.focus == Focus::Terminal {
            match &action {
                Action::Paste => {
                    match self.get_clipboard() {
                        Some(text) => {
                            self.terminal_write(text.as_bytes());
                            self.status("pasted into terminal");
                        }
                        None => self.error("clipboard is empty"),
                    }
                    return;
                }
                Action::Copy => {
                    match self.terminal_selection_text() {
                        Some(text) if !text.is_empty() => {
                            self.set_clipboard(&text);
                            self.status("copied terminal selection");
                        }
                        Some(_) => self.status("terminal selection is empty"),
                        None => self.status("no terminal selection"),
                    }
                    return;
                }
                Action::SelectAll => {
                    self.terminal_select_all();
                    self.status("selected the terminal screen");
                    return;
                }
                Action::Cut => {
                    self.status("terminal cut is not implemented");
                    return;
                }
                Action::Undo | Action::Redo => {
                    self.status("undo/redo is editor-only while the terminal has focus");
                    return;
                }
                _ => {}
            }
        }
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only())
            && matches!(
                action,
                Action::Save
                    | Action::SaveAs
                    | Action::Cut
                    | Action::Paste
                    | Action::Undo
                    | Action::Redo
                    | Action::DeleteLine
                    | Action::DuplicateLine
                    | Action::MoveLineUp
                    | Action::MoveLineDown
                    | Action::DeleteWordBackward
                    | Action::DeleteWordForward
                    | Action::ToggleComment
            )
        {
            self.status("read-only Git view");
            return;
        }
        match action {
            Action::New => {
                self.ws.new_tab();
                self.tabs.reveal_active = true;
                self.sync_layout();
                self.sync_session_tabs();
                self.find.dirty = true;
                self.focus = Focus::Editor;
                self.status("new buffer");
            }
            Action::NewFile => self.open_new_file(),
            Action::Open => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::Open,
                    then_close: None,
                });
            }
            Action::OpenFolder => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::OpenFolder,
                    then_close: None,
                });
            }
            Action::CloseFolder => self.close_folder(),
            Action::Save => self.save_active(),
            Action::SaveAs => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::SaveAs,
                    then_close: None,
                });
            }
            Action::CloseTab => self.request_close_active(),
            Action::Undo => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.undo();
                }
                self.find.dirty = true;
            }
            Action::Redo => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.redo();
                }
                self.find.dirty = true;
            }
            Action::Cut => {
                if let Some(text) = self.selected_text() {
                    self.set_clipboard(&text);
                    if let Some(doc) = self.ws.active_doc_mut() {
                        doc.delete_selection();
                    }
                    self.find.dirty = true;
                } else if let Some(text) = self
                    .ws
                    .active_doc_mut()
                    .and_then(Document::cut_current_line)
                {
                    self.set_clipboard(&text);
                    self.find.dirty = true;
                    self.status("cut line");
                }
            }
            Action::Copy => {
                if let Some(text) = self.selected_text() {
                    self.set_clipboard(&text);
                    self.status("copied");
                } else if let Some(text) =
                    self.ws.active_doc().and_then(Document::current_line_text)
                {
                    self.set_clipboard(&text);
                    self.status("copied line");
                }
            }
            Action::Paste => match self.get_clipboard() {
                Some(text) => {
                    if let Some(doc) = self.ws.active_doc_mut() {
                        doc.insert_text(&text);
                    }
                    self.find.dirty = true;
                }
                None => self.error("clipboard is empty"),
            },
            Action::SelectAll => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.select_all();
                }
            }
            Action::SelectNextOccurrence => {
                let found = self
                    .ws
                    .active_doc_mut()
                    .is_some_and(|doc| doc.select_next_occurrence());
                if !found {
                    self.status("no more occurrences");
                }
                self.find.dirty = true;
            }
            Action::Find => self.open_find(false),
            Action::FindReplace => {
                if self.find.open {
                    self.focus = Focus::Replace;
                    self.dirty = true;
                } else {
                    self.open_find(true);
                }
            }
            Action::FindNext => {
                if self.find.open {
                    self.find_step(1);
                } else {
                    self.open_find(false);
                }
            }
            Action::FindPrevious => {
                if self.find.open {
                    self.find_step(-1);
                } else {
                    self.open_find(false);
                }
            }
            Action::CycleView => {
                let next = self.view_of_active().next();
                self.set_view(next);
            }
            Action::DeleteLine => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.cut_current_line();
                }
                self.find.dirty = true;
            }
            Action::DuplicateLine => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.duplicate_line();
                }
                self.find.dirty = true;
            }
            Action::MoveLineUp => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_line_up();
                }
                self.find.dirty = true;
            }
            Action::MoveLineDown => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_line_down();
                }
                self.find.dirty = true;
            }
            Action::DeleteWordBackward => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.delete_word_backward();
                }
                self.find.dirty = true;
            }
            Action::DeleteWordForward => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.delete_word_forward();
                }
                self.find.dirty = true;
            }
            Action::MoveLineStart(select) => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_home(select);
                }
                self.editor.reveal = true;
                self.find.dirty = true;
            }
            Action::MoveLineEnd(select) => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_end(select);
                }
                self.editor.reveal = true;
                self.find.dirty = true;
            }
            Action::MoveDocStart(select) => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_doc_start(select);
                }
                self.editor.reveal = true;
                self.find.dirty = true;
            }
            Action::MoveDocEnd(select) => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.move_doc_end(select);
                }
                self.editor.reveal = true;
                self.find.dirty = true;
            }
            Action::ToggleComment => {
                if let Some(marker) = self.comment_marker() {
                    if let Some(doc) = self.ws.active_doc_mut() {
                        doc.toggle_line_comment(marker);
                    }
                    self.find.dirty = true;
                } else {
                    self.status("no line-comment marker for this document");
                }
            }
            Action::FormatDocument => self.format_active_document(),
            Action::QuickOpen => self.open_quick_open(),
            Action::ProjectSearch => self.open_project_search(),
            Action::GotoLine => self.open_goto_line(),
            Action::LspHover => self.request_lsp_hover(),
            Action::LspCompletion => self.request_lsp_completion(),
            Action::LspDefinition => self.request_lsp_definition(),
            Action::LspReferences => self.request_lsp_references(),
            Action::LspRename => self.request_lsp_rename(),
            Action::LspRestart => self.restart_language_server(),
            Action::Settings => self.toggle_settings(),
            Action::ToggleSidebar => self.dock_toggle(DockId::Left),
            Action::ToggleOutline => self.dock_toggle(DockId::Right),
            Action::ShowPanel(panel) => self.toggle_panel(panel),
            Action::ToggleTheme => {
                self.theme.toggle();
                self.session.theme = match self.theme.mode {
                    Mode::Dark => "dark".into(),
                    Mode::Light => "light".into(),
                };
                self.config_dirty = true;
                self.status(format!("theme: {}", self.theme.name()));
            }
            Action::SetView(mode) => self.set_view(mode),
            Action::ToggleScrollSync => self.toggle_sync_scroll(),
            Action::ToggleSoftWrap => {
                let on = !self.session.soft_wrap;
                self.set_soft_wrap(on);
            }
            Action::ToggleMinimap => self.toggle_minimap(),
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::SetLanguage(name) => self.set_language(name),
            Action::OpenRecentProject(path) => self.open_project(path),
            Action::OpenRecentFile(path) => self.open_path(path),
            Action::ClearRecent => self.clear_recent(),
            Action::NextTab => {
                let len = self.ws.tabs.len();
                if len > 1 {
                    self.activate_tab((self.ws.active + 1) % len);
                }
            }
            Action::PreviousTab => {
                let len = self.ws.tabs.len();
                if len > 1 {
                    self.activate_tab((self.ws.active + len - 1) % len);
                }
            }
            Action::SelectTab(slot) => {
                let len = self.ws.tabs.len();
                if len > 0 {
                    let index = if slot >= 9 {
                        Some(len - 1)
                    } else {
                        slot.checked_sub(1)
                    };
                    if let Some(index) = index {
                        self.activate_tab(index);
                    }
                }
            }
        }
        self.dirty = true;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.ws
            .active_doc()
            .and_then(|doc| doc.selected_text())
            .map(str::to_string)
    }

    pub fn get_clipboard(&self) -> Option<String> {
        self.clipboard.as_ref()?.borrow().get()
    }

    pub fn set_clipboard(&self, text: &str) {
        if let Some(clipboard) = &self.clipboard {
            clipboard.borrow_mut().set(text);
        }
    }

    /// Flushes the session when something persisted changed.
    pub fn save_session_if_dirty(&mut self) {
        if !self.config_dirty {
            return;
        }
        self.config_dirty = false;
        if let Err(error) = self.session.save(&self.config_path) {
            self.error(format!("session save failed: {error}"));
        }
    }
}
