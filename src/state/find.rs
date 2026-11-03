//! Find and replace state and commands.

use super::*;

impl AppState {
    // -- find / replace --------------------------------------------------

    pub fn open_find(&mut self, replace: bool) {
        self.settings_open = false;
        self.quick_open.open = false;
        self.tree_menu = None;
        self.lsp.dismiss_transient();
        if self.search.open {
            self.close_project_search();
        }
        let was_open = self.find.open;
        self.find.open = true;
        self.focus = if replace { Focus::Replace } else { Focus::Find };
        self.find.query.select_all();
        self.find.dirty = true;
        if !was_open {
            self.needs_rebuild = true;
        }
        self.dirty = true;
    }

    pub fn close_find(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.preedit = None;
        self.find.matches.clear();
        self.find.current = None;
        self.focus = Focus::Editor;
        self.needs_rebuild = true;
        self.dirty = true;
    }

    pub fn find_field_mut(&mut self) -> Option<&mut TextEdit> {
        match self.focus {
            Focus::Find => Some(&mut self.find.query),
            Focus::Replace => Some(&mut self.find.replace),
            Focus::Editor | Focus::Terminal => None,
        }
    }

    /// Recomputes matches when the query or the document changed.
    pub fn refresh_find(&mut self) {
        if !self.find.open {
            return;
        }
        let rev = self.ws.active_doc().map_or(u64::MAX, |doc| doc.rev());
        if !self.find.dirty && rev == self.find.rev {
            return;
        }
        let matcher = Matcher {
            query: self.find.query.text().to_string(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
        };
        self.find.matches = self
            .ws
            .active_doc()
            .map(|doc| find_all(doc.text(), &matcher))
            .unwrap_or_default();
        self.find.rev = rev;
        self.find.dirty = false;
        if self.find.matches.is_empty() {
            self.find.current = None;
        } else {
            let index = self
                .find
                .current
                .unwrap_or(0)
                .min(self.find.matches.len() - 1);
            self.find.current = Some(index);
        }
    }

    /// Moves to the next (`dir > 0`) or previous match, wrapping around.
    pub fn find_step(&mut self, dir: isize) {
        self.refresh_find();
        if self.find.matches.is_empty() {
            self.status("no matches");
            return;
        }
        let count = self.find.matches.len();
        let current = self.find.current.unwrap_or(0) as isize;
        let next = if dir >= 0 {
            (current + 1) % count as isize
        } else {
            (current - 1 + count as isize) % count as isize
        };
        self.find.current = Some(next as usize);
        let (start, end) = self.find.matches[next as usize];
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_caret(start);
            doc.extend_to(end);
        }
        self.editor.reveal = true;
        self.dirty = true;
    }

    /// Selects the current match (or the first one) without stepping.
    pub fn find_select_current(&mut self) {
        self.refresh_find();
        let Some(index) = self.find.current else {
            return;
        };
        if let Some(&(start, end)) = self.find.matches.get(index) {
            if let Some(doc) = self.ws.active_doc_mut() {
                doc.set_caret(start);
                doc.extend_to(end);
            }
            self.editor.reveal = true;
            self.dirty = true;
        }
    }

    /// Replaces the current match with the replace text.
    pub fn replace_one(&mut self) {
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only()) {
            self.status("read-only Git view");
            return;
        }
        self.refresh_find();
        let Some(index) = self.find.current else {
            self.status("no matches");
            return;
        };
        let Some(&(start, end)) = self.find.matches.get(index) else {
            return;
        };
        let replacement = self.find.replace.text().to_string();
        if let Some(doc) = self.ws.active_doc_mut() {
            if doc.replace_range(start, end, &replacement) {
                self.editor.reveal = true;
            }
        }
        self.find.dirty = true;
        self.refresh_find();
        self.find_select_current();
    }

    /// Replaces every match.
    pub fn replace_all(&mut self) {
        if self.ws.active_doc().is_some_and(|doc| doc.is_read_only()) {
            self.status("read-only Git view");
            return;
        }
        self.refresh_find();
        if self.find.matches.is_empty() {
            self.status("no matches");
            return;
        }
        let replacement = self.find.replace.text().to_string();
        // Overlapping matches (e.g. "aa" in "aaaa") share bytes; keep the first
        // non-overlapping set so the replacement cannot clobber itself.
        let mut chosen: Vec<(usize, usize)> = Vec::new();
        let mut last_end = 0;
        for &(start, end) in &self.find.matches {
            if start >= last_end {
                chosen.push((start, end));
                last_end = end;
            }
        }
        let Some(doc) = self.ws.active_doc_mut() else {
            return;
        };
        let mut replaced = 0;
        for (start, end) in chosen.iter().rev() {
            if doc.replace_range(*start, *end, &replacement) {
                replaced += 1;
            }
        }
        self.find.dirty = true;
        self.find.current = None;
        self.refresh_find();
        self.status(format!("replaced {replaced}"));
        self.dirty = true;
    }
}
