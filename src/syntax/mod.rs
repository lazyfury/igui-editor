//! Syntax highlighting: the **extension API** and the built-in providers.
//!
//! This module is the public seam for language support. A provider is a plain
//! value implementing [`SyntaxProvider`]; the host owns a [`SyntaxRegistry`] and
//! can register more providers at runtime (a future plugin host would do exactly
//! that — no plugin loading is implemented here):
//!
//! ```no_run
//! use std::sync::Arc;
//! use igui_editor::syntax::{Highlight, SyntaxProvider, SyntaxRegistry, TokenKind};
//!
//! struct MyLang;
//! impl SyntaxProvider for MyLang {
//!     fn name(&self) -> &'static str { "mylang" }
//!     fn extensions(&self) -> &'static [&'static str] { &["mylang", "ml"] }
//!     fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
//!         if line.starts_with("//") {
//!             return (vec![Highlight { start: 0, end: line.len(), kind: TokenKind::Comment }], 0);
//!         }
//!         let _ = state;
//!         (Vec::new(), 0)
//!     }
//! }
//!
//! let mut registry = SyntaxRegistry::with_builtins();
//! registry.register(Arc::new(MyLang));
//! assert!(registry.for_path(Some(std::path::Path::new("x.mylang"))).is_some());
//! ```
//!
//! ## Contract
//!
//! - Highlighting is **line based**: [`SyntaxProvider::highlight_line`] receives
//!   one line (without its newline) and a `state` carried from the previous
//!   line, and returns the tokens for this line plus the state for the next one.
//! - `state == 0` is the neutral start of a line. A provider must encode every
//!   multi-line construct (block comments, raw strings, fences) in that `u32`,
//!   so the cache can detect "the state converged" and stop re-highlighting.
//! - Tokens are half-open **byte ranges** inside the line, sorted and disjoint.
//!   Anything not covered stays plain text.
//! - `max_line_len()` lets a provider opt out of pathological lines.
//!
//! ## Declarative packs
//!
//! Data-only languages do not need code: drop a JSON file under
//! `<config>/languages/` and the app registers it after the built-ins via
//! [`load_language_packs`]. See [`LanguagePack`] for the fields and limits; a
//! pack can never shadow a built-in name.

use std::path::Path;
use std::sync::Arc;

mod cstyle;
mod markdown;
mod pack;

pub use cstyle::{Cpp, GdScript, Go, JavaScript, Json, Rust, Toml, TypeScript, C};
pub use markdown::Markdown;
pub use pack::{load_language_packs, LanguagePack, PackString};

/// How a token is colored. The renderer maps these to theme colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Plain,
    Keyword,
    TypeName,
    Function,
    String,
    Number,
    Comment,
    Constant,
    /// Preprocessor directives, attributes/annotations, Rust macros, fences.
    Macro,
    Punctuation,
}

impl TokenKind {
    /// Whether the renderer paints this kind with its own color.
    pub fn is_styled(self) -> bool {
        !matches!(self, TokenKind::Plain)
    }
}

/// One styled byte range inside a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// A language provider: pure, synchronous, and safe to share.
pub trait SyntaxProvider: Send + Sync + 'static {
    /// Stable identifier, shown in the status bar (e.g. `"rust"`).
    fn name(&self) -> &'static str;

    /// Lowercase file extensions without the dot.
    fn extensions(&self) -> &'static [&'static str];

    /// Highlights one line and returns the carried state for the next line.
    fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32);

    /// The neutral state at the start of a document (always `0` unless a
    /// provider needs a different sentinel).
    fn neutral_state(&self) -> u32 {
        0
    }

    /// Lines longer than this are returned unstyled (a guard against minified
    /// JS / generated files).
    fn max_line_len(&self) -> usize {
        4000
    }
}

/// The provider registry. Built-ins are registered by
/// [`SyntaxRegistry::with_builtins`]; extensions can [`register`] more.
///
/// [`register`]: SyntaxRegistry::register
#[derive(Clone, Default)]
pub struct SyntaxRegistry {
    providers: Vec<Arc<dyn SyntaxProvider>>,
}

impl SyntaxRegistry {
    /// An empty registry (no language support).
    pub fn new() -> Self {
        Self::default()
    }

    /// Every built-in provider: Rust, C, C++, JavaScript, TypeScript, Go,
    /// GDScript, JSON, TOML and Markdown.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(Rust));
        registry.register(Arc::new(C));
        registry.register(Arc::new(Cpp));
        registry.register(Arc::new(JavaScript));
        registry.register(Arc::new(TypeScript));
        registry.register(Arc::new(Go));
        registry.register(Arc::new(GdScript));
        registry.register(Arc::new(Json));
        registry.register(Arc::new(Toml));
        registry.register(Arc::new(Markdown));
        registry
    }

    /// Adds a provider. Registering the same name twice keeps the first one
    /// (a plugin cannot silently override a built-in).
    pub fn register(&mut self, provider: Arc<dyn SyntaxProvider>) {
        if self.by_name(provider.name()).is_none() {
            self.providers.push(provider);
        }
    }

    /// The provider for a file path, matched by lowercase extension.
    pub fn for_path(&self, path: Option<&Path>) -> Option<Arc<dyn SyntaxProvider>> {
        let extension = path
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())?
            .to_ascii_lowercase();
        self.provider_for_extension(&extension)
    }

    /// The provider for a bare extension (`"rs"`, `"ts"`).
    pub fn provider_for_extension(&self, extension: &str) -> Option<Arc<dyn SyntaxProvider>> {
        self.providers
            .iter()
            .find(|provider| {
                provider
                    .extensions()
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
            .cloned()
    }

    /// A provider by [`SyntaxProvider::name`].
    pub fn by_name(&self, name: &str) -> Option<Arc<dyn SyntaxProvider>> {
        self.providers
            .iter()
            .find(|provider| provider.name() == name)
            .cloned()
    }

    /// Every registered provider, in registration order.
    pub fn providers(&self) -> &[Arc<dyn SyntaxProvider>] {
        &self.providers
    }

    /// Provider names, in registration order (for a settings/picker UI).
    pub fn names(&self) -> Vec<&'static str> {
        self.providers
            .iter()
            .map(|provider| provider.name())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn kinds(line: &str, provider: &dyn SyntaxProvider) -> Vec<(String, TokenKind)> {
        let (tokens, _) = provider.highlight_line(line, 0);
        tokens
            .into_iter()
            .map(|token| (line[token.start..token.end].to_string(), token.kind))
            .collect()
    }

    #[test]
    fn registry_matches_extensions_and_allows_extension() {
        let mut registry = SyntaxRegistry::with_builtins();
        assert!(registry
            .for_path(Some(Path::new("a.rs")))
            .is_some_and(|p| p.name() == "rust"));
        assert!(registry
            .for_path(Some(Path::new("b.TS")))
            .is_some_and(|p| p.name() == "typescript"));
        assert!(registry.for_path(Some(Path::new("c.unknown"))).is_none());
        assert!(registry.by_name("rust").is_some());

        struct Dummy;
        impl SyntaxProvider for Dummy {
            fn name(&self) -> &'static str {
                "dummy"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &["dummy"]
            }
            fn highlight_line(&self, _line: &str, state: u32) -> (Vec<Highlight>, u32) {
                (Vec::new(), state)
            }
        }
        registry.register(Arc::new(Dummy));
        assert!(registry
            .provider_for_extension("dummy")
            .is_some_and(|p| p.name() == "dummy"));
        assert!(registry.by_name("dummy").is_some());
        let names = registry.names();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"dummy"));
    }

    #[test]
    fn rust_lexes_keywords_strings_and_comments() {
        let tokens = kinds("fn main() { let s = \"hi\"; // note", &Rust);
        assert!(tokens.contains(&("fn".into(), TokenKind::Keyword)));
        assert!(tokens.contains(&("main".into(), TokenKind::Function)));
        assert!(tokens.contains(&("\"hi\"".into(), TokenKind::String)));
        assert!(tokens.contains(&("// note".into(), TokenKind::Comment)));
    }

    #[test]
    fn rust_raw_strings_continue_across_lines() {
        let line = "let a = r#\"raw";
        let (tokens, state) = Rust.highlight_line(line, 0);
        assert!(state != 0, "the raw string must carry state");
        assert!(tokens
            .iter()
            .any(|token| token.kind == TokenKind::String && token.end == line.len()));
        let (tokens, state) = Rust.highlight_line("still raw\"#;", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(
            &"still raw\"#;"[tokens[0].start..tokens[0].end],
            "still raw\"#"
        );
    }

    #[test]
    fn rust_nested_block_comments_carry_depth() {
        let (tokens, state) = Rust.highlight_line("/* outer /* inner", 0);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        let (tokens, state) = Rust.highlight_line("still */ here", state);
        assert_ne!(state, 0, "one nested level is still open");
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        let (tokens, state) = Rust.highlight_line("end */ let y = 1;", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Keyword));
    }

    #[test]
    fn rust_distinguishes_char_literals_from_lifetimes() {
        let tokens = kinds("fn f<'a>(c: char) { let x = 'a'; }", &Rust);
        assert!(tokens.contains(&("'a".into(), TokenKind::TypeName)));
        assert!(tokens.contains(&("'a'".into(), TokenKind::String)));
    }

    #[test]
    fn c_preprocessor_lines_are_macros() {
        let tokens = kinds("#include <stdio.h>", &C);
        assert_eq!(tokens[0].0, "#include <stdio.h>");
        assert_eq!(tokens[0].1, TokenKind::Macro);
    }

    #[test]
    fn go_raw_strings_are_multiline() {
        let (tokens, state) = Go.highlight_line("s := `line one", 0);
        assert_eq!(state, 2);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::String);
        let (tokens, state) = Go.highlight_line("line two`", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::String);
    }

    #[test]
    fn javascript_template_literals_carry_state() {
        let (_, state) = JavaScript.highlight_line("const a = `hello ${name}", 0);
        assert_eq!(state, 2);
        let (tokens, state) = JavaScript.highlight_line("done`;", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::String);
    }

    #[test]
    fn gdscript_uses_hash_comments_and_annotations() {
        let tokens = kinds("@export var speed: float = 1.0 # fast", &GdScript);
        assert!(tokens.contains(&("@export".into(), TokenKind::Macro)));
        assert!(tokens.contains(&("var".into(), TokenKind::Keyword)));
        assert!(tokens.contains(&("float".into(), TokenKind::TypeName)));
        assert!(tokens.contains(&("# fast".into(), TokenKind::Comment)));
    }

    #[test]
    fn json_colors_constants_and_numbers() {
        let tokens = kinds("{\"on\": true, \"n\": 12.5}", &Json);
        assert!(tokens.contains(&("true".into(), TokenKind::Constant)));
        assert!(tokens.contains(&("12.5".into(), TokenKind::Number)));
        assert!(tokens.contains(&("\"on\"".into(), TokenKind::String)));
    }

    #[test]
    fn toml_handles_tables_and_multiline_strings() {
        let tokens = kinds("[package] # header", &Toml);
        assert!(tokens.contains(&("[package]".into(), TokenKind::TypeName)));
        assert!(tokens
            .iter()
            .any(|(text, kind)| text == "# header" && *kind == TokenKind::Comment));
        let (tokens, state) = Toml.highlight_line("text = \"\"\"line", 0);
        assert_eq!(state, 3);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::String);
        let (_, state) = Toml.highlight_line("more\"\"\"", state);
        assert_eq!(state, 0);
    }

    #[test]
    fn cpp_raw_strings_carry_state_with_an_empty_delimiter() {
        let (tokens, state) = Cpp.highlight_line("auto s = R\"(raw", 0);
        assert!(state != 0, "an unterminated raw string carries state");
        assert_eq!(tokens.last().unwrap().kind, TokenKind::String);
        let (tokens, state) = Cpp.highlight_line("more)\" + \";\"", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::String);
    }

    #[test]
    fn typescript_types_and_keywords() {
        let tokens = kinds("interface A { value: string; }", &TypeScript);
        assert!(tokens.contains(&("interface".into(), TokenKind::Keyword)));
        assert!(tokens.contains(&("A".into(), TokenKind::TypeName)));
        assert!(tokens.contains(&("string".into(), TokenKind::TypeName)));
    }

    #[test]
    fn markdown_highlights_headings_fences_and_inline_code() {
        let tokens = kinds("## Title", &Markdown);
        assert_eq!(tokens[0].1, TokenKind::Keyword);
        let (_, state) = Markdown.highlight_line("```rust", 0);
        assert_eq!(state, 1);
        let (tokens, state) = Markdown.highlight_line("fn main() {}", state);
        assert_eq!(state, 1);
        assert_eq!(tokens[0].kind, TokenKind::String);
        let (tokens, state) = Markdown.highlight_line("```", state);
        assert_eq!(state, 0);
        assert_eq!(tokens[0].kind, TokenKind::Macro);
        let tokens = kinds("use `code` here", &Markdown);
        assert!(tokens.contains(&("`code`".into(), TokenKind::String)));
    }
}
