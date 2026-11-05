//! Typing rules beyond plain insertion.
//!
//! Two behaviours live here so the editor's input handlers stay thin and every
//! rule is unit-tested without a window:
//!
//! - **Auto-pairing**: typing `(` / `[` / `{` inserts both halves and puts the
//!   caret between them, typing a closer that is already there steps over it,
//!   and a selection is wrapped instead of replaced.
//! - **Markdown list continuation**: Enter continues `- ` / `1. ` / `> ` and
//!   task-list markers, and an empty item ends the list instead of growing it.
//!
//! Deliberately absent: `'` pairing (Rust lifetimes, GDScript escapes and
//! English contractions all argue against it) and `*` / `_` emphasis pairing in
//! Markdown (it fights emphasis, lists and horizontal rules).

/// Bracket pairs the editor closes automatically, in any language.
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// Quotes the editor closes automatically.
///
/// Unlike brackets they only pair in code ([`Context::in_code`]) and never
/// directly after a word character, so a string literal never gets a second
/// quote appended and `don"` stays plain text.
const QUOTES: [char; 2] = ['"', '`'];

/// Quotes whose *second* half is deleted together with the first.
///
/// `'` is included here even though it never pairs automatically: an empty `''`
/// is still a pair worth deleting in one keystroke.
const SIDED_QUOTES: [char; 3] = ['"', '\'', '`'];

/// What the editor should do with one typed character.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Insert {
    /// Insert the character as typed.
    Plain,
    /// Insert both halves and put the caret between them.
    Pair(char, char),
    /// Wrap the selection in the pair and keep it selected.
    Surround(char, char),
    /// The caret already sits on the closing character: step over it.
    Skip,
}

/// Everything the pairing rules need to know about the caret's surroundings.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The caret sits in code: not inside a comment or a string token. The
    /// caller probes the character *before* the caret, because the syntax cache
    /// has not seen the character being typed yet.
    pub in_code: bool,
}

/// Decides what typing `ch` at (`caret`, `anchor`) should do in `text`.
pub fn decide(text: &str, caret: usize, anchor: usize, ch: char, ctx: Context) -> Insert {
    let caret = caret.min(text.len());
    let anchor = anchor.min(text.len());
    let selected = caret != anchor;
    let next = text[caret..].chars().next();
    let previous = text[..caret].chars().next_back();

    if let Some((open, close)) = bracket_of(ch) {
        if ch == close {
            // A closer that is already there is stepped over, not doubled.
            return if next == Some(close) && !selected {
                Insert::Skip
            } else {
                Insert::Plain
            };
        }
        if selected {
            return Insert::Surround(open, close);
        }
        return Insert::Pair(open, close);
    }

    if QUOTES.contains(&ch) {
        if selected {
            return Insert::Surround(ch, ch);
        }
        // Closing a quote that is already there (including ending a string
        // literal the syntax cache still sees as open).
        if next == Some(ch) {
            return Insert::Skip;
        }
        // Inside a string/comment the quote is content, not a new literal.
        if !ctx.in_code {
            return Insert::Plain;
        }
        // `don"t`, `a``b`: a quote in the middle of a word never opens a pair.
        if previous.is_some_and(is_word) {
            return Insert::Plain;
        }
        return Insert::Pair(ch, ch);
    }

    Insert::Plain
}

/// The pair around an empty `open`/`close` at `caret`, if there is one.
///
/// Used by the deleting rules (Backspace, Enter inside a pair) and therefore
/// also accepts `'`, which is never inserted automatically.
pub fn pair_around(text: &str, caret: usize) -> Option<(char, char)> {
    let caret = caret.min(text.len());
    let open = text[..caret].chars().next_back()?;
    let close = text[caret..].chars().next()?;
    if let Some(pair) = bracket_of(open) {
        return (pair.0 == open && pair.1 == close).then_some(pair);
    }
    SIDED_QUOTES
        .contains(&open)
        .then_some((open, open))
        .filter(|_| open == close)
}

/// Whether `ch` opens one of the bracket pairs.
pub fn is_bracket_open(ch: char) -> bool {
    BRACKETS.iter().any(|(open, _)| *open == ch)
}

fn bracket_of(ch: char) -> Option<(char, char)> {
    BRACKETS
        .iter()
        .copied()
        .find(|(open, close)| *open == ch || *close == ch)
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// A Markdown list item line split at its marker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListItem {
    /// Leading whitespace, repeated on the next line.
    pub indent: String,
    /// Marker text repeated on the next line (`"- "`, `"12. "`, `"- [ ] "`).
    pub marker: String,
    /// Whether Enter continues the list; `false` for an empty item, which ends
    /// it instead.
    pub continue_list: bool,
}

/// Splits the text of the current line up to the caret into a list item.
///
/// Returns `None` for anything that is not a list item, so callers can fall
/// back to a plain newline.
pub fn list_item(line: &str) -> Option<ListItem> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let (marker, content) = split_marker(rest)?;
    Some(ListItem {
        indent: indent.into(),
        marker,
        continue_list: !content.trim().is_empty(),
    })
}

/// Splits the markers off a Markdown line: block quote depth, then a list
/// marker (`-`, `*`, `+`, `1.`, `1)`, optionally a task checkbox).
///
/// The returned marker is the text the *next* line starts with, so an ordered
/// item is already incremented.
fn split_marker(rest: &str) -> Option<(String, &str)> {
    let mut marker = String::new();
    let mut rest = rest;
    let mut quoted = false;
    // Block quotes nest: `> > - item` continues as `> > - `.
    while let Some(after) = rest.strip_prefix('>') {
        quoted = true;
        marker.push_str("> ");
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    if let Some((bullet, after)) = list_marker(rest) {
        marker.push_str(&bullet);
        // Task items continue unchecked, like every other editor.
        let after = match task_box(after) {
            Some(after) => {
                marker.push_str("[ ] ");
                after
            }
            None => after,
        };
        return Some((marker, after));
    }
    quoted.then_some((marker, rest))
}

/// Splits a bullet (`-`, `*`, `+`) or ordered (`1.`, `1)`) marker off `rest`,
/// returning the marker to repeat on the next line.
fn list_marker(rest: &str) -> Option<(String, &str)> {
    match rest.chars().next()? {
        ch @ ('-' | '*' | '+') => {
            let after = rest[ch.len_utf8()..].strip_prefix(' ')?;
            Some((format!("{ch} "), after))
        }
        ch if ch.is_ascii_digit() => {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let after_digits = &rest[digits..];
            let delimiter = after_digits.chars().next()?;
            if delimiter != '.' && delimiter != ')' {
                return None;
            }
            let number = rest[..digits].parse::<u64>().ok()?;
            let after = after_digits[delimiter.len_utf8()..].strip_prefix(' ')?;
            Some((format!("{}{delimiter} ", number + 1), after))
        }
        _ => None,
    }
}

/// Strips a task-list checkbox (`[ ]`, `[x]`, `[X]`) and its trailing space.
fn task_box(rest: &str) -> Option<&str> {
    let after = rest.strip_prefix('[')?;
    if !matches!(after.chars().next()?, ' ' | 'x' | 'X') {
        return None;
    }
    after[1..].strip_prefix("] ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: Context = Context { in_code: true };
    const STRING: Context = Context { in_code: false };

    #[test]
    fn brackets_pair_and_closers_step_over() {
        assert_eq!(decide("", 0, 0, '(', CODE), Insert::Pair('(', ')'));
        assert_eq!(decide("{", 1, 1, '}', CODE), Insert::Plain);
        assert_eq!(decide("{}", 1, 1, '}', CODE), Insert::Skip);
        // Without a caret between the halves a closer is plain text.
        assert_eq!(decide("{}", 2, 2, '}', CODE), Insert::Plain);
        assert_eq!(decide("x", 1, 1, ']', CODE), Insert::Plain);
    }

    #[test]
    fn brackets_pair_even_inside_strings_and_comments() {
        // Prose and comments are full of parentheses; predictability wins.
        assert_eq!(decide("// ", 3, 3, '(', STRING), Insert::Pair('(', ')'));
        assert_eq!(decide("note ", 5, 5, '[', STRING), Insert::Pair('[', ']'));
    }

    #[test]
    fn a_selection_is_wrapped_instead_of_replaced() {
        assert_eq!(decide("abc", 1, 3, '(', CODE), Insert::Surround('(', ')'));
        assert_eq!(decide("abc", 0, 3, '"', CODE), Insert::Surround('"', '"'));
        // A closer replaces the selection like any other character.
        assert_eq!(decide("abc", 0, 3, ')', CODE), Insert::Plain);
    }

    #[test]
    fn quotes_need_code_and_a_word_boundary() {
        assert_eq!(decide("", 0, 0, '"', CODE), Insert::Pair('"', '"'));
        assert_eq!(decide("let s = ", 8, 8, '"', CODE), Insert::Pair('"', '"'));
        // Inside a string literal a quote is content.
        assert_eq!(decide("let s = \"a", 11, 11, '"', STRING), Insert::Plain);
        // Mid-word it never opens a pair.
        assert_eq!(decide("don", 3, 3, '"', CODE), Insert::Plain);
        // `'` never pairs.
        assert_eq!(decide("", 0, 0, '\'', CODE), Insert::Plain);
    }

    #[test]
    fn quote_closers_step_over_even_outside_code() {
        assert_eq!(decide("\"abc\"", 4, 4, '"', STRING), Insert::Skip);
        assert_eq!(decide("`x`", 2, 2, '`', CODE), Insert::Skip);
    }

    #[test]
    fn empty_pairs_are_found_around_the_caret() {
        assert_eq!(pair_around("()", 1), Some(('(', ')')));
        assert_eq!(pair_around("{\"\"}", 2), Some(('"', '"')));
        assert_eq!(pair_around("''", 1), Some(('\'', '\'')));
        assert_eq!(pair_around("(x)", 1), None);
        assert_eq!(pair_around("(]", 1), None);
        assert_eq!(pair_around("你(好)", 0), None);
        assert!(is_bracket_open('{'));
        assert!(!is_bracket_open('"'));
    }

    #[test]
    fn list_markers_continue_with_the_next_number() {
        assert_eq!(
            list_item("- one"),
            Some(ListItem {
                indent: String::new(),
                marker: "- ".into(),
                continue_list: true,
            })
        );
        assert_eq!(list_item("1. one").unwrap().marker, "2. ");
        assert_eq!(list_item("9) nine").unwrap().marker, "10) ");
        assert_eq!(list_item("  * one").unwrap().indent, "  ");
        assert_eq!(list_item("> quote").unwrap().marker, "> ");
        assert_eq!(list_item("> > - deep").unwrap().marker, "> > - ");
        assert_eq!(list_item("- [x] done").unwrap().marker, "- [ ] ");
        assert_eq!(list_item("- [ ] todo").unwrap().marker, "- [ ] ");
    }

    #[test]
    fn an_empty_list_item_ends_the_list() {
        let item = list_item("- ").expect("empty bullet");
        assert!(!item.continue_list);
        let item = list_item("1. ").expect("empty number");
        assert!(!item.continue_list);
        let item = list_item("  -   ").expect("empty indented bullet");
        assert!(!item.continue_list);
        assert_eq!(item.indent, "  ");
    }

    #[test]
    fn non_list_lines_are_ignored() {
        assert_eq!(list_item("text"), None);
        assert_eq!(list_item("---"), None);
        assert_eq!(list_item("-1 degree"), None);
        assert_eq!(list_item("# heading"), None);
        assert_eq!(list_item(""), None);
        assert_eq!(list_item("1.5 kg"), None);
    }
}
