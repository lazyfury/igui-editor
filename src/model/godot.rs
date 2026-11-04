//! Godot project metadata and GDScript editing rules.
//!
//! The editor owns the language server; this module only parses project-local
//! text (script headers, the global class cache) and answers small structural
//! questions. It is pure Rust so the rules are unit-testable without a window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One `class_name` entry from `.godot/global_script_class_cache.cfg`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptClass {
    pub name: String,
    pub base: String,
    /// `res://` path as written by the engine.
    pub path: String,
}

/// The `class_name` / `extends` header of a GDScript file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptHeader {
    pub class_name: Option<String>,
    /// Base as written (`Node`, `Enemy`, `"res://base.gd"`).
    pub base: Option<String>,
}

/// Whether `line` opens a GDScript block (`func`, `if`, `for`, `match`, …).
///
/// Comments are ignored and a `:` inside a quoted string does not count.
pub fn opens_block(line: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut end = line.len();
    for (index, ch) in line.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '#' => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    line[..end].trim_end().ends_with(':')
}

/// Parses the first declaration lines of a GDScript file.
///
/// Handles `class_name Foo extends Bar`, `extends Bar` and
/// `extends "res://bar.gd"` (also when `class_name` and `extends` share a line).
pub fn parse_script_header(text: &str) -> ScriptHeader {
    let mut header = ScriptHeader::default();
    for line in text.lines().take(40) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
            continue;
        }
        let Some(rest) = line.strip_prefix("class_name ") else {
            if let Some(rest) = line.strip_prefix("extends ") {
                header.base = parse_base(rest);
                if header.base.is_some() {
                    break;
                }
            }
            continue;
        };
        let rest = rest.trim();
        let mut parts = rest.splitn(2, char::is_whitespace);
        header.class_name = parts
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        if let Some(base) = parts
            .next()
            .and_then(|rest| rest.trim().strip_prefix("extends "))
        {
            header.base = parse_base(base);
        }
    }
    header
}

fn parse_base(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let base = if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find('"')?;
        &rest[..end + 2]
    } else {
        rest.split(|ch: char| ch.is_whitespace() || ch == '#')
            .next()
            .unwrap_or("")
    };
    (!base.is_empty()).then(|| base.to_string())
}

/// Parses `.godot/global_script_class_cache.cfg`.
///
/// The file is a Godot `Variant` array (`&"Name"`), not JSON; the scanner only
/// looks for the `class` / `base` / `path` fields of each entry and tolerates a
/// partially written file.
pub fn parse_class_cache(text: &str) -> Vec<ScriptClass> {
    let mut out = Vec::new();
    let mut entry = ScriptClass {
        name: String::new(),
        base: String::new(),
        path: String::new(),
    };
    for line in text.lines() {
        if line.trim_start().starts_with("list=[") {
            continue;
        }
        if let Some(value) = field_value(line, "class") {
            entry.name = value;
        } else if let Some(value) = field_value(line, "base") {
            entry.base = value;
        } else if let Some(value) = field_value(line, "path") {
            entry.path = value;
        }
        if line.contains('}') && !entry.name.is_empty() && !entry.path.is_empty() {
            out.push(std::mem::replace(
                &mut entry,
                ScriptClass {
                    name: String::new(),
                    base: String::new(),
                    path: String::new(),
                },
            ));
        }
    }
    out
}

fn field_value(line: &str, field: &str) -> Option<String> {
    let rest = line.trim().strip_prefix('"')?;
    let rest = rest.strip_prefix(field)?.strip_prefix("\":")?.trim();
    let rest = rest.strip_prefix('&').unwrap_or(rest).trim();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Resolves a `res://` path against the project root.
pub fn res_path(root: &Path, res: &str) -> Option<PathBuf> {
    let relative = res.strip_prefix("res://")?;
    Some(root.join(relative))
}

/// `class_name → entry` and resolved project path → entry.
#[derive(Clone, Debug, Default)]
pub struct ClassCache {
    pub by_name: HashMap<String, ScriptClass>,
    pub by_path: HashMap<PathBuf, ScriptClass>,
    name_paths: HashMap<String, PathBuf>,
}

impl ClassCache {
    pub fn new(root: &Path, entries: Vec<ScriptClass>) -> Self {
        let mut cache = Self::default();
        for entry in entries {
            if let Some(path) = res_path(root, &entry.path) {
                cache.name_paths.insert(entry.name.clone(), path.clone());
                cache.by_path.insert(path, entry.clone());
            }
            cache.by_name.insert(entry.name.clone(), entry);
        }
        cache
    }

    pub fn path_of(&self, class: &str) -> Option<PathBuf> {
        self.name_paths.get(class).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_openers_ignore_comments_and_strings() {
        assert!(opens_block("func _ready():"));
        assert!(opens_block("    if x > 0:  "));
        assert!(opens_block("match value:"));
        assert!(!opens_block("	pass"));
        assert!(!opens_block("var x: int"));
        assert!(!opens_block("print('a:')"));
        assert!(!opens_block("# note:"));
        assert!(!opens_block("x := {"));
    }

    #[test]
    fn headers_cover_class_name_and_path_bases() {
        let header = parse_script_header(
            "@tool\nclass_name Player extends CharacterBody2D\n\nfunc _ready():\n\tpass\n",
        );
        assert_eq!(header.class_name.as_deref(), Some("Player"));
        assert_eq!(header.base.as_deref(), Some("CharacterBody2D"));

        let header = parse_script_header("extends \"res://enemy.gd\"\n\nsignal hit\n");
        assert_eq!(header.class_name, None);
        assert_eq!(header.base.as_deref(), Some("\"res://enemy.gd\""));

        let header = parse_script_header("class_name Enemy\nextends Node2D\n");
        assert_eq!(header.class_name.as_deref(), Some("Enemy"));
        assert_eq!(header.base.as_deref(), Some("Node2D"));
    }

    #[test]
    fn class_cache_parser_reads_each_entry() {
        let text = r#"list=[{
"base": &"EnemyBase",
"class": &"Enemy",
"icon": "",
"is_abstract": false,
"is_tool": false,
"language": &"GDScript",
"path": "res://mid.gd"
}, {
"base": &"Node2D",
"class": &"EnemyBase",
"icon": "",
"path": "res://base.gd"
}]
"#;
        let entries = parse_class_cache(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Enemy");
        assert_eq!(entries[0].base, "EnemyBase");
        assert_eq!(entries[0].path, "res://mid.gd");
        assert_eq!(entries[1].name, "EnemyBase");
        assert_eq!(entries[1].base, "Node2D");

        let cache = ClassCache::new(Path::new("/project"), entries);
        assert!(cache.by_name.contains_key("Enemy"));
        assert!(cache.by_path.contains_key(Path::new("/project/mid.gd")));
    }

    #[test]
    fn res_paths_stay_inside_the_project_token() {
        assert_eq!(
            res_path(Path::new("/project"), "res://a/b.gd"),
            Some(PathBuf::from("/project/a/b.gd"))
        );
        assert_eq!(res_path(Path::new("/project"), "user://a.gd"), None);
    }
}
