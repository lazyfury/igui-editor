//! GDScript editing rules.
//!
//! The editor owns the language server; this module only answers small
//! structural questions about GDScript source text. It is pure Rust so the
//! rules are unit-testable without a window.

/// Whether `line` opens a GDScript block (`func`, `if`, `for`, `match`, …).
///
/// Comments are ignored and a `:` inside a quoted string does not count.
pub fn opens_block(line: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut end = line.len();
    for (index, ch) in line.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '#' => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    line[..end].trim_end().ends_with(':')
}

/// Whether `byte` is inside a GDScript `"""` / `'''` string.
///
/// Pressing Enter only needs this after a line ending in `:`, so the scan is
/// bounded by the document size but not paid on every keystroke. Single-line
/// strings cannot span a newline; triple-quoted strings can, and their leading
/// whitespace is content, not indentation.
pub fn in_multiline_string(text: &str, byte: usize) -> bool {
    let limit = byte.min(text.len());
    let mut chars = text[..limit].char_indices();
    let mut multiline: Option<&str> = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some((index, ch)) = chars.next() {
        if let Some(delimiter) = multiline {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if text[index..].starts_with(delimiter) {
                // Consume the other two delimiter characters.
                chars.next();
                chars.next();
                multiline = None;
            }
            continue;
        }
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
            } else if ch == '\n' {
                // An unterminated single-line string is an error; do not let it
                // make every following line look like string content.
                quote = None;
            }
            continue;
        }
        match ch {
            '#' => {
                for (_, next) in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '"' | '\'' => {
                if text[index..].starts_with("\"\"\"") {
                    chars.next();
                    chars.next();
                    multiline = Some("\"\"\"");
                } else if text[index..].starts_with("'''") {
                    chars.next();
                    chars.next();
                    multiline = Some("'''");
                } else {
                    quote = Some(ch);
                }
            }
            _ => {}
        }
    }
    multiline.is_some()
}

// -- native outline fallback ------------------------------------------------

/// Kind of a declaration found by [`outline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Class,
    Function,
    Signal,
    Constant,
    Variable,
    Enum,
}

/// One row of the native GDScript outline (used when no LSP symbols exist).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRow {
    pub name: String,
    pub kind: OutlineKind,
    /// Zero-based indentation level (a tab or four spaces per level).
    pub depth: usize,
    /// Zero-based source line.
    pub line: u32,
}

/// Scans GDScript declarations for the outline fallback.
///
/// This is deliberately line-based and tolerant: it lists top-level members and
/// inner-class members, skips function locals, comments and multiline strings,
/// and ignores annotations (`@export`, `@onready`, `@rpc`, …). The language
/// server still wins whenever it is attached; this only keeps the panel useful
/// for files opened without Godot running.
pub fn outline(text: &str) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    let mut multiline: Option<&'static str> = None;
    let mut function_depth: Option<usize> = None;
    for (index, raw) in text.lines().enumerate() {
        if let Some(delimiter) = multiline {
            if count_unescaped(raw, delimiter) % 2 == 1 {
                multiline = None;
            }
            continue;
        }
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let depth = indent_depth(raw);
        if let Some(open) = function_depth {
            if depth <= open {
                function_depth = None;
            } else {
                // Function body: locals and nested blocks are not outline rows.
                continue;
            }
        }

        if let Some(row) = declaration(raw, index as u32, depth) {
            if row.kind == OutlineKind::Function {
                function_depth = Some(depth);
            }
            rows.push(row);
        }

        // `var body = """` and friends make every following line string
        // content; declarations start again after the closing delimiter.
        if let Some(delimiter) = opens_multiline(raw) {
            multiline = Some(delimiter);
        }
    }
    rows
}

fn declaration(raw: &str, line: u32, depth: usize) -> Option<OutlineRow> {
    let mut rest = strip_annotations(raw.trim_start());
    if let Some(without) = rest.strip_prefix("static") {
        rest = without.trim_start();
    }
    let (kind, rest) = [
        ("class_name", OutlineKind::Class),
        ("class", OutlineKind::Class),
        ("func", OutlineKind::Function),
        ("signal", OutlineKind::Signal),
        ("const", OutlineKind::Constant),
        ("var", OutlineKind::Variable),
        ("enum", OutlineKind::Enum),
    ]
    .into_iter()
    .find_map(|(word, kind)| keyword(rest, word).map(|tail| (kind, tail)))?;
    let name = identifier(rest)?;
    Some(OutlineRow {
        name: name.to_string(),
        kind,
        depth,
        line,
    })
}

/// `keyword` followed by whitespace (so `class` never matches `class_name`).
fn keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?;
    let next = rest.chars().next()?;
    next.is_whitespace().then_some(rest.trim_start())
}

fn identifier(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let end = text
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(text.len());
    (end > 0).then(|| &text[..end])
}

/// Skips leading `@annotation` / `@annotation(args)` tokens.
fn strip_annotations(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if !text.starts_with('@') {
            return text;
        }
        let mut end = 1;
        for (offset, ch) in text[1..].char_indices() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                end = 1 + offset + ch.len_utf8();
            } else {
                break;
            }
        }
        text = text[end..].trim_start();
        if text.starts_with('(') {
            let mut depth = 0usize;
            let mut close = None;
            for (offset, ch) in text.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            close = Some(offset + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match close {
                Some(end) => text = text[end..].trim_start(),
                None => return "",
            }
        }
    }
}

fn indent_depth(line: &str) -> usize {
    let mut columns = 0usize;
    for ch in line.chars() {
        match ch {
            '\t' => columns += 4,
            ' ' => columns += 1,
            _ => break,
        }
        if columns > 64 {
            break;
        }
    }
    (columns / 4).min(16)
}

/// If the line opens a triple-quoted string that outlives it, returns the
/// delimiter. One-line `"""…"""` values do not open a block.
fn opens_multiline(line: &str) -> Option<&'static str> {
    let mut index = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while index < line.len() {
        let ch = line[index..].chars().next()?;
        if escaped {
            escaped = false;
            index += ch.len_utf8();
            continue;
        }
        if ch == '\\' {
            escaped = true;
            index += ch.len_utf8();
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
            index += ch.len_utf8();
            continue;
        }
        if ch == '#' {
            return None;
        }
        if line[index..].starts_with("\"\"\"") || line[index..].starts_with("'''") {
            let delimiter = if line[index..].starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            let after = &line[index + delimiter.len()..];
            match find_unescaped(after, delimiter) {
                Some(close) => index += delimiter.len() + close + delimiter.len(),
                None => return Some(delimiter),
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        }
        index += ch.len_utf8();
    }
    None
}

fn count_unescaped(line: &str, needle: &str) -> usize {
    let mut count = 0usize;
    let mut offset = 0usize;
    while let Some(found) = find_unescaped(&line[offset..], needle) {
        count += 1;
        offset += found + needle.len();
        if offset >= line.len() {
            break;
        }
    }
    count
}

fn find_unescaped(line: &str, needle: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if line[index..].starts_with(needle) {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_openers_ignore_comments_and_strings() {
        assert!(opens_block("func _ready():"));
        assert!(opens_block("    if x > 0:  "));
        assert!(opens_block("match value:"));
        assert!(!opens_block("var x: int"));
        assert!(!opens_block("print('a:')"));
        assert!(!opens_block("# note:"));
        assert!(!opens_block("x := {"));
    }

    #[test]
    fn outline_lists_members_and_skips_function_locals() {
        let text = "class_name Brick\nextends Node\n\nsignal died(reason)\n\
                    const MAX_SPEED = 12\n\
                    @export var health: int = 100\n\
                    @onready var sprite = $Sprite\n\
                    var velocity := Vector2.ZERO\n\
                    enum State { IDLE, RUN }\n\n\
                    class Inner extends Node:\n\
                    \tvar inner_value = 1\n\
                    \tfunc inner_ready():\n\
                    \t\tvar local = 2\n\n\
                    func _ready():\n\
                    \tvar local = 1\n\
                    \tif true:\n\
                    \t\tvar deeper = 3\n\n\
                    static func make():\n\
                    \tpass\n";
        let summary: Vec<(String, OutlineKind, usize, u32)> = outline(text)
            .into_iter()
            .map(|row| (row.name, row.kind, row.depth, row.line))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Brick".into(), OutlineKind::Class, 0, 0),
                ("died".into(), OutlineKind::Signal, 0, 3),
                ("MAX_SPEED".into(), OutlineKind::Constant, 0, 4),
                ("health".into(), OutlineKind::Variable, 0, 5),
                ("sprite".into(), OutlineKind::Variable, 0, 6),
                ("velocity".into(), OutlineKind::Variable, 0, 7),
                ("State".into(), OutlineKind::Enum, 0, 8),
                ("Inner".into(), OutlineKind::Class, 0, 10),
                ("inner_value".into(), OutlineKind::Variable, 1, 11),
                ("inner_ready".into(), OutlineKind::Function, 1, 12),
                ("_ready".into(), OutlineKind::Function, 0, 15),
                ("make".into(), OutlineKind::Function, 0, 20),
            ]
        );
    }

    #[test]
    fn outline_skips_comments_strings_and_anonymous_enums() {
        let text = "# func fake():\n\
                    var body = \"\"\"\n\
                    func inside_string():\n\
                    \"\"\"\n\
                    var one = \"\"\"closed\"\"\"\n\
                    enum { A, B }\n\
                    func real():\n\
                    \tpass\n";
        let rows = outline(text);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["body", "one", "real"]);
    }

    #[test]
    fn outline_handles_annotations_and_signals() {
        let text = "@tool\n\
                    @export_range(0, 10) var speed = 4\n\
                    @warning_ignore(\"unused_signal\")\n\
                    signal changed(value: int)\n";
        let rows = outline(text);
        let summary: Vec<(&str, OutlineKind)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.kind))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("speed", OutlineKind::Variable),
                ("changed", OutlineKind::Signal),
            ]
        );
    }

    #[test]
    fn multiline_strings_are_detected_for_typing_rules() {
        let doc = "var text := \"\"\"\nUsage:\n  run it\n\"\"\"\nfunc f():\n";
        let usage = doc.find("Usage:").unwrap() + "Usage:".len();
        let run = doc.find("run it").unwrap();
        let after = doc.find("func f():").unwrap();
        assert!(in_multiline_string(doc, usage));
        assert!(in_multiline_string(doc, run));
        assert!(!in_multiline_string(doc, after));
        assert!(!in_multiline_string(doc, 0));

        // Same-line strings, comments and escaped quotes do not open a block.
        assert!(!in_multiline_string("var x := \"a: b\"\n", 14));
        assert!(!in_multiline_string("# \"\"\" not a string\nUsage:\n", 25));
        assert!(!in_multiline_string("var x := \"\\\"\"\nUsage:\n", 15));
        assert!(in_multiline_string("'\'\'\'\ndoc\n", 10));
        assert!(!in_multiline_string("\"\"\"closed\"\"\"\nUsage:\n", 17));
        // An unterminated single-line string must not leak into later lines.
        assert!(!in_multiline_string("var x := \"oops\nUsage:\n", 20));
    }
}
