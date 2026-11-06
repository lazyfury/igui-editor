//! Ordered, non-overlapping styled byte spans.
//!
//! The editor paints each character exactly once: antialiased glyphs composite,
//! so painting a base line and then a colored token over it makes the token
//! heavier and its edges muddy. [`overlay`] composes the final color of every
//! byte range up front (base text, then syntax tokens, then LSP semantic
//! tokens), and the painter emits one draw per span.
//!
//! Spans are `(start, end, payload)` byte ranges, sorted ascending, contiguous
//! and disjoint. The payload is opaque to this module; the editor stores a
//! [`Color`](igui_core::Color) there.

/// Splits `spans` so `start..end` is covered by one span with `value`.
///
/// `spans` must cover the whole painted range (the caller seeds it with the
/// base text). Ranges outside the covered area are ignored; an overlay that
/// already matches a neighbor is merged so it does not add a draw call.
pub(crate) fn overlay<C: Copy + PartialEq>(
    spans: &mut Vec<(usize, usize, C)>,
    start: usize,
    end: usize,
    value: C,
) {
    if start >= end {
        return;
    }
    // First span that ends after the overlay starts.
    let Some(mut first) = spans.iter().position(|&(_, b, _)| b > start) else {
        return;
    };
    if spans[first].0 >= end {
        return;
    }
    // Split the first span when the overlay starts inside it.
    if spans[first].0 < start {
        let (a, b, payload) = spans[first];
        spans[first] = (a, start, payload);
        spans.insert(first + 1, (start, b, payload));
        first += 1;
    }
    // Split the last overlapping span when the overlay ends inside it, then
    // replace the fully covered spans with the new one.
    let mut last = first;
    while last < spans.len() && spans[last].0 < end {
        if spans[last].1 > end {
            let (a, b, payload) = spans[last];
            spans[last] = (a, end, payload);
            spans.insert(last + 1, (end, b, payload));
            last += 1;
            break;
        }
        last += 1;
    }
    spans.splice(first..last, std::iter::once((start, end, value)));

    // Merge equal neighbors so adjacent spans of one color stay one draw.
    if first + 1 < spans.len()
        && spans[first].1 == spans[first + 1].0
        && spans[first].2 == spans[first + 1].2
    {
        spans[first].1 = spans[first + 1].1;
        spans.remove(first + 1);
    }
    if first > 0 && spans[first - 1].1 == spans[first].0 && spans[first - 1].2 == spans[first].2 {
        spans[first - 1].1 = spans[first].1;
        spans.remove(first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(ranges: &[(usize, usize, u8)]) -> Vec<(usize, usize, u8)> {
        ranges.to_vec()
    }

    #[test]
    fn overlay_splits_a_span_in_the_middle() {
        let mut s = spans(&[(0, 10, 1)]);
        overlay(&mut s, 3, 6, 2);
        assert_eq!(s, spans(&[(0, 3, 1), (3, 6, 2), (6, 10, 1)]));
    }

    #[test]
    fn overlay_shares_boundaries_and_merges_equal_neighbors() {
        let mut s = spans(&[(0, 4, 1), (4, 8, 2), (8, 12, 1)]);
        overlay(&mut s, 4, 8, 1);
        assert_eq!(s, spans(&[(0, 12, 1)]));
    }

    #[test]
    fn overlay_covers_several_spans() {
        let mut s = spans(&[(0, 2, 1), (2, 4, 2), (4, 6, 3), (6, 8, 1)]);
        overlay(&mut s, 1, 7, 9);
        assert_eq!(s, spans(&[(0, 1, 1), (1, 7, 9), (7, 8, 1)]));
    }

    #[test]
    fn overlay_at_the_edges_does_not_split() {
        let mut s = spans(&[(0, 4, 1), (4, 8, 1)]);
        overlay(&mut s, 0, 4, 2);
        assert_eq!(s, spans(&[(0, 4, 2), (4, 8, 1)]));
        overlay(&mut s, 4, 8, 2);
        assert_eq!(s, spans(&[(0, 8, 2)]));
    }

    #[test]
    fn overlay_is_a_noop_outside_the_covered_range() {
        let mut s = spans(&[(2, 4, 1)]);
        overlay(&mut s, 0, 2, 2);
        overlay(&mut s, 4, 6, 2);
        overlay(&mut s, 3, 3, 2);
        assert_eq!(s, spans(&[(2, 4, 1)]));
    }

    #[test]
    fn successive_overlays_compose() {
        let mut s = spans(&[(0, 8, 1)]);
        overlay(&mut s, 2, 6, 2);
        overlay(&mut s, 4, 7, 3);
        assert_eq!(s, spans(&[(0, 2, 1), (2, 4, 2), (4, 7, 3), (7, 8, 1)]));
    }
}
