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

/// File the Godot editor uses for editor-wide settings, per platform.
fn settings_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        #[cfg(target_os = "macos")]
        dirs.push(home.join("Library/Application Support/Godot"));
        #[cfg(not(target_os = "macos"))]
        {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            dirs.push(config.join("godot"));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(std::path::PathBuf::from) {
        dirs.push(appdata.join("Godot"));
    }
    dirs
}

/// The configured GDScript language-server port, if the editor settings are
/// readable.
///
/// `IGUI_EDITOR_GODOT_SETTINGS` points at a settings file directly (portable
/// installs and tests). Otherwise the newest `editor_settings-<version>.tres`
/// in the platform settings directory wins.
pub fn discover_lsp_port() -> Option<u16> {
    if let Some(path) = std::env::var_os("IGUI_EDITOR_GODOT_SETTINGS").map(std::path::PathBuf::from)
    {
        // An explicit override wins even when the file is missing: falling
        // back to the platform directory would ignore the user's intent.
        return std::fs::read_to_string(path)
            .ok()
            .and_then(|text| lsp_port_from_settings(&text));
    }
    settings_dirs()
        .into_iter()
        .find_map(|dir| discover_lsp_port_in(&dir))
}

/// Reads the newest settings file in `dir`.
fn discover_lsp_port_in(dir: &std::path::Path) -> Option<u16> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: Option<((u32, u32), std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = settings_version(&name) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(best, _)| version > *best) {
            newest = Some((version, entry.path()));
        }
    }
    let (_, path) = newest?;
    let text = std::fs::read_to_string(path).ok()?;
    lsp_port_from_settings(&text)
}

/// `editor_settings-4.7.tres` → `(4, 7)`; the unversioned legacy file sorts
/// first, and `editor_settings-4.tres` is `(4, 0)`.
fn settings_version(name: &str) -> Option<(u32, u32)> {
    if name == "editor_settings.tres" {
        return Some((0, 0));
    }
    let rest = name
        .strip_prefix("editor_settings-")?
        .strip_suffix(".tres")?;
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Parses `network/language_server/remote_port = 6005` from a `.tres` file.
fn lsp_port_from_settings(text: &str) -> Option<u16> {
    text.lines().rev().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "network/language_server/remote_port" {
            return None;
        }
        value.trim().parse::<u16>().ok().filter(|port| *port > 0)
    })
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn parses_the_remote_port_and_ignores_other_network_settings() {
        let settings = r#"
[network]

network/debug/remote_port = 6007
network/language_server/remote_host = "127.0.0.1"
network/language_server/remote_port = 6105
"#;
        assert_eq!(lsp_port_from_settings(settings), Some(6105));
        assert_eq!(
            lsp_port_from_settings("network/debug/remote_port = 6007"),
            None
        );
        assert_eq!(
            lsp_port_from_settings("network/language_server/remote_port = 0"),
            None
        );
    }

    #[test]
    fn newest_editor_settings_file_wins() {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-godot-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("editor_settings-4.6.tres"),
            "network/language_server/remote_port = 6005\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("editor_settings-4.7.tres"),
            "network/language_server/remote_port = 6123\n",
        )
        .unwrap();
        assert_eq!(discover_lsp_port_in(&dir), Some(6123));
        assert_eq!(settings_version("editor_settings-4.7.tres"), Some((4, 7)));
        assert_eq!(settings_version("editor_settings-4.tres"), Some((4, 0)));
        assert_eq!(settings_version("editor_settings.tres"), Some((0, 0)));
        assert_eq!(settings_version("editor_settings-4.7.tmp"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
