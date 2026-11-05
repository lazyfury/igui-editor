//! Crash-recovery snapshots for dirty buffers.
//!
//! Each snapshot is a raw `<id>.txt` (the buffer text, no escaping) plus a
//! `<id>.meta` key=value file (name/path/caret). Raw text keeps multi-line and
//! non-UTF8-free content simple; only the small metadata values are escaped.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::doc::LineEnding;
use super::fsutil;

/// Borrowed values written into one recovery snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'a> {
    pub name: &'a str,
    pub path: Option<&'a Path>,
    pub text: &'a str,
    pub caret: usize,
    pub anchor: usize,
    pub eol: LineEnding,
}

/// One recoverable buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryEntry {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
    pub eol: LineEnding,
}

/// Snapshot directory under the app config.
#[derive(Clone, Debug)]
pub struct RecoveryStore {
    dir: PathBuf,
}

impl RecoveryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Writes (or replaces) one snapshot.
    pub fn save(&self, id: u64, snapshot: Snapshot<'_>) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let text_path = self.dir.join(format!("{id}.txt"));
        let meta_path = self.dir.join(format!("{id}.meta"));
        let meta = format!(
            "name={}\npath={}\ncaret={}\nanchor={}\neol={}\n",
            escape(snapshot.name),
            snapshot
                .path
                .map(|path| escape(&path.to_string_lossy()))
                .unwrap_or_default(),
            snapshot.caret,
            snapshot.anchor,
            snapshot.eol.as_str()
        );
        // Write-then-rename so a crash mid-snapshot cannot truncate the text.
        fsutil::atomic_write(&text_path, snapshot.text.as_bytes(), None)?;
        fsutil::atomic_write(&meta_path, meta.as_bytes(), None)
    }

    /// Removes one snapshot (both files).
    pub fn remove(&self, id: u64) {
        let _ = fs::remove_file(self.dir.join(format!("{id}.txt")));
        let _ = fs::remove_file(self.dir.join(format!("{id}.meta")));
    }

    /// Removes every snapshot.
    pub fn clear(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("txt" | "meta")
            ) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// All readable snapshots, in id order.
    pub fn list(&self) -> Vec<RecoveryEntry> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut found: Vec<RecoveryEntry> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("meta") {
                    return None;
                }
                let id = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
                let meta = fs::read_to_string(&path).ok()?;
                let text = fs::read_to_string(self.dir.join(format!("{id}.txt"))).ok()?;
                let mut name = String::new();
                let mut path_buf = None;
                let mut caret = 0usize;
                let mut anchor = 0usize;
                let mut eol = LineEnding::Lf;
                for line in meta.lines() {
                    let Some((key, value)) = line.split_once('=') else {
                        continue;
                    };
                    let value = unescape(value);
                    match key {
                        "name" => name = value,
                        "path" if !value.is_empty() => path_buf = Some(PathBuf::from(value)),
                        "caret" => caret = value.parse().unwrap_or(0),
                        "anchor" => anchor = value.parse().unwrap_or(0),
                        "eol" => eol = LineEnding::from_name(&value),
                        _ => {}
                    }
                }
                if name.is_empty() {
                    name = format!("recovered-{id}");
                }
                Some(RecoveryEntry {
                    id,
                    name,
                    path: path_buf,
                    text,
                    caret,
                    anchor,
                    eol,
                })
            })
            .collect();
        found.sort_by_key(|entry| entry.id);
        found
    }

    /// Whether any snapshot exists.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by tests only"))]
    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }
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
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-recovery-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_text_and_metadata() {
        let dir = temp("roundtrip");
        let store = RecoveryStore::new(&dir);
        store
            .save(
                7,
                Snapshot {
                    name: "note.md",
                    path: Some(Path::new("/tmp/a b/note.md")),
                    text: "line one\n中文\tline three\n",
                    caret: 5,
                    anchor: 2,
                    eol: LineEnding::Crlf,
                },
            )
            .unwrap();
        let entries = store.list();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.id, 7);
        assert_eq!(entry.name, "note.md");
        assert_eq!(entry.path.as_deref(), Some(Path::new("/tmp/a b/note.md")));
        assert_eq!(entry.text, "line one\n中文\tline three\n");
        assert_eq!(entry.caret, 5);
        assert_eq!(entry.anchor, 2);
        assert_eq!(entry.eol, LineEnding::Crlf);

        store.remove(7);
        assert!(store.list().is_empty());
    }

    #[test]
    fn clears_all_snapshots() {
        let dir = temp("clear");
        let store = RecoveryStore::new(&dir);
        store
            .save(
                1,
                Snapshot {
                    name: "a",
                    path: None,
                    text: "a",
                    caret: 0,
                    anchor: 0,
                    eol: LineEnding::Lf,
                },
            )
            .unwrap();
        store
            .save(
                2,
                Snapshot {
                    name: "b",
                    path: None,
                    text: "b",
                    caret: 1,
                    anchor: 1,
                    eol: LineEnding::Lf,
                },
            )
            .unwrap();
        assert_eq!(store.list().len(), 2);
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn missing_directory_is_empty_not_an_error() {
        let store = RecoveryStore::new(temp("missing").join("nested"));
        assert!(store.is_empty());
    }
}
