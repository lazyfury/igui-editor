//! Font choices for this app: the UI face, plus one fixed-pitch face shared by
//! the editor and the built-in terminal.
//!
//! The engine (`igui`) is font-family neutral: every text draw may name a
//! primary family (`PaintContext::draw_text_in`, `TextMeasurer::advance_in`).
//! *Which* families this project asks for lives here, so switching the editor
//! or terminal face is a one-line change instead of a change in the painters.
//!
//! Candidates are probed at runtime: the font server silently falls back to the
//! default family when a name is not installed, so a name alone proves nothing.
//! [`mono_family`] therefore measures a few ASCII glyphs and only accepts a
//! family whose advances agree (i.e. a genuinely fixed-pitch face).
//!
//! Wide (CJK) glyphs deliberately keep the UI family: a fixed-pitch Latin face
//! usually has no Han coverage, and the engine's generic first-covering
//! fallback can pick a face with the wrong regional glyph shapes.

use igui_core::FontWeight;
use igui_ui::layout::is_wide;
use igui_ui::TextMeasurer;

/// Primary family for chrome, overlays and every wide (CJK) glyph.
///
/// macOS gets a real CJK face as the primary (the engine's first symbol
/// fallback would otherwise draw CJK with the wrong regional shapes). Other
/// hosts keep the system default, which the platform plugin resolves. Code
/// text is drawn in [`MONO_FAMILIES`] instead; wide glyphs inside it fall back
/// to this family.
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

/// Fixed-pitch candidates for the editor and terminal, in preference order.
///
/// `Menlo` is first: it also covers the dingbat/symbol glyphs shells put in
/// their prompts (`➜`, `✗`, …). `Monaco` is fixed-pitch too but lacks them,
/// and asking the font server for an uncovered glyph makes it probe every
/// fallback face and load the whole font file — on macOS that can mean the
/// ~180 MB emoji face for one prompt character. Reorder this list to prefer
/// another face. Covers the stock macOS (`Menlo`, `SF Mono`, `Monaco`), Linux
/// (`JetBrains Mono`, `Fira Code`, `DejaVu Sans Mono`, `Noto Sans Mono`) and
/// Windows (`Cascadia Mono`, `Consolas`) faces. Missing names are skipped by
/// the probe, so one list works on every host.
pub const MONO_FAMILIES: &[&str] = &[
    "Menlo",
    "SF Mono",
    ".SF NS Mono",
    "Monaco",
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Mono",
    "Consolas",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
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
/// The editor and terminal draw with this family, and every advance is measured
/// with it, so columns stay aligned. `None` means no fixed-pitch face could be
/// found; the painters then fall back to the default family.
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

/// Family for one character in code text.
///
/// Narrow glyphs use the resolved fixed-pitch face; wide (CJK) glyphs use the
/// UI family (`None`) so Chinese/Japanese/Korean keep their regional shapes.
pub fn editor_family(ch: char, mono: Option<&str>) -> Option<&str> {
    if is_wide(ch) {
        None
    } else {
        mono
    }
}

/// Splits `text` into adjacent runs that share one family.
///
/// Callers measure or draw each run with its own family, which keeps Latin
/// columns fixed and CJK in the UI face on the same baseline.
pub fn family_runs<'a>(text: &'a str, mono: Option<&'a str>) -> FamilyRuns<'a> {
    FamilyRuns { rest: text, mono }
}

/// Iterator returned by [`family_runs`].
pub struct FamilyRuns<'a> {
    rest: &'a str,
    mono: Option<&'a str>,
}

impl<'a> Iterator for FamilyRuns<'a> {
    type Item = (&'a str, Option<&'a str>);

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.rest.chars().next()?;
        let first_wide = is_wide(first);
        let family = if first_wide { None } else { self.mono };
        let end = self
            .rest
            .char_indices()
            .skip(1)
            .find(|(_, ch)| is_wide(*ch) != first_wide)
            .map_or(self.rest.len(), |(index, _)| index);
        let (run, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some((run, family))
    }
}

/// Advance of one character in code text (see [`editor_family`]).
pub fn advance(measurer: &dyn TextMeasurer, ch: char, font_size: f32, mono: Option<&str>) -> f32 {
    measurer.advance_in(ch, font_size, editor_family(ch, mono))
}

/// Width of a code-text run, summing each family run separately.
pub fn measure_run_weighted(
    measurer: &dyn TextMeasurer,
    text: &str,
    font_size: f32,
    weight: FontWeight,
    mono: Option<&str>,
) -> f32 {
    family_runs(text, mono)
        .map(|(run, family)| measurer.measure_run_weighted_in(run, font_size, weight, family))
        .sum()
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

        fn measure_run_weighted_in(
            &self,
            text: &str,
            font_size: f32,
            _weight: FontWeight,
            family: Option<&str>,
        ) -> f32 {
            text.chars()
                .map(|ch| self.advance_in(ch, font_size, family))
                .sum()
        }
    }

    #[test]
    fn menlo_is_the_preferred_code_face() {
        // Menlo covers the dingbats shells put in prompts; Monaco does not.
        assert_eq!(MONO_FAMILIES.first(), Some(&"Menlo"));
    }

    #[test]
    fn family_runs_keep_wide_glyphs_on_the_ui_face() {
        let runs: Vec<_> = family_runs("abc中文x", Some("Menlo")).collect();
        assert_eq!(
            runs,
            vec![("abc", Some("Menlo")), ("中文", None), ("x", Some("Menlo")),]
        );
    }

    #[test]
    fn measure_run_uses_the_mono_face_only_for_narrow_runs() {
        let measurer = Probe {
            mono: Some("Monaco"),
        };
        // `Probe` gives every narrow glyph 0.6 em in Monaco and 0.55 em in the
        // default face, while wide glyphs always use the default face.
        let width =
            measure_run_weighted(&measurer, "a中", 10.0, FontWeight::NORMAL, Some("Monaco"));
        assert!((width - 11.5).abs() < 1e-4, "width was {width}");
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

    #[cfg(target_os = "macos")]
    #[test]
    fn real_font_server_resolves_menlo() {
        use std::rc::Rc;

        struct Metrics(igui_font::FontMetrics);

        impl TextMeasurer for Metrics {
            fn advance(&self, ch: char, font_size: f32) -> f32 {
                self.0.advance(ch, font_size)
            }

            fn line_height(&self, font_size: f32) -> f32 {
                self.0.line_height(font_size)
            }

            fn advance_in(&self, ch: char, font_size: f32, family: Option<&str>) -> f32 {
                self.0.advance_in(ch, font_size, family)
            }

            fn measure_run_weighted_in(
                &self,
                text: &str,
                font_size: f32,
                weight: FontWeight,
                family: Option<&str>,
            ) -> f32 {
                self.0
                    .measure_run_weighted_in(text, font_size, weight, family)
            }
        }

        let server = Rc::new(igui_font::FontServer::load_with(crate::host::font_config()));
        let measurer = Metrics(igui_font::FontMetrics::new(server));
        assert_eq!(mono_family(&measurer, 13.0).as_deref(), Some("Menlo"));

        // Mixed code keeps the Menlo advance for Latin and the UI-family
        // advance for the wide glyph.
        let expected = measurer.0.advance_in('a', 13.0, Some("Menlo"))
            + measurer.0.advance_in('中', 13.0, None);
        let mixed = measure_run_weighted(&measurer, "a中", 13.0, FontWeight::NORMAL, Some("Menlo"));
        assert!(
            (mixed - expected).abs() < 0.01,
            "mixed={mixed} expected={expected}"
        );
    }
}
