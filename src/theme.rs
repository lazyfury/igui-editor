//! The app's own theme: compact metrics, square corners, terminal accents.
//!
//! This is deliberately *not* `igui_theme::DefaultTheme`. The editor paints
//! every surface itself (`igui_ui` only), so it owns the palette and the
//! metrics: no radius anywhere, one 28 px row rhythm and a phosphor-green
//! accent on a near-black background. Editor line height comes from the
//! editor settings, not from this chrome rhythm.

use igui_core::Color;

use crate::syntax::TokenKind;

/// `0xRRGGBB` -> opaque [`Color`] (const so palette tables stay literal).
pub const fn hex(v: u32) -> Color {
    Color::new(
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
        1.0,
    )
}

/// Appearance. Both are hand-tuned; neither is a token swap of the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Dark,
    Light,
}

/// Every color the editor paints.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Page background (editor canvas, empty chrome).
    pub bg: Color,
    /// Sidebar / menubar / status bar.
    pub panel: Color,
    /// Active tab, popup surfaces.
    pub raised: Color,
    /// Split lines and panel borders.
    pub border: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text (line numbers, inactive tabs, hints).
    pub dim: Color,
    /// Tertiary text (placeholders, separators).
    pub faint: Color,
    /// Terminal accent: caret, dirty marks, active tab marker, focus.
    pub accent: Color,
    /// Selection fill behind text.
    pub selection: Color,
    /// Current editor line fill.
    pub current_line: Color,
    /// Gutter strip.
    pub gutter: Color,
    /// Hover fill for rows / menu items / tabs.
    pub hover: Color,
    /// Pressed / selected fill for rows.
    pub active: Color,
    pub error: Color,
    /// Syntax token colors (see [`Theme::token_color`]).
    pub syntax: SyntaxPalette,
}

/// Colors for syntax tokens plus LSP semantic categories. `Plain` is not
/// stored: the renderer uses the editor's base text color for it.
#[derive(Clone, Copy, Debug)]
pub struct SyntaxPalette {
    pub keyword: Color,
    pub type_name: Color,
    pub function: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub constant: Color,
    pub macro_: Color,
    pub punctuation: Color,
    /// LSP semantic tokens: local variables, parameters and fields.
    pub variable: Color,
    pub parameter: Color,
    pub property: Color,
}

impl Palette {
    /// Near-black, cool, phosphor green. The default.
    pub const fn dark() -> Self {
        Self {
            bg: hex(0x0B0E13),
            panel: hex(0x0E1218),
            raised: hex(0x121821),
            border: hex(0x1D2531),
            text: hex(0xC7D4E4),
            dim: hex(0x8494A8),
            faint: hex(0x4E5C71),
            accent: hex(0x39E0AE),
            selection: hex(0x16362F),
            current_line: hex(0x10151D),
            gutter: hex(0x0D1117),
            hover: hex(0x151C26),
            active: hex(0x19222E),
            error: hex(0xF07178),
            syntax: SyntaxPalette {
                keyword: hex(0xC792EA),
                type_name: hex(0x7DD3FC),
                function: hex(0x82E2A6),
                string: hex(0xE6C889),
                number: hex(0xF78C6C),
                comment: hex(0x55637A),
                constant: hex(0xFF9E64),
                macro_: hex(0x39E0AE),
                punctuation: hex(0x8494A8),
                variable: hex(0xB8C7DA),
                parameter: hex(0xF6C177),
                property: hex(0x9CDCFE),
            },
        }
    }

    /// Cool paper, ink, deep teal accent.
    pub const fn light() -> Self {
        Self {
            bg: hex(0xF7F9FC),
            panel: hex(0xEDF1F6),
            raised: hex(0xFFFFFF),
            border: hex(0xCDD7E3),
            text: hex(0x16202C),
            dim: hex(0x5C6C7E),
            faint: hex(0x94A2B2),
            accent: hex(0x0A8F6F),
            selection: hex(0xC9EFE4),
            current_line: hex(0xEFF4F9),
            gutter: hex(0xEEF2F7),
            hover: hex(0xE3EAF2),
            active: hex(0xDCE6F1),
            error: hex(0xC0392B),
            syntax: SyntaxPalette {
                keyword: hex(0x8B3FA8),
                type_name: hex(0x0369A1),
                function: hex(0x0F7B4F),
                string: hex(0x956400),
                number: hex(0xB45309),
                comment: hex(0x93A1B1),
                constant: hex(0xC2410C),
                macro_: hex(0x0A8F6F),
                punctuation: hex(0x5C6C7E),
                variable: hex(0x334155),
                parameter: hex(0xB45309),
                property: hex(0x0369A1),
            },
        }
    }
}

/// Pixel metrics. Compact by construction: nothing here is a "comfortable"
/// default, and the editor line height is independent of the UI rhythm.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub font_ui: f32,
    pub font_small: f32,
    pub font_editor: f32,
    /// Sidebar / menu row height.
    pub row_h: f32,
    pub menubar_h: f32,
    pub tabbar_h: f32,
    pub status_h: f32,
    pub sidebar_w: f32,
    pub scrollbar: f32,
    /// Minimap strip width (0 disables the reserved space).
    pub minimap: f32,
    /// Horizontal text padding inside the editor's text area.
    pub text_pad: f32,
    /// Gutter padding on both sides of the line numbers.
    pub gutter_pad: f32,
    /// Indentation unit, and the display width of a literal tab.
    pub tab: f32,
    /// Tab strip width cap per tab.
    pub tab_w: f32,
}

impl Metrics {
    /// Sizes are **em pixels** (see `host::font_config`): `14` for chrome,
    /// `12` for secondary text. Chrome heights leave room around the larger
    /// faces; editor line height is derived from `Settings::font_size`.
    pub const COMPACT: Self = Self {
        font_ui: 14.0,
        font_small: 12.0,
        font_editor: 14.0,
        row_h: 28.0,
        menubar_h: 32.0,
        tabbar_h: 28.0,
        status_h: 24.0,
        sidebar_w: 232.0,
        scrollbar: 10.0,
        minimap: 84.0,
        text_pad: 10.0,
        gutter_pad: 10.0,
        tab: 4.0,
        tab_w: 180.0,
    };
}

/// The active theme value. Cheap to copy; every painter takes it by reference.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub mode: Mode,
    pub p: Palette,
    pub m: Metrics,
}

impl Theme {
    pub const fn new(mode: Mode) -> Self {
        Self {
            mode,
            p: match mode {
                Mode::Dark => Palette::dark(),
                Mode::Light => Palette::light(),
            },
            m: Metrics::COMPACT,
        }
    }

    pub const fn dark() -> Self {
        Self::new(Mode::Dark)
    }

    pub fn toggle(&mut self) {
        *self = Self::new(match self.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        });
    }

    /// Color for a syntax token (`Plain` maps to the primary text color).
    pub fn token_color(&self, kind: TokenKind) -> Color {
        match kind {
            TokenKind::Plain => self.p.text,
            TokenKind::Keyword => self.p.syntax.keyword,
            TokenKind::TypeName => self.p.syntax.type_name,
            TokenKind::Function => self.p.syntax.function,
            TokenKind::String => self.p.syntax.string,
            TokenKind::Number => self.p.syntax.number,
            TokenKind::Comment => self.p.syntax.comment,
            TokenKind::Constant => self.p.syntax.constant,
            TokenKind::Macro => self.p.syntax.macro_,
            TokenKind::Punctuation => self.p.syntax.punctuation,
        }
    }

    /// Name shown in the status bar.
    pub const fn name(&self) -> &'static str {
        match self.mode {
            Mode::Dark => "DARK",
            Mode::Light => "LIGHT",
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
