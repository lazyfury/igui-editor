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
}
