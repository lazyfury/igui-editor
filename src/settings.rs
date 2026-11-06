//! Editor settings and the metrics they drive.
//!
//! Chrome metrics (menu bar, tab strip, status bar, sidebar row height) stay in
//! [`crate::theme::Metrics`]; the editor's own typography and indentation are
//! user settings here, persisted in the session file.
//!
//! Values are clamped by [`Settings::font`] / [`Settings::sidebar`] so a hand
//! edited session cannot put the editor into an unusable state.
//!
//! The settings *panel* is described by [`ROWS`]: one table that owns the row
//! order, the group each row belongs to and its widget kind. Paint and hit
//! testing read `Row::spec()` so adding a row never needs a second list.

use crate::keymap::KeymapId;
use crate::model::ignore::IgnoreMode;

/// Font size bounds (logical pixels).
pub const FONT_MIN: f32 = 11.0;
pub const FONT_MAX: f32 = 18.0;
/// Sidebar width bounds (logical pixels).
pub const SIDEBAR_MIN: f32 = 180.0;
pub const SIDEBAR_MAX: f32 = 420.0;
/// Tab widths offered by the settings panel.
pub const TAB_WIDTHS: [usize; 3] = [2, 4, 8];

/// Options of the `Choice` rows (index order is the rotation order).
pub const THEME_OPTIONS: &[&str] = &["Dark", "Light"];
pub const INDENT_OPTIONS: &[&str] = &["Spaces", "Tabs"];
pub const TAB_WIDTH_OPTIONS: &[&str] = &["2", "4", "8"];
pub const IGNORED_OPTIONS: &[&str] = &["OFF", "DIM", "HIDE"];
pub const KEYMAP_OPTIONS: &[&str] = &["VS Code", "JetBrains", "Vim"];

/// Bounds and granularity of a numeric row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberSpec {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: &'static str,
}

const FONT_NUMBER: NumberSpec = NumberSpec {
    min: FONT_MIN,
    max: FONT_MAX,
    step: 1.0,
    unit: "px",
};
const SIDEBAR_NUMBER: NumberSpec = NumberSpec {
    min: SIDEBAR_MIN,
    max: SIDEBAR_MAX,
    step: 8.0,
    unit: "px",
};

/// User-editable editor settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Editor font size in logical pixels.
    pub font_size: f32,
    /// Indentation width: spaces per level, and the displayed width of a tab.
    pub tab_width: usize,
    /// Indent with tabs instead of `tab_width` spaces.
    pub indent_tabs: bool,
    /// Draw the line-number gutter.
    pub line_numbers: bool,
    /// Draw indent guides.
    pub indent_guides: bool,
    /// Syntax highlighting on/off.
    pub syntax: bool,
    /// Autosave dirty files (with a path) after the idle delay.
    pub autosave: bool,
    /// Run language servers (start on demand, stop all when disabled).
    pub lsp: bool,
    /// Allow repository-local language-server binaries (`node_modules/.bin`).
    pub trust_local_servers: bool,
    /// Sidebar width in logical pixels.
    pub sidebar_width: f32,
    /// Show dot entries (`.git`, `.env`, …) in the Explorer.
    pub show_hidden: bool,
    /// Paint file/folder icons from the installed Material Icon Theme.
    pub file_icons: bool,
    /// Presentation of generated/temp Explorer files (Godot `.uid` sidecars).
    pub ignored_files: IgnoreMode,
    /// Selectable keyboard profile.
    pub keymap: KeymapId,
    /// Close brackets and quotes while typing, step over closers, and wrap the
    /// selection instead of replacing it.
    pub auto_pairs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            tab_width: 4,
            indent_tabs: false,
            line_numbers: true,
            indent_guides: true,
            syntax: true,
            autosave: true,
            lsp: true,
            trust_local_servers: false,
            sidebar_width: 232.0,
            show_hidden: false,
            file_icons: true,
            ignored_files: IgnoreMode::Dim,
            keymap: KeymapId::VsCode,
            auto_pairs: true,
        }
    }
}

impl Settings {
    /// A clamped copy (safe to call after reading external values).
    pub fn normalized(self) -> Self {
        Self {
            font_size: clamp_font(self.font_size),
            tab_width: clamp_tab(self.tab_width),
            indent_tabs: self.indent_tabs,
            line_numbers: self.line_numbers,
            indent_guides: self.indent_guides,
            syntax: self.syntax,
            autosave: self.autosave,
            lsp: self.lsp,
            trust_local_servers: self.trust_local_servers,
            sidebar_width: clamp_sidebar(self.sidebar_width),
            show_hidden: self.show_hidden,
            file_icons: self.file_icons,
            ignored_files: self.ignored_files,
            keymap: self.keymap,
            auto_pairs: self.auto_pairs,
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

    /// The indent used for new documents (files with existing indentation keep
    /// what they already use).
    pub fn indent_style(&self) -> crate::model::doc::IndentStyle {
        if self.indent_tabs {
            crate::model::doc::IndentStyle::Tabs
        } else {
            crate::model::doc::IndentStyle::Spaces(clamp_tab(self.tab_width) as u8)
        }
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

/// Section the panel paints above its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Appearance,
    Editor,
    View,
    Explorer,
    Language,
}

impl Group {
    pub const fn label(self) -> &'static str {
        match self {
            Group::Appearance => "Appearance",
            Group::Editor => "Editor",
            Group::View => "View",
            Group::Explorer => "Explorer",
            Group::Language => "Language",
        }
    }
}

/// How a row is presented and edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    /// Boolean switch (`Enter`/`Space` toggles).
    Toggle,
    /// Rotating choice; `< 3` options paint as a button group, more paint as
    /// `‹ value ›` arrows.
    Choice(&'static [&'static str]),
    /// Numeric value with `−`/`+` boxes and a typed field.
    Number(NumberSpec),
    /// Runs an action (Reset, rendered in the panel footer).
    Action,
}

/// One settings-panel row: the only table the panel reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSpec {
    pub row: Row,
    pub group: Group,
    pub kind: RowKind,
}

/// Rows of the settings panel, in display order. `Reset` lives in the footer.
pub const ROWS: &[RowSpec] = &[
    RowSpec {
        row: Row::Theme,
        group: Group::Appearance,
        kind: RowKind::Choice(THEME_OPTIONS),
    },
    RowSpec {
        row: Row::FontSize,
        group: Group::Appearance,
        kind: RowKind::Number(FONT_NUMBER),
    },
    RowSpec {
        row: Row::IndentStyle,
        group: Group::Editor,
        kind: RowKind::Choice(INDENT_OPTIONS),
    },
    RowSpec {
        row: Row::TabWidth,
        group: Group::Editor,
        kind: RowKind::Choice(TAB_WIDTH_OPTIONS),
    },
    RowSpec {
        row: Row::LineNumbers,
        group: Group::Editor,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::IndentGuides,
        group: Group::Editor,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::AutoPairs,
        group: Group::Editor,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::Syntax,
        group: Group::Editor,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::Autosave,
        group: Group::Editor,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::Keymap,
        group: Group::Editor,
        kind: RowKind::Choice(KEYMAP_OPTIONS),
    },
    RowSpec {
        row: Row::Minimap,
        group: Group::View,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::SoftWrap,
        group: Group::View,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::ScrollSync,
        group: Group::View,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::SidebarWidth,
        group: Group::View,
        kind: RowKind::Number(SIDEBAR_NUMBER),
    },
    RowSpec {
        row: Row::HiddenFiles,
        group: Group::Explorer,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::IgnoredFiles,
        group: Group::Explorer,
        kind: RowKind::Choice(IGNORED_OPTIONS),
    },
    RowSpec {
        row: Row::FileIcons,
        group: Group::Explorer,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::LanguageIntelligence,
        group: Group::Language,
        kind: RowKind::Toggle,
    },
    RowSpec {
        row: Row::TrustLocalServers,
        group: Group::Language,
        kind: RowKind::Toggle,
    },
];

/// Settings-panel rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Theme,
    FontSize,
    IndentStyle,
    TabWidth,
    LineNumbers,
    IndentGuides,
    AutoPairs,
    Syntax,
    Autosave,
    LanguageIntelligence,
    TrustLocalServers,
    SidebarWidth,
    HiddenFiles,
    IgnoredFiles,
    FileIcons,
    Keymap,
    Minimap,
    SoftWrap,
    ScrollSync,
    Reset,
}

impl Row {
    /// Every row, including the footer-only `Reset`.
    pub const ALL: [Row; 20] = [
        Row::Theme,
        Row::FontSize,
        Row::IndentStyle,
        Row::TabWidth,
        Row::LineNumbers,
        Row::IndentGuides,
        Row::AutoPairs,
        Row::Syntax,
        Row::Autosave,
        Row::LanguageIntelligence,
        Row::TrustLocalServers,
        Row::SidebarWidth,
        Row::HiddenFiles,
        Row::IgnoredFiles,
        Row::FileIcons,
        Row::Keymap,
        Row::Minimap,
        Row::SoftWrap,
        Row::ScrollSync,
        Row::Reset,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Row::Theme => "Theme",
            Row::FontSize => "Font Size",
            Row::IndentStyle => "Indent Style",
            Row::TabWidth => "Indent Width",
            Row::LineNumbers => "Line Numbers",
            Row::IndentGuides => "Indent Guides",
            Row::AutoPairs => "Auto Pairs",
            Row::Syntax => "Syntax Highlight",
            Row::Autosave => "Autosave",
            Row::LanguageIntelligence => "Language Intelligence",
            Row::TrustLocalServers => "Trust Local Servers",
            Row::SidebarWidth => "Sidebar Width",
            Row::HiddenFiles => "Hidden Files",
            Row::IgnoredFiles => "Ignored Files",
            Row::FileIcons => "File Icons",
            Row::Keymap => "Keymap",
            Row::Minimap => "Minimap",
            Row::SoftWrap => "Soft Wrap",
            Row::ScrollSync => "Scroll Sync",
            Row::Reset => "Reset to Defaults",
        }
    }

    /// The panel entry for this row (`None` for `Reset`, which is a footer
    /// button rather than a list row).
    pub fn spec(self) -> Option<&'static RowSpec> {
        ROWS.iter().find(|spec| spec.row == self)
    }

    /// Widget kind; `Reset` is an action.
    pub fn kind(self) -> RowKind {
        self.spec().map_or(RowKind::Action, |spec| spec.kind)
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
        assert_eq!(
            settings.indent_style(),
            crate::model::doc::IndentStyle::Spaces(4)
        );
        assert!(!settings.indent_tabs);
    }

    #[test]
    fn every_row_has_one_spec_and_one_group() {
        for row in Row::ALL {
            if row == Row::Reset {
                assert!(row.spec().is_none());
                assert_eq!(row.kind(), RowKind::Action);
                continue;
            }
            let spec = row.spec().unwrap_or_else(|| panic!("{row:?} has no spec"));
            assert_eq!(spec.row, row);
            assert_eq!(
                ROWS.iter().filter(|candidate| candidate.row == row).count(),
                1,
                "{row:?} is listed more than once"
            );
            assert!(!spec.group.label().is_empty());
            assert_ne!(spec.kind, RowKind::Action);
        }
        assert_eq!(ROWS.len(), Row::ALL.len() - 1);
    }

    #[test]
    fn row_kinds_match_the_widget_they_paint() {
        assert!(matches!(Row::Theme.kind(), RowKind::Choice(THEME_OPTIONS)));
        assert!(matches!(
            Row::IndentStyle.kind(),
            RowKind::Choice(INDENT_OPTIONS)
        ));
        assert!(matches!(
            Row::IgnoredFiles.kind(),
            RowKind::Choice(IGNORED_OPTIONS)
        ));
        assert!(matches!(
            Row::Keymap.kind(),
            RowKind::Choice(KEYMAP_OPTIONS)
        ));
        assert!(matches!(
            Row::FontSize.kind(),
            RowKind::Number(NumberSpec { unit: "px", .. })
        ));
        assert!(matches!(
            Row::SidebarWidth.kind(),
            RowKind::Number(NumberSpec { unit: "px", .. })
        ));
        for row in [
            Row::LineNumbers,
            Row::IndentGuides,
            Row::AutoPairs,
            Row::Syntax,
            Row::Autosave,
            Row::LanguageIntelligence,
            Row::TrustLocalServers,
            Row::HiddenFiles,
            Row::FileIcons,
            Row::Minimap,
            Row::SoftWrap,
            Row::ScrollSync,
        ] {
            assert_eq!(row.kind(), RowKind::Toggle, "{row:?}");
        }
    }

    #[test]
    fn small_choice_sets_use_a_button_group() {
        // Two options fit side by side; three or more would crowd the row and
        // paint as `‹ value ›` arrows instead.
        assert!(THEME_OPTIONS.len() < 3);
        assert!(INDENT_OPTIONS.len() < 3);
        assert!(TAB_WIDTH_OPTIONS.len() >= 3);
        assert!(IGNORED_OPTIONS.len() >= 3);
        assert!(KEYMAP_OPTIONS.len() >= 3);
    }

    #[test]
    fn groups_are_contiguous_in_display_order() {
        let mut seen: Vec<Group> = Vec::new();
        for spec in ROWS {
            if seen.last() != Some(&spec.group) {
                assert!(
                    !seen.contains(&spec.group),
                    "group {:?} is split across the panel",
                    spec.group
                );
                seen.push(spec.group);
            }
        }
    }

    #[test]
    fn language_intelligence_is_on_by_default_and_toggleable() {
        let settings = Settings::default();
        assert!(settings.lsp);
        assert!(settings.normalized().lsp);
        assert!(!settings.trust_local_servers);
        assert!(!settings.normalized().trust_local_servers);
        assert!(ROWS
            .iter()
            .any(|spec| spec.row == Row::LanguageIntelligence));
        assert!(ROWS.iter().any(|spec| spec.row == Row::TrustLocalServers));
        assert!(ROWS.iter().any(|spec| spec.row == Row::IndentStyle));
    }

    #[test]
    fn explorer_visibility_defaults_keep_dotfiles_hidden_and_dim_tool_files() {
        let settings = Settings::default();
        assert!(!settings.show_hidden);
        assert_eq!(
            settings.ignored_files,
            crate::model::ignore::IgnoreMode::Dim
        );
        assert!(ROWS.iter().any(|spec| spec.row == Row::HiddenFiles));
        assert!(ROWS.iter().any(|spec| spec.row == Row::IgnoredFiles));
        assert_eq!(Row::HiddenFiles.kind(), RowKind::Toggle);
        assert!(matches!(
            Row::IgnoredFiles.kind(),
            RowKind::Choice(IGNORED_OPTIONS)
        ));
        assert_eq!(settings.normalized(), settings);
    }

    #[test]
    fn clamps_keep_values_usable() {
        assert_eq!(clamp_font(9.0), FONT_MIN);
        assert_eq!(clamp_font(99.0), FONT_MAX);
        assert_eq!(clamp_sidebar(10.0), SIDEBAR_MIN);
        assert_eq!(clamp_sidebar(9999.0), SIDEBAR_MAX);
        assert_eq!(clamp_tab(3), 4);
        assert_eq!(clamp_tab(8), 8);
    }
}
