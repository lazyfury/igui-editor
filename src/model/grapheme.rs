//! Grapheme-cluster boundaries for editing commands.
//!
//! Byte offsets in [`Document`](super::doc::Document) stay on `char`
//! boundaries; these helpers additionally keep emoji ZWJ sequences, skin-tone
//! modifiers and combining marks together when the caret moves or deletes.

use unicode_segmentation::UnicodeSegmentation;

/// Start of the grapheme immediately before `byte`.
pub fn prev_boundary(text: &str, byte: usize) -> usize {
    let byte = floor_boundary(text, byte);
    text[..byte]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(start, _)| start)
}

/// End of the grapheme immediately after `byte`.
pub fn next_boundary(text: &str, byte: usize) -> usize {
    let byte = floor_boundary(text, byte);
    text[byte..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |grapheme| byte + grapheme.len())
}

fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combining_marks_and_zwj_sequences_are_one_grapheme() {
        let text = "e\u{301}x";
        assert_eq!(next_boundary(text, 0), 3);
        assert_eq!(prev_boundary(text, 3), 0);
        let emoji = "👍🏽";
        assert_eq!(next_boundary(emoji, 0), emoji.len());
        assert_eq!(prev_boundary(emoji, emoji.len()), 0);
    }
}
