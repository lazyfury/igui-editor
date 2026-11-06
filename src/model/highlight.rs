//! Incremental syntax-highlight cache.
//!
//! The cache stores, per line, the tokens and the *entry state* the line was
//! tokenized with. After an edit it re-tokenizes from the first touched line and
//! stops as soon as the outgoing state matches the cached one **and** the edit
//! range is behind it — that is the classic "state converged, the rest is
//! unchanged" shortcut, so typing in a large file does not rehighlight
//! everything.
//!
//! Convergence is only trusted up to the *frontier*: the number of lines whose
//! entry states were produced by this revision before the edit. During the
//! initial budgeted pass the frontier is behind the document, so an edit that
//! converges early resumes the catch-up from the frontier instead of stopping —
//! otherwise typing at the top of a large file would restart the same budgeted
//! pass every frame and never finish. The same frontier keeps the tokenized
//! prefix alive when an edit changes the line count (newline/join), instead of
//! discarding the whole cache.
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
        // Lines `0..frontier` were tokenized with the entry states now in
        // `entry`, so their tokens survive an edit that converges.
        let mut frontier = if was_complete {
            line_count
        } else {
            self.resume.min(line_count)
        };
        if self.doc != doc.id()
            || self.provider_name != Some(provider.name())
            // `clear` keeps the name but drops the provider (syntax turned off
            // or re-enabled), so the cached tokens are stale either way.
            || self.provider.is_none()
        {
            was_complete = false;
            frontier = 0;
            self.doc = doc.id();
            self.rev = doc.rev();
            self.provider_name = Some(provider.name());
            self.provider = Some(provider);
            self.lines = vec![Vec::new(); line_count];
            self.entry = vec![0; line_count + 1];
            self.resume = 0;
            self.complete = false;
            self.disabled = false;
        } else if self.lines.len() != line_count {
            // An edit added or removed lines. The tokenized prefix is still
            // valid: the vectors are resized in place and the pass resumes at
            // the first touched line instead of retokenizing the document.
            let first = doc
                .edit_span()
                .map_or(0, |(first, _)| first)
                .min(line_count);
            self.lines.resize(line_count, Vec::new());
            self.entry.resize(line_count + 1, 0);
            self.rev = doc.rev();
            self.resume = first.min(frontier);
            frontier = self.resume;
            was_complete = false;
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
                frontier.min(first)
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

            // Convergence is only trusted when the cached state after this
            // line was produced with the same entry state, i.e. the line is
            // inside the frontier. Beyond it there is no cached suffix, so a
            // matching (stale) entry means nothing.
            let converged = doc.edit_span().is_some_and(|(_, last)| index >= last)
                && previous_next == Some(next)
                && index < frontier;
            if !converged {
                continue;
            }
            if was_complete {
                self.complete = true;
                break;
            }
            // The edit settled on a cached state: every line before the
            // frontier keeps its tokens, so resume the catch-up from there
            // instead of retokenizing the same lines on the next frame.
            self.resume = frontier;
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
    fn typing_on_the_first_line_converges_locally() {
        let mut text = String::new();
        for index in 0..200 {
            text.push_str(&format!("const x{index} = \"s\";\n"));
        }
        let (dir, mut doc) = fixture("first-line", &text, "js");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(cache.sync(&doc, &registry, &settings, FRAME_LINE_BUDGET, None));
        doc.set_caret(0);
        doc.insert_text("x");
        assert!(cache.sync(&doc, &registry, &settings, FRAME_LINE_BUDGET, None));
        assert!(
            cache.resume <= 2,
            "typing on line 0 re-tokenized {} lines",
            cache.resume
        );
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
    fn typing_at_the_top_of_a_large_file_finishes_the_catch_up_pass() {
        // The first pass is budgeted, so it spans several frames. An edit at
        // line 0 converges immediately; the pass must still resume from the
        // frontier and reach the end instead of restarting from the top.
        let mut text = String::new();
        for index in 0..3000 {
            text.push_str(&format!("let x{index} = \"s\";\n"));
        }
        let (dir, mut doc) = fixture("top-typing", &text, "rs");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(!cache.sync(&doc, &registry, &settings, 1000, None));
        assert!(cache.tokens(2999).is_none());

        doc.set_caret(0);
        let mut rounds = 0;
        loop {
            doc.insert_text("x");
            rounds += 1;
            if cache.sync(&doc, &registry, &settings, 1000, None) {
                break;
            }
            assert!(
                rounds < 8,
                "typing at line 0 restarted the catch-up pass forever ({rounds} rounds)"
            );
        }
        assert!(
            cache.tokens(2999).is_some_and(|tokens| !tokens.is_empty()),
            "the last line was never tokenized"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newline_keeps_the_tokenized_prefix() {
        let mut text = String::new();
        for index in 0..100 {
            text.push_str(&format!("let x{index} = \"s\";\n"));
        }
        let (dir, mut doc) = fixture("newline-prefix", &text, "rs");
        let mut cache = HighlightCache::new();
        let registry = SyntaxRegistry::with_builtins();
        let settings = Settings::default();
        assert!(cache.sync(&doc, &registry, &settings, 5000, None));

        let start = doc.line_start(50);
        doc.set_caret(start);
        doc.insert_text("\n");
        // One line of budget: only the edited line can be re-tokenized, so the
        // lines before it must still come from the previous pass.
        assert!(!cache.sync(&doc, &registry, &settings, 1, None));
        assert!(
            cache.tokens(10).is_some_and(|tokens| !tokens.is_empty()),
            "a line count change discarded the tokenized prefix"
        );
        // The rest is caught up without touching the prefix again.
        assert!(cache.sync(&doc, &registry, &settings, 5000, None));
        assert!(cache.tokens(100).is_some());
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
