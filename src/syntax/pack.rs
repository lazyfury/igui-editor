//! Declarative language packs: JSON files under `<config>/languages/*.json`.
//!
//! This is the data-only half of the extension seam: a pack can add keywords,
//! comments, strings and simple flags to the existing C-family tokenizer
//! without shipping code. Built-ins always win a name clash (the registry keeps
//! the first registration), and code plugins remain out of scope.
//!
//! ```json
//! {
//!   "name": "lua",
//!   "extensions": ["lua"],
//!   "line_comment": ["--"],
//!   "block_comment": ["--[[", "]]"],
//!   "keywords": ["function", "local", "end"],
//!   "constants": ["true", "false", "nil"],
//!   "strings": [{ "quote": "\"", "escapes": true }],
//!   "lsp": {
//!     "id": "lua-ls",
//!     "language": "lua",
//!     "extensions": ["lua"],
//!     "project_markers": ["stylua.toml"],
//!     "command": "lua-language-server",
//!     "args": ["--stdio"]
//!   }
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use super::cstyle::{self, LangSpec, StringSpec};
use super::SyntaxProvider;
use crate::services::LanguageServerSpec;

/// Packs loaded per launch.
pub const MAX_PACKS: usize = 64;
/// Largest pack file in bytes.
pub const MAX_PACK_BYTES: u64 = 64 * 1024;

/// One JSON language pack.
#[derive(Clone, Debug, Deserialize)]
pub struct LanguagePack {
    pub name: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub line_comment: Vec<String>,
    #[serde(default)]
    pub block_comment: Option<Vec<String>>,
    #[serde(default)]
    pub nested_block: bool,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub constants: Vec<String>,
    #[serde(default)]
    pub strings: Vec<PackString>,
    #[serde(default)]
    pub triple: bool,
    #[serde(default)]
    pub template: bool,
    #[serde(default)]
    pub template_escapes: bool,
    #[serde(default)]
    pub preprocessor: bool,
    #[serde(default)]
    pub annotations: bool,
    #[serde(default)]
    pub toml_tables: bool,
    #[serde(default)]
    pub case_types: bool,
    /// Optional language-server contribution for this data-only pack.
    #[serde(default)]
    pub lsp: Option<LanguageServerSpec>,
}

/// One quote style.
#[derive(Clone, Debug, Deserialize)]
pub struct PackString {
    /// Exactly one ASCII character, e.g. `"` or `'`.
    pub quote: String,
    #[serde(default)]
    pub escapes: bool,
}

impl LanguagePack {
    /// Parses and validates one JSON document.
    pub fn parse(json: &str) -> Result<Self, String> {
        let pack: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        pack.validate()?;
        Ok(pack)
    }

    /// Checks names, extensions, markers and list sizes.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 32 {
            return Err("name must be 1..32 characters".into());
        }
        if self.extensions.is_empty() || self.extensions.len() > 16 {
            return Err("extensions must contain 1..16 entries".into());
        }
        for extension in &self.extensions {
            if extension.is_empty()
                || extension.len() > 12
                || extension.starts_with('.')
                || extension.contains('/')
            {
                return Err(format!("invalid extension {extension:?}"));
            }
        }
        if self.line_comment.len() > 4
            || self
                .line_comment
                .iter()
                .any(|marker| marker.is_empty() || marker.len() > 8 || marker.contains('\n'))
        {
            return Err("invalid line_comment markers".into());
        }
        if let Some(block) = &self.block_comment {
            if block.len() != 2
                || block
                    .iter()
                    .any(|marker| marker.is_empty() || marker.len() > 8 || marker.contains('\n'))
            {
                return Err("block_comment must be two non-empty markers".into());
            }
        }
        for (label, list) in [
            ("keywords", &self.keywords),
            ("types", &self.types),
            ("constants", &self.constants),
        ] {
            if list.len() > 2_000 || list.iter().any(|item| item.len() > 64 || item.is_empty()) {
                return Err(format!("invalid {label} list"));
            }
        }
        if self.strings.len() > 8 {
            return Err("too many string styles".into());
        }
        for string in &self.strings {
            let mut chars = string.quote.chars();
            let Some(quote) = chars.next() else {
                return Err("string quote must not be empty".into());
            };
            if !quote.is_ascii() || chars.next().is_some() {
                return Err("string quote must be one ASCII character".into());
            }
        }
        if let Some(server) = &self.lsp {
            Self::validate_server(server)?;
        }
        Ok(())
    }

    /// Validation shared by data-only packs and future dynamic plugin manifests.
    fn validate_server(server: &LanguageServerSpec) -> Result<(), String> {
        let short = |value: &str| !value.is_empty() && value.len() <= 64 && !value.contains('\n');
        if !short(&server.id) || !short(&server.language) || !short(&server.command) {
            return Err("lsp id/language/command must be 1..64 characters".into());
        }
        if server.command.len() > 512 {
            return Err("lsp command is too long".into());
        }
        if server.extensions.is_empty() || server.extensions.len() > 16 {
            return Err("lsp extensions must contain 1..16 entries".into());
        }
        if server.project_markers.len() > 8 || server.workspace_markers.len() > 8 {
            return Err("too many lsp project/workspace markers".into());
        }
        for marker in server
            .project_markers
            .iter()
            .chain(server.workspace_markers.iter())
        {
            if marker.is_empty() || marker.len() > 128 || marker.contains(['/', '\\']) {
                return Err(format!("invalid lsp marker {marker:?}"));
            }
        }
        if server.language_ids.len() > 16 {
            return Err("too many lsp language ids".into());
        }
        for (extension, language) in &server.language_ids {
            if extension.is_empty()
                || extension.len() > 12
                || extension.starts_with('.')
                || extension.contains('/')
                || language.is_empty()
                || language.len() > 64
                || language.contains('\n')
            {
                return Err("invalid lsp language id".into());
            }
        }
        if let Some(install) = &server.install {
            if install.command.len() > 512 || install.command.contains('\n') {
                return Err("invalid lsp install command".into());
            }
            if install.tool.len() > 128 || install.language.len() > 64 {
                return Err("invalid lsp install hint".into());
            }
        }
        if server.args.len() > 32 || server.args.iter().any(|arg| arg.len() > 512) {
            return Err("invalid lsp args".into());
        }
        if server
            .port_arg
            .as_ref()
            .is_some_and(|arg| arg.is_empty() || arg.len() > 64 || arg.contains('\n'))
        {
            return Err("invalid lsp port_arg".into());
        }
        if server.port == Some(0) {
            return Err("lsp port must be non-zero".into());
        }
        Ok(())
    }

    fn into_provider(self) -> Result<Arc<dyn SyntaxProvider>, String> {
        self.validate()?;
        let strings: Vec<StringSpec> = if self.strings.is_empty() {
            vec![
                StringSpec {
                    quote: b'"',
                    escapes: true,
                },
                StringSpec {
                    quote: b'\'',
                    escapes: true,
                },
            ]
        } else {
            self.strings
                .iter()
                .map(|string| {
                    let quote = string.quote.as_bytes()[0];
                    StringSpec {
                        quote,
                        escapes: string.escapes,
                    }
                })
                .collect()
        };
        let spec: &'static LangSpec = Box::leak(Box::new(LangSpec {
            name: leak(&self.name),
            extensions: leak_strs(self.extensions),
            line_comment: leak_strs(self.line_comment),
            block_comment: self
                .block_comment
                .map(|block| (leak(&block[0]), leak(&block[1]))),
            nested_block: self.nested_block,
            strings: Box::leak(strings.into_boxed_slice()),
            triple: self.triple,
            rust: false,
            cpp_raw: false,
            template: self.template,
            template_escapes: self.template_escapes,
            preprocessor: self.preprocessor,
            annotations: self.annotations,
            toml_tables: self.toml_tables,
            case_types: self.case_types,
            keywords: leak_strs(self.keywords),
            types: leak_strs(self.types),
            constants: leak_strs(self.constants),
        }));
        Ok(cstyle::provider(spec))
    }
}

/// Loads every `*.json` under `dir` (sorted, capped). Returns providers plus
/// human-readable errors for skipped files; a missing directory is not an
/// error.
pub fn load_language_packs(
    dir: &Path,
) -> (
    Vec<Arc<dyn SyntaxProvider>>,
    Vec<LanguageServerSpec>,
    Vec<String>,
) {
    let mut providers = Vec::new();
    let mut servers = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return (providers, servers, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths.truncate(MAX_PACKS);
    for path in paths {
        let name = path.display().to_string();
        match fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_PACK_BYTES => {
                errors.push(format!("{name}: larger than 64 KB"));
                continue;
            }
            Err(error) => {
                errors.push(format!("{name}: {error}"));
                continue;
            }
            _ => {}
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{name}: {error}"));
                continue;
            }
        };
        match LanguagePack::parse(&text) {
            Ok(pack) => {
                let server = pack.lsp.clone();
                match pack.into_provider() {
                    Ok(provider) => {
                        providers.push(provider);
                        if let Some(server) = server {
                            servers.push(server);
                        }
                    }
                    Err(error) => errors.push(format!("{name}: {error}")),
                }
            }
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    (providers, servers, errors)
}

fn leak(value: &str) -> &'static str {
    Box::leak(value.to_string().into_boxed_str())
}

fn leak_strs(values: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = values.iter().map(|value| leak(value)).collect();
    Box::leak(leaked.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::syntax::{Highlight, SyntaxRegistry, TokenKind};

    fn dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-pack-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lua_pack() -> &'static str {
        r#"{
            "name": "lua",
            "extensions": ["lua"],
            "line_comment": ["--"],
            "block_comment": ["--[[", "]]"],
            "keywords": ["function", "local", "end"],
            "constants": ["true", "false", "nil"],
            "strings": [{"quote": "\"", "escapes": true}]
        }"#
    }

    #[test]
    fn parses_and_highlights_declarative_packs() {
        let pack = LanguagePack::parse(lua_pack()).expect("valid pack");
        assert_eq!(pack.name, "lua");
        let provider = pack.into_provider().expect("provider");
        assert_eq!(provider.name(), "lua");
        assert_eq!(provider.extensions(), &["lua"]);
        let (tokens, _) = provider.highlight_line("local function f() end -- note", 0);
        assert!(tokens
            .iter()
            .any(|token: &Highlight| token.kind == TokenKind::Keyword));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Comment));
    }

    #[test]
    fn pack_can_contribute_a_language_server() {
        let pack = LanguagePack::parse(
            r#"{
                "name": "lua",
                "extensions": ["lua"],
                "lsp": {
                    "id": "lua-ls",
                    "language": "lua",
                    "extensions": ["lua"],
                    "project_markers": ["stylua.toml"],
                    "command": "lua-language-server",
                    "args": ["--stdio"]
                }
            }"#,
        )
        .expect("valid server pack");
        let server = pack.lsp.expect("server spec");
        assert_eq!(server.id, "lua-ls");
        assert_eq!(server.args, ["--stdio"]);
    }

    #[test]
    fn lsp_server_fields_are_validated() {
        let mut server: crate::services::LanguageServerSpec = serde_json::from_str(
            r#"{"id":"lua-ls","language":"lua","extensions":["lua"],
                "project_markers":["stylua.toml"],"command":"lua-language-server"}"#,
        )
        .expect("server spec");
        assert!(LanguagePack::validate_server(&server).is_ok());

        server.project_markers = vec!["../up".into()];
        assert!(LanguagePack::validate_server(&server).is_err());
        server.project_markers.clear();

        server.language_ids.insert("lua".into(), String::new());
        assert!(LanguagePack::validate_server(&server).is_err());
        server.language_ids.clear();

        server.install = Some(crate::services::ToolInstall {
            tool: "lua-language-server".into(),
            language: "Lua".into(),
            command: "one\ntwo".into(),
            note: None,
        });
        assert!(LanguagePack::validate_server(&server).is_err());
        server.install.as_mut().unwrap().command = "brew install lua-language-server".into();
        assert!(LanguagePack::validate_server(&server).is_ok());
    }

    #[test]
    fn lsp_server_decodes_new_optional_fields() {
        let json = r#"{"name":"x","extensions":["x"],"lsp":{
            "id":"x","language":"x","extensions":["x"],"command":"x",
            "workspace_markers":["go.work"],
            "language_ids":{"tsx":"typescriptreact"},
            "loose_files":true,
            "install":{"tool":"x","language":"X","command":"install x"}
        }}"#;
        let pack = LanguagePack::parse(json).expect("valid pack");
        let server = pack.lsp.expect("lsp spec");
        assert_eq!(server.workspace_markers, vec!["go.work"]);
        assert_eq!(
            server.language_id(std::path::Path::new("a.tsx")),
            "typescriptreact"
        );
        assert!(server.loose_files);
        assert!(server.install.is_some());
    }

    #[test]
    fn pack_rejects_incomplete_language_server() {
        assert!(LanguagePack::parse(
            r#"{"name":"x","extensions":["x"],"lsp":{"id":"x","command":"x"}}"#
        )
        .is_err());
    }

    #[test]
    fn rejects_bad_packs() {
        assert!(LanguagePack::parse(r#"{"name":"","extensions":["x"]}"#).is_err());
        assert!(LanguagePack::parse(r#"{"name":"x","extensions":[]}"#).is_err());
        assert!(LanguagePack::parse(
            r#"{"name":"x","extensions":["x"],"strings":[{"quote":"ab"}]}"#
        )
        .is_err());
        assert!(
            LanguagePack::parse(r#"{"name":"x","extensions":["x"],"block_comment":["/*"]}"#)
                .is_err()
        );
    }

    #[test]
    fn load_dir_skips_broken_files_and_keeps_good_ones() {
        let dir = dir("load");
        fs::write(dir.join("lua.json"), lua_pack()).unwrap();
        fs::write(dir.join("broken.json"), "{ not json").unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let (providers, servers, errors) = load_language_packs(&dir);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name(), "lua");
        assert!(servers.is_empty());
        assert_eq!(errors.len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn builtins_win_name_clashes() {
        let mut registry = SyntaxRegistry::with_builtins();
        let pack = LanguagePack::parse(lua_pack()).unwrap();
        registry.register(pack.into_provider().unwrap());
        let fake_rust =
            LanguagePack::parse(r#"{"name":"rust","extensions":["fake"],"keywords":["whatever"]}"#)
                .unwrap();
        registry.register(fake_rust.into_provider().unwrap());
        assert_eq!(registry.by_name("rust").unwrap().extensions(), &["rs"]);
        assert!(registry.by_name("lua").is_some());
    }
}
