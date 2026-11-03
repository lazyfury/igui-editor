//! Local Git plumbing: status/diff through the system `git` CLI.
//!
//! All commands run on a worker thread. The UI receives immutable snapshots and
//! never blocks on Git; write operations are added on top of the same request
//! channel. No shell is involved.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};

const OUTPUT_CAP: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GitKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflicted,
    Untracked,
}

impl GitKind {
    pub const fn code(self) -> char {
        match self {
            GitKind::Modified => 'M',
            GitKind::Added => 'A',
            GitKind::Deleted => 'D',
            GitKind::Renamed => 'R',
            GitKind::Conflicted => 'U',
            GitKind::Untracked => '?',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitEntry {
    /// Path relative to the repository root.
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub kind: GitKind,
    pub staged: bool,
    pub conflict: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub root: Option<PathBuf>,
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub staged: Vec<GitEntry>,
    pub changes: Vec<GitEntry>,
    pub conflicts: Vec<GitEntry>,
    pub error: Option<String>,
}

impl RepoStatus {
    pub fn is_repo(&self) -> bool {
        self.root.is_some() && self.error.is_none()
    }
}

#[derive(Clone, Debug, Default)]
pub struct GitDiff {
    pub path: PathBuf,
    pub staged: bool,
    pub text: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum GitRequest {
    Status {
        generation: u64,
        cwd: PathBuf,
    },
    Diff {
        generation: u64,
        cwd: PathBuf,
        path: PathBuf,
        staged: bool,
    },
    Stage {
        generation: u64,
        cwd: PathBuf,
        paths: Vec<PathBuf>,
    },
    Unstage {
        generation: u64,
        cwd: PathBuf,
        paths: Vec<PathBuf>,
    },
    Commit {
        generation: u64,
        cwd: PathBuf,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub enum GitEvent {
    Status {
        generation: u64,
        status: RepoStatus,
    },
    Diff {
        generation: u64,
        diff: GitDiff,
    },
    Write {
        generation: u64,
        result: Result<String, String>,
    },
}

/// Worker handle. Dropping the sender stops the thread after its current job.
pub struct GitWorker {
    tx: Sender<GitRequest>,
    rx: Receiver<GitEvent>,
}

impl GitWorker {
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = mpsc::channel::<GitRequest>();
        let (event_tx, event_rx) = mpsc::channel::<GitEvent>();
        std::thread::Builder::new()
            .name("igui-git".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    match request {
                        GitRequest::Status { generation, cwd } => {
                            let status = run_status(&cwd);
                            if event_tx
                                .send(GitEvent::Status { generation, status })
                                .is_err()
                            {
                                break;
                            }
                        }
                        GitRequest::Diff {
                            generation,
                            cwd,
                            path,
                            staged,
                        } => {
                            let diff = run_diff(&cwd, &path, staged);
                            if event_tx.send(GitEvent::Diff { generation, diff }).is_err() {
                                break;
                            }
                        }
                        GitRequest::Stage {
                            generation,
                            cwd,
                            paths,
                        } => {
                            let result = run_stage(&cwd, &paths);
                            if event_tx
                                .send(GitEvent::Write { generation, result })
                                .is_err()
                            {
                                break;
                            }
                        }
                        GitRequest::Unstage {
                            generation,
                            cwd,
                            paths,
                        } => {
                            let result = run_unstage(&cwd, &paths);
                            if event_tx
                                .send(GitEvent::Write { generation, result })
                                .is_err()
                            {
                                break;
                            }
                        }
                        GitRequest::Commit {
                            generation,
                            cwd,
                            message,
                        } => {
                            let result = run_commit(&cwd, &message);
                            if event_tx
                                .send(GitEvent::Write { generation, result })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
            .ok();
        Self {
            tx: request_tx,
            rx: event_rx,
        }
    }

    pub fn send(&self, request: GitRequest) -> bool {
        self.tx.send(request).is_ok()
    }

    pub fn try_recv(&self) -> Option<GitEvent> {
        self.rx.try_recv().ok()
    }
}

fn command(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .map_err(|error| format!("git: {error}"))
}

fn stderr(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines().next().unwrap_or("git failed").to_string()
}

fn repo_root(cwd: &Path) -> Result<PathBuf, String> {
    let output = command(cwd, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Err(stderr(&output));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err("not a git repository".into());
    }
    Ok(std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(path)))
}

/// Reads repository status synchronously (worker entry point / tests).
pub fn run_status(cwd: &Path) -> RepoStatus {
    let root = match repo_root(cwd) {
        Ok(root) => root,
        Err(error) => {
            return RepoStatus {
                error: Some(error),
                ..RepoStatus::default()
            };
        }
    };
    let output = match command(&root, &["status", "--porcelain=v2", "-z", "--branch"]) {
        Ok(output) => output,
        Err(error) => {
            return RepoStatus {
                root: Some(root),
                error: Some(error),
                ..RepoStatus::default()
            };
        }
    };
    if !output.status.success() {
        return RepoStatus {
            root: Some(root),
            error: Some(stderr(&output)),
            ..RepoStatus::default()
        };
    }
    let mut status = parse_status(&root, &output.stdout);
    status.root = Some(root);
    status
}

pub fn parse_status(root: &Path, bytes: &[u8]) -> RepoStatus {
    let mut status = RepoStatus {
        root: Some(root.to_path_buf()),
        branch: "(detached)".into(),
        ..RepoStatus::default()
    };
    let mut chunks = bytes.split(|byte| *byte == 0);
    while let Some(chunk) = chunks.next() {
        if chunk.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(chunk);
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            status.branch = rest.to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut parts = rest.split_whitespace();
            status.ahead = parts
                .next()
                .and_then(|value| value.strip_prefix('+'))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            status.behind = parts
                .next()
                .and_then(|value| value.strip_prefix('-'))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            continue;
        }
        let first = chunk.first().copied().unwrap_or(b' ');
        if first == b'1' {
            add_xy(&mut status, &line, None, false);
        } else if first == b'2' {
            let old = chunks
                .next()
                .map(|chunk| PathBuf::from(String::from_utf8_lossy(chunk).into_owned()));
            add_xy(&mut status, &line, old, false);
        } else if first == b'u' {
            add_xy(&mut status, &line, None, true);
        } else if first == b'?' {
            let path = line
                .strip_prefix("? ")
                .map(PathBuf::from)
                .unwrap_or_default();
            if !path.as_os_str().is_empty() {
                status.changes.push(GitEntry {
                    path,
                    old_path: None,
                    kind: GitKind::Untracked,
                    staged: false,
                    conflict: false,
                });
            }
        }
    }
    status.staged.sort_by(|a, b| a.path.cmp(&b.path));
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status.conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn add_xy(status: &mut RepoStatus, line: &str, old_path: Option<PathBuf>, conflict: bool) {
    let Some(xy) = line.split(' ').nth(1) else {
        return;
    };
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    let path = match line.splitn(11, ' ').last() {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => return,
    };
    if conflict {
        status.conflicts.push(GitEntry {
            path,
            old_path,
            kind: GitKind::Conflicted,
            staged: false,
            conflict: true,
        });
        return;
    }
    if let Some(kind) = kind_from(x) {
        status.staged.push(GitEntry {
            path: path.clone(),
            old_path: old_path.clone(),
            kind,
            staged: true,
            conflict: false,
        });
    }
    if let Some(kind) = kind_from(y) {
        status.changes.push(GitEntry {
            path,
            old_path,
            kind,
            staged: false,
            conflict: false,
        });
    }
}

fn kind_from(code: char) -> Option<GitKind> {
    match code {
        '.' | ' ' => None,
        'A' => Some(GitKind::Added),
        'D' => Some(GitKind::Deleted),
        'R' | 'C' => Some(GitKind::Renamed),
        'U' => Some(GitKind::Conflicted),
        _ => Some(GitKind::Modified),
    }
}

/// Reads a unified diff synchronously (worker entry point / tests).
pub fn run_diff(cwd: &Path, path: &Path, staged: bool) -> GitDiff {
    let root = match repo_root(cwd) {
        Ok(root) => root,
        Err(error) => {
            return GitDiff {
                path: path.to_path_buf(),
                staged,
                text: error,
                binary: false,
                truncated: false,
            };
        }
    };
    let relative = path.strip_prefix(&root).unwrap_or(path);
    let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--unified=3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    let relative_text = relative.to_string_lossy().into_owned();
    args.push(relative_text.as_str());
    let output = match command(&root, &args) {
        Ok(output) => output,
        Err(error) => {
            return GitDiff {
                path: relative.to_path_buf(),
                staged,
                text: error,
                binary: false,
                truncated: false,
            };
        }
    };
    let raw = output.stdout;
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    let mut truncated = text.len() > OUTPUT_CAP;
    if truncated {
        text.truncate(OUTPUT_CAP);
        text.push_str("\n… diff truncated …\n");
    }
    let binary = text.contains('\0') || text.contains("Binary files");
    if text.is_empty() && !staged {
        // Untracked files have no diff; show their content as additions.
        let full = root.join(relative);
        if let Ok(bytes) = std::fs::read(&full) {
            let binary = bytes.contains(&0);
            if binary {
                text = format!("Binary file: {}\n", relative.display());
            } else {
                let content = String::from_utf8_lossy(&bytes);
                let lines = content.lines().count();
                text = format!(
                    "--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n",
                    relative.display(),
                    lines
                );
                for line in content.lines() {
                    text.push('+');
                    text.push_str(line);
                    text.push('\n');
                }
                truncated = text.len() > OUTPUT_CAP;
                if truncated {
                    text.truncate(OUTPUT_CAP);
                    text.push_str("\n… diff truncated …\n");
                }
            }
            return GitDiff {
                path: relative.to_path_buf(),
                staged,
                text,
                binary,
                truncated,
            };
        }
    }
    GitDiff {
        path: relative.to_path_buf(),
        staged,
        text,
        binary,
        truncated,
    }
}

fn relative_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

pub fn run_stage(cwd: &Path, paths: &[PathBuf]) -> Result<String, String> {
    let root = repo_root(cwd)?;
    let paths = relative_paths(&root, paths);
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    let output = command(&root, &args)?;
    if output.status.success() {
        Ok(format!("staged {} path(s)", paths.len()))
    } else {
        Err(stderr(&output))
    }
}

pub fn run_unstage(cwd: &Path, paths: &[PathBuf]) -> Result<String, String> {
    let root = repo_root(cwd)?;
    let paths = relative_paths(&root, paths);
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    let output = command(&root, &args)?;
    if output.status.success() {
        Ok(format!("unstaged {} path(s)", paths.len()))
    } else {
        Err(stderr(&output))
    }
}

pub fn run_commit(cwd: &Path, message: &str) -> Result<String, String> {
    let root = repo_root(cwd)?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["commit", "-F", "-"])
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("git: {error}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(message.as_bytes())
            .map_err(|error| format!("git commit stdin: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("git commit: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(stderr(&output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2_branch_and_entries() {
        let input = b"# branch.oid abc\0# branch.head main\0# branch.ab +2 -1\0\
1 M. N... 100644 100644 100644 a b src/app.rs\0\
1 .M N... 100644 100644 100644 a b src/state.rs\0\
2 R. N... 100644 100644 100644 a b new.rs\0old.rs\0\
? notes.md\0\
u UU N... 100644 100644 100644 a b conflict.rs\0";
        let status = parse_status(Path::new("/repo"), input);
        assert_eq!(status.branch, "main");
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.staged.len(), 2);
        assert_eq!(status.changes.len(), 2);
        assert_eq!(status.conflicts.len(), 1);
        let renamed = status
            .staged
            .iter()
            .find(|entry| entry.path == Path::new("new.rs"))
            .expect("rename entry");
        assert_eq!(renamed.kind, GitKind::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some(Path::new("old.rs")));
        assert!(status
            .changes
            .iter()
            .any(|entry| entry.kind == GitKind::Untracked));
    }

    #[test]
    fn status_and_diff_round_trip_in_a_temp_repo() {
        let dir = std::env::temp_dir().join(format!("igui-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "new\n").unwrap();

        let status = run_status(&dir);
        assert!(status.is_repo());
        assert_eq!(status.changes.len(), 2);
        let diff = run_diff(&dir, &dir.join("a.txt"), false);
        assert!(diff.text.contains("-one") && diff.text.contains("+two"));
        let untracked = run_diff(&dir, &dir.join("new.txt"), false);
        assert!(untracked.text.contains("+new"));
        run_stage(&dir, &[dir.join("a.txt"), dir.join("new.txt")]).unwrap();
        let staged = run_status(&dir);
        assert_eq!(staged.staged.len(), 2);
        run_unstage(&dir, &[dir.join("new.txt")]).unwrap();
        let partial = run_status(&dir);
        assert_eq!(partial.staged.len(), 1);
        run_stage(&dir, &[dir.join("new.txt")]).unwrap();
        run_commit(&dir, "update files\n").unwrap();
        let clean = run_status(&dir);
        assert!(clean.staged.is_empty() && clean.changes.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
