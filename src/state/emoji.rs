//! Color emoji sprites and their backend textures.
//!
//! The pure side lives in [`crate::model::emoji`]: classification, the
//! memory-mapped face and bitmap decoding. This module caches decoded glyphs as
//! textures so a repaint never decodes a PNG again.
//!
//! Painters call [`EmojiStore::sprite`] with the text's font size; the store
//! derives the device ppem from the window scale factor and returns a texture
//! id plus its native pixel size. The host installs the upload closure in
//! `EditorApp::init` (headless runs leave it unset and still record the
//! `DrawImage` command).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use igui_render::TextureId;

use crate::model::emoji;

/// One color glyph, cached for the process lifetime of the texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmojiSprite {
    pub texture: TextureId,
    /// Native bitmap size in pixels (the painter scales it to the em box).
    pub width: u32,
    pub height: u32,
}

type Uploader = Box<dyn FnMut(TextureId, u32, u32, &[u8]) -> Result<(), String>>;

/// Cap on cached glyphs. A document or terminal scroll can show at most a few
/// hundred distinct emoji; clearing the cache bounds GPU memory instead of
/// unbounded growth after a long session.
const MAX_SPRITES: usize = 1024;

struct Cache {
    sprites: HashMap<(char, u16), Option<EmojiSprite>>,
    next_id: u32,
    scale: f32,
    uploader: Option<Uploader>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            sprites: HashMap::new(),
            next_id: 1,
            scale: 1.0,
            uploader: None,
        }
    }
}

/// Decoded color glyphs for the current scale.
#[derive(Default)]
pub struct EmojiStore {
    cache: RefCell<Cache>,
}

impl EmojiStore {
    /// Installs the texture upload path (the windowed host owns the backend).
    pub fn set_uploader(&mut self, uploader: Uploader) {
        self.cache.get_mut().uploader = Some(uploader);
    }

    /// Updates the device pixel ratio used to pick a font strike.
    pub fn set_scale(&self, scale: f32) {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let mut cache = self.cache.borrow_mut();
        if (cache.scale - scale).abs() > 0.01 {
            cache.scale = scale;
            // Keys include ppem, so stale entries would never be reused; drop
            // them rather than keep two DPI versions of every glyph.
            cache.sprites.clear();
        }
    }

    /// Whether a color emoji face was found on this host.
    pub fn available(&self) -> bool {
        emoji::system_font().is_some()
    }

    /// The emoji face used (for status text and diagnostics).
    pub fn font_path(&self) -> Option<&'static Path> {
        emoji::system_font().map(emoji::EmojiFont::path)
    }

    /// Device pixels per em for `font_size` at the current scale.
    fn ppem(&self, font_size: f32) -> u16 {
        let scale = self.cache.borrow().scale;
        (font_size * scale).round().clamp(8.0, 512.0) as u16
    }

    /// Returns (and lazily decodes/uploads) the sprite for `ch` at `font_size`.
    ///
    /// The result is cached even when it is `None`, so a face without a bitmap
    /// for the code point is not re-probed every frame.
    pub fn sprite(&self, ch: char, font_size: f32) -> Option<EmojiSprite> {
        let ppem = self.ppem(font_size);
        let mut cache = self.cache.borrow_mut();
        if let Some(entry) = cache.sprites.get(&(ch, ppem)) {
            return *entry;
        }
        let sprite = emoji::system_font().and_then(|font| {
            let bitmap = font.bitmap(ch, ppem)?;
            let texture = TextureId::new(cache.next_id.max(1));
            cache.next_id = texture.raw().saturating_add(1);
            if let Some(uploader) = cache.uploader.as_mut() {
                if uploader(texture, bitmap.width, bitmap.height, &bitmap.rgba).is_err() {
                    return None;
                }
            }
            Some(EmojiSprite {
                texture,
                width: bitmap.width,
                height: bitmap.height,
            })
        });
        if cache.sprites.len() >= MAX_SPRITES {
            cache.sprites.clear();
        }
        cache.sprites.insert((ch, ppem), sprite);
        sprite
    }
}

impl std::fmt::Debug for EmojiStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache = self.cache.borrow();
        f.debug_struct("EmojiStore")
            .field("cached", &cache.sprites.len())
            .field("scale", &cache.scale)
            .field("available", &emoji::system_font().is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_sprite_is_the_same_texture() {
        let Some(ch) = ['😀', '🚀', '🎉']
            .into_iter()
            .find(|ch| emoji::system_font().is_some_and(|font| font.covers(*ch)))
        else {
            return; // No color emoji face on this host.
        };
        let store = EmojiStore::default();
        let first = store.sprite(ch, 13.0).expect("first sprite");
        let second = store.sprite(ch, 13.0).expect("cached sprite");
        assert_eq!(first, second);
        assert!(first.texture.is_valid());
    }

    #[test]
    fn a_different_strike_gets_its_own_entry() {
        let Some(ch) = ['😀', '🚀', '🎉']
            .into_iter()
            .find(|ch| emoji::system_font().is_some_and(|font| font.covers(*ch)))
        else {
            return;
        };
        let store = EmojiStore::default();
        let small = store.sprite(ch, 13.0).expect("small sprite");
        store.set_scale(2.0);
        let large = store.sprite(ch, 13.0).expect("large sprite");
        assert_ne!(small.texture, large.texture);
    }
}
