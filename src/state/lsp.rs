//! LSP polling, hover, completion, definitions and docs.

use super::*;

impl AppState {
    /// Rebuilds the cached PROBLEMS rows from every live language server.
    /// Called on a diagnostic update, not per frame.
    pub fn refresh_problems(&mut self) {
        self.problems.rows = self.lsp.workspace_diagnostics();
        let rows = &self.problems.rows;
        self.problems.errors = rows
            .iter()
            .filter(|row| row.diagnostic.severity == 1)
            .count();
        self.problems.warnings = rows
            .iter()
            .filter(|row| row.diagnostic.severity == 2)
            .count();
        if self.problems.scroll_max > 0.0 && self.problems.rows.is_empty() {
            self.problems.scroll = 0.0;
            self.problems.hover = None;
        }
        self.dirty = true;
    }

    /// Pumps the language server and applies a pending go-to-definition.
    pub fn tick_lsp(&mut self) {
        self.lsp.set_workspace_binaries(
            self.settings.trust_local_servers || model_lsp::workspace_binaries_enabled(),
        );
        self.lsp
            .tick(&self.ws, &self.services.language_servers, self.settings.lsp);
        // File changes collected by the editor and the filesystem poll reach the
        // servers as `workspace/didChangeWatchedFiles`; a self-rescanning server
        // (Godot) instead gets a delayed re-parse of the open documents.
        let events = std::mem::take(&mut self.file_events);
        if !events.is_empty() {
            self.lsp.notify_file_events(&events);
        }
        self.tooling.tick();
        if let Some(notice) = self.lsp.take_notice() {
            self.status(notice);
        }
        if let Some(issue) = self.lsp.take_issue() {
            self.show_tool_prompt(issue);
        }
        if self.lsp.take_changed() {
            self.refresh_problems();
            self.dirty = true;
        }
        if let Some(location) = self.lsp.take_location() {
            self.open_lsp_location(location);
        }
        if let Some((symbol, locations)) = self.lsp.take_references() {
            self.show_references(symbol, locations);
        }
        if let Some((context, placeholder)) = self.lsp.take_prepare_rename() {
            match placeholder {
                Some(placeholder) => self.open_rename_symbol_prompt(context, Some(placeholder)),
                None => self.status("rename is not available here"),
            }
        }
        if let Some((context, edits)) = self.lsp.take_rename() {
            self.apply_symbol_rename(context, edits);
        }
        if let Some((class, symbol, data)) = self.lsp.take_native_result() {
            if self.docs.class == class
                && self.docs.symbol == symbol
                && self
                    .ws
                    .tabs
                    .iter()
                    .any(|doc| doc.view_key() == Some("godot-docs"))
            {
                self.show_docs_tab(&data);
            }
        }
        if let Some(target) = self.lsp.take_native_docs_target() {
            self.open_native_docs(&target);
        }
    }

    pub(crate) fn open_lsp_location(&mut self, location: model_lsp::Location) {
        if !location.path.is_file() {
            return;
        }
        let display = location.path.display().to_string();
        self.open_path(location.path);
        if let Some(doc) = self.ws.active_doc_mut() {
            let byte = model_lsp::byte_for_position(doc, location.line, location.character);
            doc.set_caret(byte);
            self.editor.reveal = true;
            self.status(format!("definition: {display}"));
            self.dirty = true;
        }
    }

    pub fn request_lsp_hover(&mut self) {
        self.hover_target = None;
        self.hover_leave_at = None;
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if self.lsp.request_hover(doc) {
            self.dirty = true;
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    /// Updates the delayed mouse-hover target. Called from the editor pointer
    /// move callback; a change hides the old card and restarts the timer.
    pub fn set_hover_target(&mut self, doc: u64, byte: usize, anchor: Vec2) {
        let changed = self
            .hover_target
            .is_none_or(|target| target.doc != doc || target.byte != byte);
        if changed {
            self.hover_leave_at = None;
            let diagnostic = self
                .ws
                .active_doc()
                .filter(|active| active.id() == doc)
                .and_then(|active| self.lsp.diagnostic_at(active, byte));
            let has_diagnostic = diagnostic.is_some();
            if let Some(diagnostic) = diagnostic {
                let label = match diagnostic.severity {
                    1 => "ERROR",
                    2 => "WARNING",
                    3 => "INFO",
                    _ => "HINT",
                };
                self.lsp.hover = Some(model_lsp::HoverUi {
                    text: format!(
                        "{label}  {}:{}\n{}",
                        diagnostic.start_line + 1,
                        diagnostic.start_char + 1,
                        diagnostic.message
                    ),
                    native: None,
                });
                self.lsp.hover_anchor = Some(anchor);
            }
            self.hover_target = Some(HoverTarget {
                doc,
                byte,
                anchor,
                since: Instant::now(),
                requested: has_diagnostic,
            });
            self.dirty = true;
        }
    }

    pub fn clear_hover_target(&mut self) {
        self.hover_target = None;
    }

    /// Fires the pending hover request once the dwell time has passed.
    pub fn tick_hover(&mut self) {
        if let Some(at) = self.hover_leave_at {
            if at.elapsed() >= super::schedule::HOVER_DISMISS {
                self.hover_leave_at = None;
                self.lsp.hover = None;
                self.lsp.hover_anchor = None;
                self.lsp.hover_rect = igui_core::Rect::ZERO;
                self.dirty = true;
            }
        }
        let Some(target) = self.hover_target else {
            return;
        };
        if target.requested || target.since.elapsed() < super::schedule::HOVER_DELAY {
            return;
        }
        let Some(doc) = self.ws.active_doc() else {
            self.hover_target = None;
            return;
        };
        if doc.id() != target.doc {
            self.hover_target = None;
            return;
        }
        if !matches!(
            self.lsp.status(),
            crate::model::lsp::LspStatus::Ready | crate::model::lsp::LspStatus::Starting
        ) {
            self.hover_target = None;
            return;
        }
        if self.lsp.request_hover_at(doc, target.byte, target.anchor) {
            if let Some(target) = self.hover_target.as_mut() {
                target.requested = true;
            }
        }
    }

    /// Drops occurrence highlights and the debounce that would replace them.
    pub(crate) fn clear_occurrence_highlights(&mut self) {
        self.occurrence_at = None;
        self.occurrence_key = None;
        if let Some(path) = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        {
            self.lsp.clear_document_highlights(&path);
        }
    }

    /// Debounced `textDocument/documentHighlight` for the symbol under the
    /// caret. Clears immediately when the editor no longer qualifies (focus,
    /// selection, read-only, no server support) so no stale tint survives.
    pub fn tick_occurrences(&mut self) {
        let now = Instant::now();
        let eligible = {
            let Some(doc) = self.ws.active_doc() else {
                self.clear_occurrence_highlights();
                return;
            };
            self.focus == Focus::Editor
                && self.modal.is_none()
                && !doc.is_read_only()
                && doc.path().is_some()
                && doc.selection().is_none()
                && self.lsp.capabilities().document_highlight
                && caret_on_word(doc)
        };
        if !eligible {
            self.clear_occurrence_highlights();
            return;
        }
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let key = (doc.id(), doc.rev(), doc.caret());
        if self.occurrence_key != Some(key) {
            self.occurrence_key = Some(key);
            self.occurrence_at = Some(now + Duration::from_millis(120));
            if let Some(path) = doc.path() {
                self.lsp.clear_document_highlights(path);
            }
            self.dirty = true;
            return;
        }
        let Some(at) = self.occurrence_at else {
            return;
        };
        if now < at {
            return;
        }
        self.occurrence_at = None;
        if self.lsp.request_document_highlights(doc) {
            self.dirty = true;
        } else {
            // The session may still be starting; retry shortly instead of
            // waiting for the next caret move.
            self.occurrence_at = Some(now + Duration::from_millis(250));
        }
    }

    pub fn request_lsp_definition_at(&mut self, byte: usize) -> bool {
        self.hover_target = None;
        self.hover_leave_at = None;
        self.lsp.hover = None;
        self.lsp.hover_anchor = None;
        let Some(doc) = self.ws.active_doc() else {
            return false;
        };
        if self.lsp.request_definition_at(doc, byte) {
            self.dirty = true;
            true
        } else {
            self.status(self.lsp_unavailable());
            false
        }
    }

    /// Native class at a byte, used by Ctrl/Cmd-click documentation.
    pub fn native_class_at(&self, byte: usize) -> Option<String> {
        let doc = self.ws.active_doc()?;
        let text = doc.text();
        let mut start = byte.min(text.len());
        while start > 0 {
            let len = text[..start].chars().next_back()?.len_utf8();
            let ch = text[start - len..start].chars().next()?;
            if !(ch.is_alphanumeric() || ch == '_') {
                break;
            }
            start -= len;
        }
        let mut end = byte.min(text.len());
        while end < text.len() {
            let ch = text[end..].chars().next()?;
            if !(ch.is_alphanumeric() || ch == '_') {
                break;
            }
            end += ch.len_utf8();
        }
        let word = &text[start..end];
        self.lsp.is_native_class(word).then(|| word.to_string())
    }

    /// Opens an embedded Godot native-symbol document for `target` in the
    /// normal editor tab strip.
    pub fn open_native_docs(&mut self, target: &model_lsp::NativeTarget) {
        self.open_native_docs_at(target.class.clone(), target.symbol.clone());
    }

    pub fn open_native_docs_at(&mut self, class: String, symbol: String) {
        self.docs.class = class.clone();
        self.docs.symbol = symbol.clone();
        self.lsp.dismiss_transient();
        if let Some(data) = self.lsp.native_symbol_cached(&class, &symbol) {
            self.show_docs_tab(&data);
            return;
        }
        self.show_docs_text(
            format!("GODOT DOCS\n\nLoading documentation for {class}.{symbol}…\n"),
            format!("{symbol} — Godot Docs"),
            Vec::new(),
        );
        if !self.lsp.request_native_symbol(&class, &symbol) {
            self.show_docs_text(
                format!(
                    "GODOT DOCS\n\nDocumentation unavailable for {class}.{symbol}.\n\n{}",
                    self.lsp_unavailable()
                ),
                format!("{symbol} — Godot Docs"),
                Vec::new(),
            );
        }
        self.dirty = true;
    }

    pub(crate) fn show_docs_tab(&mut self, symbol: &crate::model::lsp::NativeSymbol) {
        let rendered = crate::model::docs::render(symbol);
        let name = format!("{} — Godot Docs", symbol.name);
        self.show_docs_text(rendered.text, name, rendered.links);
    }

    pub(crate) fn show_docs_text(
        &mut self,
        text: String,
        name: String,
        links: Vec<(usize, usize, crate::model::bbcode::DocLink)>,
    ) {
        let index = self.ws.show_docs_view("godot-docs", name, text);
        self.ws.select(index);
        if let Some(doc) = self.ws.active_doc() {
            self.docs.links.insert(doc.id(), links);
        }
        self.tabs.reveal_active = true;
        self.focus = Focus::Editor;
        self.editor.preedit = None;
        self.editor.drag = Drag::None;
        self.sync_layout();
        self.find.dirty = true;
        self.dirty = true;
    }

    /// Link under `byte` in the active documentation tab.
    pub fn docs_link_at(&self, byte: usize) -> Option<crate::model::bbcode::DocLink> {
        let doc = self.ws.active_doc()?;
        if !doc.is_docs_view() {
            return None;
        }
        self.docs
            .links
            .get(&doc.id())?
            .iter()
            .find(|(start, end, _)| *start <= byte && byte < *end)
            .map(|(_, _, link)| link.clone())
    }

    /// Opens the resolved documentation for the selected completion item.
    /// Native targets go through the docs tab; local methods use a plain tab.
    pub fn open_completion_docs(&mut self) {
        let Some(completion) = self.lsp.completion.as_ref() else {
            return;
        };
        let Some(item) = completion.items.get(completion.selected) else {
            return;
        };
        let text = completion
            .preview
            .clone()
            .unwrap_or_else(|| item.detail.clone());
        let target = completion.native.clone();
        let label = item.label.clone();
        if let Some(target) = target {
            self.open_native_docs(&target);
            return;
        }
        self.docs.class = "completion".into();
        self.docs.symbol = label.clone();
        self.show_docs_text(
            format!("GODOT DOCS\n\n{label}\n\n{text}"),
            format!("{label} — Godot Docs"),
            Vec::new(),
        );
        self.lsp.dismiss_transient();
        self.dirty = true;
    }

    /// Opens a URL from a BBCode `[url]` span in the platform browser.
    ///
    /// Only web/mail schemes are accepted: the URL comes from language-server
    /// data and must never reach the OS opener as a file/command target.
    pub fn open_external_url(&mut self, url: &str) {
        let url = url.trim();
        if !external_url_allowed(url) {
            self.error("refusing to open an unsupported URL");
            return;
        }
        let result = {
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open").arg(url).spawn()
            }
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open").arg(url).spawn()
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("rundll32")
                    .args(["url.dll,FileProtocolHandler", url])
                    .spawn()
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "opening URLs is not supported on this platform",
                ))
            }
        };
        match result {
            Ok(_) => self.status("opened documentation link"),
            Err(error) => self.error(format!("could not open URL: {error}")),
        }
    }

    /// Schedules completion after a short typing pause. Repeated characters
    /// replace the deadline, so this is a debounce rather than a request per
    /// keystroke.
    pub fn schedule_auto_completion(&mut self, text: &str) {
        if self.focus != Focus::Editor || self.modal.is_some() {
            return;
        }
        let Some(last) = text.chars().last() else {
            return;
        };
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only()) {
            return;
        }
        if last.is_ascii_alphanumeric() || matches!(last, '_') {
            self.auto_completion_at = Some(Instant::now() + Duration::from_millis(140));
        } else if last == '.' || self.lsp.completion_triggers().contains(&last) {
            // A quote only triggers when it opened the literal under the caret;
            // the closing quote must not pop a path list. Everything else in
            // the server's list (`.`, `$`, `%` for GDScript) is immediate.
            let quote = matches!(last, '\'' | '"');
            let inside_string = quote
                && self.ws.active_doc().is_some_and(|doc| {
                    model_lsp::enclosing_string_span(doc.text(), doc.caret()).is_some()
                });
            if !quote || inside_string {
                self.auto_completion_at = Some(Instant::now());
            }
        }
    }

    pub fn auto_completion_pending(&self) -> bool {
        self.auto_completion_at.is_some()
    }

    /// Schedules signature help for a call argument trigger, and refreshes a
    /// visible card while the user keeps typing inside the call.
    pub fn schedule_signature_help(&mut self, text: &str) {
        if self.focus != Focus::Editor || self.modal.is_some() {
            return;
        }
        let Some(last) = text.chars().last() else {
            return;
        };
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only()) {
            return;
        }
        if last == ')' {
            self.clear_signature_help();
            return;
        }
        if !matches!(last, '(' | ',') {
            if self.lsp.signature_help.is_some()
                && (last.is_ascii_alphanumeric() || matches!(last, '_' | '.' | ' ' | '"' | '\''))
            {
                self.signature_help_at = Some(Instant::now() + Duration::from_millis(140));
            }
            return;
        }
        if self.lsp.capabilities().signature_help {
            self.signature_help_at = Some(Instant::now());
        }
    }

    pub(crate) fn clear_signature_help(&mut self) {
        self.signature_help_at = None;
        self.signature_help_doc = None;
        if self.lsp.signature_help.take().is_some() {
            self.dirty = true;
        }
    }

    /// Fires the pending signature-help request and drops a card that belongs
    /// to a document the editor is no longer showing.
    pub fn tick_signature_help(&mut self) {
        let active_id = self.ws.active_doc().map(|doc| doc.id());
        if let Some((doc_id, _, _)) = self.signature_help_doc {
            if Some(doc_id) != active_id {
                self.clear_signature_help();
                return;
            }
        }
        let Some(at) = self.signature_help_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        self.signature_help_at = None;
        if self.focus != Focus::Editor || self.modal.is_some() {
            return;
        }
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if doc.is_read_only() || !self.lsp.capabilities().signature_help {
            return;
        }
        let key = (doc.id(), doc.rev(), doc.caret());
        if self.lsp.request_signature_help(doc) {
            self.signature_help_doc = Some(key);
            self.dirty = true;
        }
    }

    /// Fires the debounced completion request without surfacing "no LSP"
    /// errors while the user is merely typing.
    pub fn tick_auto_completion(&mut self) {
        let Some(at) = self.auto_completion_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        self.auto_completion_at = None;
        if self.focus != Focus::Editor
            || self.modal.is_some()
            || self.lsp.completion.is_some()
            || self.lsp.hover.is_some()
        {
            return;
        }
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if self.lsp.request_completion(doc) {
            self.dirty = true;
        }
    }

    /// Probes hover at a byte so Ctrl/Cmd-click can open native docs for
    /// methods, virtual methods, global functions and properties.
    pub fn request_lsp_native_docs_at(&mut self, byte: usize) -> bool {
        let Some(doc) = self.ws.active_doc() else {
            return false;
        };
        self.lsp.request_native_docs_at(doc, byte)
    }

    pub fn request_lsp_completion(&mut self) {
        self.auto_completion_at = None;
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if self.lsp.request_completion(doc) {
            self.dirty = true;
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    pub fn request_lsp_definition(&mut self) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if self.lsp.request_definition(doc) {
            self.dirty = true;
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    /// Drops and restarts the active document's server (hard reset). Saves and
    /// file changes never do this; the command exists for a wedged server or a
    /// Godot project whose scan has to happen now.
    pub fn restart_language_server(&mut self) {
        let Some(path) = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(PathBuf::from)
        else {
            self.status("no language server for this file");
            return;
        };
        if self.lsp.restart(&path) {
            self.status("language server restarting");
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    pub(crate) fn lsp_unavailable(&self) -> String {
        if !self.lsp.enabled() {
            return "language servers are off (Settings → Language Intelligence)".into();
        }
        match self.lsp.status() {
            model_lsp::LspStatus::Error(error) => format!("LSP: {error}"),
            model_lsp::LspStatus::Starting => "LSP starting…".into(),
            _ => "no language server for this file".into(),
        }
    }

    /// Applies the selected completion item, replacing the identifier prefix
    /// under the caret. Returns whether a completion was consumed.
    pub fn apply_lsp_completion(&mut self) -> bool {
        let Some(item) = self.lsp.take_completion() else {
            return false;
        };
        let Some(doc) = self.ws.active_doc_mut() else {
            return false;
        };
        let caret = doc.caret();
        // Godot returns `"res://path"` (quotes included, no textEdit range), so
        // a quoted insertion inside a string replaces the whole literal. A
        // plain identifier still replaces just the word under the caret.
        let quoted = item.insert.starts_with(['\'', '"']);
        if let Some((start, end)) =
            model_lsp::enclosing_string_span(doc.text(), caret).filter(|_| quoted)
        {
            self.auto_completion_at = None;
            if doc.replace_range(start, end, &item.insert) {
                self.editor.reveal = true;
                self.status(format!("completed {}", item.label));
                self.dirty = true;
            }
            return true;
        }
        let mut start = caret;
        while start > 0 {
            let previous = doc.text()[..start]
                .chars()
                .next_back()
                .map(|ch| ch.len_utf8())
                .unwrap_or(0);
            let ch = doc.text()[start - previous..start]
                .chars()
                .next()
                .unwrap_or(' ');
            if !(ch.is_alphanumeric() || ch == '_') {
                break;
            }
            start -= previous;
        }
        self.auto_completion_at = None;
        if doc.replace_range(start, caret, &item.insert) {
            self.editor.reveal = true;
            self.status(format!("completed {}", item.label));
            self.dirty = true;
        }
        true
    }

    /// Keys owned by the completion popup. Returns false for keys the editor
    /// should still handle.
    pub fn lsp_completion_key(&mut self, key: Key) -> bool {
        if self.lsp.completion.is_none() {
            return false;
        }
        match key {
            Key::Escape => self.lsp.completion = None,
            Key::ArrowUp => self.lsp.move_completion(-1),
            Key::ArrowDown => self.lsp.move_completion(1),
            Key::Enter | Key::Tab => {
                self.apply_lsp_completion();
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }
}

/// Whether the caret touches an identifier character (a server call on
/// whitespace or punctuation would only return an empty list).
fn caret_on_word(doc: &Document) -> bool {
    let text = doc.text();
    let caret = doc.caret().min(text.len());
    let before = text[..caret].chars().next_back();
    let at = text[caret..].chars().next();
    [before, at]
        .into_iter()
        .flatten()
        .any(|ch| ch.is_alphanumeric() || ch == '_')
}

/// URL schemes accepted by the documentation viewer.
fn external_url_allowed(url: &str) -> bool {
    if url.is_empty() || url.len() > 4096 || url.chars().any(char::is_control) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_urls_are_scheme_restricted() {
        assert!(external_url_allowed(
            "https://docs.godotengine.org/en/stable/"
        ));
        assert!(external_url_allowed("http://example.com/a?b=1&c=2"));
        assert!(external_url_allowed("mailto:user@example.com"));
        assert!(!external_url_allowed("file:///etc/passwd"));
        assert!(!external_url_allowed("javascript:alert(1)"));
        assert!(!external_url_allowed("-a Calculator"));
        assert!(!external_url_allowed("https://example.com/\ncalc"));
    }
}
