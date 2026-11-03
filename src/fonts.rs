//! Font choices for this app: one face for the UI/editor, one fixed-pitch face
//! for the built-in terminal.
//!
//! The engine (`igui`) is font-family neutral: every text draw may name a
//! primary family (`PaintContext::draw_text_in`, `TextMeasurer::advance_in`).
//! *Which* families this project asks for lives here, so switching the terminal
//! face is a one-line change instead of a change in the terminal painter.
//!
//! Candidates are probed at runtime: the font server silently falls back to the
//! default family when a name is not installed, so a name alone proves nothing.
//! [`mono_family`] therefore measures a few ASCII glyphs and only accepts a
//! family whose advances agree (i.e. a genuinely fixed-pitch face).

use igui_ui::TextMeasurer;

/// Primary family for the editor, chrome and overlays.
///
/// macOS gets a real CJK face as the primary (the engine's first symbol
/// fallback would otherwise draw CJK as box glyphs). Other hosts keep the
/// system default, which the platform plugin resolves.
pub const UI_FAMILY: Option<&str> = if cfg!(target_os = "macos") {
    Some("PingFang SC")
} else {
    None
};

/// Face files used as the default on non-macOS hosts, first hit wins.
///
/// These are fallbacks for a system without font discovery; the terminal's
/// fixed-pitch family is chosen at runtime by [`mono_family`].
#[cfg(not(target_os = "macos"))]
pub const FALLBACK_FACE_FILES: &[&str] = &[
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
    "/usr/share/fonts/truetype/firacode/FiraCode-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

/// Fixed-pitch candidates for the terminal, in preference order.
///
/// Covers the stock macOS (`SF Mono`, `Menlo`, `Monaco`), Linux
/// (`JetBrains Mono`, `Fira Code`, `DejaVu Sans Mono`, `Noto Sans Mono`) and
/// Windows (`Cascadia Mono`, `Consolas`) faces. Missing names are skipped by
/// the probe, so one list works on every host.
pub const MONO_FAMILIES: &[&str] = &[
    "SF Mono",
    ".SF NS Mono",
    "Menlo",
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Mono",
    "Consolas",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Monaco",
    "Andale Mono",
    "Lucida Console",
    "Courier New",
];

/// Glyphs used to decide whether a family is fixed-pitch.
///
/// A proportional face (`PingFang SC` on macOS) returns very different advances
/// for these; a fixed-pitch face returns the same value for all of them.
const PROBE_GLYPHS: [char; 4] = ['M', 'i', 'W', ' '];

/// Returns the first available fixed-pitch family at `font_size`.
///
/// Every draw in the terminal uses this family, and every cell width is
/// measured with it, so columns stay aligned. `None` means no fixed-pitch face
/// could be found; the painter then falls back to the default family on the
/// same fixed cell grid.
pub fn mono_family(measurer: &dyn TextMeasurer, font_size: f32) -> Option<String> {
    MONO_FAMILIES.iter().copied().find_map(|family| {
        let mut advances = PROBE_GLYPHS
            .iter()
            .map(|ch| measurer.advance_in(*ch, font_size, Some(family)));
        let first = advances.next()?;
        if first < 1.0 {
            return None;
        }
        let fixed = advances.all(|advance| (advance - first).abs() < 0.05);
        fixed.then(|| family.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use igui_core::FontWeight;

    /// A measurer with one fixed-pitch family and a proportional default.
    struct Probe {
        mono: Option<&'static str>,
    }

    impl TextMeasurer for Probe {
        fn advance(&self, ch: char, font_size: f32) -> f32 {
            match ch {
                ' ' => font_size * 0.25,
                'i' => font_size * 0.2,
                'M' | 'W' => font_size * 0.7,
                _ => font_size * 0.55,
            }
        }

        fn line_height(&self, font_size: f32) -> f32 {
            font_size * 1.4
        }

        fn advance_in(&self, ch: char, font_size: f32, family: Option<&str>) -> f32 {
            match (family, self.mono) {
                (Some(requested), Some(mono)) if requested.eq_ignore_ascii_case(mono) => {
                    font_size * 0.6
                }
                _ => self.advance(ch, font_size),
            }
        }

        fn advance_weighted_in(
            &self,
            ch: char,
            font_size: f32,
            _weight: FontWeight,
            family: Option<&str>,
        ) -> f32 {
            self.advance_in(ch, font_size, family)
        }
    }

    #[test]
    fn picks_the_only_fixed_pitch_candidate() {
        let measurer = Probe {
            mono: Some("JetBrains Mono"),
        };
        assert_eq!(
            mono_family(&measurer, 14.0).as_deref(),
            Some("JetBrains Mono")
        );
    }

    #[test]
    fn skips_candidates_that_resolve_proportionally() {
        // The probe only reports fixed advances for this name, which is later
        // in the candidate list, so the earlier names must be rejected.
        let measurer = Probe {
            mono: Some("Menlo"),
        };
        assert_eq!(mono_family(&measurer, 14.0).as_deref(), Some("Menlo"));
    }

    #[test]
    fn no_fixed_pitch_face_falls_back_to_none() {
        let measurer = Probe { mono: None };
        assert_eq!(mono_family(&measurer, 14.0), None);
    }
}
