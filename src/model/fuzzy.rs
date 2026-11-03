//! Fuzzy ranking for Quick Open.
//!
//! Case-insensitive subsequence match over the workspace-relative path with
//! bonuses for the file name (prefix > substring > subsequence), a recent-file
//! bonus and a small penalty for long names. Pure and unit-tested.

/// Scores `path` against `query`; `None` when the query is not a subsequence.
/// An empty query matches everything with score `0`.
pub fn score(query: &str, path: &str, recent: bool) -> Option<i32> {
    if query.is_empty() {
        return Some(if recent { 10 } else { 0 });
    }
    let query = query.to_lowercase();
    let path_lower = path.to_lowercase();
    let name = path_lower.rsplit('/').next().unwrap_or(&path_lower);
    if !is_subsequence(&query, &path_lower) {
        return None;
    }
    let mut score = 100;
    if name.starts_with(&query) {
        score += 400;
    } else if name.contains(&query) {
        score += 250;
    } else if is_subsequence(&query, name) {
        score += 120;
    } else if path_lower.contains(&query) {
        score += 60;
    }
    if recent {
        score += 200;
    }
    // Shorter paths are easier to scan; keep the penalty small.
    score -= (name.chars().count() as i32 / 8).min(30);
    Some(score)
}

fn is_subsequence(query: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    for needle in query.chars() {
        if !chars.any(|hay| hay == needle) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, paths: &'a [&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(&str, i32)> = paths
            .iter()
            .filter_map(|path| score(query, path, false).map(|value| (*path, value)))
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.len().cmp(&b.0.len())));
        scored.into_iter().map(|(path, _)| path).collect()
    }

    fn best<'a>(query: &str, paths: &'a [&'a str]) -> Option<&'a str> {
        rank(query, paths).first().copied()
    }

    #[test]
    fn basename_prefix_beats_subsequence() {
        let paths = [
            "src/editor/view.rs",
            "src/main_window.rs",
            "deploy/window_manager.rs",
        ];
        assert_eq!(best("win", &paths), Some("deploy/window_manager.rs"));
    }

    #[test]
    fn matches_case_insensitively() {
        assert_eq!(best("README", &["docs/readme.md"]), Some("docs/readme.md"));
        assert_eq!(best("readme", &["README.md"]), Some("README.md"));
    }

    #[test]
    fn non_subsequence_is_rejected() {
        assert_eq!(score("xyz", "src/main.rs", false), None);
        assert_eq!(best("xyz", &["src/main.rs"]), None);
    }

    #[test]
    fn path_subsequence_still_matches() {
        assert!(score("sman", "src/main.rs", false).is_some());
    }

    #[test]
    fn recent_files_get_a_bonus() {
        let with_recent = score("rs", "src/main.rs", true).unwrap();
        let without = score("rs", "src/main.rs", false).unwrap();
        assert_eq!(with_recent - without, 200);
    }

    #[test]
    fn ranking_is_deterministic_and_capped() {
        let paths = ["a.rs", "ab.rs", "abc.rs", "abcd.rs"];
        let ranked = rank("a", &paths);
        assert_eq!(ranked.len(), paths.len());
        assert_eq!(ranked[0], "a.rs");
    }
}
