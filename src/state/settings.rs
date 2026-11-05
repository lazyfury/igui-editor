//! Editor settings, language overrides and analysis.

use super::*;

impl AppState {
    // -- settings --------------------------------------------------------

    /// Editor typography derived from the current settings.
    pub fn editor_metrics(&self) -> EditorMetrics {
        self.settings.editor()
    }

    /// The override for a document (path first, then untitled id).
    pub fn language_override_for(&self, doc: &Document) -> Option<&str> {
        doc.path()
            .and_then(|path| self.language_by_path.get(path))
            .or_else(|| self.language_by_id.get(&doc.id()))
            .map(String::as_str)
    }

    /// The active document's override, if any.
    pub fn language_override_of_active(&self) -> Option<&str> {
        self.ws
            .active_doc()
            .and_then(|doc| self.language_override_for(doc))
    }

    /// Provider for the active document (override, then extension).
    pub(crate) fn comment_marker(&self) -> Option<&'static str> {
        self.language_of_active()
            .and_then(|provider| provider.line_comment())
    }

    pub fn language_of_active(&self) -> Option<Arc<dyn SyntaxProvider>> {
        let doc = self.ws.active_doc()?;
        self.language_override_for(doc)
            .and_then(|name| self.syntax.by_name(name))
            .or_else(|| self.syntax.for_path(doc.path()))
    }

    /// Provider name for the active document (shown in the status bar), even
    /// when highlighting is disabled by a size limit.
    pub fn language_name(&self) -> Option<&'static str> {
        self.language_of_active().map(|provider| provider.name())
    }

    /// Indentation for the active document: the file's own style, else the
    /// editor setting (spaces by default).
    pub(crate) fn active_indent_style(&self) -> crate::model::doc::IndentStyle {
        self.ws
            .active_doc()
            .and_then(Document::detected_indent)
            .unwrap_or_else(|| self.settings.indent_style())
    }

    /// Sets (or clears with `None`) the active document's language override.
    pub fn set_language(&mut self, name: Option<String>) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if let Some(name) = &name {
            if self.syntax.by_name(name).is_none() {
                self.error(format!("unknown language: {name}"));
                return;
            }
        }
        let id = doc.id();
        let path = doc.path().map(Path::to_path_buf);
        match (path, name) {
            (Some(path), Some(name)) => {
                self.language_by_path.insert(path, name.clone());
                self.status(format!("language: {name}"));
            }
            (Some(path), None) => {
                self.language_by_path.remove(&path);
                self.status("language: auto");
            }
            (None, Some(name)) => {
                self.language_by_id.insert(id, name.clone());
                self.status(format!("language: {name}"));
            }
            (None, None) => {
                self.language_by_id.remove(&id);
                self.status("language: auto");
            }
        }
        self.sync_languages_session();
        self.highlight.reset();
        self.find.dirty = true;
        self.dirty = true;
    }

    pub(crate) fn sync_languages_session(&mut self) {
        let mut entries: Vec<(PathBuf, String)> = self
            .language_by_path
            .iter()
            .map(|(path, name)| (path.clone(), name.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.session.languages = entries;
        self.config_dirty = true;
    }

    /// Keeps the editor and preview in step while in Split view. One side is
    /// authoritative per frame (whichever changed), so the pair never loops.
    pub fn sync_scroll_positions(&mut self) {
        let split = self.view_of_active() == ViewMode::Split;
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let rev = doc.rev();
        let line_h = self.editor_metrics().line_h.max(1.0);
        let editor_scroll = doc.scroll_y;
        let preview_scroll = self.preview.scroll;
        let editor_changed = (editor_scroll - self.last_editor_scroll).abs() > 0.5;
        let preview_changed = (preview_scroll - self.last_preview_scroll).abs() > 0.5;
        // Most frames neither pane moved; do not touch the sync state (and do
        // not mark the frame dirty) in that case.
        if !(split && self.sync_scroll && rev == self.sync_rev)
            || !(editor_changed || preview_changed)
        {
            self.last_editor_scroll = editor_scroll;
            self.last_preview_scroll = preview_scroll;
            self.sync_rev = rev;
            return;
        }
        let anchors = self.preview.anchors.clone();
        if anchors.len() < 2 {
            self.last_editor_scroll = editor_scroll;
            self.last_preview_scroll = preview_scroll;
            return;
        }
        let soft_wrap = self.session.soft_wrap
            && !self.editor.wrap.is_empty()
            && self.editor.wrap.key().is_some();
        let editor_line = if soft_wrap {
            let row = (editor_scroll / line_h).floor() as usize;
            self.editor
                .wrap
                .line_of_row(row.min(self.editor.wrap.len().saturating_sub(1)))
                .unwrap_or(0) as f32
        } else {
            editor_scroll / line_h
        };
        if editor_changed && !preview_changed {
            self.preview.scroll = preview_y_for_line(&anchors, editor_line);
        } else if preview_changed && !editor_changed {
            let line = line_for_preview_y(&anchors, preview_scroll);
            if let Some(doc) = self.ws.active_doc_mut() {
                if soft_wrap {
                    let row = self.editor.wrap.first_row_of_line(line.floor() as usize);
                    doc.scroll_y = row as f32 * line_h;
                } else {
                    doc.scroll_y = line * line_h;
                }
            }
        }
        self.last_editor_scroll = self
            .ws
            .active_doc()
            .map_or(editor_scroll, |doc| doc.scroll_y);
        self.last_preview_scroll = self.preview.scroll;
        self.sync_rev = rev;
        self.dirty = true;
    }

    /// Enables/disables soft wrap, refusing files outside the hard limits.
    pub fn set_soft_wrap(&mut self, on: bool) {
        let was_on = self.session.soft_wrap;
        if on && !was_on {
            if let Some(doc) = self.ws.active_doc() {
                self.editor.wrap_scroll_x = doc.scroll_x;
            }
        }
        if !on && was_on {
            if let Some(doc) = self.ws.active_doc_mut() {
                doc.scroll_x = self.editor.wrap_scroll_x;
            }
        }
        if on {
            if let Some(doc) = self.ws.active_doc() {
                if doc.text().len() > crate::model::wrap::MAX_WRAP_BYTES
                    || doc.line_count() > crate::model::wrap::MAX_WRAP_LINES
                {
                    self.error(format!(
                        "soft wrap is off above {} MB / {}k lines",
                        crate::model::wrap::MAX_WRAP_BYTES / (1024 * 1024),
                        crate::model::wrap::MAX_WRAP_LINES / 1000
                    ));
                    return;
                }
            }
        }
        if self.session.soft_wrap != on {
            self.session.soft_wrap = on;
            self.config_dirty = true;
            self.editor.reveal = on;
            self.status(if on { "soft wrap on" } else { "soft wrap off" });
            self.dirty = true;
        }
    }

    pub fn toggle_sync_scroll(&mut self) {
        self.sync_scroll = !self.sync_scroll;
        self.session.sync_scroll = self.sync_scroll;
        self.config_dirty = true;
        self.status(format!(
            "scroll sync: {}",
            if self.sync_scroll { "ON" } else { "OFF" }
        ));
        self.dirty = true;
    }

    pub fn open_settings(&mut self) {
        self.settings_open = true;
        self.quick_open.open = false;
        if self.search.open {
            self.close_project_search();
        }
        self.menu_open = None;
        self.settings_row = self.settings_row.min(Row::Reset as usize);
        self.dirty = true;
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
        self.dirty = true;
    }

    pub fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    /// Display value of a settings row.
    pub fn settings_value(&self, row: Row) -> String {
        match row {
            Row::Theme => self.theme.name().to_string(),
            Row::FontSize => format!("{} px", self.settings.font_size as i32),
            Row::IndentStyle => {
                if self.settings.indent_tabs {
                    "TABS".into()
                } else {
                    "SPACES".into()
                }
            }
            Row::TabWidth => format!("{}", self.settings.tab_width),
            Row::LineNumbers => on_off(self.settings.line_numbers),
            Row::IndentGuides => on_off(self.settings.indent_guides),
            Row::AutoPairs => on_off(self.settings.auto_pairs),
            Row::Syntax => on_off(self.settings.syntax),
            Row::Autosave => on_off(self.settings.autosave),
            Row::ImagePreview => on_off(self.settings.image_preview),
            Row::LanguageIntelligence => on_off(self.settings.lsp),
            Row::TrustLocalServers => on_off(self.settings.trust_local_servers),
            Row::SidebarWidth => format!("{} px", self.settings.sidebar_width as i32),
            Row::HiddenFiles => on_off(self.settings.show_hidden),
            Row::IgnoredFiles => self.settings.ignored_files.label().into(),
            Row::Keymap => self.settings.keymap.name().into(),
            Row::Reset => "DEFAULT".into(),
        }
    }

    /// Steps a row (`delta` is -1 or +1). Returns whether anything changed.
    pub fn adjust_setting(&mut self, row: Row, delta: i32) -> bool {
        match row {
            Row::Theme => {
                if delta != 0 {
                    self.theme.toggle();
                    self.settings_changed();
                }
            }
            Row::FontSize => {
                self.settings.step_font(delta);
                self.settings_changed();
            }
            Row::IndentStyle => {
                self.settings.indent_tabs = !self.settings.indent_tabs;
                self.settings_changed();
                self.status(format!(
                    "indent: {}",
                    if self.settings.indent_tabs {
                        "TABS"
                    } else {
                        "SPACES"
                    }
                ));
            }
            Row::TabWidth => {
                self.settings.step_tab(delta);
                self.settings_changed();
            }
            Row::LineNumbers => {
                self.settings.line_numbers = !self.settings.line_numbers;
                self.settings_changed();
            }
            Row::IndentGuides => {
                self.settings.indent_guides = !self.settings.indent_guides;
                self.settings_changed();
            }
            Row::AutoPairs => {
                self.settings.auto_pairs = !self.settings.auto_pairs;
                self.settings_changed();
                self.status(format!(
                    "auto pairs: {}",
                    if self.settings.auto_pairs {
                        "ON"
                    } else {
                        "OFF"
                    }
                ));
            }
            Row::Syntax => {
                self.settings.syntax = !self.settings.syntax;
                self.settings_changed();
            }
            Row::Autosave => {
                self.settings.autosave = !self.settings.autosave;
                self.settings_changed();
            }
            Row::ImagePreview => {
                self.settings.image_preview = !self.settings.image_preview;
                self.settings_changed();
                self.status(format!(
                    "image preview: {}",
                    if self.settings.image_preview {
                        "ON"
                    } else {
                        "OFF"
                    }
                ));
            }
            Row::LanguageIntelligence => {
                self.set_lsp_enabled(!self.settings.lsp);
            }
            Row::TrustLocalServers => {
                self.settings.trust_local_servers = !self.settings.trust_local_servers;
                self.settings_changed();
                self.status(format!(
                    "local language servers: {}",
                    if self.settings.trust_local_servers {
                        "TRUSTED"
                    } else {
                        "BLOCKED"
                    }
                ));
            }
            Row::SidebarWidth => {
                self.settings.step_sidebar(delta as f32 * 8.0);
                self.settings_changed();
                self.pending_sidebar_sync = true;
            }
            Row::HiddenFiles => {
                self.settings.show_hidden = !self.settings.show_hidden;
                self.settings_changed();
                self.apply_explorer_filter();
                self.status(format!(
                    "hidden files: {}",
                    if self.settings.show_hidden {
                        "ON"
                    } else {
                        "OFF"
                    }
                ));
            }
            Row::IgnoredFiles => {
                self.settings.ignored_files = if delta < 0 {
                    self.settings.ignored_files.previous()
                } else {
                    self.settings.ignored_files.next()
                };
                self.settings_changed();
                self.apply_explorer_filter();
                self.status(format!(
                    "ignored files: {}",
                    self.settings.ignored_files.label()
                ));
            }
            Row::Keymap => {
                if delta != 0 {
                    self.settings.keymap = if delta > 0 {
                        self.settings.keymap.next()
                    } else {
                        let mut previous = self.settings.keymap;
                        for _ in 0..KeymapId::ALL.len() - 1 {
                            previous = previous.next();
                        }
                        previous
                    };
                    self.rebuild_keymap();
                    self.settings_changed();
                    self.status(format!("keymap: {}", self.settings.keymap.name()));
                }
            }
            Row::Reset => self.reset_settings(),
        }
        self.dirty = true;
        true
    }

    /// Enter/Space on a row: toggles booleans, resets, or steps other rows.
    pub fn activate_setting(&mut self, row: Row) {
        self.adjust_setting(row, 1);
    }

    pub fn reset_settings(&mut self) {
        self.settings = Settings::default();
        self.rebuild_keymap();
        self.settings_changed();
        self.apply_explorer_filter();
        self.pending_sidebar_sync = true;
        self.status("settings reset");
    }

    /// Pushes the Explorer visibility settings into the sidebar tree. The
    /// filter is a display rule only: watches and language servers still see
    /// every file. Returns whether the visible rows changed.
    pub(crate) fn apply_explorer_filter(&mut self) -> bool {
        let changed = self.ws.tree.set_filter(crate::model::fs_tree::Filter {
            show_hidden: self.settings.show_hidden,
            ignore: self.settings.ignored_files,
        });
        if changed {
            self.sidebar.hover = None;
            self.dirty = true;
        }
        changed
    }

    /// Writes the current settings into the session (call after a live drag).
    pub fn persist_settings(&mut self) {
        self.settings_changed();
    }

    pub(crate) fn settings_changed(&mut self) {
        self.session.font = self.settings.font_size;
        self.session.tab = self.settings.tab_width;
        self.session.indent_tabs = self.settings.indent_tabs;
        self.session.line_numbers = self.settings.line_numbers;
        self.session.indent_guides = self.settings.indent_guides;
        self.session.auto_pairs = self.settings.auto_pairs;
        self.session.syntax = self.settings.syntax;
        self.session.autosave = self.settings.autosave;
        self.session.image_preview = self.settings.image_preview;
        self.session.lsp = self.settings.lsp;
        self.session.trust_local_servers = self.settings.trust_local_servers;
        self.session.sidebar_width = self.settings.sidebar_width;
        self.session.show_hidden = self.settings.show_hidden;
        self.session.ignored_files = self.settings.ignored_files.session_value().into();
        self.session.keymap = self.settings.keymap.session_value().into();
        self.session.theme = match self.theme.mode {
            Mode::Dark => "dark".into(),
            Mode::Light => "light".into(),
        };
        self.config_dirty = true;
        self.find.dirty = true;
        self.editor.reveal = true;
        self.dirty = true;
    }

    /// Applies settings-driven work for this frame: incremental highlighting and
    /// bracket matching. Returns whether more highlight work remains.
    pub fn tick_analysis(&mut self) -> bool {
        if self.pending_sidebar_sync {
            self.pending_sidebar_sync = false;
        }
        let language_override = self.language_override_of_active().map(str::to_string);
        let behind = {
            let AppState {
                ws,
                settings,
                syntax,
                highlight,
                ..
            } = self;
            match ws.active_doc() {
                Some(doc) => {
                    // A budgeted pass changes the painted tokens without
                    // changing the document revision; repaint for both the
                    // running and the completing frame.
                    let was_behind = highlight.is_behind();
                    highlight.sync(
                        doc,
                        syntax,
                        settings,
                        crate::model::highlight::FRAME_LINE_BUDGET,
                        language_override.as_deref(),
                    );
                    let behind = highlight.is_behind();
                    behind || was_behind
                }
                None => {
                    highlight.reset();
                    false
                }
            }
        };
        if behind {
            self.dirty = true;
        }

        let key = self.ws.active_doc().map(|doc| {
            (
                doc.id(),
                doc.rev(),
                doc.caret(),
                self.settings.syntax && self.highlight.is_active(),
            )
        });
        if key != self.bracket_key {
            self.bracket_key = key;
            self.bracket = self.ws.active_doc().and_then(|doc| {
                match_bracket(doc.text(), doc.caret(), |byte| self.code_at(doc, byte))
            });
        }
        behind
    }

    /// Whether `byte` in `doc` is code rather than comment or string content.
    ///
    /// Bracket matching and quote pairing both use this so a bracket inside a
    /// literal never pairs and never matches. Lines the incremental highlight
    /// cache has not reached yet count as code.
    pub(crate) fn code_at(&self, doc: &Document, byte: usize) -> bool {
        let line = doc.line_of(byte);
        let offset = byte - doc.line_start(line);
        match self.highlight.tokens(line) {
            Some(tokens) => !tokens.iter().any(|token| {
                token.start <= offset
                    && offset < token.end
                    && matches!(token.kind, TokenKind::Comment | TokenKind::String)
            }),
            None => true,
        }
    }

    /// What typing `ch` should do in the active document.
    ///
    /// The code probe looks at the character *before* the caret: the highlight
    /// cache has not seen the character being typed yet, so a quote typed at the
    /// end of an open string literal must still count as string content.
    pub(crate) fn typed_insert(&self, ch: char) -> typing::Insert {
        let Some(doc) = self.ws.active_doc() else {
            return typing::Insert::Plain;
        };
        let caret = doc.caret();
        let line_start = doc.line_start(doc.line_of(caret));
        let probe = doc.text()[line_start..caret]
            .chars()
            .next_back()
            .map(|previous| caret - previous.len_utf8())
            .unwrap_or(caret);
        typing::decide(
            doc.text(),
            caret,
            doc.anchor(),
            ch,
            typing::Context {
                in_code: self.code_at(doc, probe),
            },
        )
    }
}
