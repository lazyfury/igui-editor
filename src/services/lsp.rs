//! Built-in Rust language-server plugin.
//!
//! The plugin is intentionally declarative: it contributes a
//! [`LanguageServerSpec`] and LSP capabilities. The generic client in
//! `model::lsp` owns the process and JSON-RPC; a future external plugin host can
//! publish the same spec without linking a dylib.

use super::{
    BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec, PluginHost, ServiceManifest,
};

pub struct RustLspPlugin;

pub const CAPABILITIES: &[Capability] = &[
    Capability::LspDiagnostics,
    Capability::LspHover,
    Capability::LspCompletion,
    Capability::LspDefinition,
    Capability::LspSymbols,
];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "lsp.hover",
        title: "Show Hover",
        capability: Some(Capability::LspHover),
    },
    CommandSpec {
        id: "lsp.completion",
        title: "Complete",
        capability: Some(Capability::LspCompletion),
    },
    CommandSpec {
        id: "lsp.definition",
        title: "Go to Definition",
        capability: Some(Capability::LspDefinition),
    },
];

/// Rust is the first language server. `rust-analyzer` is resolved from `PATH`
/// (or `IGUI_EDITOR_RUST_ANALYZER` for tests / custom installs).
pub fn rust_server() -> LanguageServerSpec {
    LanguageServerSpec {
        id: "rust".into(),
        language: "rust".into(),
        extensions: vec!["rs".into()],
        project_markers: vec!["Cargo.toml".into()],
        command: "rust-analyzer".into(),
        command_env: Some("IGUI_EDITOR_RUST_ANALYZER".into()),
        args: Vec::new(),
        transport: super::LanguageServerTransport::Stdio,
        port_arg: None,
        port: None,
    }
}

impl BuiltinPlugin for RustLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.rust"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.rust",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(rust_server());
    }
}
