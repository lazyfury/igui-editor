//! Built-in Godot GDScript language-server plugin.
//!
//! Godot's editor ships its own GDScript language server. It listens on a
//! loopback TCP port (unlike rust-analyzer's stdio), so this plugin contributes
//! a TCP transport spec; the generic client owns process startup and JSON-RPC.
//!
//! A running Godot editor already serves the project — and already ran its
//! import/scan, which is what creates `<script>.gd.uid` — so the client
//! attaches to it first and only spawns a private `--headless --editor`
//! instance when no editor is listening. `IGUI_EDITOR_GODOT_LSP` overrides the
//! address (`host:port`, `:port` or `port`).

use super::{
    AttachTarget, BuiltinPlugin, Capability, CommandSpec, LanguageServerSpec,
    LanguageServerTransport, PluginHost, ServiceManifest,
};

pub struct GodotLspPlugin;

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

/// The default Godot executable. `IGUI_EDITOR_GODOT` overrides it for a custom
/// install or a test server.
pub fn godot_server() -> LanguageServerSpec {
    let command = if cfg!(target_os = "macos") {
        "/Applications/Godot.app/Contents/MacOS/Godot"
    } else {
        "godot"
    };
    LanguageServerSpec {
        id: "gdscript".into(),
        language: "gdscript".into(),
        extensions: vec!["gd".into()],
        project_markers: vec!["project.godot".into()],
        workspace_markers: Vec::new(),
        language_ids: Default::default(),
        loose_files: false,
        command: command.into(),
        command_env: Some("IGUI_EDITOR_GODOT".into()),
        args: vec![
            "--headless".into(),
            "--editor".into(),
            "--path".into(),
            ".".into(),
        ],
        transport: LanguageServerTransport::Tcp,
        port_arg: Some("--lsp-port".into()),
        port: None,
        // Godot's editor language server (Editor Settings → Network → Language
        // Server, port 6008 by default). Attaching keeps our diagnostics in step
        // with the editor that does the importing.
        attach: Some(AttachTarget {
            host: "127.0.0.1".into(),
            port: 6008,
            env: Some("IGUI_EDITOR_GODOT_LSP".into()),
        }),
        install: None,
    }
}

impl BuiltinPlugin for GodotLspPlugin {
    fn id(&self) -> &'static str {
        "builtin.lsp.godot"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "lsp.godot",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.language_server(godot_server());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn godot_spec_is_a_tcp_project_server() {
        let spec = godot_server();
        assert_eq!(spec.id, "gdscript");
        assert_eq!(spec.language, "gdscript");
        assert_eq!(spec.extensions, vec!["gd"]);
        assert_eq!(spec.project_markers, vec!["project.godot"]);
        assert_eq!(spec.transport, LanguageServerTransport::Tcp);
        assert_eq!(spec.port_arg.as_deref(), Some("--lsp-port"));
        assert!(spec.port.is_none());
        assert!(spec.args.iter().any(|arg| arg == "--headless"));
        let attach = spec.attach.as_ref().expect("godot attaches to the editor");
        assert_eq!(attach.host, "127.0.0.1");
        assert_eq!(attach.port, 6008);
        assert_eq!(attach.env.as_deref(), Some("IGUI_EDITOR_GODOT_LSP"));
    }

    #[test]
    fn transport_fields_round_trip_through_json() {
        let spec = godot_server();
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"transport\":\"tcp\""));
        let decoded: LanguageServerSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, spec);
    }
}
