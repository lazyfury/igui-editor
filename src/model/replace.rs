//! Planned, conflict-aware replace-in-files.
//!
//! The search worker already produced hits; this module turns them into file
//! plans by re-matching each file's **current** text (never trusting stale
//! offsets), then applies them atomically only when the file is unchanged since
//! planning. Open tabs are skipped by the caller, so unsaved buffers are never
//! overwritten behind the app's back.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::find::{find_all_capped, Matcher, MAX_MATCHES};
use super::fsutil;
use super::search::MAX_FILE_BYTES;

/// Files replaced in one run.
pub const MAX_REPLACE_FILES: usize = 200;

/// One file ready to be rewritten.
#[derive(Clone, Debug)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub new_text: String,
    pub count: usize,
}

/// Result of applying a plan.
#[derive(Clone, Debug, Default)]
pub struct AppliedReplace {
    pub files: usize,
    pub occurrences: usize,
    pub errors: Vec<String>,
}

/// Groups `paths`, re-matches each file and builds replacement plans.
///
/// `is_open` marks files that are open in a tab; they are reported as skipped.
pub fn plan(
    paths: impl IntoIterator<Item = PathBuf>,
    matcher: &Matcher,
    replacement: &str,
    is_open: impl Fn(&Path) -> bool,
) -> (Vec<PlannedFile>, Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let mut plans = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if plans.len() >= MAX_REPLACE_FILES {
            errors.push(format!("skipped the rest (>{MAX_REPLACE_FILES} files)"));
            break;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        if is_open(&path) {
            errors.push(format!("{}: open in a tab (skipped)", path.display()));
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            errors.push(format!("{}: unreadable", path.display()));
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            errors.push(format!("{}: larger than 1 MB (skipped)", path.display()));
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            errors.push(format!("{}: not UTF-8 text", path.display()));
            continue;
        };
        let (ranges, truncated) = find_all_capped(&text, matcher);
        if truncated {
            errors.push(format!(
                "{}: more than {MAX_MATCHES} matches (skipped)",
                path.display()
            ));
            continue;
        }
        if ranges.is_empty() {
            continue;
        }
        // Keep the first non-overlapping set (same rule as in-editor replace).
        let mut chosen = Vec::new();
        let mut last_end = 0;
        for (start, end) in ranges {
            if start >= last_end {
                chosen.push((start, end));
                last_end = end;
            }
        }
        let mut new_text = text.clone();
        for (start, end) in chosen.iter().rev() {
            new_text.replace_range(*start..*end, replacement);
        }
        plans.push(PlannedFile {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            new_text,
            count: chosen.len(),
        });
    }
    (plans, errors)
}

/// Writes the plans atomically (temp + rename), skipping files changed since
/// planning.
pub fn apply(plans: Vec<PlannedFile>) -> AppliedReplace {
    let mut applied = AppliedReplace::default();
    for plan in plans {
        let Ok(metadata) = fs::metadata(&plan.path) else {
            applied
                .errors
                .push(format!("{}: disappeared", plan.path.display()));
            continue;
        };
        if metadata.len() != plan.len || metadata.modified().ok() != plan.modified {
            applied.errors.push(format!(
                "{}: changed on disk (skipped)",
                plan.path.display()
            ));
            continue;
        }
        let permissions = fs::metadata(&plan.path).ok().map(|meta| meta.permissions());
        match fsutil::atomic_write(&plan.path, plan.new_text.as_bytes(), permissions) {
            Ok(()) => {
                applied.files += 1;
                applied.occurrences += plan.count;
            }
            Err(error) => applied
                .errors
                .push(format!("{}: {error}", plan.path.display())),
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-replace-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn matcher(query: &str) -> Matcher {
        Matcher {
            query: query.into(),
            case_sensitive: false,
            whole_word: false,
        }
    }

    #[test]
    fn plans_and_applies_replacements() {
        let dir = temp("apply");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "hello world\nhello again\n").unwrap();
        fs::write(&b, "nothing\n").unwrap();
        let (plans, errors) = plan(
            vec![a.clone(), b.clone(), a.clone()],
            &matcher("hello"),
            "bye",
            |_| false,
        );
        assert!(errors.is_empty());
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].count, 2);
        assert_eq!(plans[0].new_text, "bye world\nbye again\n");
        let applied = apply(plans);
        assert_eq!(applied.files, 1);
        assert_eq!(applied.occurrences, 2);
        assert_eq!(fs::read_to_string(&a).unwrap(), "bye world\nbye again\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "nothing\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn skips_open_files_and_conflicts() {
        let dir = temp("conflict");
        let a = dir.join("a.txt");
        fs::write(&a, "hello\n").unwrap();
        let (plans, errors) = plan(vec![a.clone()], &matcher("hello"), "bye", |_| true);
        assert!(plans.is_empty());
        assert_eq!(errors.len(), 1);

        let (plans, _) = plan(vec![a.clone()], &matcher("hello"), "bye", |_| false);
        assert_eq!(plans.len(), 1);
        fs::write(&a, "hello changed\n").unwrap();
        let applied = apply(plans);
        assert_eq!(applied.files, 0);
        assert!(!applied.errors.is_empty());
        assert_eq!(fs::read_to_string(&a).unwrap(), "hello changed\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn skips_files_over_the_match_cap_instead_of_partially_replacing() {
        let dir = temp("cap");
        let a = dir.join("a.txt");
        fs::write(&a, "a".repeat(MAX_MATCHES + 1)).unwrap();
        let (plans, errors) = plan(vec![a.clone()], &matcher("a"), "b", |_| false);
        assert!(plans.is_empty());
        assert!(errors.iter().any(|error| error.contains("matches")));
        assert_eq!(fs::read_to_string(&a).unwrap(), "a".repeat(MAX_MATCHES + 1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_only_non_overlapping_matches() {
        let dir = temp("overlap");
        let a = dir.join("a.txt");
        fs::write(&a, "aaaa").unwrap();
        let (plans, _) = plan(vec![a.clone()], &matcher("aa"), "b", |_| false);
        assert_eq!(plans[0].count, 2);
        assert_eq!(plans[0].new_text, "bb");
        let _ = fs::remove_dir_all(dir);
    }
}
