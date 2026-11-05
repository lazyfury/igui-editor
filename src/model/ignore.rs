//! Explorer visibility rules for tool noise.
//!
//! This is a **display** filter, not a data filter: the external-change poll
//! and the language servers still see every file, so hiding a row can never
//! make a tool's change invisible to the editor. Godot's `<script>.gd.uid`
//! companions are the motivating case — the editor writes one next to every
//! script and they carry no text worth opening.

/// How the Explorer presents ignored tool files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IgnoreMode {
    /// Show them like any other file.
    Off,
    /// Show them, but dimmed (the default: still openable, clearly secondary).
    #[default]
    Dim,
    /// Leave them out of the tree entirely.
    Hide,
}

impl IgnoreMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Dim, Self::Hide];

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Dim,
            Self::Dim => Self::Hide,
            Self::Hide => Self::Off,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Off => Self::Hide,
            Self::Dim => Self::Off,
            Self::Hide => Self::Dim,
        }
    }

    /// Value shown in the settings panel.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Dim => "DIM",
            Self::Hide => "HIDE",
        }
    }

    /// Session representation (`ignored_files=`).
    pub const fn session_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Dim => "dim",
            Self::Hide => "hide",
        }
    }

    /// Unknown or missing session values keep the default.
    pub fn from_session(value: &str) -> Self {
        match value {
            "off" => Self::Off,
            "hide" => Self::Hide,
            _ => Self::Dim,
        }
    }
}

/// Whether a file name is tool noise rather than content: generated sidecars
/// (Godot's `.uid`) and editor/OS temp files. Matched on the name only.
pub fn is_ignored(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let suffix = |end: &str| lower.ends_with(end);
    // Godot writes `<script>.gd.uid` next to every script it imports.
    suffix(".uid")
        || suffix(".tmp")
        || suffix(".temp")
        || suffix(".bak")
        || suffix(".orig")
        || suffix(".rej")
        || suffix(".swp")
        || suffix(".swo")
        || suffix(".part")
        || lower == "thumbs.db"
        || lower == "desktop.ini"
        || name.ends_with('~')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn godot_sidecars_and_temp_files_are_ignored() {
        for name in [
            "player.gd.uid",
            "gdscriptxx.uid",
            "scratch.tmp",
            "notes.md~",
            ".main.rs.swp",
            "report.bak",
            "Thumbs.db",
            "desktop.ini",
        ] {
            assert!(is_ignored(name), "{name} should be ignored");
        }
        for name in [
            "player.gd",
            "main.rs",
            "notes.md",
            "Cargo.toml",
            "uid_notes.md",
            ".gitignore",
        ] {
            assert!(!is_ignored(name), "{name} must stay visible");
        }
    }

    #[test]
    fn modes_cycle_and_round_trip_through_the_session() {
        assert_eq!(IgnoreMode::default(), IgnoreMode::Dim);
        assert_eq!(IgnoreMode::Off.next(), IgnoreMode::Dim);
        assert_eq!(IgnoreMode::Dim.next(), IgnoreMode::Hide);
        assert_eq!(IgnoreMode::Hide.next(), IgnoreMode::Off);
        assert_eq!(IgnoreMode::Off.previous(), IgnoreMode::Hide);
        for mode in IgnoreMode::ALL {
            assert_eq!(
                IgnoreMode::from_session(mode.session_value()),
                mode,
                "{mode:?} must round-trip"
            );
        }
        assert_eq!(IgnoreMode::from_session("nonsense"), IgnoreMode::Dim);
    }
}
