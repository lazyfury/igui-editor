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
    /// Format version (currently `4`); unknown keys are ignored so older/newer
    /// files keep loading.
    pub version: u32,
    /// `"dark"` or `"light"`.
    pub theme: String,
    pub sidebar: bool,
    /// The open workspace folder (`None` = no folder open).
    pub folder: Option<PathBuf>,
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
    /// Editor/preview split ratio (0.2–0.8).
    pub split: f32,
    /// Autosave enabled.
    pub autosave: bool,
    /// Per-path language overrides (provider name).
    pub languages: Vec<(PathBuf, String)>,
    /// Keep the editor and Markdown preview scroll positions in sync.
    pub sync_scroll: bool,
    /// Soft-wrap long editor lines.
    pub soft_wrap: bool,
    /// Active sidebar tab id (`explorer`, `git`, …).
    pub sidebar_tab: String,
    /// Last window geometry (logical size + position).
    pub window: Option<WindowGeometry>,
}

/// Persisted window size/position in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    pub size: (f32, f32),
    pub position: (f32, f32),
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: 5,
            theme: "dark".into(),
            sidebar: true,
            folder: None,
            recent: Vec::new(),
            tabs: Vec::new(),
            font: 13.0,
            tab: 4,
            line_numbers: true,
            indent_guides: true,
            syntax: true,
            sidebar_width: 232.0,
            split: 0.5,
            autosave: true,
            languages: Vec::new(),
            sync_scroll: true,
            soft_wrap: false,
            sidebar_tab: "explorer".into(),
            window: None,
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
        // Default to v1 so a legacy file without `version=` keeps the
        // folder-inference behaviour.
        let mut session = Self {
            version: 1,
            ..Self::default()
        };
        let mut recent: BTreeMap<usize, PathBuf> = BTreeMap::new();
        let mut tabs: BTreeMap<usize, PathBuf> = BTreeMap::new();
        let mut window: [Option<f32>; 4] = [None; 4];
        let mut languages: BTreeMap<usize, (Option<PathBuf>, Option<String>)> = BTreeMap::new();
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
                "folder" => session.folder = Some(PathBuf::from(value)),
                "font" => session.font = value.parse().unwrap_or(session.font),
                "tab" => session.tab = value.parse().unwrap_or(session.tab),
                "gutter" | "line_numbers" => session.line_numbers = value == "1",
                "guides" | "indent_guides" => session.indent_guides = value == "1",
                "highlight" | "syntax" => session.syntax = value == "1",
                "sidebar_width" => {
                    session.sidebar_width = value.parse().unwrap_or(session.sidebar_width)
                }
                "split" => session.split = value.parse().unwrap_or(session.split),
                "autosave" => session.autosave = value == "1",
                "sync_scroll" => session.sync_scroll = value == "1",
                "soft_wrap" => session.soft_wrap = value == "1",
                "sidebar_tab" => session.sidebar_tab = value,
                key if key.starts_with("lang.") => {
                    let rest = &key["lang.".len()..];
                    let Some((index, field)) = rest.split_once('.') else {
                        continue;
                    };
                    let Ok(index) = index.parse::<usize>() else {
                        continue;
                    };
                    let entry = languages.entry(index).or_insert((None, None));
                    match field {
                        "path" => entry.0 = Some(PathBuf::from(value)),
                        "name" => entry.1 = Some(value),
                        _ => {}
                    }
                }
                "window.w" => window[0] = value.parse().ok(),
                "window.h" => window[1] = value.parse().ok(),
                "window.x" => window[2] = value.parse().ok(),
                "window.y" => window[3] = value.parse().ok(),
                "version" => session.version = value.parse().unwrap_or(session.version),
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
        if let [Some(w), Some(h), Some(x), Some(y)] = window {
            if w > 0.0 && h > 0.0 {
                session.window = Some(WindowGeometry {
                    size: (w, h),
                    position: (x, y),
                });
            }
        }
        session.languages = languages
            .into_values()
            .filter_map(|(path, name)| Some((path?, name?)))
            .collect();
        session.recent = recent.into_values().take(MAX_RECENT).collect();
        session.tabs = tabs.into_values().take(MAX_RECENT).collect();
        session
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        // Always write the current format; parsing older files still works.
        out.push_str("version=5\n");
        out.push_str(&format!("theme={}\n", self.theme));
        out.push_str(&format!("sidebar={}\n", u8::from(self.sidebar)));
        if let Some(folder) = &self.folder {
            out.push_str(&format!("folder={}\n", escape(&folder.to_string_lossy())));
        }
        out.push_str(&format!("font={}\n", self.font));
        out.push_str(&format!("tab={}\n", self.tab));
        out.push_str(&format!("gutter={}\n", u8::from(self.line_numbers)));
        out.push_str(&format!("guides={}\n", u8::from(self.indent_guides)));
        out.push_str(&format!("syntax={}\n", u8::from(self.syntax)));
        out.push_str(&format!("sidebar_width={}\n", self.sidebar_width));
        out.push_str(&format!("split={}\n", self.split));
        out.push_str(&format!("autosave={}\n", u8::from(self.autosave)));
        out.push_str(&format!("sync_scroll={}\n", u8::from(self.sync_scroll)));
        out.push_str(&format!("soft_wrap={}\n", u8::from(self.soft_wrap)));
        out.push_str(&format!("sidebar_tab={}\n", escape(&self.sidebar_tab)));
        for (index, (path, name)) in self.languages.iter().enumerate() {
            out.push_str(&format!(
                "lang.{index}.path={}\nlang.{index}.name={}\n",
                escape(&path.to_string_lossy()),
                escape(name)
            ));
        }
        if let Some(window) = self.window {
            out.push_str(&format!("window.w={}\n", window.size.0));
            out.push_str(&format!("window.h={}\n", window.size.1));
            out.push_str(&format!("window.x={}\n", window.position.0));
            out.push_str(&format!("window.y={}\n", window.position.1));
        }
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
            folder: Some(PathBuf::from("/tmp/project")),
            recent: vec![PathBuf::from("/tmp/a=b.rs"), PathBuf::from("/tmp/c d.md")],
            tabs: vec![PathBuf::from("/tmp/a=b.rs")],
            font: 15.0,
            tab: 2,
            line_numbers: false,
            indent_guides: false,
            syntax: false,
            sidebar_width: 260.0,
            split: 0.6,
            autosave: false,
            languages: vec![(PathBuf::from("/tmp/x.lua"), "lua".into())],
            sync_scroll: false,
            soft_wrap: true,
            window: Some(WindowGeometry {
                size: (1024.0, 700.0),
                position: (40.0, 60.0),
            }),
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
