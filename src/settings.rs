//! Editor settings and the metrics they drive.
//!
//! Chrome metrics (menu bar, tab strip, status bar, sidebar row height) stay in
//! [`crate::theme::Metrics`]; the editor's own typography and indentation are
//! user settings here, persisted in the session file.
//!
//! Values are clamped by [`Settings::font`] / [`Settings::sidebar`] so a hand
//! edited session cannot put the editor into an unusable state.

/// Font size bounds (logical pixels).
pub const FONT_MIN: f32 = 11.0;
pub const FONT_MAX: f32 = 18.0;
/// Sidebar width bounds (logical pixels).
pub const SIDEBAR_MIN: f32 = 180.0;
pub const SIDEBAR_MAX: f32 = 420.0;
/// Tab widths offered by the settings panel.
pub const TAB_WIDTHS: [usize; 3] = [2, 4, 8];

/// User-editable editor settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Editor font size in logical pixels.
    pub font_size: f32,
    /// Indentation unit and the displayed width of a literal tab.
    pub tab_width: usize,
    /// Draw the line-number gutter.
    pub line_numbers: bool,
    /// Draw indent guides.
    pub indent_guides: bool,
    /// Syntax highlighting on/off.
    pub syntax: bool,
    /// Autosave dirty files (with a path) after the idle delay.
    pub autosave: bool,
    /// Sidebar width in logical pixels.
    pub sidebar_width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            tab_width: 4,
            line_numbers: true,
            indent_guides: true,
            syntax: true,
            autosave: true,
            sidebar_width: 232.0,
        }
    }
}

impl Settings {
    /// A clamped copy (safe to call after reading external values).
    pub fn normalized(self) -> Self {
        Self {
            font_size: clamp_font(self.font_size),
            tab_width: clamp_tab(self.tab_width),
            line_numbers: self.line_numbers,
            indent_guides: self.indent_guides,
            syntax: self.syntax,
            autosave: self.autosave,
            sidebar_width: clamp_sidebar(self.sidebar_width),
        }
    }

    /// Metrics the editor and the preview derive from these settings.
    pub fn editor(&self) -> EditorMetrics {
        let font = clamp_font(self.font_size);
        EditorMetrics {
            font,
            line_h: (font * 1.45).round().max(16.0),
            tab_w: clamp_tab(self.tab_width) as f32,
        }
    }

    /// Steps the font size by `delta` (clamped, rounded to integer pixels).
    pub fn step_font(&mut self, delta: i32) {
        self.font_size = clamp_font(self.font_size + delta as f32);
    }

    /// Cycles the tab width through 2 / 4 / 8.
    pub fn step_tab(&mut self, delta: i32) {
        let index = TAB_WIDTHS
            .iter()
            .position(|width| *width == self.tab_width)
            .unwrap_or(1) as i32;
        let next = (index + delta).rem_euclid(TAB_WIDTHS.len() as i32) as usize;
        self.tab_width = TAB_WIDTHS[next];
    }

    /// Steps the sidebar width by `delta` pixels (clamped, 8 px granularity).
    pub fn step_sidebar(&mut self, delta: f32) {
        let stepped = clamp_sidebar(self.sidebar_width + delta);
        self.sidebar_width = clamp_sidebar((stepped / 4.0).round() * 4.0);
    }
}

/// Editor-specific metrics derived from [`Settings`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorMetrics {
    pub font: f32,
    pub line_h: f32,
    /// Tab width in pixels (monospace: one tab = this many space advances).
    pub tab_w: f32,
}

impl EditorMetrics {
    /// Character column count of one tab, for hit-testing and guides.
    pub fn tab_columns(&self) -> usize {
        self.tab_w.round().max(1.0) as usize
    }
}

pub fn clamp_font(value: f32) -> f32 {
    value.round().clamp(FONT_MIN, FONT_MAX)
}

pub fn clamp_sidebar(value: f32) -> f32 {
    value.round().clamp(SIDEBAR_MIN, SIDEBAR_MAX)
}

pub fn clamp_tab(value: usize) -> usize {
    if TAB_WIDTHS.contains(&value) {
        value
    } else {
        TAB_WIDTHS[1]
    }
}

/// Rows of the settings panel, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Theme,
    FontSize,
    TabWidth,
    LineNumbers,
    IndentGuides,
    Syntax,
    Autosave,
    SidebarWidth,
    Reset,
}

pub const ROWS: [Row; 9] = [
    Row::Theme,
    Row::FontSize,
    Row::TabWidth,
    Row::LineNumbers,
    Row::IndentGuides,
    Row::Syntax,
    Row::Autosave,
    Row::SidebarWidth,
    Row::Reset,
];

impl Row {
    pub const fn label(self) -> &'static str {
        match self {
            Row::Theme => "Theme",
            Row::FontSize => "Font Size",
            Row::TabWidth => "Tab Width",
            Row::LineNumbers => "Line Numbers",
            Row::IndentGuides => "Indent Guides",
            Row::Syntax => "Syntax Highlight",
            Row::Autosave => "Autosave",
            Row::SidebarWidth => "Sidebar Width",
            Row::Reset => "Reset to Defaults",
        }
    }

    /// Whether the row is a boolean toggle (Enter/Space toggles it).
    pub const fn is_toggle(self) -> bool {
        matches!(self, Row::LineNumbers | Row::IndentGuides | Row::Syntax)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_metric_derivation() {
        let settings = Settings::default();
        let metrics = settings.editor();
        assert_eq!(metrics.font, 13.0);
        assert_eq!(metrics.line_h, 19.0);
        assert_eq!(metrics.tab_columns(), 4);
    }

    #[test]
    fn font_steps_clamp_and_stay_integral() {
        let mut settings = Settings::default();
        for _ in 0..20 {
            settings.step_font(1);
        }
        assert_eq!(settings.font_size, FONT_MAX);
        for _ in 0..20 {
            settings.step_font(-1);
        }
        assert_eq!(settings.font_size, FONT_MIN);
    }

    #[test]
    fn tab_cycles_through_the_offered_widths() {
        let mut settings = Settings::default();
        settings.step_tab(1);
        assert_eq!(settings.tab_width, 8);
        settings.step_tab(1);
        assert_eq!(settings.tab_width, 2);
        settings.step_tab(-1);
        assert_eq!(settings.tab_width, 8);
        settings.tab_width = 3;
        assert_eq!(settings.normalized().tab_width, 4);
    }

    #[test]
    fn sidebar_steps_stay_in_range_and_on_the_grid() {
        let mut settings = Settings::default();
        settings.step_sidebar(1000.0);
        assert_eq!(settings.sidebar_width, SIDEBAR_MAX);
        settings.step_sidebar(-1000.0);
        assert_eq!(settings.sidebar_width, SIDEBAR_MIN);
        settings.sidebar_width = 233.0;
        settings.step_sidebar(0.0);
        assert_eq!(settings.sidebar_width % 4.0, 0.0);
    }
}
