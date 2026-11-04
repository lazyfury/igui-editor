//! INHERITED / SIGNALS sidebar panels.
//!
//! Both panels share one resolved base-class chain for the active GDScript:
//! `INHERITED` lists its properties/variables, `SIGNALS` its signals. The chain
//! comes from the project's `.godot/global_script_class_cache.cfg` (name →
//! path), each script's `extends` header, and the native class map that Godot
//! sends in `gdscript/capabilities`. Members come from the LSP caches:
//! `documentSymbol` for project scripts, `textDocument/nativeSymbol` for native
//! classes (signals kind 24, properties kind 7, variables kind 13).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use igui_core::Rect;

use super::*;
use crate::model::godot::{self, ClassCache, ScriptHeader};
use crate::model::lsp::{LspStatus, NativeSymbol, OutlineSymbol};

/// One class in the active script's base chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainClass {
    pub name: String,
    pub path: Option<PathBuf>,
    pub native: bool,
    pub unresolved: bool,
}

/// Row under the pointer, recorded during paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelHit {
    Group(usize),
    Member { group: usize, index: usize },
}

/// Per-tab scroll and hit-test state (geometry is recorded by paint).
#[derive(Default)]
pub struct MemberPanelUi {
    pub scroll: f32,
    pub scroll_max: f32,
    pub hover: Option<usize>,
    pub hits: Vec<(Rect, PanelHit)>,
    pub collapsed: HashSet<String>,
}

/// Shared state for both panels. The chain is resolved only when the active
/// document, the class cache or the server state changes; paint reads the LSP
/// member caches directly so it never clones a member list per frame.
#[derive(Default)]
pub struct GodotUi {
    pub title: String,
    pub chain: Vec<ChainClass>,
    pub message: Option<String>,
    pub inherited: MemberPanelUi,
    pub signals: MemberPanelUi,
    active_path: Option<PathBuf>,
    active_rev: u64,
    root: Option<PathBuf>,
    members_requested: bool,
    last_ready: bool,
    stale: bool,
}

impl GodotUi {
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.members_requested = false;
    }
}

impl AppState {
    /// Invalidates the panels from the file-event path.
    pub(crate) fn invalidate_godot_panel(&mut self) {
        self.godot.invalidate();
    }

    /// Rebuilds the chain and keeps member requests in flight. Called once a
    /// frame; file reads happen only when something changed.
    pub fn tick_godot_panel(&mut self) {
        let active = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(Path::to_path_buf);
        let rev = self.ws.active_doc().map(|doc| doc.rev()).unwrap_or(0);
        let gdscript = self.language_name() == Some("gdscript");
        let ready = *self.lsp.status() == LspStatus::Ready;
        if !gdscript || active.is_none() {
            if !self.godot.chain.is_empty() || self.godot.message.is_none() {
                self.godot.chain.clear();
                self.godot.title.clear();
                self.godot.message = Some(if gdscript {
                    "Open a .gd file".into()
                } else {
                    "No GDScript file".into()
                });
                self.godot.invalidate();
            }
            return;
        }
        let path = active.expect("checked above");
        let root = self.godot_project_root(&path);
        // `gdscript/capabilities` can arrive after the first chain resolution;
        // a class that was unresolved then is native now.
        let unresolved_now_native = self
            .godot
            .chain
            .iter()
            .any(|class| class.unresolved && self.lsp.is_native_class(&class.name));
        let stale = self.godot.stale
            || self.godot.active_path.as_deref() != Some(path.as_path())
            || self.godot.active_rev != rev
            || self.godot.last_ready != ready
            || unresolved_now_native;

        self.godot.active_path = Some(path.clone());
        self.godot.active_rev = rev;
        self.godot.last_ready = ready;
        self.godot.root = root.clone();

        if stale {
            let header = self
                .ws
                .active_doc()
                .map(|doc| crate::model::godot::parse_script_header(header_text(doc.text())))
                .unwrap_or_default();
            let cache = root.as_deref().map(read_class_cache).unwrap_or_default();
            let chain = resolve_chain(root.as_deref(), &header, &cache, &self.lsp, |path| {
                self.read_script_header(path)
            });
            let title = header
                .class_name
                .clone()
                .or_else(|| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "script".into());
            self.godot.title = title;
            self.godot.chain = chain;
            self.godot.message = match (root, self.godot.chain.is_empty()) {
                (None, _) => Some("project.godot not found".into()),
                (Some(_), true) => Some("No parent class".into()),
                _ => None,
            };
            self.godot.stale = false;
            self.godot.members_requested = false;
        }

        if !self.godot.members_requested && ready {
            self.request_godot_members();
        }
    }

    fn godot_project_root(&self, path: &Path) -> Option<PathBuf> {
        for dir in path.ancestors().skip(1) {
            if dir.join("project.godot").is_file() {
                return Some(dir.to_path_buf());
            }
        }
        self.ws
            .folder()
            .filter(|root| root.join("project.godot").is_file())
            .map(Path::to_path_buf)
    }

    /// First 4 KB of a script, from an open tab (unsaved edits win) or disk.
    fn read_script_header(&self, path: &Path) -> Option<String> {
        if let Some(doc) = self.ws.tabs.iter().find(|doc| doc.path() == Some(path)) {
            return Some(header_text(doc.text()).to_string());
        }
        read_prefix(path)
    }

    fn request_godot_members(&mut self) {
        self.godot.members_requested = true;
        let requests: Vec<(String, Option<PathBuf>, bool)> = self
            .godot
            .chain
            .iter()
            .map(|class| (class.name.clone(), class.path.clone(), class.native))
            .collect();
        for (name, path, native) in requests {
            if native {
                let _ = self.lsp.request_native_symbol(&name, "");
            } else if let Some(path) = path {
                let _ = self.lsp.request_base_symbols(&path);
            }
        }
    }

    /// Total signals in the resolved base chain (dock badge).
    pub fn godot_signal_count(&self) -> usize {
        self.godot_member_count(true)
    }

    /// Total properties/variables in the resolved base chain (dock badge).
    pub fn godot_property_count(&self) -> usize {
        self.godot_member_count(false)
    }

    fn godot_member_count(&self, signals: bool) -> usize {
        self.godot
            .chain
            .iter()
            .map(|class| {
                if class.native {
                    self.lsp
                        .native_class_symbol(&class.name)
                        .map(|symbol| {
                            if signals {
                                native_signals(symbol).count()
                            } else {
                                native_properties(symbol).count()
                            }
                        })
                        .unwrap_or(0)
                } else if let Some(path) = &class.path {
                    self.lsp
                        .base_symbols(path)
                        .map(|rows| {
                            if signals {
                                script_signals(rows).count()
                            } else {
                                script_properties(rows).count()
                            }
                        })
                        .unwrap_or(0)
                } else {
                    0
                }
            })
            .sum()
    }
}

/// Native signals (`LSP SymbolKind::Event`).
pub fn native_signals(symbol: &NativeSymbol) -> impl Iterator<Item = &NativeSymbol> {
    symbol.children.iter().filter(|child| child.kind == 24)
}

/// Native properties (`LSP SymbolKind::Property`).
pub fn native_properties(symbol: &NativeSymbol) -> impl Iterator<Item = &NativeSymbol> {
    symbol.children.iter().filter(|child| child.kind == 7)
}

/// Project-script signals (`documentSymbol` kind 24).
pub fn script_signals(rows: &[OutlineSymbol]) -> impl Iterator<Item = &OutlineSymbol> {
    rows.iter().filter(|row| row.kind == 24)
}

/// Project-script properties: GDScript `var`/`@export` (13) and properties (7).
pub fn script_properties(rows: &[OutlineSymbol]) -> impl Iterator<Item = &OutlineSymbol> {
    rows.iter().filter(|row| row.kind == 13 || row.kind == 7)
}

fn header_text(text: &str) -> &str {
    let mut end = text.len().min(4096);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn read_prefix(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = [0u8; 4096];
    let read = file.read(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer[..read]).into_owned())
}

fn read_class_cache(root: &Path) -> ClassCache {
    let path = root.join(".godot").join("global_script_class_cache.cfg");
    let Ok(text) = std::fs::read_to_string(path) else {
        return ClassCache::default();
    };
    ClassCache::new(root, crate::model::godot::parse_class_cache(&text))
}

/// Resolves the base chain, parents only, direct parent first.
fn resolve_chain(
    root: Option<&Path>,
    header: &ScriptHeader,
    cache: &ClassCache,
    lsp: &crate::model::lsp::LspController,
    mut read: impl FnMut(&Path) -> Option<String>,
) -> Vec<ChainClass> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut base = header
        .base
        .clone()
        .unwrap_or_else(|| "RefCounted".to_string());

    for _ in 0..16 {
        if base.is_empty() || !seen.insert(base.clone()) {
            break;
        }
        if let Some(quoted) = base
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        {
            let Some(path) = root.and_then(|root| godot::res_path(root, quoted)) else {
                out.push(ChainClass {
                    name: quoted.to_string(),
                    path: None,
                    native: false,
                    unresolved: true,
                });
                break;
            };
            let header = read(&path).map(|text| godot::parse_script_header(&text));
            let name = header
                .as_ref()
                .and_then(|header| header.class_name.clone())
                .or_else(|| cache.by_path.get(&path).map(|entry| entry.name.clone()))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("script")
                        .to_string()
                });
            let unresolved = header.is_none();
            out.push(ChainClass {
                name,
                path: Some(path),
                native: false,
                unresolved,
            });
            let Some(header) = header else {
                break;
            };
            base = header.base.unwrap_or_else(|| "RefCounted".into());
            continue;
        }

        if let Some(entry) = cache.by_name.get(&base) {
            let path = cache.path_of(&base);
            let header = path
                .as_deref()
                .and_then(&mut read)
                .map(|text| godot::parse_script_header(&text));
            out.push(ChainClass {
                unresolved: path.is_none(),
                name: base.clone(),
                path,
                native: false,
            });
            base = header
                .and_then(|header| header.base)
                .unwrap_or_else(|| entry.base.clone());
            if base.is_empty() {
                base = "RefCounted".into();
            }
            continue;
        }

        if lsp.is_native_class(&base) {
            let mut current = base;
            loop {
                let Some(next) = lsp.native_base(&current).map(str::to_string) else {
                    out.push(ChainClass {
                        name: current,
                        path: None,
                        native: true,
                        unresolved: false,
                    });
                    break;
                };
                out.push(ChainClass {
                    name: current.clone(),
                    path: None,
                    native: true,
                    unresolved: false,
                });
                if next.is_empty() || !seen.insert(next.clone()) {
                    break;
                }
                current = next;
            }
            break;
        }

        out.push(ChainClass {
            name: base,
            path: None,
            native: false,
            unresolved: true,
        });
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_filters_use_the_lsp_kinds() {
        let symbol = crate::model::lsp::NativeSymbol {
            name: "Node".into(),
            native_class: "Node".into(),
            detail: String::new(),
            documentation: String::new(),
            kind: 5,
            children: vec![
                crate::model::lsp::NativeSymbol {
                    name: "ready".into(),
                    native_class: "Node".into(),
                    detail: "signal".into(),
                    documentation: String::new(),
                    kind: 24,
                    children: Vec::new(),
                },
                crate::model::lsp::NativeSymbol {
                    name: "name".into(),
                    native_class: "Node".into(),
                    detail: "property".into(),
                    documentation: String::new(),
                    kind: 7,
                    children: Vec::new(),
                },
            ],
        };
        assert_eq!(native_signals(&symbol).count(), 1);
        assert_eq!(native_properties(&symbol).count(), 1);

        let rows = vec![
            OutlineSymbol {
                name: "hit".into(),
                detail: "signal hit()".into(),
                kind: 24,
                depth: 0,
                line: 1,
                character: 0,
            },
            OutlineSymbol {
                name: "hp".into(),
                detail: "var hp: int".into(),
                kind: 13,
                depth: 0,
                line: 2,
                character: 0,
            },
        ];
        assert_eq!(script_signals(&rows).count(), 1);
        assert_eq!(script_properties(&rows).count(), 1);
    }

    fn chain_names(chain: &[ChainClass]) -> Vec<&str> {
        chain.iter().map(|class| class.name.as_str()).collect()
    }

    fn native_fixture() -> crate::model::lsp::LspController {
        let mut lsp = crate::model::lsp::LspController::default();
        lsp.test_set_native_classes(&[
            ("Node2D", "CanvasItem"),
            ("CanvasItem", "Node"),
            ("Node", "Object"),
            ("Object", ""),
        ]);
        lsp
    }

    #[test]
    fn chain_resolves_project_scripts_then_the_native_tail() {
        let root = Path::new("/project");
        let cache = ClassCache::new(
            root,
            crate::model::godot::parse_class_cache(
                r#"list=[{
"base": &"EnemyBase",
"class": &"Enemy",
"path": "res://mid.gd"
}, {
"base": &"Node2D",
"class": &"EnemyBase",
"path": "res://base.gd"
}]
"#,
            ),
        );
        let files = std::collections::HashMap::from([
            (
                PathBuf::from("/project/mid.gd"),
                "class_name Enemy\nextends EnemyBase\n".to_string(),
            ),
            (
                PathBuf::from("/project/base.gd"),
                "class_name EnemyBase\nextends Node2D\n".to_string(),
            ),
        ]);
        let header = ScriptHeader {
            class_name: Some("Player".into()),
            base: Some("Enemy".into()),
        };
        let chain = resolve_chain(Some(root), &header, &cache, &native_fixture(), |path| {
            files.get(path).cloned()
        });
        assert_eq!(
            chain_names(&chain),
            [
                "Enemy",
                "EnemyBase",
                "Node2D",
                "CanvasItem",
                "Node",
                "Object"
            ]
        );
        assert_eq!(chain[0].path.as_deref(), Some(Path::new("/project/mid.gd")));
        assert!(chain[2].native && !chain[1].native);
    }

    #[test]
    fn chain_accepts_a_path_base_and_reports_unresolved_names() {
        let root = Path::new("/project");
        let cache = ClassCache::default();
        let files = std::collections::HashMap::from([(
            PathBuf::from("/project/plain.gd"),
            "extends Node\n".to_string(),
        )]);
        let header = ScriptHeader {
            class_name: None,
            base: Some("\"res://plain.gd\"".into()),
        };
        let chain = resolve_chain(Some(root), &header, &cache, &native_fixture(), |path| {
            files.get(path).cloned()
        });
        assert_eq!(chain_names(&chain), ["plain", "Node", "Object"]);

        let header = ScriptHeader {
            class_name: None,
            base: Some("MissingBase".into()),
        };
        let chain = resolve_chain(Some(root), &header, &cache, &native_fixture(), |_| None);
        assert_eq!(chain.len(), 1);
        assert!(chain[0].unresolved);
    }

    #[test]
    fn header_text_keeps_only_the_head() {
        let text = "extends Node\n".to_string() + &"x".repeat(10_000);
        assert_eq!(header_text(&text).len(), 4096);
    }
}
