//! Session persistence: theme, sidebar, recent files and open tabs.
//!
//! A tiny, hand-written `key=value` file (no serde), written atomically
//! (temp file + rename) so a crash mid-save cannot truncate the session. Values
//! escape `\`, newline and carriage return; keys are fixed, so splitting on the
//! first `=` is unambiguous.
//!
//! The path is platform-neutral by design: macOS uses
//! `~/Library/Application Support/igui-editor/session.conf`, everything else
//! follows `$XDG_CONFIG_HOME` (falling back to `~/.config`). `IGUI_EDITOR_CONFIG`
//! overrides it for tests and portable installs.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 8;

/// Persisted application state. Missing/corrupt files fall back to defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// Format version (`1`); unknown keys are ignored so older/newer files keep
    /// loading.
    pub version: u32,
    /// `"dark"` or `"light"`.
    pub theme: String,
    pub sidebar: bool,
    pub recent: Vec<PathBuf>,
    pub tabs: Vec<PathBuf>,
    /// Editor font size.
    pub font: f32,
    /// Tab width in spaces.
    pub tab: usize,
    pub line_numbers: bool,
    pub indent_guides: bool,
    pub syntax: bool,
    /// Sidebar width.
    pub sidebar_width: f32,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            theme: "dark".into(),
            sidebar: true,
            recent: Vec::new(),
            tabs: Vec::new(),
            font: 13.0,
            tab: 4,
            line_numbers: true,
            indent_guides: true,
            syntax: true,
            sidebar_width: 232.0,
        }
    }
}

impl Session {
    /// Reads the session, returning defaults when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let mut session = Self::default();
        let mut recent: BTreeMap<usize, PathBuf> = BTreeMap::new();
        let mut tabs: BTreeMap<usize, PathBuf> = BTreeMap::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unescape(value);
            match key {
                "theme" => {
                    if value == "light" || value == "dark" {
                        session.theme = value;
                    }
                }
                "sidebar" => session.sidebar = value == "1",
                "font" => session.font = value.parse().unwrap_or(session.font),
                "tab" => session.tab = value.parse().unwrap_or(session.tab),
                "gutter" | "line_numbers" => session.line_numbers = value == "1",
                "guides" | "indent_guides" => session.indent_guides = value == "1",
                "highlight" | "syntax" => session.syntax = value == "1",
                "sidebar_width" => {
                    session.sidebar_width = value.parse().unwrap_or(session.sidebar_width)
                }
                "version" => {}
                key if key.starts_with("recent.") => {
                    if let Ok(index) = key["recent.".len()..].parse::<usize>() {
                        recent.insert(index, PathBuf::from(value));
                    }
                }
                key if key.starts_with("tabs.") => {
                    if let Ok(index) = key["tabs.".len()..].parse::<usize>() {
                        tabs.insert(index, PathBuf::from(value));
                    }
                }
                _ => {}
            }
        }
        session.recent = recent.into_values().take(MAX_RECENT).collect();
        session.tabs = tabs.into_values().take(MAX_RECENT).collect();
        session
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("version={}\n", self.version));
        out.push_str(&format!("theme={}\n", self.theme));
        out.push_str(&format!("sidebar={}\n", u8::from(self.sidebar)));
        out.push_str(&format!("font={}\n", self.font));
        out.push_str(&format!("tab={}\n", self.tab));
        out.push_str(&format!("gutter={}\n", u8::from(self.line_numbers)));
        out.push_str(&format!("guides={}\n", u8::from(self.indent_guides)));
        out.push_str(&format!("syntax={}\n", u8::from(self.syntax)));
        out.push_str(&format!("sidebar_width={}\n", self.sidebar_width));
        for (index, path) in self.recent.iter().take(MAX_RECENT).enumerate() {
            out.push_str(&format!(
                "recent.{index}={}\n",
                escape(&path.to_string_lossy())
            ));
        }
        for (index, path) in self.tabs.iter().take(MAX_RECENT).enumerate() {
            out.push_str(&format!(
                "tabs.{index}={}\n",
                escape(&path.to_string_lossy())
            ));
        }
        out
    }

    /// Writes the session atomically (parent directories are created).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, self.render())?;
        fs::rename(&tmp, path)
    }

    /// Moves `path` to the front of the recent list (deduplicated, capped).
    pub fn push_recent(&mut self, path: &Path) {
        self.recent.retain(|candidate| candidate != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    /// Replaces the restored-tab list.
    pub fn set_tabs(&mut self, tabs: Vec<PathBuf>) {
        self.tabs = tabs.into_iter().take(MAX_RECENT).collect();
    }

    /// `dark`/`light` as the app's mode flag.
    pub fn is_dark(&self) -> bool {
        self.theme != "light"
    }
}

/// The default session path (platform-specific; `IGUI_EDITOR_CONFIG` wins).
pub fn path() -> PathBuf {
    if let Some(override_path) = std::env::var_os("IGUI_EDITOR_CONFIG") {
        return PathBuf::from(override_path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        return home
            .join("Library/Application Support/igui-editor")
            .join("session.conf");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    base.join("igui-editor").join("session.conf")
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-session-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("session.conf")
    }

    #[test]
    fn round_trips_theme_sidebar_and_lists() {
        let path = temp_path("roundtrip");
        let session = Session {
            theme: "light".into(),
            sidebar: false,
            recent: vec![PathBuf::from("/tmp/a=b.rs"), PathBuf::from("/tmp/c d.md")],
            tabs: vec![PathBuf::from("/tmp/a=b.rs")],
            font: 15.0,
            tab: 2,
            line_numbers: false,
            indent_guides: false,
            syntax: false,
            sidebar_width: 260.0,
            ..Session::default()
        };
        session.save(&path).unwrap();
        let loaded = Session::load(&path);
        assert_eq!(loaded, session);
        assert!(
            !path.with_extension("tmp").exists(),
            "temp file must be gone"
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_uses_defaults() {
        let session = Session::load(&temp_path("missing"));
        assert_eq!(session.theme, "dark");
        assert!(session.sidebar);
        assert!(session.recent.is_empty());
    }

    #[test]
    fn values_escape_backslash_and_newlines() {
        let path = temp_path("escape");
        let weird = PathBuf::from("/tmp/back\\slash and space");
        let session = Session {
            recent: vec![weird.clone()],
            ..Session::default()
        };
        session.save(&path).unwrap();
        let loaded = Session::load(&path);
        assert_eq!(loaded.recent, vec![weird]);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn recent_is_deduplicated_and_capped() {
        let mut session = Session::default();
        for index in 0..12 {
            session.push_recent(Path::new(&format!("/tmp/file-{index}")));
        }
        session.push_recent(Path::new("/tmp/file-11"));
        assert_eq!(session.recent.len(), MAX_RECENT);
        assert_eq!(session.recent[0], PathBuf::from("/tmp/file-11"));
        let unique = session
            .recent
            .iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), session.recent.len());
    }

    #[test]
    fn corrupt_lines_are_ignored() {
        let session = Session::parse("theme=neon\nsidebar=1\nrecent.nope=/x\nno equals\n");
        assert_eq!(session.theme, "dark");
        assert!(session.sidebar);
        assert!(session.recent.is_empty());
    }
}
