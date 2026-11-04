//! Built-in Go language-server plugin (`gopls`).
//!
//! `gopls` speaks JSON-RPC over stdio and wants a module or workspace root.
//! `go.work` wins over a nested `go.mod`, so a multi-module repository runs one
//! server for the whole workspace instead of one per module.

use super::{
    BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec, PluginHost, ServiceManifest,
    ToolInstall,
};

pub struct GoLspPlugin;

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

/// `IGUI_EDITOR_GOPLS` overrides the executable (tests, custom installs).
pub fn go_server() -> LanguageServerSpec {
    LanguageServerSpec {
        id: "go".into(),
        language: "go".into(),
        extensions: vec!["go".into()],
        project_markers: vec!["go.mod".into()],
        workspace_markers: vec!["go.work".into()],
        language_ids: Default::default(),
        loose_files: false,
        command: "gopls".into(),
        command_env: Some("IGUI_EDITOR_GOPLS".into()),
        args: Vec::new(),
        transport: super::LanguageServerTransport::Stdio,
        port_arg: None,
        port: None,
        install: Some(ToolInstall {
            tool: "gopls".into(),
            language: "Go".into(),
            command: "go install golang.org/x/tools/gopls@latest".into(),
            note: Some("requires a Go toolchain; gopls lands in ~/go/bin".into()),
        }),
    }
}

impl BuiltinPlugin for GoLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.go"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.go",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(go_server());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_spec_is_a_stdio_module_server() {
        let spec = go_server();
        assert_eq!(spec.id, "go");
        assert_eq!(spec.language, "go");
        assert_eq!(spec.extensions, vec!["go"]);
        assert_eq!(spec.project_markers, vec!["go.mod"]);
        assert_eq!(spec.workspace_markers, vec!["go.work"]);
        assert_eq!(spec.transport, super::super::LanguageServerTransport::Stdio);
        assert_eq!(spec.command, "gopls");
        assert!(spec.install.is_some());
        assert!(!spec.loose_files);
    }

    #[test]
    fn missing_go_tool_has_a_copyable_install_command() {
        let install = go_server().install.expect("install hint");
        assert_eq!(install.tool, "gopls");
        assert!(install.command.contains("go install"));
        assert!(!install.command.contains('\n'));
    }
}
