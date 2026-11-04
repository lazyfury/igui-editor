//! Built-in C / C++ language-server plugin (`clangd`).
//!
//! `clangd` speaks stdio and is happiest with `compile_commands.json` or a
//! CMake/`.clangd` project root; `.git` keeps a repository at one root when no
//! build database exists. Header files are ambiguous (C or C++), so `.h` is
//! mapped to `c` and the rest to `cpp`.

use super::{
    BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec, PluginHost, ServiceManifest,
    ToolInstall,
};

pub struct ClangLspPlugin;

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

/// `IGUI_EDITOR_CLANGD` overrides the executable (tests, custom installs).
pub fn clang_server() -> LanguageServerSpec {
    let mut language_ids = std::collections::BTreeMap::new();
    for extension in ["cpp", "cc", "cxx", "hpp", "hh", "hxx"] {
        language_ids.insert(extension.to_string(), "cpp".to_string());
    }
    language_ids.insert("c".into(), "c".into());
    language_ids.insert("h".into(), "c".into());
    LanguageServerSpec {
        id: "clangd".into(),
        language: "cpp".into(),
        extensions: vec![
            "c".into(),
            "h".into(),
            "cpp".into(),
            "cc".into(),
            "cxx".into(),
            "hpp".into(),
            "hh".into(),
            "hxx".into(),
        ],
        project_markers: vec![
            "compile_commands.json".into(),
            "CMakeLists.txt".into(),
            ".clangd".into(),
            ".git".into(),
        ],
        workspace_markers: vec![".clangd".into(), ".git".into()],
        language_ids,
        loose_files: true,
        command: "clangd".into(),
        command_env: Some("IGUI_EDITOR_CLANGD".into()),
        args: Vec::new(),
        transport: super::LanguageServerTransport::Stdio,
        port_arg: None,
        port: None,
        attach: None,
        install: Some(ToolInstall {
            tool: "clangd".into(),
            language: "C / C++".into(),
            command: "brew install llvm".into(),
            note: Some(
                "Xcode command line tools also ship clangd (`xcode-select --install`)".into(),
            ),
        }),
    }
}

impl BuiltinPlugin for ClangLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.clangd"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.clangd",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(clang_server());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clang_spec_splits_c_and_cpp_language_ids() {
        let spec = clang_server();
        assert_eq!(spec.language_id(std::path::Path::new("a.c")), "c");
        assert_eq!(spec.language_id(std::path::Path::new("a.h")), "c");
        assert_eq!(spec.language_id(std::path::Path::new("a.cpp")), "cpp");
        assert_eq!(spec.language_id(std::path::Path::new("a.hpp")), "cpp");
        assert_eq!(spec.language_id(std::path::Path::new("A.C")), "c");
        assert!(spec.loose_files);
        assert_eq!(spec.command, "clangd");
    }

    #[test]
    fn clang_spec_prefers_a_workspace_root() {
        let spec = clang_server();
        assert!(spec.project_markers.iter().any(|marker| marker == ".git"));
        assert!(spec.workspace_markers.iter().any(|marker| marker == ".git"));
    }
}
