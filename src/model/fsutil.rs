//! Small file-system helpers shared by the model's writers.
//!
//! Saves, recovery snapshots, replace-in-files and the session file all use
//! `temp + fsync + rename`, so a crash cannot leave a partially written file
//! behind. The helper deliberately lives in the model: callers keep their own
//! policy (permissions, line endings, conflict checks).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Writes `contents` to `path` atomically.
///
/// The temp file lives next to the destination so the final rename stays on
/// one filesystem. Existing permissions are copied when `permissions` is set.
pub fn atomic_write(
    path: &Path,
    contents: &[u8],
    permissions: Option<fs::Permissions>,
) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let (tmp, mut file) = create_temp(path)?;
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = permissions {
            fs::set_permissions(&tmp, permissions)?;
        }
        // `rename` replaces an existing file on Unix. Windows needs the old
        // destination removed first (this is the only non-atomic branch).
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp, path)?;
        sync_parent(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    let mut last = None;
    for attempt in 0..10u32 {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".igui-{}-{attempt}.tmp", std::process::id()));
        let tmp = PathBuf::from(name);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("could not create a temporary file")))
}

#[cfg(unix)]
fn sync_parent(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-fsutil-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_replaces_and_keeps_permissions() {
        let dir = temp_dir("write");
        let path = dir.join("note.txt");
        fs::write(&path, "old").unwrap();
        let permissions = fs::metadata(&path).unwrap().permissions();
        atomic_write(&path, b"new\n", Some(permissions)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".igui-")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn creates_missing_parents_and_new_files() {
        let dir = temp_dir("parents");
        let path = dir.join("a/b/note.txt");
        atomic_write(&path, b"hello", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        let _ = fs::remove_dir_all(dir);
    }
}
