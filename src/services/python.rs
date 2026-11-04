//! Built-in Python language-server plugin (`pyright-langserver`).
//!
//! Pyright speaks stdio (`--stdio`) and works with loose files, so the spec
//! starts even without a project marker. `pyrightconfig.json` at a repository
//! root wins over a nested package config, which keeps monorepos on one server.

use super::{
    BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec, PluginHost, ServiceManifest,
    ToolInstall,
};

pub struct PythonLspPlugin;

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

/// `IGUI_EDITOR_PYRIGHT` overrides the executable (tests, custom installs).
pub fn python_server() -> LanguageServerSpec {
    LanguageServerSpec {
        id: "python".into(),
        language: "python".into(),
        extensions: vec!["py".into(), "pyi".into()],
        project_markers: vec![
            "pyproject.toml".into(),
            "setup.py".into(),
            "setup.cfg".into(),
            "requirements.txt".into(),
            ".git".into(),
        ],
        workspace_markers: vec!["pyrightconfig.json".into()],
        language_ids: Default::default(),
        loose_files: true,
        command: "pyright-langserver".into(),
        command_env: Some("IGUI_EDITOR_PYRIGHT".into()),
        args: vec!["--stdio".into()],
        transport: super::LanguageServerTransport::Stdio,
        port_arg: None,
        port: None,
        attach: None,
        install: Some(ToolInstall {
            tool: "pyright-langserver".into(),
            language: "Python".into(),
            command: "npm install -g pyright".into(),
            note: Some("or install the `pyright` Python package".into()),
        }),
    }
}

impl BuiltinPlugin for PythonLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.python"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.python",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(python_server());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_spec_is_a_stdio_loose_file_server() {
        let spec = python_server();
        assert_eq!(spec.id, "python");
        assert_eq!(spec.language, "python");
        assert_eq!(spec.extensions, vec!["py", "pyi"]);
        assert_eq!(spec.args, vec!["--stdio"]);
        assert!(spec.loose_files);
        assert!(spec.install.is_some());
        assert_eq!(spec.language_id(std::path::Path::new("mod.py")), "python");
    }
}
