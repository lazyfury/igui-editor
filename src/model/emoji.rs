//! Color emoji: classification and bitmap extraction from a system emoji font.
//!
//! The engine's text stack rasterizes outlines only (`ab_glyph`), so an emoji
//! face — Apple Color Emoji is an `sbix` bitmap font on macOS, Noto Color Emoji
//! is `CBDT` on Linux, Segoe UI Emoji is `COLR`/bitmap on Windows — advances the
//! caret but paints no ink. Rather than change the engine, the app reads the
//! bitmap itself and paints it as an image at the advance the layout already
//! reserved (see [`crate::ui::emoji`]).
//!
//! This module is pure: classification and decoding take bytes and return
//! pixels. Discovery and the texture cache live in `state/`.
//!
//! Scope: one image per **base code point**. Emoji modifiers, variation
//! selectors and joiners are skipped, so `👍🏽` paints one thumb and `👨‍👩‍👧` paints
//! its three components side by side. That matches the layout, which measures
//! per code point; shaping a joined sequence into one glyph would desync the
//! caret from the paint.

use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

/// Environment override for tests, portable installs and unsupported hosts.
pub const FONT_ENV: &str = "IGUI_EMOJI_FONT";

/// Platform emoji font files, first hit wins.
fn platform_fonts() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &[
            "/System/Library/Fonts/Apple Color Emoji.ttc",
            "/System/Library/Fonts/Supplemental/Apple Color Emoji.ttc",
        ]
    }
    #[cfg(target_os = "linux")]
    {
        &[
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/emoji/NotoColorEmoji.ttf",
            "/usr/share/fonts/truetype/noto-emoji/NotoColorEmoji.ttf",
        ]
    }
    #[cfg(target_os = "windows")]
    {
        &[r"C:\Windows\Fonts\seguiemj.ttf"]
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        &[]
    }
}

/// Candidate emoji font files: the environment override first, then the
/// platform list. Missing files are skipped by [`EmojiFont::discover`].
pub fn font_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os(FONT_ENV) {
        if !path.is_empty() {
            out.push(PathBuf::from(path));
        }
    }
    out.extend(platform_fonts().iter().map(PathBuf::from));
    out
}

/// Why a code point is (or is not) painted as color emoji.
///
/// Kept as a value so tests can describe the classification instead of
/// asserting two loose booleans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmojiKind {
    /// Emoji by default even without `FE0F` (`😀`, `🇨`).
    Presentation,
    /// Text by default; needs a following `FE0F` (`❤️`, `☀️`).
    TextWithSelector,
    /// A zero-width component (joiner, modifier, variation selector, tag).
    Component,
    /// Ordinary text.
    Text,
}

/// Classifies one code point. `next` is the following character, used to spot
/// the emoji variation selector that promotes a text-default symbol.
pub fn classify(ch: char, next: Option<char>) -> EmojiKind {
    if is_component(ch) {
        return EmojiKind::Component;
    }
    if is_default_presentation(ch) {
        return EmojiKind::Presentation;
    }
    if is_text_default(ch) {
        if next == Some('\u{FE0F}') {
            return EmojiKind::TextWithSelector;
        }
        return EmojiKind::Text;
    }
    EmojiKind::Text
}

/// Whether `ch` is painted as color emoji (base or promoted symbol).
pub fn is_emoji(ch: char, next: Option<char>) -> bool {
    matches!(
        classify(ch, next),
        EmojiKind::Presentation | EmojiKind::TextWithSelector
    )
}

/// Zero-width/component code points: skipped when painting, never their own
/// image.
pub fn is_component(ch: char) -> bool {
    matches!(
        ch as u32,
        0xFE0E | 0xFE0F            // variation selectors (text/emoji)
            | 0x200D               // zero-width joiner
            | 0x20E3               // combining enclosing keycap
            | 0x1F3FB..=0x1F3FF    // skin-tone modifiers
            | 0x1F9B0..=0x1F9B3    // red/hair components
            | 0xE0020..=0xE007F    // tag characters (subdivision flags)
    )
}

/// Unicode `Emoji_Presentation` ranges: color even without `FE0F`.
fn is_default_presentation(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1F000..=0x1FAFF      // emoji, supplemental symbols, flags, cards
            | 0x231A..=0x231B  // watch, hourglass
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
    )
}

/// Text-default symbols that become emoji when followed by `FE0F`.
///
/// The list covers the symbol blocks the emoji data marks as `Emoji` but not
/// `Emoji_Presentation`; it deliberately does not promote ordinary dingbats
/// such as `✓` unless the document asks for emoji presentation.
fn is_text_default(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00A9
            | 0x00AE
            | 0x203C
            | 0x2049
            | 0x2122
            | 0x2139
            | 0x2194..=0x2199
            | 0x21A9..=0x21AA
            | 0x2328
            | 0x23CF
            | 0x23ED..=0x23EF
            | 0x23F1..=0x23F2
            | 0x23F8..=0x23FA
            | 0x24C2
            | 0x25AA..=0x25AB
            | 0x25B6
            | 0x25C0
            | 0x25FB..=0x25FC
            | 0x2600..=0x2604
            | 0x260E
            | 0x2611
            | 0x2618
            | 0x261D
            | 0x2620
            | 0x2622..=0x2623
            | 0x2626
            | 0x262A
            | 0x262E..=0x262F
            | 0x2638..=0x263A
            | 0x2640
            | 0x2642
            | 0x265F..=0x2660
            | 0x2663
            | 0x2665..=0x2666
            | 0x2668
            | 0x267B
            | 0x267E
            | 0x2692..=0x2697
            | 0x2699
            | 0x269B..=0x269C
            | 0x26A0
            | 0x26A7
            | 0x26B0..=0x26B1
            | 0x26C8
            | 0x26CF
            | 0x26D1
            | 0x26D3
            | 0x26E9
            | 0x26F0..=0x26F1
            | 0x26F4
            | 0x26F7..=0x26F9
            | 0x26FB..=0x26FC
            | 0x2702
            | 0x2708..=0x2709
            | 0x270C..=0x270D
            | 0x270F
            | 0x2712
            | 0x2714
            | 0x2716
            | 0x271D
            | 0x2721
            | 0x2733..=0x2734
            | 0x2744
            | 0x2747
            | 0x2763..=0x2764
            | 0x27A1
            | 0x2934..=0x2935
            | 0x2B05..=0x2B07
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
    )
}

/// One decoded color glyph, ready to upload as an RGBA8 texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiBitmap {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8, row-major.
    pub rgba: Vec<u8>,
    /// Strike density the bitmap was designed for (device pixels per em).
    pub pixels_per_em: u16,
}

/// The process-wide color emoji face, discovered once (mmap is virtual until a
/// glyph page is touched).
pub fn system_font() -> Option<&'static EmojiFont> {
    static FONT: std::sync::OnceLock<Option<EmojiFont>> = std::sync::OnceLock::new();
    FONT.get_or_init(EmojiFont::discover).as_ref()
}

/// A memory-mapped color emoji face.
///
/// The font file is never copied: Apple Color Emoji is ~183 MB, and only the
/// pages backing the glyphs actually painted are touched. The mapping is leaked
/// once per process (one face for the whole app), matching the engine's own
/// lazy face loading.
pub struct EmojiFont {
    face: ttf_parser::Face<'static>,
    path: PathBuf,
}

impl EmojiFont {
    /// Loads the first readable candidate from [`font_candidates`].
    pub fn discover() -> Option<Self> {
        font_candidates()
            .into_iter()
            .find_map(|path| Self::load(&path))
    }

    /// Loads one specific font file (single-face TTC index 0).
    pub fn load(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let mapping = unsafe { Mmap::map(&file) }.ok()?;
        let mapping: &'static Mmap = Box::leak(Box::new(mapping));
        let face = ttf_parser::Face::parse(&mapping[..], 0).ok()?;
        Some(Self {
            face,
            path: path.to_path_buf(),
        })
    }

    /// The file the face was loaded from (for status text and tests).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the face has a glyph for `ch`.
    pub fn covers(&self, ch: char) -> bool {
        self.face.glyph_index(ch).is_some()
    }

    /// Decodes the color bitmap for `ch` at `ppem` device pixels per em.
    ///
    /// `None` means the face has no raster image for the code point (a JPEG or
    /// outline-only glyph) — the caller then paints nothing for it.
    pub fn bitmap(&self, ch: char, ppem: u16) -> Option<EmojiBitmap> {
        let glyph = self.face.glyph_index(ch)?;
        let image = self.face.glyph_raster_image(glyph, ppem.max(1))?;
        let (width, height, rgba) = decode_raster(
            image.format,
            image.data,
            image.width as u32,
            image.height as u32,
        )?;
        if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
            return None;
        }
        Some(EmojiBitmap {
            width,
            height,
            rgba,
            pixels_per_em: image.pixels_per_em,
        })
    }
}

impl std::fmt::Debug for EmojiFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmojiFont")
            .field("path", &self.path)
            .finish()
    }
}

/// Decodes one `RasterGlyphImage` payload into straight-alpha RGBA8.
fn decode_raster(
    format: ttf_parser::RasterImageFormat,
    data: &[u8],
    width: u32,
    height: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    use ttf_parser::RasterImageFormat;
    let pixels = width as usize * height as usize;
    match format {
        RasterImageFormat::PNG => decode_png(data),
        RasterImageFormat::BitmapPremulBgra32 => {
            if width == 0 || height == 0 || data.len() != pixels * 4 {
                return None;
            }
            let mut rgba = Vec::with_capacity(data.len());
            for pixel in data.chunks_exact(4) {
                let (blue, green, red, alpha) = (pixel[0], pixel[1], pixel[2], pixel[3]);
                let unpremultiply = |value: u8| -> u8 {
                    if alpha == 0 {
                        0
                    } else {
                        ((value as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
                    }
                };
                rgba.extend_from_slice(&[
                    unpremultiply(red),
                    unpremultiply(green),
                    unpremultiply(blue),
                    alpha,
                ]);
            }
            Some((width, height, rgba))
        }
        RasterImageFormat::BitmapGray8 => {
            if width == 0 || height == 0 || data.len() != pixels {
                return None;
            }
            let mut rgba = Vec::with_capacity(pixels * 4);
            for &gray in data {
                rgba.extend_from_slice(&[gray, gray, gray, 255]);
            }
            Some((width, height, rgba))
        }
        _ => None,
    }
}

/// Decodes a PNG payload into RGBA8 (expanding palette/gray and stripping 16-bit
/// samples).
fn decode_png(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(bytes.len() / 3 * 4);
            for pixel in bytes.chunks_exact(3) {
                out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(bytes.len() * 4);
            for &gray in bytes {
                out.extend_from_slice(&[gray, gray, gray, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(bytes.len() / 2 * 4);
            for pixel in bytes.chunks_exact(2) {
                out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            out
        }
        png::ColorType::Indexed => return None,
    };
    let expected = info.width as usize * info.height as usize * 4;
    (rgba.len() == expected).then_some((info.width, info.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_emoji_presentation_and_text_symbols() {
        assert_eq!(classify('😀', None), EmojiKind::Presentation);
        assert_eq!(classify('👍', None), EmojiKind::Presentation);
        assert_eq!(classify('🇨', None), EmojiKind::Presentation);
        assert_eq!(classify('❤', Some('\u{FE0F}')), EmojiKind::TextWithSelector);
        assert_eq!(classify('❤', None), EmojiKind::Text);
        assert_eq!(classify('☀', Some('\u{FE0F}')), EmojiKind::TextWithSelector);
        assert_eq!(classify('✓', Some('\u{FE0F}')), EmojiKind::Text);
        assert_eq!(classify('中', None), EmojiKind::Text);
        assert_eq!(classify('a', None), EmojiKind::Text);
    }

    #[test]
    fn components_are_never_painted_on_their_own() {
        for ch in [
            '\u{FE0F}',
            '\u{FE0E}',
            '\u{200D}',
            '\u{20E3}',
            '\u{1F3FB}',
            '\u{1F3FF}',
            '\u{1F9B0}',
            '\u{E0067}',
        ] {
            assert_eq!(classify(ch, None), EmojiKind::Component, "{ch:?}");
            assert!(!is_emoji(ch, Some('\u{FE0F}')), "{ch:?}");
        }
    }

    #[test]
    fn skin_tone_and_joiner_sequences_keep_one_base_each() {
        let thumbs: Vec<char> = "👍🏽".chars().collect();
        let painted: Vec<char> = thumbs
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, ch)| is_emoji(*ch, thumbs.get(index + 1).copied()))
            .map(|(_, ch)| ch)
            .collect();
        assert_eq!(painted, vec!['👍']);

        let family: Vec<char> = "👨‍👩‍👧".chars().collect();
        let painted: Vec<char> = family
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, ch)| is_emoji(*ch, family.get(index + 1).copied()))
            .map(|(_, ch)| ch)
            .collect();
        assert_eq!(painted, vec!['👨', '👩', '👧']);
    }

    #[test]
    fn font_candidates_prefer_the_environment_override() {
        let previous = std::env::var_os(FONT_ENV);
        std::env::set_var(FONT_ENV, "/tmp/igui-test-emoji.ttf");
        let candidates = font_candidates();
        match previous {
            Some(value) => std::env::set_var(FONT_ENV, value),
            None => std::env::remove_var(FONT_ENV),
        }
        assert_eq!(
            candidates.first().map(PathBuf::as_path),
            Some(Path::new("/tmp/igui-test-emoji.ttf"))
        );
    }

    /// A 2×2 RGBA PNG (red/green top row, blue/white bottom row).
    const PNG_2X2: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x0c, 0x81, 0x34, 0x18, 0x00, 0x00, 0x49, 0xc8, 0x09, 0xf7, 0x03,
        0xd9, 0x64, 0xf1, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn decodes_png_payloads_to_straight_rgba() {
        let (width, height, rgba) = decode_png(PNG_2X2).expect("fixture decodes");
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            rgba,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]
        );
        assert!(decode_png(b"not a png").is_none());
    }

    #[test]
    fn unpremultiplies_bgra_color_bitmaps() {
        // Half-transparent full red, premultiplied: B=0 G=0 R=128 A=128.
        let data = [0u8, 0, 128, 128];
        let (width, height, rgba) = decode_raster(
            ttf_parser::RasterImageFormat::BitmapPremulBgra32,
            &data,
            1,
            1,
        )
        .expect("premul bitmap decodes");
        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba, vec![255, 0, 0, 128]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_color_emoji_yields_a_color_bitmap() {
        let Some(font) = EmojiFont::discover() else {
            return; // No color emoji face on this host: nothing to verify.
        };
        assert!(font.covers('😀'));
        let bitmap = font.bitmap('😀', 32).expect("Apple Color Emoji has 😀");
        assert!(bitmap.width > 8 && bitmap.height > 8, "{bitmap:?}");
        assert_eq!(
            bitmap.rgba.len(),
            bitmap.width as usize * bitmap.height as usize * 4
        );
        assert!(
            bitmap.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0),
            "the bitmap carried no ink"
        );
        assert!(
            bitmap
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2]),
            "the bitmap was not color"
        );
    }
}
