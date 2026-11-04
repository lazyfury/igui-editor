//! A private Godot editor profile for the headless language server.
//!
//! Godot's GDScript LSP has no `workspace/didChangeWatchedFiles` handler. Its
//! project scan — and therefore its global `class_name` registry — only
//! refreshes when the editor rescans, which happens on window focus or on the
//! opt-in "rescan while unfocused" timer (`interface/editor/behavior/
//! import_resources_when_unfocused`, off by default). A spawned
//! `--headless --editor` process never gains focus, so a script created after
//! startup stayed unknown forever and a deleted one stayed resolved.
//!
//! [`LanguageServerSpec::file_changes`] marks Godot as a
//! [`FileChangePolicy::ServerRescan`] server; the client then gives the child a
//! private data directory whose editor settings enable that timer. The user's
//! own Godot settings are never read or written, and the attach path (a running
//! Godot editor) is unaffected.
//!
//! [`LanguageServerSpec::file_changes`]: crate::services::LanguageServerSpec::file_changes
//! [`FileChangePolicy::ServerRescan`]: crate::services::FileChangePolicy::ServerRescan

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// `(major, minor)` per executable, or `None` when `--version` is unusable.
type VersionCache = HashMap<PathBuf, Option<(u32, u32)>>;

/// Editor setting that makes Godot scan the project a couple of times a second
/// even without window focus. It is what updates the global class registry.
const RESCAN_SETTING: &str = "interface/editor/behavior/import_resources_when_unfocused";

/// Environment variables that point a spawned Godot at `base`.
///
/// The paths are the engine's own per-platform config directories: macOS reads
/// `~/Library/Application Support/Godot`, Linux `$XDG_CONFIG_HOME/godot`, and
/// Windows `%APPDATA%\Godot`.
pub fn prepare(command: &Path) -> Option<Vec<(OsString, OsString)>> {
    // A wrapper script may use `HOME` (version managers) to find the engine.
    // Hiding it behind a private profile would break the launch, so such a
    // launcher keeps the user's environment and only loses the auto-rescan.
    if is_script(command) {
        return None;
    }
    let (major, minor) = version(command)?;
    prepare_in(&base_dir()?, major, minor)
}

/// Whether `path` starts with a `#!` shebang.
fn is_script(path: &Path) -> bool {
    use std::io::Read;

    let mut head = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| head == *b"#!")
}

/// Prepares a profile under `base` (split out so tests never touch the real
/// user cache directory).
fn prepare_in(base: &Path, major: u32, minor: u32) -> Option<Vec<(OsString, OsString)>> {
    let settings = settings_path(base, major, minor)?;
    write_settings(&settings)?;
    Some(env_overrides(base))
}

fn settings_name(major: u32, minor: u32) -> String {
    format!("editor_settings-{major}.{minor}.tres")
}

/// `[gd_resource]` with only the settings the client needs. Godot keeps its
/// defaults for everything else and rewrites the full file when it exits.
fn settings_contents() -> String {
    format!(
        "[gd_resource type=\"EditorSettings\" format=3]\n\n[resource]\n{RESCAN_SETTING} = true\n"
    )
}

/// The app-owned profile root. A stable location keeps Godot's documentation
/// cache and import metadata warm between language-server restarts.
fn base_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home?.join("Library/Caches/igui-editor/godot-lsp"));
    }
    if cfg!(target_os = "windows") {
        return std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("igui-editor").join("godot-lsp"));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".cache")))?;
    Some(cache.join("igui-editor").join("godot-lsp"))
}

fn config_dir(base: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        base.join("home")
            .join("Library")
            .join("Application Support")
            .join("Godot")
    } else if cfg!(target_os = "windows") {
        base.join("appdata").join("Godot")
    } else {
        base.join("config").join("godot")
    }
}

fn settings_path(base: &Path, major: u32, minor: u32) -> Option<PathBuf> {
    Some(config_dir(base).join(settings_name(major, minor)))
}

fn env_overrides(base: &Path) -> Vec<(OsString, OsString)> {
    if cfg!(target_os = "macos") {
        return vec![("HOME".into(), base.join("home").into_os_string())];
    }
    if cfg!(target_os = "windows") {
        let appdata = base.join("appdata");
        return vec![
            ("APPDATA".into(), appdata.clone().into_os_string()),
            ("LOCALAPPDATA".into(), appdata.into_os_string()),
        ];
    }
    vec![
        (
            "XDG_CONFIG_HOME".into(),
            base.join("config").into_os_string(),
        ),
        ("XDG_DATA_HOME".into(), base.join("data").into_os_string()),
        ("XDG_CACHE_HOME".into(), base.join("cache").into_os_string()),
    ]
}

/// Writes the settings atomically; a running Godot may rewrite it on exit, so
/// every spawn re-applies the one setting this profile exists for.
fn write_settings(path: &Path) -> Option<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let temporary = path.with_extension("tres.tmp");
    std::fs::write(&temporary, settings_contents()).ok()?;
    std::fs::rename(&temporary, path).ok()?;
    Some(())
}

/// `major.minor` from `godot --version` (`4.7.2.stable.official…`). Cached per
/// executable: the editor-settings file name is versioned.
fn version(command: &Path) -> Option<(u32, u32)> {
    static CACHE: OnceLock<Mutex<VersionCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(VersionCache::new()));
    if let Some(known) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(command).copied())
    {
        return known;
    }
    let parsed = Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|output| parse_version(&String::from_utf8_lossy(&output.stdout)));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(command.to_path_buf(), parsed);
    }
    parsed
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    // `4.7.2.stable.official.ed1daf0bf`, but a wrapper may print a prefix.
    for token in text.split_whitespace() {
        let mut parts = token.split('.');
        let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
            continue;
        };
        return Some((major, minor));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-godot-profile-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn version_parsing_accepts_the_engine_banner_and_wrapper_noise() {
        assert_eq!(
            parse_version("4.7.2.stable.official.ed1daf0bf\n"),
            Some((4, 7))
        );
        assert_eq!(parse_version("godot 4.3.1 stable"), Some((4, 3)));
        assert_eq!(parse_version("not a version"), None);
    }

    #[test]
    fn settings_enable_the_unfocused_rescan() {
        let contents = settings_contents();
        assert!(contents.contains("type=\"EditorSettings\""));
        assert!(contents.contains("import_resources_when_unfocused = true"));
    }

    #[test]
    fn profile_paths_follow_the_engine_config_layout() {
        let base = Path::new("/tmp/profile");
        let path = settings_path(base, 4, 7).unwrap();
        assert_eq!(path.file_name().unwrap(), "editor_settings-4.7.tres");
        if cfg!(target_os = "macos") {
            assert!(path.starts_with(base.join("home/Library/Application Support/Godot")));
        } else if cfg!(target_os = "windows") {
            assert!(path.starts_with(base.join("appdata/Godot")));
        } else {
            assert!(path.starts_with(base.join("config/godot")));
        }
        let env: HashMap<OsString, OsString> = env_overrides(base).into_iter().collect();
        let get = |key: &str| env.get(std::ffi::OsStr::new(key)).map(OsString::as_os_str);
        if cfg!(target_os = "macos") {
            let home = base.join("home");
            assert_eq!(get("HOME"), Some(home.as_os_str()));
        } else if cfg!(target_os = "windows") {
            let appdata = base.join("appdata");
            assert_eq!(get("APPDATA"), Some(appdata.as_os_str()));
        } else {
            let config = base.join("config");
            assert_eq!(get("XDG_CONFIG_HOME"), Some(config.as_os_str()));
        }
    }

    #[test]
    fn prepare_writes_a_private_settings_file() {
        let dir = temp("write");
        let env = prepare_in(&dir, 4, 7).expect("a profile");
        let settings = settings_path(&dir, 4, 7).unwrap();
        let contents = std::fs::read_to_string(settings).unwrap();
        assert!(contents.contains("import_resources_when_unfocused = true"));
        assert!(!env.is_empty());
        // Re-preparing is idempotent (a running server may have rewritten it).
        assert!(prepare_in(&dir, 4, 7).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_scripts_keep_the_user_environment() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp("wrapper");
        let script = dir.join("godot-wrapper");
        std::fs::write(&script, "#!/bin/sh\nexec godot \"$@\"\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        assert!(is_script(&script));
        assert!(prepare(&script).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn version_is_read_from_the_configured_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp("version");
        let script = dir.join("fake-godot");
        std::fs::write(&script, "#!/bin/sh\nprintf '4.6.3.stable.official\\n'\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        assert_eq!(version(&script), Some((4, 6)));
        assert_eq!(version(&script), Some((4, 6)), "the answer is cached");
        let _ = std::fs::remove_dir_all(dir);
    }
}
