//! Symbol rename (`F2`): prepare, prompt, workspace edit application.
//!
//! The language server owns validation; the editor owns safety. Every edit is
//! checked against the document it came from (`old_name` still at the range,
//! revisions unchanged) before any file is touched, and all edits per file are
//! applied as one undo step. Closed files are written immediately; open tabs
//! stay dirty so the normal save/autosave path owns their persistence.

use super::*;

/// Files one rename may touch.
const MAX_RENAME_FILES: usize = 50;
/// Closed files larger than this are not rewritten by a rename.
const MAX_RENAME_BYTES: u64 = 8 * 1024 * 1024;

impl AppState {
    /// `F2`: validate the symbol with `prepareRename` and open the prompt.
    pub fn request_lsp_rename(&mut self) {
        self.lsp.dismiss_transient();
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if doc.is_read_only() {
            self.status("read-only view");
            return;
        }
        let Some(old_name) = super::references::word_at(doc, doc.caret()) else {
            self.status("place the caret on a symbol first");
            return;
        };
        let capabilities = self.lsp.capabilities();
        if !capabilities.rename {
            self.status("LSP: this server has no rename support");
            return;
        }
        let path = doc.path().map(Path::to_path_buf);
        let Some(path) = path else {
            return;
        };
        let context = model_lsp::RenameContext {
            doc_id: doc.id(),
            rev: doc.rev(),
            path,
            old_name: old_name.clone(),
            new_name: String::new(),
        };
        if !capabilities.prepare_rename {
            self.open_rename_symbol_prompt(context, None);
            return;
        }
        if self.lsp.request_prepare_rename(doc, &old_name) {
            self.status(format!("rename {old_name}…"));
        } else {
            self.status(self.lsp_unavailable());
        }
    }

    /// Opens the prompt after `prepareRename` accepted the symbol. Rejects a
    /// document that changed while the request was in flight.
    pub(crate) fn open_rename_symbol_prompt(
        &mut self,
        context: model_lsp::RenameContext,
        placeholder: Option<String>,
    ) {
        let live = self.ws.tabs.iter().any(|doc| {
            doc.id() == context.doc_id
                && doc.rev() == context.rev
                && doc.path() == Some(&context.path)
        });
        if !live {
            self.status("rename: the document changed; try again");
            return;
        }
        self.settings_open = false;
        self.quick_open.open = false;
        self.references.open = false;
        self.close_find();
        self.menu_open = None;
        self.tab_menu = None;
        self.tree_menu = None;
        self.new_file.open = false;
        if self.search.open {
            self.close_project_search();
        }
        let name = placeholder
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| context.old_name.clone());
        self.rename_symbol.old_name = context.old_name.clone();
        self.rename_symbol.context = Some(context);
        self.rename_symbol.name = TextEdit::new(name);
        self.rename_symbol.name.select_all();
        self.rename_symbol.error = None;
        self.rename_symbol.preedit = None;
        self.rename_symbol.open = true;
        self.focus = Focus::Editor;
        self.terminal.focused = false;
        self.dirty = true;
    }

    pub fn close_rename_symbol(&mut self) {
        self.lsp.cancel_rename();
        self.hide_rename_symbol();
    }

    /// Hides the prompt without cancelling a rename request that was just sent.
    fn hide_rename_symbol(&mut self) {
        self.rename_symbol.open = false;
        self.rename_symbol.context = None;
        self.rename_symbol.error = None;
        self.rename_symbol.preedit = None;
        self.dirty = true;
    }

    /// Enter in the prompt: validate the name and ask the server for edits.
    pub fn submit_rename_symbol(&mut self) {
        let name = self.rename_symbol.name.text().trim().to_string();
        if !valid_identifier(&name) {
            self.rename_symbol.error = Some("use an identifier (letters, digits, _ or $)".into());
            self.dirty = true;
            return;
        }
        if name == self.rename_symbol.old_name {
            self.rename_symbol.error = Some("the name is unchanged".into());
            self.dirty = true;
            return;
        }
        let Some(context) = self.rename_symbol.context.clone() else {
            self.close_rename_symbol();
            return;
        };
        let Some(doc) = self.ws.tabs.iter().find(|doc| doc.id() == context.doc_id) else {
            self.rename_symbol.error = Some("the document was closed".into());
            self.dirty = true;
            return;
        };
        if doc.rev() != context.rev {
            self.rename_symbol.error = Some("the document changed; try again".into());
            self.dirty = true;
            return;
        }
        if self.lsp.request_rename(doc, &name, &context.old_name) {
            self.hide_rename_symbol();
            self.status(format!("renaming {} to {name}…", context.old_name));
        } else {
            self.rename_symbol.error = Some(self.lsp_unavailable());
            self.dirty = true;
        }
    }

    /// Applies a `textDocument/rename` response after validating every range
    /// against the same text the server saw.
    pub(crate) fn apply_symbol_rename(
        &mut self,
        context: model_lsp::RenameContext,
        edits: Vec<model_lsp::WorkspaceTextEdit>,
    ) {
        if edits.is_empty() {
            self.status(format!("rename: no edits for {}", context.old_name));
            return;
        }
        let Some(doc) = self.ws.tabs.iter().find(|doc| doc.id() == context.doc_id) else {
            self.error("rename: the document was closed");
            return;
        };
        if doc.rev() != context.rev {
            self.error("rename: the document changed; try again");
            return;
        }

        let mut grouped: std::collections::BTreeMap<PathBuf, Vec<model_lsp::WorkspaceTextEdit>> =
            std::collections::BTreeMap::new();
        for edit in edits {
            grouped.entry(edit.path.clone()).or_default().push(edit);
        }
        if grouped.len() > MAX_RENAME_FILES {
            self.error(format!(
                "rename would touch {} files (limit {MAX_RENAME_FILES})",
                grouped.len()
            ));
            return;
        }

        struct Plan {
            path: PathBuf,
            open: Option<usize>,
            closed: Option<Document>,
            ranges: Vec<(usize, usize, String)>,
        }
        let mut plans = Vec::new();
        for (path, edits) in grouped {
            let open = self
                .ws
                .tabs
                .iter()
                .position(|doc| doc.path() == Some(path.as_path()));
            let (ranges, closed) = if let Some(index) = open {
                match rename_ranges(&self.ws.tabs[index], &edits, &context.old_name) {
                    Ok(ranges) => (ranges, None),
                    Err(error) => {
                        self.error(format!("rename: {error}"));
                        return;
                    }
                }
            } else {
                let metadata = std::fs::metadata(&path).ok();
                if metadata
                    .as_ref()
                    .is_some_and(|m| m.len() > MAX_RENAME_BYTES)
                {
                    self.error(format!("rename: {} is too large", path.display()));
                    return;
                }
                let Some(closed) = Document::open(0, &path).ok() else {
                    self.error(format!("rename: cannot read {}", path.display()));
                    return;
                };
                match rename_ranges(&closed, &edits, &context.old_name) {
                    Ok(ranges) => (ranges, Some(closed)),
                    Err(error) => {
                        self.error(format!("rename: {error}"));
                        return;
                    }
                }
            };
            plans.push(Plan {
                path,
                open,
                closed,
                ranges,
            });
        }

        // Closed files are written first; a write failure leaves open buffers
        // untouched (partial disk writes are still possible across files).
        for plan in &mut plans {
            let Some(closed) = plan.closed.as_mut() else {
                continue;
            };
            let applied = closed.replace_edits(&plan.ranges);
            if applied != plan.ranges.len() {
                self.error(format!(
                    "rename: edits no longer fit {}",
                    plan.path.display()
                ));
                return;
            }
            if let Err(error) = closed.save() {
                self.error(format!(
                    "rename: cannot save {}: {error}",
                    plan.path.display()
                ));
                return;
            }
        }
        let mut files = 0usize;
        let mut changed = 0usize;
        for plan in &plans {
            let Some(index) = plan.open else {
                files += 1;
                changed += plan.ranges.len();
                continue;
            };
            let applied = self.ws.tabs[index].replace_edits(&plan.ranges);
            if applied != plan.ranges.len() {
                self.error(format!(
                    "rename: edits no longer fit {}",
                    plan.path.display()
                ));
                return;
            }
            files += 1;
            changed += applied;
        }
        self.find.dirty = true;
        self.editor.reveal = true;
        self.dirty = true;
        self.status(format!(
            "renamed {} → {} ({} edits in {} files)",
            context.old_name, context.new_name, changed, files
        ));
    }
}

/// Validates a LSP rename edit list against `doc` and converts it to byte
/// ranges. Every range must still contain the old identifier.
fn rename_ranges(
    doc: &Document,
    edits: &[model_lsp::WorkspaceTextEdit],
    old_name: &str,
) -> Result<Vec<(usize, usize, String)>, String> {
    let text = doc.text();
    let mut ranges = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = model_lsp::byte_for_position(doc, edit.start_line, edit.start_char);
        let end = model_lsp::byte_for_position(doc, edit.end_line, edit.end_char);
        if start > end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return Err(format!("invalid range in {}", edit.path.display()));
        }
        if &text[start..end] != old_name {
            return Err(format!(
                "{} changed since the rename was requested (expected {old_name:?}, found {:?})",
                edit.path.display(),
                &text[start..end]
            ));
        }
        ranges.push((start, end, edit.new_text.clone()));
    }
    ranges.sort_by_key(|edit| (edit.0, edit.1));
    let mut last_end = 0;
    for (start, end, _) in &ranges {
        if *start < last_end {
            return Err("overlapping edits in the rename result".into());
        }
        last_end = *end;
    }
    Ok(ranges)
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_names_are_identifiers() {
        assert!(valid_identifier("hero"));
        assert!(valid_identifier("_hero2"));
        assert!(valid_identifier("$hero"));
        assert!(!valid_identifier(""));
        assert!(!valid_identifier("2hero"));
        assert!(!valid_identifier("hero name"));
        assert!(!valid_identifier("hero()"));
    }

    #[test]
    fn rename_ranges_reject_stale_text() {
        let mut doc = Document::untitled(1, 1);
        doc.insert_text("var player = 1\n");
        let range = |start: u32, end: u32| model_lsp::WorkspaceTextEdit {
            path: "/tmp/a.gd".into(),
            start_line: 0,
            start_char: start,
            end_line: 0,
            end_char: end,
            new_text: "hero".into(),
        };
        let ranges = rename_ranges(&doc, &[range(4, 10)], "player").unwrap();
        assert_eq!(ranges, vec![(4, 10, "hero".to_string())]);
        assert!(rename_ranges(&doc, &[range(4, 10)], "wrong").is_err());
    }
}
