//! VS Code file-icon themes: JSON parsing and name lookup.
//!
//! The Explorer reads the installed **Material Icon Theme** extension at
//! runtime. Its `dist/material-icons.json` follows the VS Code icon-theme
//! format: named `iconDefinitions` with a relative `iconPath`, plus tables that
//! map file names, file extensions and folder names to a definition. Nothing is
//! vendored into this repository and no artwork is generated at build time.
//!
//! This module is pure: [`IconTheme::parse`] takes the JSON text and the
//! directory it came from, and every lookup is a plain map read. IO, discovery
//! and cache invalidation live in `state/fileicons.rs`; rasterization lives in
//! `ui/fileicon.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Environment override: a theme JSON file or an extension directory.
pub const THEME_ENV: &str = "IGUI_ICON_THEME";

/// Directory names that install VS Code extensions, in probing order.
const EXTENSION_DIRS: &[&str] = &[
    ".vscode/extensions",
    ".vscode-insiders/extensions",
    ".vscode-oss/extensions",
    ".cursor/extensions",
    ".windsurf/extensions",
];

/// The extension folder prefix of the Material Icon Theme.
const MATERIAL_PREFIX: &str = "pkief.material-icon-theme-";

/// Relative location of the theme JSON inside the extension.
const THEME_FILE: &str = "dist/material-icons.json";

/// Resolved definition tables of one theme (dark or the `light` override).
#[derive(Debug, Default, Clone)]
struct Tables {
    file_names: HashMap<String, PathBuf>,
    file_extensions: HashMap<String, PathBuf>,
    folder_names: HashMap<String, PathBuf>,
    folder_names_expanded: HashMap<String, PathBuf>,
}

/// A parsed VS Code icon theme.
#[derive(Debug, Clone)]
pub struct IconTheme {
    /// Path of the JSON the theme was parsed from (diagnostics/tests).
    source: PathBuf,
    definitions: HashMap<String, PathBuf>,
    file_names: HashMap<String, PathBuf>,
    file_extensions: HashMap<String, PathBuf>,
    folder_names: HashMap<String, PathBuf>,
    folder_names_expanded: HashMap<String, PathBuf>,
    light: Tables,
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    root_folder: Option<String>,
    root_folder_expanded: Option<String>,
}

impl IconTheme {
    /// Reads and parses one theme JSON file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read icon theme {}: {error}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Self::parse(&text, base).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Parses theme JSON. `base` resolves relative `iconPath` values (the
    /// directory containing the JSON, exactly like VS Code).
    pub fn parse(text: &str, base: &Path) -> Result<Self, String> {
        let raw: RawTheme = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut definitions = HashMap::new();
        for (id, definition) in raw.icon_definitions {
            // Font-based definitions (`fontCharacter`) have no SVG to draw;
            // dropping them makes lookups fall back to the default icon.
            if let Some(icon_path) = definition.icon_path.as_deref() {
                if icon_path.is_empty() {
                    continue;
                }
                let path = Path::new(icon_path);
                let resolved = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    base.join(path)
                };
                definitions.insert(id, normalize(&resolved));
            }
        }
        if definitions.is_empty() {
            return Err("the theme has no SVG icon definitions".into());
        }
        let resolve = |ids: HashMap<String, String>| -> HashMap<String, PathBuf> {
            ids.into_iter()
                .filter_map(|(name, id)| {
                    definitions
                        .get(&id)
                        .map(|path| (name.to_ascii_lowercase(), path.clone()))
                })
                .collect()
        };
        let light = raw.light.unwrap_or_default();
        Ok(Self {
            file_names: resolve(raw.file_names),
            file_extensions: resolve(raw.file_extensions),
            folder_names: resolve(raw.folder_names),
            folder_names_expanded: resolve(raw.folder_names_expanded),
            light: Tables {
                file_names: resolve(light.file_names),
                file_extensions: resolve(light.file_extensions),
                folder_names: resolve(light.folder_names),
                folder_names_expanded: resolve(light.folder_names_expanded),
            },
            file: raw.file,
            folder: raw.folder,
            folder_expanded: raw.folder_expanded,
            root_folder: raw.root_folder,
            root_folder_expanded: raw.root_folder_expanded,
            definitions,
            source: base.to_path_buf(),
        })
    }

    /// Path of the JSON this theme came from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Number of usable SVG definitions (diagnostics/self-check).
    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    /// Icon for a file name, following VS Code's precedence.
    ///
    /// Exact lowercased name wins, then compound-extension suffixes from the
    /// longest to the shortest (`foo.d.ts` tries `d.ts`, then `ts`), then the
    /// theme's default `file` icon. A light theme checks its override maps
    /// first.
    pub fn file_icon(&self, name: &str, light: bool) -> Option<&Path> {
        let lower = name.to_ascii_lowercase();
        if lower.is_empty() {
            return self.default_file();
        }
        if light {
            if let Some(path) = self.light.file_names.get(&lower) {
                return Some(path.as_path());
            }
        }
        if let Some(path) = self.file_names.get(&lower) {
            return Some(path.as_path());
        }
        let mut start = 0usize;
        while let Some(offset) = lower[start..].find('.') {
            let suffix_start = start + offset + 1;
            let suffix = &lower[suffix_start..];
            if !suffix.is_empty() {
                if light {
                    if let Some(path) = self.light.file_extensions.get(suffix) {
                        return Some(path.as_path());
                    }
                }
                if let Some(path) = self.file_extensions.get(suffix) {
                    return Some(path.as_path());
                }
            }
            start = suffix_start;
        }
        self.default_file()
    }

    /// Icon for a folder. `root` selects the root-folder default; a named
    /// folder can override both the closed and expanded artwork.
    pub fn folder_icon(
        &self,
        name: &str,
        expanded: bool,
        root: bool,
        light: bool,
    ) -> Option<&Path> {
        if root {
            let id = if expanded {
                self.root_folder_expanded
                    .as_ref()
                    .or(self.root_folder.as_ref())
            } else {
                self.root_folder.as_ref()
            };
            if let Some(path) = id.and_then(|id| self.definitions.get(id)) {
                return Some(path.as_path());
            }
        }
        let lower = name.to_ascii_lowercase();
        if light {
            if let Some(path) = named_folder(
                &self.light.folder_names,
                &self.light.folder_names_expanded,
                &lower,
                expanded,
            ) {
                return Some(path);
            }
        }
        if let Some(path) = named_folder(
            &self.folder_names,
            &self.folder_names_expanded,
            &lower,
            expanded,
        ) {
            return Some(path);
        }
        let default = if expanded {
            self.folder_expanded.as_ref().or(self.folder.as_ref())
        } else {
            self.folder.as_ref()
        };
        default
            .and_then(|id| self.definitions.get(id))
            .map(PathBuf::as_path)
    }

    fn default_file(&self) -> Option<&Path> {
        self.file
            .as_ref()
            .and_then(|id| self.definitions.get(id))
            .map(PathBuf::as_path)
    }
}

/// Lexically normalizes `./` and `..` segments so cache keys and diagnostics do
/// not depend on how the theme wrote a relative path.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let pop = !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                        | Some(Component::ParentDir)
                        | None
                );
                if pop {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Named-folder lookup shared by the dark tables and the light overrides.
fn named_folder<'a>(
    names: &'a HashMap<String, PathBuf>,
    expanded_names: &'a HashMap<String, PathBuf>,
    name: &str,
    expanded: bool,
) -> Option<&'a Path> {
    let path = if expanded {
        expanded_names.get(name).or_else(|| names.get(name))
    } else {
        names.get(name)
    };
    path.map(PathBuf::as_path)
}

/// Raw JSON shape (unknown keys are ignored, font definitions tolerated).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawTheme {
    #[serde(default)]
    icon_definitions: HashMap<String, RawDefinition>,
    #[serde(default)]
    file_names: HashMap<String, String>,
    #[serde(default)]
    file_extensions: HashMap<String, String>,
    #[serde(default)]
    folder_names: HashMap<String, String>,
    #[serde(default)]
    folder_names_expanded: HashMap<String, String>,
    #[serde(default)]
    light: Option<RawLight>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    folder_expanded: Option<String>,
    #[serde(default)]
    root_folder: Option<String>,
    #[serde(default)]
    root_folder_expanded: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawDefinition {
    #[serde(default)]
    icon_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawLight {
    #[serde(default)]
    file_names: HashMap<String, String>,
    #[serde(default)]
    file_extensions: HashMap<String, String>,
    #[serde(default)]
    folder_names: HashMap<String, String>,
    #[serde(default)]
    folder_names_expanded: HashMap<String, String>,
}

/// Resolves the theme JSON to load: `IGUI_ICON_THEME` first, then the newest
/// installed Material Icon Theme.
pub fn discover() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(THEME_ENV) {
        if !path.is_empty() {
            if let Some(json) = theme_json_in(Path::new(&path)) {
                return Some(json);
            }
        }
    }
    let home = home_dir()?;
    discover_in(
        &EXTENSION_DIRS
            .iter()
            .map(|dir| home.join(dir))
            .collect::<Vec<_>>(),
    )
}

/// Discovery over explicit extension roots (tests and portable installs).
pub fn discover_in(extension_dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for root in extension_dirs {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(version) = name
                .to_ascii_lowercase()
                .strip_prefix(MATERIAL_PREFIX)
                .map(version_key)
            else {
                continue;
            };
            let Some(json) = theme_json_in(&entry.path()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(best, _)| version > *best) {
                best = Some((version, json));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Theme JSON inside a file or an extension directory.
fn theme_json_in(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    for relative in [THEME_FILE, "material-icons.json"] {
        let candidate = path.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Numeric version key so `5.40.0` sorts above `5.9.0`.
fn version_key(value: &str) -> Vec<u64> {
    value
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small theme covering every precedence rule.
    const FIXTURE: &str = r##"{
        "iconDefinitions": {
            "file": {"iconPath": "./../icons/file.svg"},
            "rust": {"iconPath": "./../icons/rust.svg"},
            "typescript": {"iconPath": "./../icons/typescript.svg"},
            "dts": {"iconPath": "./../icons/dts.svg"},
            "readme": {"iconPath": "./../icons/readme.svg"},
            "folder": {"iconPath": "./../icons/folder.svg"},
            "folder-open": {"iconPath": "./../icons/folder-open.svg"},
            "folder-src": {"iconPath": "./../icons/folder-src.svg"},
            "folder-src-open": {"iconPath": "./../icons/folder-src-open.svg"},
            "rust-light": {"iconPath": "./../icons/rust-light.svg"},
            "font-only": {"fontCharacter": "\\uE001"}
        },
        "file": "file",
        "folder": "folder",
        "folderExpanded": "folder-open",
        "fileNames": {"readme.md": "readme", "Cargo.toml": "rust"},
        "fileExtensions": {"rs": "rust", "ts": "typescript", "d.ts": "dts"},
        "folderNames": {"src": "folder-src", "font-only": "font-only"},
        "folderNamesExpanded": {"src": "folder-src-open"},
        "light": {
            "fileExtensions": {"rs": "rust-light"}
        }
    }"##;

    fn theme() -> IconTheme {
        IconTheme::parse(FIXTURE, Path::new("/themes/ext/dist")).expect("fixture parses")
    }

    #[test]
    fn resolves_relative_icon_paths_against_the_json_directory() {
        let theme = theme();
        assert_eq!(
            theme.file_icon("main.rs", false),
            Some(Path::new("/themes/ext/icons/rust.svg"))
        );
        assert_eq!(theme.definition_count(), 10, "font definitions are dropped");
    }

    #[test]
    fn exact_file_names_beat_extensions_and_are_case_insensitive() {
        let theme = theme();
        assert_eq!(
            theme.file_icon("README.MD", false),
            Some(Path::new("/themes/ext/icons/readme.svg"))
        );
        assert_eq!(
            theme.file_icon("Cargo.TOML", false),
            Some(Path::new("/themes/ext/icons/rust.svg"))
        );
        assert_eq!(
            theme.file_icon("main.RS", false),
            Some(Path::new("/themes/ext/icons/rust.svg"))
        );
    }

    #[test]
    fn compound_extensions_win_over_the_last_segment() {
        let theme = theme();
        assert_eq!(
            theme.file_icon("types.d.ts", false),
            Some(Path::new("/themes/ext/icons/dts.svg"))
        );
        assert_eq!(
            theme.file_icon("types.ts", false),
            Some(Path::new("/themes/ext/icons/typescript.svg"))
        );
    }

    #[test]
    fn unknown_files_use_the_default_and_font_definitions_fall_back() {
        let theme = theme();
        assert_eq!(
            theme.file_icon("notes.xyz", false),
            Some(Path::new("/themes/ext/icons/file.svg"))
        );
    }

    #[test]
    fn light_overrides_are_checked_first() {
        let theme = theme();
        assert_eq!(
            theme.file_icon("main.rs", true),
            Some(Path::new("/themes/ext/icons/rust-light.svg"))
        );
        // Files without a light override keep the dark artwork.
        assert_eq!(
            theme.file_icon("README.md", true),
            Some(Path::new("/themes/ext/icons/readme.svg"))
        );
    }

    #[test]
    fn folders_use_named_and_expanded_artwork() {
        let theme = theme();
        assert_eq!(
            theme.folder_icon("src", false, false, false),
            Some(Path::new("/themes/ext/icons/folder-src.svg"))
        );
        assert_eq!(
            theme.folder_icon("src", true, false, false),
            Some(Path::new("/themes/ext/icons/folder-src-open.svg"))
        );
        assert_eq!(
            theme.folder_icon("other", false, false, false),
            Some(Path::new("/themes/ext/icons/folder.svg"))
        );
        assert_eq!(
            theme.folder_icon("other", true, false, false),
            Some(Path::new("/themes/ext/icons/folder-open.svg"))
        );
        // A definition that only exists as a font falls back to the default.
        assert_eq!(
            theme.folder_icon("font-only", false, false, false),
            Some(Path::new("/themes/ext/icons/folder.svg"))
        );
    }

    #[test]
    fn parses_the_installed_material_theme_when_present() {
        let Some(json) = discover() else {
            return; // No VS Code extension installed on this host.
        };
        let theme = IconTheme::load(&json).expect("installed theme parses");
        assert!(theme.definition_count() > 100);
        for (name, expected) in [
            ("main.rs", "rust"),
            ("index.ts", "typescript"),
            ("README.md", "readme"),
            ("project.godot", "godot"),
            ("Cargo.toml", "toml"),
        ] {
            let icon = theme
                .file_icon(name, false)
                .unwrap_or_else(|| panic!("{name} has no icon"));
            let file = icon.file_name().unwrap_or_default().to_string_lossy();
            assert!(
                file.contains(expected),
                "{name} resolved to {file}, expected {expected}"
            );
        }
    }

    #[test]
    fn discovery_prefers_the_newest_installed_version() {
        let dir = std::env::temp_dir().join(format!(
            "igui-icon-discover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        for version in ["5.9.0", "5.40.0", "5.100.1"] {
            let extension = dir.join(format!("{MATERIAL_PREFIX}{version}"));
            std::fs::create_dir_all(extension.join("dist")).unwrap();
            std::fs::write(extension.join(THEME_FILE), FIXTURE).unwrap();
        }
        let found = discover_in(std::slice::from_ref(&dir)).expect("a theme was found");
        assert!(found
            .to_string_lossy()
            .ends_with("5.100.1/dist/material-icons.json"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
