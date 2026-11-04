//! Built-in TypeScript / JavaScript language-server plugin.
//!
//! `typescript-language-server` speaks stdio (`--stdio`) and needs one server
//! per project; the LSP `languageId` differs per extension, so the spec carries
//! an explicit extension → language map. Workspace markers keep a monorepo on a
//! single root (`pnpm-workspace.yaml` and friends) instead of one server per
//! package.

use super::{
    BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec, PluginHost, ServiceManifest,
    ToolInstall,
};

pub struct TypeScriptLspPlugin;

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

/// `IGUI_EDITOR_TS_SERVER` overrides the executable (tests, custom installs).
pub fn typescript_server() -> LanguageServerSpec {
    let mut language_ids = std::collections::BTreeMap::new();
    for extension in ["ts", "mts", "cts"] {
        language_ids.insert(extension.to_string(), "typescript".to_string());
    }
    for extension in ["js", "mjs", "cjs"] {
        language_ids.insert(extension.to_string(), "javascript".to_string());
    }
    language_ids.insert("tsx".into(), "typescriptreact".into());
    language_ids.insert("jsx".into(), "javascriptreact".into());
    LanguageServerSpec {
        id: "typescript".into(),
        language: "typescript".into(),
        extensions: vec![
            "ts".into(),
            "tsx".into(),
            "mts".into(),
            "cts".into(),
            "js".into(),
            "jsx".into(),
            "mjs".into(),
            "cjs".into(),
        ],
        project_markers: vec![
            "package.json".into(),
            "tsconfig.json".into(),
            "jsconfig.json".into(),
        ],
        workspace_markers: vec![
            "pnpm-workspace.yaml".into(),
            "lerna.json".into(),
            "nx.json".into(),
            "turbo.json".into(),
            "rush.json".into(),
            "tsconfig.base.json".into(),
        ],
        language_ids,
        loose_files: true,
        command: "typescript-language-server".into(),
        command_env: Some("IGUI_EDITOR_TS_SERVER".into()),
        args: vec!["--stdio".into()],
        transport: super::LanguageServerTransport::Stdio,
        port_arg: None,
        port: None,
        install: Some(ToolInstall {
            tool: "typescript-language-server".into(),
            language: "TypeScript / JavaScript".into(),
            command: "npm install -g typescript-language-server typescript".into(),
            note: Some("a project-local install under node_modules/.bin also works".into()),
        }),
    }
}

impl BuiltinPlugin for TypeScriptLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.typescript"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.typescript",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(typescript_server());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_spec_maps_every_extension_to_a_language_id() {
        let spec = typescript_server();
        let expected = [
            ("a.ts", "typescript"),
            ("a.mts", "typescript"),
            ("a.cts", "typescript"),
            ("a.tsx", "typescriptreact"),
            ("a.js", "javascript"),
            ("a.mjs", "javascript"),
            ("a.cjs", "javascript"),
            ("a.jsx", "javascriptreact"),
            ("A.TSX", "typescriptreact"),
        ];
        for (name, language) in expected {
            assert_eq!(
                spec.language_id(std::path::Path::new(name)),
                language,
                "{name}"
            );
        }
        assert_eq!(spec.args, vec!["--stdio"]);
        assert!(spec.loose_files);
    }

    #[test]
    fn node_modules_bin_is_searched_for_the_server() {
        let spec = typescript_server();
        assert!(spec.matches_path(std::path::Path::new("/p/a.tsx")));
        assert!(spec.install.is_some());
    }
}
