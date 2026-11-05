//! Incremental syntax-highlight cache.
//!
//! The cache stores, per line, the tokens and the *entry state* the line was
//! tokenized with. After an edit it re-tokenizes from the first touched line and
//! stops as soon as the outgoing state matches the cached one **and** the edit
//! range is behind it — that is the classic "state converged, the rest is
//! unchanged" shortcut, so typing in a large file does not rehighlight
//! everything. The shortcut is only allowed when the cache was complete before
//! the edit; while the initial budgeted pass is still behind, every remaining
//! line must be tokenized because there is no cached suffix to reuse.
//!
//! Work is budgeted per frame; an unfinished pass resumes next frame
//! (`AppState` reports `needs_frame` while the cache is behind).
//!
//! Highlighting is off for files above the byte/line limits or when no provider
//! matches; the editor then paints plain text.

use std::sync::Arc;

use crate::settings::Settings;
use crate::syntax::{Highlight, SyntaxProvider, SyntaxRegistry};

use super::doc::Document;

/// Files above this size are not highlighted.
pub const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;
/// Files with more lines are not highlighted.
pub const MAX_HIGHLIGHT_LINES: usize = 20_000;
/// Lines tokenized per frame while catching up.
pub const FRAME_LINE_BUDGET: usize = 2_000;

/// Per-document token cache.
#[derive(Default)]
pub struct HighlightCache {
    doc: u64,
    rev: u64,
    provider: Option<Arc<dyn SyntaxProvider>>,
    provider_name: Option<&'static str>,
    lines: Vec<Vec<Highlight>>,
    /// `entry[i]` is the state at the start of line `i`; `entry[len]` is EOF.
    entry: Vec<u32>,
    /// Next line to process; lines before it are valid for this revision.
    resume: usize,
    complete: bool,
    /// Set when a provider exists but highlighting is disabled by policy.
    disabled: bool,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tokens for `line`, if that line has been processed for the current
    /// revision. `None` means "paint it plain" (still valid, just not ready).
    pub fn tokens(&self, line: usize) -> Option<&[Highlight]> {
        let valid = if self.complete {
            self.lines.len()
        } else {
            self.resume
        };
        if line >= valid {
            return None;
        }
        self.lines.get(line).map(Vec::as_slice)
    }

    /// The active provider name (shown in the status bar), even when
    /// highlighting is disabled by a size limit.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn language(&self) -> Option<&'static str> {
        self.provider_name
    }

    /// Whether a pass is still in progress for this document.
    pub fn is_behind(&self) -> bool {
        !self.complete && !self.disabled
    }

    /// Whether highlight tokens are being produced.
    pub fn is_active(&self) -> bool {
        self.provider.is_some() && !self.disabled
    }

    /// Drops all cached tokens (no document / editor closed).
    pub fn reset(&mut self) {
        self.clear(0, 0, None);
        self.disabled = true;
    }

    fn clear(&mut self, doc: u64, rev: u64, provider_name: Option<&'static str>) {
        self.doc = doc;
        self.rev = rev;
        self.provider = None;
        self.provider_name = provider_name;
        self.lines.clear();
        self.entry.clear();
        self.resume = 0;
        self.complete = true;
        self.disabled = true;
    }

    /// Brings the cache up to date with `doc`, spending at most `budget` lines.
    /// Returns `true` when it is fully up to date.
    pub fn sync(
        &mut self,
        doc: &Document,
        registry: &SyntaxRegistry,
        settings: &Settings,
        budget: usize,
        language_override: Option<&str>,
    ) -> bool {
        let registered = language_override
            .and_then(|name| registry.by_name(name))
            .or_else(|| registry.for_path(doc.path()));
        let name = registered.as_ref().map(|provider| provider.name());
        let provider = if settings.syntax { registered } else { None };
        let line_count = doc.line_count();
        let too_big = doc.text().len() > MAX_HIGHLIGHT_BYTES || line_count > MAX_HIGHLIGHT_LINES;

        let Some(provider) = provider.filter(|_| !too_big) else {
            if self.provider_name != name || self.doc != doc.id() || !self.disabled {
                self.clear(doc.id(), doc.rev(), name);
            }
            return true;
        };

        let mut was_complete = self.complete;
        if self.doc != doc.id()
            || self.provider_name != Some(provider.name())
            || self.lines.len() != line_count
        {
            was_complete = false;
            self.doc = doc.id();
            self.rev = doc.rev();
            self.provider_name = Some(provider.name());
            self.provider = Some(provider);
            self.lines = vec![Vec::new(); line_count];
            self.entry = vec![0; line_count + 1];
            self.resume = 0;
            self.complete = false;
            self.disabled = false;
        } else if self.rev != doc.rev() {
            self.rev = doc.rev();
            self.complete = false;
            self.disabled = false;
            // If the initial pass has not reached the edit yet, resume from
            // the oldest unprocessed line. Jumping straight to the edit would
            // leave every line before it with no tokens, which looks like the
            // following code turning white.
            let first = doc
                .edit_span()
                .map_or(0, |(first, _)| first)
                .min(line_count);
            self.resume = if was_complete {
                first
            } else {
                self.resume.min(first)
            };
        } else if self.complete {
            return true;
        }

        let Some(provider) = self.provider.clone() else {
            self.complete = true;
            return true;
        };
        let budget = budget.max(1);
        let mut processed = 0;
        while self.resume < line_count && processed < budget {
            let index = self.resume;
            let previous_next = self.entry.get(index + 1).copied();
            let line = doc.line(index);
            let (tokens, next) = if line.len() > provider.max_line_len() {
                (Vec::new(), self.entry[index])
            } else {
                provider.highlight_line(line, self.entry[index])
            };
            self.lines[index] = tokens;
            self.entry[index + 1] = next;
            self.resume += 1;
            processed += 1;

            // The convergence shortcut is only safe when every line was
            // already tokenized before this edit. During the initial budgeted
            // pass there are no cached tokens after `resume`, so stopping here
            // would leave later lines empty (plain/white).
            let converged = was_complete
                && doc.edit_span().is_some_and(|(_, last)| index >= last)
                && previous_next == Some(next);
            if converged {
                self.complete = true;
                break;
            }
        }
        if self.resume >= line_count {
            self.complete = true;
        }
        self.complete
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::model::doc::Document;
    use crate::syntax::SyntaxRegistry;

    fn fixture(tag: &str, text: &str, extension: &str) -> (PathBuf, Document) {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-highlight-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("file.{extension}"));
        fs::write(&path, text).unwrap();
        let doc = Document::open(7, &path).unwrap();
        (dir, doc)
    }

    #[test]
    fn tokenizes_rust_and_stops_at_the_edit() {
        let (dir, mut doc) = fixture("rust", "/* a\nb\n*/ let x = 1;\n", "rs");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(cache.sync(&doc, &registry, &settings, FRAME_LINE_BUDGET, None));
        assert_eq!(cache.language(), Some("rust"));
        assert_eq!(
            cache.tokens(0).unwrap()[0].kind,
            crate::syntax::TokenKind::Comment
        );
        assert_eq!(
            cache.tokens(1).unwrap()[0].kind,
            crate::syntax::TokenKind::Comment
        );
        assert!(cache
            .tokens(2)
            .unwrap()
            .iter()
            .any(|token| token.kind == crate::syntax::TokenKind::Keyword));

        // Edit the last line: only that line is re-tokenized (state converged).
        let end = doc.text().len();
        doc.set_caret(end);
        doc.insert_text("// tail");
        assert!(cache.sync(&doc, &registry, &settings, FRAME_LINE_BUDGET, None));
        assert!(cache
            .tokens(2)
            .unwrap()
            .iter()
            .any(|token| token.kind == crate::syntax::TokenKind::Comment));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn budgeted_pass_resumes_until_complete() {
        let mut text = String::new();
        for index in 0..10 {
            text.push_str(&format!("let x{index} = \"s\";\n"));
        }
        let (dir, doc) = fixture("budget", &text, "rs");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(!cache.sync(&doc, &registry, &settings, 3, None));
        assert!(cache.tokens(2).is_some());
        assert!(cache.tokens(5).is_none());
        loop {
            if cache.sync(&doc, &registry, &settings, 3, None) {
                break;
            }
        }
        assert!(cache.tokens(9).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_files_keep_the_language_but_stop_highlighting() {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-highlight-big-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.rs");
        let mut text = String::with_capacity(MAX_HIGHLIGHT_BYTES + 32);
        while text.len() <= MAX_HIGHLIGHT_BYTES {
            text.push_str("let value = 1;\n");
        }
        fs::write(&path, text).unwrap();
        let doc = Document::open(11, &path).unwrap();
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        assert!(cache.sync(
            &doc,
            &registry,
            &Settings::default(),
            FRAME_LINE_BUDGET,
            None
        ));
        assert!(!cache.is_active());
        assert_eq!(cache.language(), Some("rust"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_by_settings_or_size() {
        let (dir, doc) = fixture("off", "fn main() {}\n", "rs");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings {
            syntax: false,
            ..Settings::default()
        };
        assert!(cache.sync(&doc, &registry, &settings, 100, None));
        assert!(!cache.is_active());
        assert!(cache.tokens(0).is_none());
        assert_eq!(cache.language(), Some("rust"));
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::model::doc::Document;

    #[test]
    fn editing_after_a_budgeted_initial_pass_does_not_skip_lines() {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-highlight-large-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.gd");
        let text = "extends Node\n".to_string()
            + &(0..3000)
                .map(|i| format!("var value_{i} = {i}\n"))
                .collect::<String>();
        std::fs::write(&path, text).unwrap();
        let mut doc = Document::open(1, &path).unwrap();
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(!cache.sync(&doc, &registry, &settings, 2000, None));
        let line_start = doc.line_start(2500);
        doc.set_caret(line_start);
        doc.insert_text("var replaced = 0");
        assert!(cache.sync(&doc, &registry, &settings, 5000, None));
        assert!(
            cache.tokens(2100).is_some_and(|tokens| !tokens.is_empty()),
            "line 2100 was skipped"
        );
        assert!(
            cache.tokens(2500).is_some_and(|tokens| !tokens.is_empty()),
            "edited line was skipped"
        );
        assert!(
            cache.tokens(2600).is_some_and(|tokens| !tokens.is_empty()),
            "line 2600 was skipped"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_middle_line_keeps_following_highlighting() {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-highlight-middle-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.gd");
        let text = "extends Node\n\nfunc _ready():\n\tvar a = 1\n\tprint(\"a\")\n\nfunc two():\n\tprint(\"b\")\n\treturn 2\n";
        std::fs::write(&path, text).unwrap();
        let mut doc = Document::open(1, &path).unwrap();
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(cache.sync(&doc, &registry, &settings, 100, None));
        // Edit line 3 (inside _ready), keeping line count unchanged.
        let line_start = doc.line_start(3);
        doc.set_caret(line_start);
        doc.insert_text("var b = 2");
        assert!(cache.sync(&doc, &registry, &settings, 100, None));
        assert!(
            cache.tokens(7).is_some_and(|tokens| !tokens.is_empty()),
            "line 7 lost highlighting"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
