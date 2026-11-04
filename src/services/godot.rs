//! Built-in Godot GDScript language-server plugin.
//!
//! Godot's editor ships its own GDScript language server. It listens on a
//! loopback TCP port (unlike rust-analyzer's stdio), so this plugin contributes
//! a TCP transport spec; the generic client owns process startup and JSON-RPC.
//!
//! The server lives inside the running Godot editor — the process that imports
//! the project and writes `<script>.gd.uid` — so the client only attaches; it
//! never starts a second, headless editor. `IGUI_EDITOR_GODOT_LSP` overrides the
//! address (`host:port`, `:port` or `port`; default `127.0.0.1:6005`).

use super::{
    AttachTarget, BuiltinPlugin, Capability, CommandSpec, FileChangePolicy, LanguageServerSpec,
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

/// The editor language-server spec. The command is only a label: the server is
/// never spawned, so the executable does not have to exist.
pub fn godot_server() -> LanguageServerSpec {
    LanguageServerSpec {
        id: "gdscript".into(),
        language: "gdscript".into(),
        extensions: vec!["gd".into()],
        project_markers: vec!["project.godot".into()],
        workspace_markers: Vec::new(),
        language_ids: Default::default(),
        loose_files: false,
        command: "godot".into(),
        command_env: None,
        // Attach-only: the server lives inside the running editor and there is
        // no headless fallback (a second editor would compete over the
        // project's import state and would not be the editor you are using).
        args: Vec::new(),
        transport: LanguageServerTransport::Tcp,
        port_arg: None,
        port: None,
        // Godot's editor language server (Editor Settings → Network → Language
        // Server; `network/language_server/remote_port` defaults to 6005).
        attach: Some(AttachTarget {
            host: "127.0.0.1".into(),
            port: 6005,
            env: Some("IGUI_EDITOR_GODOT_LSP".into()),
        }),
        // Godot's LSP has no `workspace/didChangeWatchedFiles` handler. It only
        // rescans the project on editor focus (or with the "rescan while
        // unfocused" timer), which is what updates its global class registry.
        file_changes: FileChangePolicy::ServerRescan,
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
        assert!(spec.args.is_empty(), "the editor is never spawned");
        assert!(spec.port_arg.is_none());
        assert!(spec.port.is_none());
        let attach = spec.attach.as_ref().expect("godot attaches to the editor");
        assert_eq!(attach.host, "127.0.0.1");
        assert_eq!(attach.port, 6005);
        assert_eq!(attach.env.as_deref(), Some("IGUI_EDITOR_GODOT_LSP"));
        // Godot ignores watched-file notifications; the client re-parses open
        // documents once the editor's own scan refreshes its class registry.
        assert_eq!(spec.file_changes, FileChangePolicy::ServerRescan);
    }

    #[test]
    fn transport_fields_round_trip_through_json() {
        let spec = godot_server();
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"transport\":\"tcp\""));
        assert!(json.contains("\"file_changes\":\"server_rescan\""));
        let decoded: LanguageServerSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, spec);
    }
}
