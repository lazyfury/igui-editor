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
