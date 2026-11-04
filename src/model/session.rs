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

use super::fsutil;

const MAX_RECENT: usize = 8;

/// Persisted application state. Missing/corrupt files fall back to defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// Format version (currently `15`); unknown keys are ignored so
    /// older/newer files keep loading.
    pub version: u32,
    /// `"dark"` or `"light"`.
    pub theme: String,
    pub sidebar: bool,
    pub outline: bool,
    /// Right outline panel width.
    pub outline_width: f32,
    /// The open project root (`None` = no project open).
    pub project: Option<PathBuf>,
    /// Recently opened projects (directories).
    pub recent_projects: Vec<PathBuf>,
    /// Recently opened files, including files dropped onto the window.
    pub recent_files: Vec<PathBuf>,
    pub tabs: Vec<PathBuf>,
    /// Path of the tab that was active when the session was saved.
    ///
    /// Stored as a path, not an index, so a missing file in `tabs` cannot shift
    /// the restored selection onto the wrong tab.
    pub active_tab: Option<PathBuf>,
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
    /// Language servers enabled.
    pub lsp: bool,
    /// Allow repository-local language-server binaries.
    pub trust_local_servers: bool,
    /// Aggressive Godot LSP reload on file changes and explicit saves.
    pub lsp_reload: bool,
    /// Per-path language overrides (provider name).
    pub languages: Vec<(PathBuf, String)>,
    /// Keep the editor and Markdown preview scroll positions in sync.
    pub sync_scroll: bool,
    /// Soft-wrap long editor lines.
    pub soft_wrap: bool,
    /// Active sidebar tab id (`explorer`, `git`, …).
    pub sidebar_tab: String,
    pub terminal_open: bool,
    pub terminal_height: f32,
    /// Active bottom-dock tab (`terminal`, `problems`).
    pub bottom_tab: String,
    /// Keyboard profile (`vscode`, `idea`, `vim`).
    pub keymap: String,
    /// Last window geometry (logical size + position).
    pub window: Option<WindowGeometry>,
    /// Restore the window maximized (zoomed).
    pub window_maximized: bool,
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
            version: 15,
            theme: "dark".into(),
            sidebar: true,
            outline: true,
            outline_width: 224.0,
            project: None,
            recent_projects: Vec::new(),
            recent_files: Vec::new(),
            tabs: Vec::new(),
            active_tab: None,
            font: 13.0,
            tab: 4,
            line_numbers: true,
            indent_guides: true,
            syntax: true,
            sidebar_width: 232.0,
            split: 0.5,
            autosave: true,
            lsp: true,
            trust_local_servers: false,
            lsp_reload: false,
            languages: Vec::new(),
            sync_scroll: true,
            soft_wrap: false,
            sidebar_tab: "explorer".into(),
            terminal_open: false,
            terminal_height: 220.0,
            bottom_tab: "terminal".into(),
            keymap: "vscode".into(),
            window: None,
            window_maximized: false,
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
        let mut recent_projects: BTreeMap<usize, PathBuf> = BTreeMap::new();
        let mut recent_files: BTreeMap<usize, PathBuf> = BTreeMap::new();
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
                "outline" => session.outline = value == "1",
                "outline_width" => {
                    session.outline_width = value.parse().unwrap_or(session.outline_width)
                }
                "project" | "folder" => session.project = Some(PathBuf::from(value)),
                "active" => session.active_tab = Some(PathBuf::from(value)),
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
                "lsp" => session.lsp = value == "1",
                "trust_local_servers" => session.trust_local_servers = value == "1",
                "lsp_reload" => session.lsp_reload = value == "1",
                "sync_scroll" => session.sync_scroll = value == "1",
                "soft_wrap" => session.soft_wrap = value == "1",
                "sidebar_tab" => session.sidebar_tab = value,
                "terminal_open" => session.terminal_open = value == "1",
                "terminal_height" => {
                    session.terminal_height = value.parse().unwrap_or(session.terminal_height)
                }
                "bottom_tab" => session.bottom_tab = value,
                "keymap" => session.keymap = value,
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
                "window.max" => session.window_maximized = value == "1",
                "version" => session.version = value.parse().unwrap_or(session.version),
                key if key.starts_with("recent_project.") => {
                    if let Ok(index) = key["recent_project.".len()..].parse::<usize>() {
                        recent_projects.insert(index, PathBuf::from(value));
                    }
                }
                key if key.starts_with("recent_file.") => {
                    if let Ok(index) = key["recent_file.".len()..].parse::<usize>() {
                        recent_files.insert(index, PathBuf::from(value));
                    }
                }
                // Pre-project sessions mixed projects and files in `recent.*`.
                // Drop those entries and rebuild both lists for v14.
                key if key.starts_with("recent.") => {}
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
        if session.version >= 14 {
            session.recent_projects = recent_projects.into_values().take(MAX_RECENT).collect();
            session.recent_files = recent_files.into_values().take(MAX_RECENT).collect();
        }
        session.tabs = tabs.into_values().take(MAX_RECENT).collect();
        session
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        // Always write the current format; parsing older files still works.
        out.push_str("version=15\n");
        out.push_str(&format!("theme={}\n", self.theme));
        out.push_str(&format!("sidebar={}\n", u8::from(self.sidebar)));
        out.push_str(&format!("outline={}\n", u8::from(self.outline)));
        out.push_str(&format!("outline_width={}\n", self.outline_width));
        if let Some(project) = &self.project {
            out.push_str(&format!("project={}\n", escape(&project.to_string_lossy())));
        }
        if let Some(active) = &self.active_tab {
            out.push_str(&format!("active={}\n", escape(&active.to_string_lossy())));
        }
        out.push_str(&format!("font={}\n", self.font));
        out.push_str(&format!("tab={}\n", self.tab));
        out.push_str(&format!("gutter={}\n", u8::from(self.line_numbers)));
        out.push_str(&format!("guides={}\n", u8::from(self.indent_guides)));
        out.push_str(&format!("syntax={}\n", u8::from(self.syntax)));
        out.push_str(&format!("sidebar_width={}\n", self.sidebar_width));
        out.push_str(&format!("split={}\n", self.split));
        out.push_str(&format!("autosave={}\n", u8::from(self.autosave)));
        out.push_str(&format!("lsp={}\n", u8::from(self.lsp)));
        out.push_str(&format!(
            "trust_local_servers={}\n",
            u8::from(self.trust_local_servers)
        ));
        out.push_str(&format!("lsp_reload={}\n", u8::from(self.lsp_reload)));
        out.push_str(&format!("sync_scroll={}\n", u8::from(self.sync_scroll)));
        out.push_str(&format!("soft_wrap={}\n", u8::from(self.soft_wrap)));
        out.push_str(&format!("sidebar_tab={}\n", escape(&self.sidebar_tab)));
        out.push_str(&format!("terminal_open={}\n", u8::from(self.terminal_open)));
        out.push_str(&format!("terminal_height={}\n", self.terminal_height));
        out.push_str(&format!("bottom_tab={}\n", escape(&self.bottom_tab)));
        out.push_str(&format!("keymap={}\n", self.keymap));
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
        out.push_str(&format!("window.max={}\n", u8::from(self.window_maximized)));
        for (index, path) in self.recent_projects.iter().take(MAX_RECENT).enumerate() {
            out.push_str(&format!(
                "recent_project.{index}={}\n",
                escape(&path.to_string_lossy())
            ));
        }
        for (index, path) in self.recent_files.iter().take(MAX_RECENT).enumerate() {
            out.push_str(&format!(
                "recent_file.{index}={}\n",
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
        fsutil::atomic_write(path, self.render().as_bytes(), None)
    }

    /// Moves a project root to the front of the recent-project list.
    pub fn push_recent_project(&mut self, path: &Path) {
        self.recent_projects.retain(|candidate| candidate != path);
        self.recent_projects.insert(0, path.to_path_buf());
        self.recent_projects.truncate(MAX_RECENT);
    }

    /// Moves a file to the front of the recent-file list.
    pub fn push_recent_file(&mut self, path: &Path) {
        self.recent_files.retain(|candidate| candidate != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(MAX_RECENT);
    }

    /// Clears both recent lists.
    pub fn clear_recents(&mut self) {
        self.recent_projects.clear();
        self.recent_files.clear();
    }

    /// Replaces the restored-tab list.
    ///
    /// Clears `active_tab` if it is no longer among the saved tabs.
    pub fn set_tabs(&mut self, tabs: Vec<PathBuf>) {
        self.tabs = tabs.into_iter().take(MAX_RECENT).collect();
        if self
            .active_tab
            .as_ref()
            .is_some_and(|active| !self.tabs.contains(active))
        {
            self.active_tab = None;
        }
    }

    /// Records the active tab by path (or clears it for an untitled/synthetic
    /// view).
    pub fn set_active_tab(&mut self, active: Option<PathBuf>) {
        self.active_tab = active.filter(|path| self.tabs.contains(path));
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
            project: Some(PathBuf::from("/tmp/project")),
            recent_projects: vec![PathBuf::from("/tmp/project")],
            recent_files: vec![PathBuf::from("/tmp/a=b.rs"), PathBuf::from("/tmp/c d.md")],
            tabs: vec![PathBuf::from("/tmp/a=b.rs")],
            active_tab: Some(PathBuf::from("/tmp/a=b.rs")),
            font: 15.0,
            tab: 2,
            line_numbers: false,
            indent_guides: false,
            syntax: false,
            sidebar_width: 260.0,
            split: 0.6,
            autosave: false,
            lsp: false,
            trust_local_servers: true,
            lsp_reload: true,
            languages: vec![(PathBuf::from("/tmp/x.lua"), "lua".into())],
            sync_scroll: false,
            soft_wrap: true,
            keymap: "vim".into(),
            outline_width: 260.0,
            window: Some(WindowGeometry {
                size: (1024.0, 700.0),
                position: (40.0, 60.0),
            }),
            window_maximized: true,
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
    fn bottom_dock_tab_round_trips() {
        let path = temp_path("bottom-tab");
        let session = Session {
            bottom_tab: "problems".into(),
            terminal_open: true,
            ..Session::default()
        };
        session.save(&path).unwrap();
        let loaded = Session::load(&path);
        assert_eq!(loaded.bottom_tab, "problems");
        assert!(loaded.terminal_open);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_uses_defaults() {
        let session = Session::load(&temp_path("missing"));
        assert_eq!(session.theme, "dark");
        assert!(session.sidebar);
        assert!(session.recent_projects.is_empty());
        assert!(session.recent_files.is_empty());
    }

    #[test]
    fn values_escape_backslash_and_newlines() {
        let path = temp_path("escape");
        let weird = PathBuf::from("/tmp/back\\slash and space");
        let session = Session {
            recent_files: vec![weird.clone()],
            ..Session::default()
        };
        session.save(&path).unwrap();
        let loaded = Session::load(&path);
        assert_eq!(loaded.recent_files, vec![weird]);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn active_tab_is_cleared_when_it_leaves_the_saved_tabs() {
        let mut session = Session {
            tabs: vec![PathBuf::from("/tmp/a.rs"), PathBuf::from("/tmp/b.rs")],
            active_tab: Some(PathBuf::from("/tmp/b.rs")),
            ..Session::default()
        };
        session.set_tabs(vec![PathBuf::from("/tmp/a.rs")]);
        assert_eq!(session.active_tab, None);
        session.set_tabs(vec![PathBuf::from("/tmp/a.rs"), PathBuf::from("/tmp/b.rs")]);
        session.set_active_tab(Some(PathBuf::from("/tmp/b.rs")));
        assert_eq!(session.active_tab, Some(PathBuf::from("/tmp/b.rs")));
    }

    #[test]
    fn old_sessions_default_language_servers_to_on() {
        let path = temp_path("lsp-default");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "version=11\ntheme=dark\n").unwrap();
        let session = Session::load(&path);
        assert!(session.lsp);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn old_sessions_without_an_active_tab_load_with_none() {
        let path = temp_path("no-active");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "version=9\ntheme=light\ntabs.0=/tmp/a.rs\n").unwrap();
        let session = Session::load(&path);
        assert_eq!(session.theme, "light");
        assert_eq!(session.tabs, vec![PathBuf::from("/tmp/a.rs")]);
        assert_eq!(session.active_tab, None);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn recent_is_deduplicated_and_capped() {
        let mut session = Session::default();
        for index in 0..12 {
            session.push_recent_file(Path::new(&format!("/tmp/file-{index}")));
        }
        session.push_recent_file(Path::new("/tmp/file-11"));
        assert_eq!(session.recent_files.len(), MAX_RECENT);
        assert_eq!(session.recent_files[0], PathBuf::from("/tmp/file-11"));
        let unique = session
            .recent_files
            .iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), session.recent_files.len());
    }

    #[test]
    fn corrupt_lines_are_ignored() {
        let session = Session::parse("theme=neon\nsidebar=1\nrecent.nope=/x\nno equals\n");
        assert_eq!(session.theme, "dark");
        assert!(session.sidebar);
        assert!(session.recent_projects.is_empty());
        assert!(session.recent_files.is_empty());
    }
}
