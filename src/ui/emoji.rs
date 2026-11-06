//! Painting color emoji inside text runs.
//!
//! The engine draws text as glyphs; a color emoji face has bitmaps and no
//! outlines, so this module splits a run at emoji code points and paints the
//! cached sprite (see [`crate::state::EmojiStore`]) at the advance the layout
//! already reserved. Because the advance is measured exactly like text, the
//! caret, selection, soft wrap and hit-testing need no emoji-specific code.
//!
//! Only **base** code points get an image. Joiners, variation selectors and
//! skin-tone modifiers still advance by their measured width but paint nothing
//! (the system face's own per-code-point advance keeps the line consistent).
//! See [`crate::model::emoji`] for the scope note on ZWJ sequences.

use igui_core::{Color, Rect, Size, Vec2};
use igui_render::{Paint, PaintContext};
use igui_ui::TextMeasurer;

use crate::fonts;
use crate::model::emoji;
use crate::state::EmojiStore;

/// Paints `text` with color emoji and returns the advance it occupied.
///
/// `origin` is the baseline start (the same point `draw_text_in` expects). The
/// caller keeps its own x accounting; the return value is convenience for
/// family-run loops and must equal the measured width of `text`.
#[allow(clippy::too_many_arguments)]
pub fn paint_text(
    ctx: &mut PaintContext,
    store: &EmojiStore,
    measurer: &dyn TextMeasurer,
    text: &str,
    origin: Vec2,
    font_size: f32,
    family: Option<&str>,
    color: Color,
) -> f32 {
    let mut x = origin.x;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        let next = text[index + ch.len_utf8()..].chars().next();
        if !emoji::is_emoji(ch, next) {
            continue;
        }
        if start < index {
            let run = &text[start..index];
            ctx.draw_text_in(
                run,
                Vec2::new(x, origin.y),
                font_size,
                igui_core::FontWeight::NORMAL,
                igui_render::TextAlign::Left,
                family,
                color,
            );
            x += measurer.measure_run_weighted_in(
                run,
                font_size,
                igui_core::FontWeight::NORMAL,
                family,
            );
        }
        let advance = fonts::advance(measurer, ch, font_size, family);
        if let Some(sprite) = store.sprite(ch, font_size) {
            // An emoji fills roughly one em; keep it inside its advance so a
            // narrow fallback never overlaps the next character.
            let side = advance.min(font_size * 1.15).max(font_size * 0.6);
            let left = x + (advance - side) * 0.5;
            // Baseline-anchored box: the ink sits on the text baseline with a
            // small descender, matching the line box without re-baselining.
            let top = origin.y - side * 0.82;
            ctx.draw_image(
                sprite.texture,
                Rect::from_min_size(Vec2::new(left, top), Size::new(side, side)),
                None,
                Paint::default(),
            );
        }
        x += advance;
        start = index + ch.len_utf8();
    }
    if start < text.len() {
        let run = &text[start..];
        ctx.draw_text_in(
            run,
            Vec2::new(x, origin.y),
            font_size,
            igui_core::FontWeight::NORMAL,
            igui_render::TextAlign::Left,
            family,
            color,
        );
        x +=
            measurer.measure_run_weighted_in(run, font_size, igui_core::FontWeight::NORMAL, family);
    }
    x - origin.x
}

/// Paints one terminal cell's contents as an emoji sprite when it is one.
///
/// Returns `false` when the cell is ordinary text (the caller then draws it),
/// so the terminal keeps its cell/grid code unchanged for normal output.
#[allow(
    dead_code,
    reason = "wired into the terminal painter in the next commit"
)]
pub fn paint_cell(
    ctx: &mut PaintContext,
    store: &EmojiStore,
    text: &str,
    rect: Rect,
    font_size: f32,
) -> bool {
    let Some(ch) = text
        .chars()
        .find(|ch| emoji::is_emoji(*ch, Some('\u{FE0F}')))
    else {
        return false;
    };
    let Some(sprite) = store.sprite(ch, font_size) else {
        return false;
    };
    let side = rect.size.width.min(rect.size.height).min(font_size * 1.3);
    if side <= 0.0 {
        return false;
    }
    let destination = Rect::from_center_size(rect.center(), Size::new(side, side));
    ctx.draw_image(sprite.texture, destination, None, Paint::default());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe;

    impl TextMeasurer for Probe {
        fn advance(&self, ch: char, font_size: f32) -> f32 {
            if emoji::is_emoji(ch, None) {
                font_size
            } else if ch.is_ascii() {
                font_size * 0.6
            } else {
                font_size
            }
        }

        fn line_height(&self, font_size: f32) -> f32 {
            font_size * 1.4
        }
    }

    #[test]
    fn plain_text_is_one_run() {
        let store = EmojiStore::default();
        let mut ctx = PaintContext::new();
        let width = paint_text(
            &mut ctx,
            &store,
            &Probe,
            "hello",
            Vec2::new(10.0, 20.0),
            10.0,
            None,
            Color::WHITE,
        );
        assert!((width - 30.0).abs() < 1e-4);
        let texts = ctx
            .draw_list()
            .iter()
            .filter(|command| matches!(command, igui_render::DrawCommand::DrawText { .. }))
            .count();
        assert_eq!(texts, 1);
    }

    #[test]
    fn emoji_splits_the_run_and_keeps_the_advance() {
        let store = EmojiStore::default();
        let mut ctx = PaintContext::new();
        let width = paint_text(
            &mut ctx,
            &store,
            &Probe,
            "a😀b",
            Vec2::new(0.0, 20.0),
            10.0,
            None,
            Color::WHITE,
        );
        // 6 + 10 + 6 px, independent of whether the host has an emoji face.
        assert!((width - 22.0).abs() < 1e-4);
        let texts: Vec<&str> = ctx
            .draw_list()
            .iter()
            .filter_map(|command| match command {
                igui_render::DrawCommand::DrawText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["a", "b"]);
        assert!(!texts.iter().any(|text| text.contains('😀')));
    }

    #[test]
    fn component_code_points_do_not_split_or_paint() {
        let store = EmojiStore::default();
        let mut ctx = PaintContext::new();
        paint_text(
            &mut ctx,
            &store,
            &Probe,
            "👍🏽",
            Vec2::new(0.0, 20.0),
            10.0,
            None,
            Color::WHITE,
        );
        let texts: Vec<&str> = ctx
            .draw_list()
            .iter()
            .filter_map(|command| match command {
                igui_render::DrawCommand::DrawText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["🏽"], "the modifier stays in the text pass");
    }
}
