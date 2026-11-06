//! Editor settings, the settings-panel input state, language overrides and
//! analysis.

use super::*;

/// In-progress typed edit of a numeric row.
pub struct SettingsEdit {
    pub row: Row,
    pub field: TextEdit,
}

/// Settings-panel input state: the search query, the selected row, an optional
/// typed number edit and the scroll offset. Paint records the field carets so
/// the host can place the IME candidate window.
pub struct SettingsUi {
    pub query: TextEdit,
    /// Selected row (never `Reset`, which is a footer button).
    pub row: Row,
    pub editing: Option<SettingsEdit>,
    pub scroll: f32,
    /// In-progress IME composition in the search or edit field.
    pub preedit: Option<String>,
    /// Caret of the search field, in viewport space (IME placement).
    pub search_caret: Rect,
    /// Caret of the typed number field while `editing` is set.
    pub edit_caret: Rect,
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self {
            query: TextEdit::new(""),
            row: Row::Theme,
            editing: None,
            scroll: 0.0,
            preedit: None,
            search_caret: Rect::ZERO,
            edit_caret: Rect::ZERO,
        }
    }
}

impl SettingsUi {
    /// Rows matching the search query, in display order. An empty query lists
    /// every row; the row label and its group name both match.
    pub fn visible_rows(&self) -> Vec<&'static crate::settings::RowSpec> {
        let query = self.query.text().to_ascii_lowercase();
        crate::settings::ROWS
            .iter()
            .filter(|spec| {
                query.is_empty()
                    || spec.row.label().to_ascii_lowercase().contains(&query)
                    || spec.group.label().to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    /// Whether a row is visible under the current query.
    pub fn shows(&self, row: Row) -> bool {
        self.visible_rows().iter().any(|spec| spec.row == row)
    }
}

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

    /// Shows/hides the minimap strip; persisted in the session.
    pub fn toggle_minimap(&mut self) {
        self.session.minimap = !self.session.minimap;
        self.config_dirty = true;
        self.status(format!(
            "minimap: {}",
            if self.session.minimap { "ON" } else { "OFF" }
        ));
        self.dirty = true;
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
        let previous = self.settings_ui.row;
        self.settings_ui = SettingsUi::default();
        if previous.spec().is_some() {
            self.settings_ui.row = previous;
        }
        self.dirty = true;
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
        self.settings_ui.editing = None;
        self.settings_ui.preedit = None;
        self.dirty = true;
    }

    pub fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    /// Rows the search currently matches (the selection domain).
    pub fn settings_visible(&self) -> Vec<&'static crate::settings::RowSpec> {
        self.settings_ui.visible_rows()
    }

    /// Group shown in the right pane: the selected row's group.
    pub fn settings_group(&self) -> Group {
        self.settings_ui
            .row
            .spec()
            .map_or(Group::Appearance, |spec| spec.group)
    }

    /// Rows the right pane lists: the active group, or every search match.
    ///
    /// Selection still walks the search-filtered list across groups, so `↑`/`↓`
    /// crossing a group boundary switches the pane and the left navigation.
    pub fn settings_pane(&self) -> Vec<&'static crate::settings::RowSpec> {
        if self.settings_ui.query.text().is_empty() {
            let group = self.settings_group();
            crate::settings::ROWS
                .iter()
                .filter(|spec| spec.group == group)
                .collect()
        } else {
            self.settings_ui.visible_rows()
        }
    }

    /// Groups in panel order (each exactly once).
    pub fn settings_groups() -> Vec<Group> {
        let mut groups: Vec<Group> = Vec::new();
        for spec in crate::settings::ROWS {
            if groups.last() != Some(&spec.group) {
                groups.push(spec.group);
            }
        }
        groups
    }

    /// Selects the first visible row of a group (left-navigation click).
    pub fn settings_show_group(&mut self, group: Group) {
        if self.settings_editing().is_some() {
            self.settings_commit_edit();
        }
        self.settings_ui.query.set_text("");
        self.settings_ui.preedit = None;
        self.settings_ui.scroll = 0.0;
        if let Some(spec) = crate::settings::ROWS
            .iter()
            .find(|spec| spec.group == group)
        {
            self.settings_ui.row = spec.row;
        }
        self.dirty = true;
    }

    /// Moves the selection to the next/previous group that still has a match.
    pub fn settings_select_group(&mut self, delta: i32) {
        let visible = self.settings_ui.visible_rows();
        let groups = Self::settings_groups();
        if visible.is_empty() || groups.is_empty() || delta == 0 {
            return;
        }
        let current = self.settings_group();
        let index = groups
            .iter()
            .position(|group| *group == current)
            .unwrap_or(0) as i32;
        let count = groups.len() as i32;
        for step in 1..=count {
            let next = (index + delta.signum() * step).rem_euclid(count) as usize;
            if let Some(spec) = visible.iter().find(|spec| spec.group == groups[next]) {
                self.settings_ui.row = spec.row;
                self.settings_ui.scroll = 0.0;
                self.dirty = true;
                return;
            }
        }
    }

    /// Index of a `Choice` row's current value in its option list.
    pub fn settings_choice_index(&self, row: Row) -> Option<usize> {
        Some(match row {
            Row::Theme => match self.theme.mode {
                Mode::Dark => 0,
                Mode::Light => 1,
            },
            Row::IndentStyle => usize::from(self.settings.indent_tabs),
            Row::TabWidth => TAB_WIDTHS
                .iter()
                .position(|width| *width == clamp_tab(self.settings.tab_width))
                .unwrap_or(1),
            Row::IgnoredFiles => IgnoreMode::ALL
                .iter()
                .position(|mode| *mode == self.settings.ignored_files)
                .unwrap_or(1),
            Row::Keymap => KeymapId::ALL
                .iter()
                .position(|keymap| *keymap == self.settings.keymap)
                .unwrap_or(0),
            _ => return None,
        })
    }

    /// Picks one option of a `Choice` row (button-group clicks).
    pub fn settings_set_choice(&mut self, row: Row, index: usize) -> bool {
        let RowKind::Choice(options) = row.kind() else {
            return false;
        };
        let index = index.min(options.len().saturating_sub(1));
        match row {
            Row::Theme => {
                let mode = if index == 0 { Mode::Dark } else { Mode::Light };
                if self.theme.mode != mode {
                    self.theme = Theme::new(mode);
                    self.settings_changed();
                }
            }
            Row::IndentStyle => {
                self.settings.indent_tabs = index == 1;
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
                self.settings.tab_width = TAB_WIDTHS[index];
                self.settings_changed();
            }
            Row::IgnoredFiles => {
                self.settings.ignored_files = IgnoreMode::ALL[index];
                self.settings_changed();
                self.apply_explorer_filter();
                self.status(format!(
                    "ignored files: {}",
                    self.settings.ignored_files.label()
                ));
            }
            Row::Keymap => {
                self.settings.keymap = KeymapId::ALL[index];
                self.rebuild_keymap();
                self.settings_changed();
                self.status(format!("keymap: {}", self.settings.keymap.name()));
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    /// Current numeric value of a `Number` row.
    pub fn settings_number_value(&self, row: Row) -> Option<f32> {
        match row {
            Row::FontSize => Some(self.settings.font_size),
            Row::SidebarWidth => Some(self.settings.sidebar_width),
            _ => None,
        }
    }

    /// Sets a `Number` row from a stepped or typed value (rounded to the row's
    /// step, then clamped).
    pub fn settings_set_number(&mut self, row: Row, value: f32) -> bool {
        let RowKind::Number(spec) = row.kind() else {
            return false;
        };
        let value = (value / spec.step).round() * spec.step;
        let value = value.clamp(spec.min, spec.max);
        match row {
            Row::FontSize => self.settings.font_size = clamp_font(value),
            Row::SidebarWidth => {
                self.settings.sidebar_width = clamp_sidebar(value);
                self.pending_sidebar_sync = true;
            }
            _ => return false,
        }
        self.settings_changed();
        self.dirty = true;
        true
    }

    /// On/off state of a `Toggle` row.
    pub fn settings_toggle_state(&self, row: Row) -> bool {
        match row {
            Row::LineNumbers => self.settings.line_numbers,
            Row::IndentGuides => self.settings.indent_guides,
            Row::AutoPairs => self.settings.auto_pairs,
            Row::Syntax => self.settings.syntax,
            Row::Autosave => self.settings.autosave,
            Row::LanguageIntelligence => self.settings.lsp,
            Row::TrustLocalServers => self.settings.trust_local_servers,
            Row::HiddenFiles => self.settings.show_hidden,
            Row::Minimap => self.session.minimap,
            Row::SoftWrap => self.session.soft_wrap,
            Row::ScrollSync => self.sync_scroll,
            _ => false,
        }
    }

    /// Flips a `Toggle` row. View toggles reuse the menu commands so the panel
    /// and the View menu never fork their behavior.
    pub fn toggle_setting(&mut self, row: Row) -> bool {
        match row {
            Row::LineNumbers => self.settings.line_numbers = !self.settings.line_numbers,
            Row::IndentGuides => self.settings.indent_guides = !self.settings.indent_guides,
            Row::AutoPairs => {
                self.settings.auto_pairs = !self.settings.auto_pairs;
                self.status(format!(
                    "auto pairs: {}",
                    if self.settings.auto_pairs {
                        "ON"
                    } else {
                        "OFF"
                    }
                ));
            }
            Row::Syntax => self.settings.syntax = !self.settings.syntax,
            Row::Autosave => self.settings.autosave = !self.settings.autosave,
            Row::LanguageIntelligence => {
                self.set_lsp_enabled(!self.settings.lsp);
                return true;
            }
            Row::TrustLocalServers => {
                self.settings.trust_local_servers = !self.settings.trust_local_servers;
                self.status(format!(
                    "local language servers: {}",
                    if self.settings.trust_local_servers {
                        "TRUSTED"
                    } else {
                        "BLOCKED"
                    }
                ));
            }
            Row::HiddenFiles => {
                self.settings.show_hidden = !self.settings.show_hidden;
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
            Row::Minimap => {
                self.toggle_minimap();
                return true;
            }
            Row::SoftWrap => {
                let on = !self.session.soft_wrap;
                self.set_soft_wrap(on);
                return true;
            }
            Row::ScrollSync => {
                self.toggle_sync_scroll();
                return true;
            }
            _ => return false,
        }
        self.settings_changed();
        self.dirty = true;
        true
    }

    /// Display value of a settings row.
    pub fn settings_value(&self, row: Row) -> String {
        match row.kind() {
            RowKind::Toggle => on_off(self.settings_toggle_state(row)),
            RowKind::Choice(options) => {
                let index = self.settings_choice_index(row).unwrap_or(0);
                options[index].to_string()
            }
            RowKind::Number(spec) => {
                let value = self.settings_number_value(row).unwrap_or(spec.min);
                format!("{} {}", value.round() as i32, spec.unit)
            }
            RowKind::Action => "DEFAULT".into(),
        }
    }

    /// Steps a row (`delta` is -1 or +1). Returns whether anything changed.
    pub fn adjust_setting(&mut self, row: Row, delta: i32) -> bool {
        match row.kind() {
            RowKind::Toggle => {
                if delta != 0 {
                    self.toggle_setting(row);
                }
            }
            RowKind::Choice(options) => {
                if delta != 0 && !options.is_empty() {
                    let index = self.settings_choice_index(row).unwrap_or(0) as i32;
                    let count = options.len() as i32;
                    let next = (index + delta).rem_euclid(count) as usize;
                    self.settings_set_choice(row, next);
                }
            }
            RowKind::Number(spec) => {
                if delta != 0 {
                    let current = self.settings_number_value(row).unwrap_or(spec.min);
                    self.settings_set_number(row, current + spec.step * delta as f32);
                }
            }
            RowKind::Action => self.reset_settings(),
        }
        self.dirty = true;
        true
    }

    /// Enter/Space on a row: numbers start a typed edit, everything else acts
    /// like a `+` step (toggles flip, choices advance).
    pub fn activate_setting(&mut self, row: Row) {
        if matches!(row.kind(), RowKind::Number(_)) {
            self.settings_begin_edit(row);
        } else {
            self.adjust_setting(row, 1);
        }
    }

    /// Starts typing a numeric row: the field is seeded with the current value
    /// and fully selected.
    pub fn settings_begin_edit(&mut self, row: Row) -> bool {
        let Some(value) = self.settings_number_value(row) else {
            return false;
        };
        if !matches!(row.kind(), RowKind::Number(_)) {
            return false;
        }
        let mut field = TextEdit::new(format!("{}", value.round() as i32));
        field.select_all();
        self.settings_ui.editing = Some(SettingsEdit { row, field });
        self.settings_ui.preedit = None;
        self.dirty = true;
        true
    }

    /// Drops the typed number edit without applying it.
    pub fn settings_cancel_edit(&mut self) {
        if self.settings_ui.editing.take().is_some() {
            self.settings_ui.preedit = None;
            self.dirty = true;
        }
    }

    /// Applies the typed number edit. An empty or unparsable value keeps the
    /// old setting.
    pub fn settings_commit_edit(&mut self) -> bool {
        let Some(edit) = self.settings_ui.editing.take() else {
            return false;
        };
        self.settings_ui.preedit = None;
        self.dirty = true;
        let text = edit.field.text().trim().to_string();
        let Ok(value) = text.parse::<f32>() else {
            return false;
        };
        self.settings_set_number(edit.row, value)
    }

    /// Row whose number is being typed, if any.
    pub fn settings_editing(&self) -> Option<Row> {
        self.settings_ui.editing.as_ref().map(|edit| edit.row)
    }

    /// Committed text from the keyboard/IME: digits while editing a number,
    /// otherwise the search query.
    pub fn settings_text(&mut self, text: &str) {
        if let Some(edit) = self.settings_ui.editing.as_mut() {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            if !digits.is_empty() && edit.field.text().len() + digits.len() <= 5 {
                edit.field.insert(&digits);
            }
        } else {
            self.settings_ui.query.insert(text);
            self.settings_query_changed();
        }
        self.settings_ui.preedit = None;
        self.dirty = true;
    }

    /// Re-filters after a query edit and keeps the selection on a visible row.
    pub fn settings_query_changed(&mut self) {
        if !self.settings_ui.shows(self.settings_ui.row) {
            if let Some(spec) = self.settings_ui.visible_rows().first() {
                self.settings_ui.row = spec.row;
            }
        }
        self.settings_ui.scroll = 0.0;
        self.dirty = true;
    }

    /// Moves the selected row through the filtered list.
    pub fn settings_select(&mut self, delta: i32) {
        let visible = self.settings_ui.visible_rows();
        if visible.is_empty() {
            return;
        }
        let index = visible
            .iter()
            .position(|spec| spec.row == self.settings_ui.row)
            .unwrap_or(0);
        let next = (index as i32 + delta).clamp(0, visible.len() as i32 - 1) as usize;
        let previous_group = self.settings_group();
        self.settings_ui.row = visible[next].row;
        if self.settings_group() != previous_group {
            self.settings_ui.scroll = 0.0;
        }
        self.dirty = true;
    }

    pub fn reset_settings(&mut self) {
        self.settings = Settings::default();
        self.theme = Theme::new(Mode::Dark);
        self.sync_scroll = true;
        self.session.sync_scroll = true;
        self.session.minimap = true;
        self.set_soft_wrap(false);
        self.rebuild_keymap();
        self.settings_changed();
        self.apply_explorer_filter();
        self.pending_sidebar_sync = true;
        self.settings_ui = SettingsUi::default();
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
