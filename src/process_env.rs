//! Environment setup for child processes launched by the packaged app.
//!
//! macOS apps opened from Finder inherit a minimal environment: no shell
//! profile has run, so `PATH` often lacks Homebrew, rustup/cargo and
//! `~/.local/bin`. The built-in terminal starts a login shell, but LSP, Git and
//! other native services are spawned directly by the app. This module adds
//! common tool locations once at startup so those children resolve commands
//! like `cargo`, `rust-analyzer` and Homebrew tools.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Standard locations searched after the inherited `PATH`.
const FALLBACK_DIRS: &[&str] = &[
    // Homebrew's rustup formula (the common macOS cargo/rust-analyzer install).
    "/opt/homebrew/opt/rustup/bin",
    "/usr/local/opt/rustup/bin",
    "/opt/homebrew/opt/rust/bin",
    "/usr/local/opt/rust/bin",
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/opt/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Home-relative tool directories (`$HOME/<entry>`).
const HOME_DIRS: &[&str] = &[".cargo/bin", ".local/bin", "bin"];

/// Builds a `PATH` value with `existing` first and existing fallback
/// directories appended once. `exists` is injected for tests.
pub fn path_with_fallbacks(
    existing: Option<&OsStr>,
    home: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> OsString {
    let mut paths: Vec<PathBuf> = existing
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    let mut add = |path: PathBuf| {
        if exists(&path) && !paths.iter().any(|known| known == &path) {
            paths.push(path);
        }
    };
    for dir in FALLBACK_DIRS {
        add(PathBuf::from(dir));
    }
    if let Some(home) = home {
        for dir in HOME_DIRS {
            add(home.join(dir));
        }
    }
    std::env::join_paths(paths).unwrap_or_else(|_| existing.unwrap_or_default().to_os_string())
}

/// Installs the augmented `PATH` for this process (and therefore all children).
pub fn install() {
    let existing = std::env::var_os("PATH");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = path_with_fallbacks(existing.as_deref(), home.as_deref(), |path| path.is_dir());
    if path != existing.unwrap_or_default() {
        std::env::set_var("PATH", path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_entries_stay_first_and_fallbacks_are_deduped() {
        let home = Path::new("/Users/test");
        let existing =
            std::env::join_paths([Path::new("/usr/bin"), Path::new("/opt/homebrew/bin")]).unwrap();
        let path = path_with_fallbacks(Some(&existing), Some(home), |_| true);
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(entries[0], PathBuf::from("/usr/bin"));
        assert_eq!(entries[1], PathBuf::from("/opt/homebrew/bin"));
        assert_eq!(
            entries
                .iter()
                .filter(|path| *path == Path::new("/usr/bin"))
                .count(),
            1
        );
        assert!(entries.contains(&home.join(".cargo/bin")));
        assert!(entries.contains(&PathBuf::from("/opt/homebrew/opt/rustup/bin")));
    }

    #[test]
    fn missing_directories_are_not_added() {
        let path = path_with_fallbacks(None, None, |path| path == Path::new("/usr/bin"));
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(entries, vec![PathBuf::from("/usr/bin")]);
    }

    #[test]
    fn empty_inherited_path_still_gets_standard_dirs() {
        let path = path_with_fallbacks(Some(OsStr::new("")), None, |_| true);
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert!(entries.contains(&PathBuf::from("/usr/bin")));
        assert!(entries.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(!entries.iter().any(|path| path.as_os_str().is_empty()));
    }
}
