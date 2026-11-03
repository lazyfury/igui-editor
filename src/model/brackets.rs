//! Bracket matching for the editor's caret.
//!
//! Text-scan based, with a caller-provided `is_code` predicate so the editor can
//! skip brackets that live inside comments or strings (it feeds the syntax
//! cache). Scans are capped so a pathological file cannot stall a keystroke.

/// Maximum bytes scanned in each direction.
pub const SCAN_LIMIT: usize = 20_000;

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

fn pair_of(ch: char) -> Option<(char, char)> {
    PAIRS
        .iter()
        .copied()
        .find(|(open, close)| *open == ch || *close == ch)
}

/// Matches the bracket at (or immediately before) `caret`.
///
/// Returns the byte positions of both brackets, open first. `is_code(byte)`
/// decides whether a position may participate (false inside strings/comments).
pub fn match_bracket(
    text: &str,
    caret: usize,
    mut is_code: impl FnMut(usize) -> bool,
) -> Option<(usize, usize)> {
    let caret = caret.min(text.len());
    let candidate = |position: usize, ch: char| -> Option<(usize, char)> {
        pair_of(ch)?;
        Some((position, ch))
    };
    let (index, ch) = if let Some(ch) = text[caret..].chars().next() {
        if let Some(found) = candidate(caret, ch) {
            found
        } else {
            let previous = previous_char_start(text, caret)?;
            candidate(previous, text[previous..].chars().next()?)?
        }
    } else {
        let previous = previous_char_start(text, caret)?;
        candidate(previous, text[previous..].chars().next()?)?
    };

    let (open, close) = pair_of(ch)?;
    if !is_code(index) {
        return None;
    }
    let mut depth = 0usize;
    if ch == open {
        let mut scanned = 0usize;
        for (offset, candidate) in text[index..].char_indices() {
            scanned += candidate.len_utf8();
            if scanned > SCAN_LIMIT {
                return None;
            }
            let position = index + offset;
            if !is_code(position) {
                continue;
            }
            if candidate == open {
                depth += 1;
            } else if candidate == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((index, position));
                }
            }
        }
        None
    } else {
        let mut scanned = 0usize;
        depth = 1;
        for (position, candidate) in text[..index].char_indices().rev() {
            scanned += candidate.len_utf8();
            if scanned > SCAN_LIMIT {
                return None;
            }
            if !is_code(position) {
                continue;
            }
            if candidate == close {
                depth += 1;
            } else if candidate == open {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((position, index));
                }
            }
        }
        None
    }
}

fn previous_char_start(text: &str, byte: usize) -> Option<usize> {
    if byte == 0 {
        return None;
    }
    let mut index = byte - 1;
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(_: usize) -> bool {
        true
    }

    #[test]
    fn matches_forward_and_backward() {
        let text = "fn main() { call(arg); }";
        let open = text.find('(').unwrap();
        let close = text.find(')').unwrap();
        assert_eq!(match_bracket(text, open, code), Some((open, close)));
        assert_eq!(match_bracket(text, close + 1, code), Some((open, close)));
        let brace = text.find('{').unwrap();
        assert_eq!(
            match_bracket(text, brace, code),
            Some((brace, text.rfind('}').unwrap()))
        );
    }

    #[test]
    fn nesting_and_non_brackets() {
        let text = "a(b[c]d)e";
        let b = text.find('[').unwrap();
        assert_eq!(match_bracket(text, b, code), Some((b, b + 2)));
        assert_eq!(match_bracket(text, text.find('x').unwrap_or(0), code), None);
        assert_eq!(match_bracket("plain", 0, code), None);
    }

    #[test]
    fn respects_the_code_predicate() {
        let text = "(\")\")";
        let open = 0;
        let quote = text.find('"').unwrap();
        let inner = text.find(')').unwrap();
        // The first `)` sits inside a string: the predicate excludes it.
        let pairs = |byte: usize| byte != inner && byte != quote;
        assert_eq!(
            match_bracket(text, open, pairs),
            Some((open, text.rfind(')').unwrap()))
        );
    }

    #[test]
    fn unicode_boundaries_are_safe() {
        let text = "你(好)";
        let open = text.find('(').unwrap();
        assert_eq!(
            match_bracket(text, open, code),
            Some((open, text.find(')').unwrap()))
        );
    }
}
