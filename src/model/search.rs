//! Project-wide literal search on a background thread.
//!
//! The walker reuses the Quick Open index rules, skips binary/huge files, and
//! streams matches over an `mpsc` channel so the UI stays responsive. A new
//! query cancels the previous worker with an `AtomicBool`; results are capped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use super::find::{find_all, Matcher};
use super::workspace_index::FileIndex;

/// Files inspected per search.
pub const MAX_FILES_SCANNED: usize = 5_000;
/// Matches returned overall.
pub const MAX_HITS: usize = 500;
/// Matches returned per file.
pub const MAX_HITS_PER_FILE: usize = 20;
/// Files larger than this are skipped.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One match inside a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    /// Byte offset of the match start in the file.
    pub offset: usize,
    /// 0-based line index and the byte range inside that line.
    pub line: usize,
    pub start_in_line: usize,
    pub end_in_line: usize,
    /// The whole line, for the result preview.
    pub preview: String,
}

/// Messages streamed from a search worker.
#[derive(Clone, Debug)]
pub enum SearchMessage {
    Progress { files: usize },
    Hits(Vec<SearchHit>),
    Done { files: usize },
}

/// A running search: cancellation flag plus the receiving side.
pub struct SearchWorker {
    cancel: Arc<AtomicBool>,
    rx: Receiver<SearchMessage>,
}

impl SearchWorker {
    /// Starts scanning `root` with `matcher`.
    pub fn spawn(root: PathBuf, matcher: Matcher) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        thread::spawn(move || run(root, matcher, flag, tx));
        Self { cancel, rx }
    }

    /// Non-blocking receive of the next message.
    pub fn try_recv(&self) -> Option<SearchMessage> {
        self.rx.try_recv().ok()
    }

    /// Asks the worker to stop (it checks between files).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(root: PathBuf, matcher: Matcher, cancel: Arc<AtomicBool>, tx: Sender<SearchMessage>) {
    let index = FileIndex::build(&root);
    let mut files = 0usize;
    let mut hits = 0usize;
    let mut batch: Vec<SearchHit> = Vec::new();

    for path in index.files {
        if cancel.load(Ordering::Relaxed) || hits >= MAX_HITS || files >= MAX_FILES_SCANNED {
            break;
        }
        files += 1;
        if std::fs::metadata(&path).map_or(true, |meta| meta.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // A NUL in the first block marks a binary file.
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let mut line_starts = vec![0usize];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        for (per_file, (start, end)) in find_all(&text, &matcher).into_iter().enumerate() {
            if hits >= MAX_HITS || per_file >= MAX_HITS_PER_FILE {
                break;
            }
            let line = line_starts.partition_point(|offset| *offset <= start) - 1;
            let line_start = line_starts[line];
            let line_end = text[line_start..]
                .find('\n')
                .map_or(text.len(), |offset| line_start + offset);
            batch.push(SearchHit {
                path: path.clone(),
                offset: start,
                line,
                start_in_line: start - line_start,
                end_in_line: end - line_start,
                preview: text[line_start..line_end].to_string(),
            });
            hits += 1;
            if batch.len() >= 20 {
                let _ = tx.send(SearchMessage::Hits(std::mem::take(&mut batch)));
            }
        }
        if batch.len() >= 20 {
            let _ = tx.send(SearchMessage::Hits(std::mem::take(&mut batch)));
        }
        if files % 100 == 0 {
            let _ = tx.send(SearchMessage::Progress { files });
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    if !batch.is_empty() {
        let _ = tx.send(SearchMessage::Hits(batch));
    }
    let _ = tx.send(SearchMessage::Progress { files });
    let _ = tx.send(SearchMessage::Done { files });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-search-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("a.txt"), "hello world\nHELLO again\n").unwrap();
        fs::write(dir.join("src/b.txt"), "nothing here\nsay hello\n").unwrap();
        fs::write(dir.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        fs::write(dir.join("big.txt"), "hello".repeat(MAX_FILE_BYTES as usize)).unwrap();
        dir
    }

    fn collect(root: PathBuf, matcher: Matcher) -> (Vec<SearchHit>, usize) {
        let worker = SearchWorker::spawn(root, matcher);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut hits = Vec::new();
        let mut files = 0;
        while Instant::now() < deadline {
            match worker.try_recv() {
                Some(SearchMessage::Hits(batch)) => hits.extend(batch),
                Some(SearchMessage::Progress { files: scanned }) => files = scanned,
                Some(SearchMessage::Done { files: scanned }) => {
                    files = scanned;
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        (hits, files)
    }

    #[test]
    fn finds_matches_and_skips_binary_and_huge_files() {
        let dir = fixture("walk");
        let (hits, files) = collect(
            dir.clone(),
            Matcher {
                query: "hello".into(),
                case_sensitive: false,
                whole_word: false,
            },
        );
        assert_eq!(
            hits.len(),
            3,
            "case-insensitive: two in a.txt, one in b.txt"
        );
        assert!(files >= 2);
        assert!(hits.iter().all(|hit| !hit.path.ends_with("bin.dat")));
        assert!(hits.iter().all(|hit| !hit.path.ends_with("big.txt")));
        let line = hits
            .iter()
            .find(|hit| hit.path.ends_with("src/b.txt"))
            .expect("hit in b.txt");
        assert_eq!(line.line, 1);
        assert_eq!(line.preview, "say hello");
        assert_eq!(&line.preview[line.start_in_line..line.end_in_line], "hello");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancellation_stops_the_worker() {
        let dir = fixture("cancel");
        let worker = SearchWorker::spawn(
            dir.clone(),
            Matcher {
                query: "e".into(),
                case_sensitive: false,
                whole_word: false,
            },
        );
        worker.cancel();
        // The worker must still terminate (Done) rather than hang.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut done = false;
        while Instant::now() < deadline {
            if let Some(SearchMessage::Done { .. }) = worker.try_recv() {
                done = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(done);
        let _ = fs::remove_dir_all(dir);
    }
}
