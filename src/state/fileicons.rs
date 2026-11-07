//! Runtime file icons for the Explorer.
//!
//! The pure theme logic lives in [`crate::model::icon_theme`]; this module owns
//! discovery, the SVG source cache and the visible-row preload. Paint looks up
//! a cached source by icon path, so `ui/` never opens a file.
//!
//! Loading is lazy: nothing is read until the Explorer is visible with **File
//! Icons** on, so a session that never opens a project (or turns icons off)
//! pays nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::model::icon_theme::{self, IconTheme};

/// Refuse absurdly large SVG files (a theme is a few hundred KB at most).
const MAX_ICON_BYTES: u64 = 256 * 1024;
/// Cache cap; the visible window needs a few dozen, so this is generous.
const MAX_SOURCES: usize = 1024;

/// One theme plus its loaded SVG sources.
#[derive(Default)]
pub struct FileIcons {
    theme: Option<IconTheme>,
    /// Discovery/parse already ran (successfully or not).
    loaded: bool,
    error: Option<String>,
    sources: HashMap<PathBuf, Option<Rc<str>>>,
}

impl FileIcons {
    /// Loads the theme once. Returns whether anything changed (a repaint is
    /// warranted).
    pub fn load(&mut self) -> bool {
        if self.loaded {
            return false;
        }
        self.loaded = true;
        let Some(json) = icon_theme::discover() else {
            self.error = Some("Material Icon Theme is not installed".into());
            return true;
        };
        match IconTheme::load(&json) {
            Ok(theme) => {
                self.theme = Some(theme);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }

    /// Whether discovery/parse already ran.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Whether a theme is ready to resolve names.
    pub fn is_ready(&self) -> bool {
        self.theme.is_some()
    }

    /// Last load failure, for status/diagnostics.
    #[cfg_attr(
        not(feature = "selfcheck"),
        allow(dead_code, reason = "reported by the headless self-check")
    )]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Path of the loaded theme JSON.
    #[cfg_attr(
        not(feature = "selfcheck"),
        allow(dead_code, reason = "reported by the headless self-check")
    )]
    pub fn source(&self) -> Option<&Path> {
        self.theme.as_ref().map(IconTheme::source)
    }

    /// Resolved icon file for one tree row.
    pub fn icon_for(
        &self,
        path: &Path,
        is_dir: bool,
        expanded: bool,
        light: bool,
    ) -> Option<&Path> {
        let theme = self.theme.as_ref()?;
        let name = path.file_name()?.to_string_lossy();
        if is_dir {
            theme.folder_icon(&name, expanded, false, light)
        } else {
            theme.file_icon(&name, light)
        }
    }

    /// Cached SVG text for an icon path.
    pub fn source_for(&self, icon: &Path) -> Option<&str> {
        self.sources.get(icon)?.as_deref()
    }

    /// Reads an icon's SVG once. Returns whether the cache changed.
    pub fn ensure_source(&mut self, icon: &Path) -> bool {
        if self.sources.contains_key(icon) {
            return false;
        }
        if self.sources.len() >= MAX_SOURCES {
            self.sources.clear();
        }
        let source = std::fs::metadata(icon)
            .ok()
            .filter(|metadata| metadata.len() <= MAX_ICON_BYTES)
            .and_then(|_| std::fs::read_to_string(icon).ok())
            .map(Rc::<str>::from);
        self.sources.insert(icon.to_path_buf(), source);
        true
    }
}

impl std::fmt::Debug for FileIcons {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileIcons")
            .field("theme", &self.source())
            .field("loaded", &self.loaded)
            .field("sources", &self.sources.len())
            .finish()
    }
}

impl super::AppState {
    /// Whether the Explorer should reserve the icon column.
    pub(crate) fn file_icons_active(&self) -> bool {
        self.settings.file_icons && self.file_icons.is_ready()
    }

    /// Resolved `(cache key, SVG text)` for one Explorer row, if icons are on
    /// and the theme has an entry.
    pub(crate) fn file_icon(
        &self,
        path: &Path,
        is_dir: bool,
        expanded: bool,
    ) -> Option<(&str, &str)> {
        if !self.settings.file_icons {
            return None;
        }
        let light = self.theme.mode == crate::theme::Mode::Light;
        let icon = self.file_icons.icon_for(path, is_dir, expanded, light)?;
        let key = icon.to_str()?;
        let source = self.file_icons.source_for(icon)?;
        Some((key, source))
    }

    /// Loads the theme and the SVG sources needed by the visible Explorer
    /// window. Runs before paint; painters only read the cache.
    pub fn tick_file_icons(&mut self) {
        if !self.settings.file_icons
            || !self.ws.folder_open
            || !self.show_sidebar
            || self.sidebar_tab != "explorer"
        {
            return;
        }
        if !self.file_icons.is_loaded() && self.file_icons.load() {
            self.dirty = true;
        }
        if !self.file_icons.is_ready() {
            return;
        }
        let rows = self.ws.tree.rows();
        if rows.is_empty() {
            return;
        }
        let row_h = self.theme.m.row_h.max(1.0);
        let view_h = self.sidebar.rect.size.height.max(0.0);
        let first = (self.sidebar.scroll / row_h).floor().max(0.0) as usize;
        let visible = (view_h / row_h).ceil().max(1.0) as usize + 1;
        let last = (first + visible).min(rows.len());
        let light = self.theme.mode == crate::theme::Mode::Light;
        let mut changed = false;
        for row in &rows[first..last] {
            let Some(icon) = self
                .file_icons
                .icon_for(&row.path, row.is_dir, row.expanded, light)
            else {
                continue;
            };
            let icon = icon.to_path_buf();
            changed |= self.file_icons.ensure_source(&icon);
        }
        if changed {
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `THEME_ENV` is process-global; tests that change it must not run in
    /// parallel with each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-fileicons-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const THEME: &str = r##"{
        "iconDefinitions": {
            "file": {"iconPath": "../icons/file.svg"},
            "rust": {"iconPath": "../icons/rust.svg"},
            "folder": {"iconPath": "../icons/folder.svg"},
            "folder-open": {"iconPath": "../icons/folder-open.svg"}
        },
        "file": "file",
        "folder": "folder",
        "folderExpanded": "folder-open",
        "fileExtensions": {"rs": "rust"}
    }"##;

    #[test]
    fn reads_an_icon_once_and_remembers_failures() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_dir("source");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join("dist/material-icons.json"), THEME).unwrap();
        std::fs::write(
            dir.join("icons/rust.svg"),
            r##"<svg viewBox="0 0 8 8"><path fill="#f00" d="M0 0h8v8H0z"/></svg>"##,
        )
        .unwrap();
        let mut icons = FileIcons::default();
        let previous = std::env::var_os(icon_theme::THEME_ENV);
        std::env::set_var(icon_theme::THEME_ENV, dir.join("dist/material-icons.json"));
        let changed = icons.load();
        match previous {
            Some(value) => std::env::set_var(icon_theme::THEME_ENV, value),
            None => std::env::remove_var(icon_theme::THEME_ENV),
        }
        assert!(changed);
        assert!(icons.is_ready());
        assert!(!icons.load(), "a loaded theme is not re-read");
        let icon = icons
            .icon_for(Path::new("/tmp/main.rs"), false, false, false)
            .expect("rust icon")
            .to_path_buf();
        assert!(icon.ends_with("icons/rust.svg"));
        assert!(icons.source_for(&icon).is_none());
        assert!(icons.ensure_source(&icon));
        assert!(!icons.ensure_source(&icon), "the source is cached");
        assert!(icons.source_for(&icon).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_theme_reports_an_error_and_resolves_nothing() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_dir("broken");
        let json = dir.join("material-icons.json");
        std::fs::write(&json, "{ this is not json").unwrap();
        let mut icons = FileIcons::default();
        let previous = std::env::var_os(icon_theme::THEME_ENV);
        std::env::set_var(icon_theme::THEME_ENV, &json);
        assert!(icons.load());
        match previous {
            Some(value) => std::env::set_var(icon_theme::THEME_ENV, value),
            None => std::env::remove_var(icon_theme::THEME_ENV),
        }
        assert!(!icons.is_ready());
        assert!(icons.error().is_some());
        assert!(!icons.load(), "a failed load is not retried every frame");
        assert!(icons
            .icon_for(Path::new("/tmp/main.rs"), false, false, false)
            .is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
