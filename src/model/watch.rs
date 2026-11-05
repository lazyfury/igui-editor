//! External file-change detection: directory snapshots, change diffs, and the
//! small glob dialect language servers use to register file watchers.
//!
//! The app cannot use an OS filesystem watcher (no dependency, and a watcher is
//! explicitly out of scope), so the visible part of the tree is polled: a
//! snapshot is one directory's entries with a cheap metadata identity. Two
//! snapshots of the same directory diff into [`FileEvent`]s, which the state
//! layer turns into a sidebar refresh and `workspace/didChangeWatchedFiles`
//! notifications.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// How many entries one directory contributes to a snapshot. A huge directory
/// cannot stall the poll or the sidebar refresh.
pub const MAX_DIR_ENTRIES: usize = 2_000;

/// How many alternatives a `{a,b}` watcher pattern may expand into.
const MAX_BRACE_EXPANSIONS: usize = 32;

/// What happened to a path that is not open as a document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileEventKind {
    Created,
    Changed,
    Deleted,
}

impl FileEventKind {
    /// LSP `FileChangeType`: 1 created, 2 changed, 3 deleted.
    pub const fn lsp_type(self) -> u32 {
        match self {
            FileEventKind::Created => 1,
            FileEventKind::Changed => 2,
            FileEventKind::Deleted => 3,
        }
    }

    /// LSP `FileSystemWatcher.kind` bit: 1 create, 2 change, 4 delete.
    pub const fn watch_bit(self) -> u32 {
        match self {
            FileEventKind::Created => 1,
            FileEventKind::Changed => 2,
            FileEventKind::Deleted => 4,
        }
    }
}

/// One path that changed on disk, in the order it must be reported.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: FileEventKind,
}

/// Metadata identity of one directory entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntryStamp {
    pub path: PathBuf,
    pub is_dir: bool,
    pub len: u64,
    pub modified: u128,
}

/// Reads one directory into a snapshot sorted by path. Hidden entries are
/// included: the Explorer's visibility filter is display-only, and a hidden
/// file can still matter to a language server (and `.gd.uid`-style companions
/// are not dot files anyway).
pub fn snapshot(dir: &Path) -> Option<Vec<EntryStamp>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut out = Vec::new();
    for entry in entries.flatten().take(MAX_DIR_ENTRIES) {
        // `metadata` follows symlinks; a dangling link is simply skipped.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        out.push(EntryStamp {
            path: entry.path(),
            is_dir: metadata.is_dir(),
            len: metadata.len(),
            modified: modified_nanos(&metadata),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Some(out)
}

fn modified_nanos(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// Differences between two snapshots of the same directory. Directories only
/// report create/delete: their mtime changes whenever a child does, and the
/// children are snapshotted themselves.
pub fn diff(previous: &[EntryStamp], current: &[EntryStamp]) -> Vec<FileEvent> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0, 0);
    while old < previous.len() || new < current.len() {
        match (previous.get(old), current.get(new)) {
            (Some(a), Some(b)) if a.path == b.path => {
                if !a.is_dir && (a.len != b.len || a.modified != b.modified) {
                    out.push(FileEvent {
                        path: b.path.clone(),
                        kind: FileEventKind::Changed,
                    });
                }
                old += 1;
                new += 1;
            }
            (Some(a), Some(b)) if a.path < b.path => {
                out.push(FileEvent {
                    path: a.path.clone(),
                    kind: FileEventKind::Deleted,
                });
                old += 1;
            }
            (Some(_), Some(b)) => {
                out.push(FileEvent {
                    path: b.path.clone(),
                    kind: FileEventKind::Created,
                });
                new += 1;
            }
            (Some(a), None) => {
                out.push(FileEvent {
                    path: a.path.clone(),
                    kind: FileEventKind::Deleted,
                });
                old += 1;
            }
            (None, Some(b)) => {
                out.push(FileEvent {
                    path: b.path.clone(),
                    kind: FileEventKind::Created,
                });
                new += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// Matches one LSP watcher glob against a `/`-separated path.
///
/// Supported: `**` (any number of segments), `*` and `?` inside one segment,
/// and `{a,b}` alternatives. Absolute patterns compare against absolute paths,
/// relative patterns against relative paths. Anything fancier is treated
/// literally rather than silently matching everything.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let path = normalize(path);
    let mut expanded = Vec::new();
    expand_braces(&normalize(pattern), &mut expanded);
    expanded.iter().any(|pattern| {
        let pattern: Vec<&str> = pattern.split('/').collect();
        let path: Vec<&str> = path.split('/').collect();
        segments_match(&pattern, &path)
    })
}

fn normalize(value: &str) -> String {
    let value = value.replace('\\', "/");
    let value = value.strip_prefix("./").unwrap_or(&value).to_string();
    value
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((segment, rest)) => path.split_first().is_some_and(|(first, tail)| {
            segment_matches(segment, first) && segments_match(rest, tail)
        }),
    }
}

/// Classic greedy `*` / `?` match inside one segment (no `/` spanning).
fn segment_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut mark = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn expand_braces(pattern: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_BRACE_EXPANSIONS {
        return;
    }
    let Some(open) = pattern.find('{') else {
        out.push(pattern.to_string());
        return;
    };
    let Some(close) = pattern[open..].find('}').map(|offset| open + offset) else {
        out.push(pattern.to_string());
        return;
    };
    let body = &pattern[open + 1..close];
    if body.contains('{') {
        // Nested braces are not part of the patterns servers ship.
        out.push(pattern.to_string());
        return;
    }
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    for alternative in body.split(',') {
        expand_braces(&format!("{prefix}{alternative}{suffix}"), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-watch-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snapshot_diff_reports_created_changed_and_deleted() {
        let dir = temp("diff");
        let keep = dir.join("keep.txt");
        let gone = dir.join("gone.txt");
        fs::write(&keep, "one").unwrap();
        fs::write(&gone, "bye").unwrap();
        let first = snapshot(&dir).unwrap();

        fs::write(&keep, "one but longer").unwrap();
        fs::remove_file(&gone).unwrap();
        fs::write(dir.join("new.gd.uid"), "uid://x").unwrap();
        let second = snapshot(&dir).unwrap();

        let events = diff(&first, &second);
        let find = |name: &str| {
            events
                .iter()
                .find(|event| event.path.file_name().is_some_and(|file| file == name))
                .map(|event| event.kind)
        };
        assert_eq!(find("keep.txt"), Some(FileEventKind::Changed));
        assert_eq!(find("gone.txt"), Some(FileEventKind::Deleted));
        assert_eq!(find("new.gd.uid"), Some(FileEventKind::Created));
        assert_eq!(events.len(), 3, "{events:?}");

        // No change, no events.
        let third = snapshot(&dir).unwrap();
        assert!(diff(&second, &third).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn directories_only_report_create_and_delete() {
        let dir = temp("dir");
        let nested = dir.join("nested");
        fs::create_dir(&nested).unwrap();
        let first = snapshot(&dir).unwrap();
        fs::write(nested.join("a.txt"), "a").unwrap();
        let second = snapshot(&dir).unwrap();
        // The directory's own mtime moved, but only children changes count.
        assert!(
            diff(&first, &second).is_empty(),
            "{:?}",
            diff(&first, &second)
        );
        fs::remove_dir_all(&nested).unwrap();
        let third = snapshot(&dir).unwrap();
        assert_eq!(
            diff(&second, &third),
            vec![FileEvent {
                path: nested,
                kind: FileEventKind::Deleted
            }]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn globs_cover_the_shapes_servers_register() {
        assert!(glob_matches("**/*.gd", "player.gd"));
        assert!(glob_matches("**/*.gd", "scripts/player.gd"));
        assert!(glob_matches("**/*", "a/b/c.txt"));
        assert!(glob_matches("scripts/*.gd", "scripts/player.gd"));
        assert!(!glob_matches("scripts/*.gd", "scripts/sub/player.gd"));
        assert!(glob_matches("**/*.{gd,ts}", "src/a.ts"));
        assert!(!glob_matches("**/*.{gd,ts}", "src/a.rs"));
        assert!(glob_matches("?.gd", "a.gd"));
        assert!(!glob_matches("?.gd", "ab.gd"));
        assert!(glob_matches(
            "/root/project/**/*.gd",
            "/root/project/a/b.gd"
        ));
        assert!(!glob_matches("src/*.gd", "other/a.gd"));
        assert!(glob_matches("**/a/**/z.rs", "x/a/y/z.rs"));
    }

    #[test]
    fn unmatched_braces_are_literal() {
        assert!(glob_matches("a{b.gd", "a{b.gd"));
        assert!(!glob_matches("a{b.gd", "ab.gd"));
    }
}
