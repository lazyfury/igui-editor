//! Plain-text search over a document.
//!
//! Case-insensitive matching folds character by character (ASCII fast path,
//! Unicode `to_lowercase` otherwise) and scans every character boundary, so
//! overlapping matches are reported. Whole-word mode rejects a match when the
//! neighbouring characters are word characters.
//!
//! Pure logic: no UI, no document ownership — callers pass `&str` and get byte
//! ranges back.

/// Matches retained by a single find pass. The UI reports truncation instead
/// of freezing or exhausting memory on a pathological query.
pub const MAX_MATCHES: usize = 10_000;

/// One search configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matcher {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

/// All matches of `matcher.query` in `text`, capped at [`MAX_MATCHES`].
pub fn find_all(text: &str, matcher: &Matcher) -> Vec<(usize, usize)> {
    find_all_capped(text, matcher).0
}

/// All matches plus whether the cap was reached.
///
/// The scan keeps only two boundary iterators, so a large document no longer
/// allocates a `usize` per character just to find matches.
pub fn find_all_capped(text: &str, matcher: &Matcher) -> (Vec<(usize, usize)>, bool) {
    if matcher.query.is_empty() || text.is_empty() {
        return (Vec::new(), false);
    }
    let query_chars = matcher.query.chars().count();
    let boundaries = || {
        text.char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
    };
    let starts = boundaries();
    let mut ends = boundaries();
    for _ in 0..query_chars {
        if ends.next().is_none() {
            return (Vec::new(), false);
        }
    }

    let mut matches = Vec::new();
    for start in starts {
        if start == text.len() {
            break;
        }
        let Some(end) = ends.next() else {
            break;
        };
        let candidate = &text[start..end];
        if !eq_ignore_case(candidate, &matcher.query, matcher.case_sensitive) {
            continue;
        }
        if matcher.whole_word {
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                continue;
            }
        }
        if matches.len() >= MAX_MATCHES {
            return (matches, true);
        }
        matches.push((start, end));
    }
    (matches, false)
}

fn eq_ignore_case(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        return a == b;
    }
    let mut left = a.chars().flat_map(char::to_lowercase);
    let mut right = b.chars().flat_map(char::to_lowercase);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x == y => {}
            _ => return false,
        }
    }
}

/// Start of the first whole-word, case-sensitive occurrence of `needle` at or
/// after `from`.
///
/// Used by Select Next Occurrence: it stops at the first hit so a repeated press
/// stays cheap on a large document, and it never wraps (unlike Find, the search
/// has a visible end). Overlapping hits are skipped, not returned.
pub fn next_occurrence(text: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut index = from.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    while let Some(offset) = text[index..].find(needle) {
        let start = index + offset;
        let end = start + needle.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char) {
            return Some(start);
        }
        // Step one *character* past the hit so overlapping occurrences of the
        // same word are still considered, and the loop always advances.
        index = start + needle.chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(query: &str) -> Matcher {
        Matcher {
            query: query.into(),
            case_sensitive: false,
            whole_word: false,
        }
    }

    #[test]
    fn finds_all_occurrences_case_insensitively() {
        let text = "# Notes\n\n## Notes\n- notes\n";
        let matches = find_all(text, &matcher("notes"));
        assert_eq!(matches.len(), 3);
        for (start, end) in matches {
            assert_eq!(&text[start..end].to_lowercase(), "notes");
        }
    }

    #[test]
    fn case_sensitive_mode_is_exact() {
        let text = "Notes notes NOTES";
        let matcher = Matcher {
            query: "notes".into(),
            case_sensitive: true,
            whole_word: false,
        };
        assert_eq!(find_all(text, &matcher), vec![(6, 11)]);
    }

    #[test]
    fn whole_word_rejects_partial_hits() {
        let text = "cat category cat_cat cat.";
        let matcher = Matcher {
            query: "cat".into(),
            case_sensitive: true,
            whole_word: true,
        };
        assert_eq!(find_all(text, &matcher), vec![(0, 3), (21, 24)]);
    }

    #[test]
    fn next_occurrence_advances_whole_words_only() {
        let text = "one two one one\n_one one_";
        assert_eq!(next_occurrence(text, "one", 0), Some(0));
        assert_eq!(next_occurrence(text, "one", 3), Some(8));
        assert_eq!(next_occurrence(text, "one", 11), Some(12));
        // `_one one_` has no standalone occurrence.
        assert_eq!(next_occurrence(text, "one", 16), None);
        assert_eq!(next_occurrence(text, "", 0), None);
        assert_eq!(next_occurrence(text, "one", text.len()), None);
        // A byte offset inside a character does not panic.
        assert_eq!(next_occurrence("你 one", "one", 1), Some(4));
    }

    #[test]
    fn next_occurrence_skips_overlapping_hits() {
        // Every "aa" in these inputs touches another word character.
        assert_eq!(next_occurrence("aaa", "aa", 0), None);
        assert_eq!(next_occurrence("aaaa", "aa", 0), None);
        assert_eq!(next_occurrence("aa aa", "aa", 0), Some(0));
        assert_eq!(next_occurrence("aa aa", "aa", 1), Some(3));
    }

    #[test]
    fn returns_overlapping_matches() {
        let matcher = Matcher {
            query: "aa".into(),
            case_sensitive: true,
            whole_word: false,
        };
        assert_eq!(find_all("aaaa", &matcher), vec![(0, 2), (1, 3), (2, 4)]);
    }

    #[test]
    fn handles_multibyte_text_without_splitting_chars() {
        let text = "你好，世界";
        let matches = find_all(text, &matcher("世界"));
        assert_eq!(matches.len(), 1);
        let (start, end) = matches[0];
        assert_eq!(&text[start..end], "世界");
    }

    #[test]
    fn caps_pathological_matches() {
        let text = "a".repeat(MAX_MATCHES + 5);
        let (matches, truncated) = find_all_capped(&text, &matcher("a"));
        assert_eq!(matches.len(), MAX_MATCHES);
        assert!(truncated);
    }

    #[test]
    fn exact_cap_is_not_reported_as_truncated() {
        let text = "a".repeat(MAX_MATCHES);
        let (matches, truncated) = find_all_capped(&text, &matcher("a"));
        assert_eq!(matches.len(), MAX_MATCHES);
        assert!(!truncated);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(find_all("abc", &matcher("")).is_empty());
        assert!(find_all("", &matcher("a")).is_empty());
    }
}
