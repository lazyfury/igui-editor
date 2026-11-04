//! Native service / built-in plugin descriptors.
//!
//! A plugin is a compile-time registration unit; a service is the capability
//! implementation. Built-in plugins call native Rust services directly, but the
//! manifest/capability/DTO shape is serializable so a future external extension
//! host can map the same commands over JSON-RPC without redesigning Git or the
//! terminal.

pub mod clang;
pub mod git;
pub mod go;
pub mod godot;
pub mod lsp;
pub mod python;
pub mod terminal;
pub mod typescript;

use std::collections::BTreeMap;

/// Capability ids are stable strings surfaced in manifests and, later, extension
/// permission prompts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Capability {
    GitReadStatus,
    GitReadDiff,
    GitReadLog,
    GitReadCommit,
    GitWriteStage,
    GitWriteCommit,
    TerminalCreate,
    TerminalWrite,
    TerminalResize,
    TerminalClose,
    LspDiagnostics,
    LspHover,
    LspCompletion,
    LspDefinition,
    LspSymbols,
}

impl Capability {
    pub const fn id(self) -> &'static str {
        match self {
            Capability::GitReadStatus => "git.read.status",
            Capability::GitReadDiff => "git.read.diff",
            Capability::GitReadLog => "git.read.log",
            Capability::GitReadCommit => "git.read.commit",
            Capability::GitWriteStage => "git.write.stage",
            Capability::GitWriteCommit => "git.write.commit",
            Capability::TerminalCreate => "terminal.create",
            Capability::TerminalWrite => "terminal.write",
            Capability::TerminalResize => "terminal.resize",
            Capability::TerminalClose => "terminal.close",
            Capability::LspDiagnostics => "lsp.diagnostics",
            Capability::LspHover => "lsp.hover",
            Capability::LspCompletion => "lsp.completion",
            Capability::LspDefinition => "lsp.definition",
            Capability::LspSymbols => "lsp.symbols",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceManifest {
    pub id: &'static str,
    pub version: &'static str,
    pub capabilities: &'static [Capability],
}

#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub capability: Option<Capability>,
}

/// Sidebar tab contributed by a built-in plugin (e.g. Explorer / Git).
#[derive(Clone, Copy, Debug)]
pub struct SidebarContribution {
    pub id: &'static str,
    pub label: &'static str,
}

/// Copy-paste installation hint shown when a language server executable is
/// missing. The editor never installs tools itself; the prompt only offers the
/// command so the user can run it in the terminal.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ToolInstall {
    /// Executable name, e.g. `gopls`.
    #[serde(default)]
    pub tool: String,
    /// Human language name, e.g. `Go`.
    #[serde(default)]
    pub language: String,
    /// Shell command that installs the tool.
    #[serde(default)]
    pub command: String,
    /// Optional extra context (server alternatives, caveats).
    #[serde(default)]
    pub note: Option<String>,
}

/// How the client reaches a language server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageServerTransport {
    /// Spawn a child and speak JSON-RPC over its stdin/stdout.
    #[default]
    Stdio,
    /// Spawn a child that listens on loopback TCP, then connect to it.
    Tcp,
}

/// Declarative language-server contribution. The command is resolved from
/// `PATH`, or from `command_env` when that variable is set. `project_markers`
/// keeps a server idle until a real project root exists (e.g. `Cargo.toml`).
///
/// Owned strings (rather than `&'static str`) are deliberate: the same shape
/// can be deserialized by a future extension host or a data-only language pack
/// without changing the LSP client.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct LanguageServerSpec {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub project_markers: Vec<String>,
    /// Workspace-level markers (`go.work`, `pnpm-workspace.yaml`, …). When one
    /// is found inside the open folder, the *outermost* match is the root, so a
    /// monorepo starts one server instead of one per package.
    #[serde(default)]
    pub workspace_markers: Vec<String>,
    /// Extension → LSP `languageId` overrides (`.tsx` → `typescriptreact`).
    #[serde(default)]
    pub language_ids: std::collections::BTreeMap<String, String>,
    /// Start even when no marker exists (the file's folder becomes the root).
    #[serde(default)]
    pub loose_files: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub command_env: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub transport: LanguageServerTransport,
    /// Argument prepended to the dynamically allocated loopback port, e.g.
    /// `--lsp-port` for Godot. When absent, `port` must be set.
    #[serde(default)]
    pub port_arg: Option<String>,
    /// Fixed port for a spawned TCP server. Built-in Godot allocates a fresh
    /// loopback port per launch instead.
    #[serde(default)]
    pub port: Option<u16>,
    /// Shown when the command cannot be found; never executed automatically.
    #[serde(default)]
    pub install: Option<ToolInstall>,
}

impl LanguageServerSpec {
    /// LSP `languageId` for a file: an explicit extension mapping wins, then
    /// the spec's single language.
    pub fn language_id(&self, path: &std::path::Path) -> &str {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| {
                self.language_ids
                    .iter()
                    .find(|(candidate, _)| candidate.eq_ignore_ascii_case(ext))
                    .map(|(_, language)| language.as_str())
            })
            .unwrap_or(&self.language)
    }

    pub fn matches_path(&self, path: &std::path::Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                self.extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(ext))
            })
    }
}

/// A compile-time plugin. No dynamic loading and no Rust dylib ABI.
pub trait BuiltinPlugin {
    fn id(&self) -> &'static str;
    fn manifest(&self) -> ServiceManifest;
    fn register(&self, host: &mut PluginHost);
}

#[derive(Default)]
pub struct ServiceRegistry {
    pub plugins: Vec<&'static str>,
    pub services: BTreeMap<&'static str, ServiceManifest>,
    pub commands: BTreeMap<&'static str, CommandSpec>,
    pub sidebar_tabs: Vec<SidebarContribution>,
    pub language_servers: Vec<LanguageServerSpec>,
}

pub struct PluginHost<'a> {
    pub registry: &'a mut ServiceRegistry,
}

impl PluginHost<'_> {
    pub fn service(&mut self, manifest: ServiceManifest, commands: &[CommandSpec]) {
        self.registry.services.insert(manifest.id, manifest);
        for command in commands {
            self.registry.commands.insert(command.id, *command);
        }
    }

    pub fn language_server(&mut self, server: LanguageServerSpec) {
        if !self
            .registry
            .language_servers
            .iter()
            .any(|entry| entry.id == server.id)
        {
            self.registry.language_servers.push(server);
        }
    }

    pub fn sidebar_tab(&mut self, tab: SidebarContribution) {
        if !self
            .registry
            .sidebar_tabs
            .iter()
            .any(|entry| entry.id == tab.id)
        {
            self.registry.sidebar_tabs.push(tab);
        }
    }
}

/// Register all built-ins. Explorer is core (registered here so the Git plugin
/// can add a sibling tab without hard-coding a two-tab layout).
pub fn install_builtins() -> ServiceRegistry {
    let mut registry = ServiceRegistry::default();
    registry.sidebar_tabs.push(SidebarContribution {
        id: "explorer",
        label: "EXPLORER",
    });
    let plugin = git::GitPlugin;
    let terminal = terminal::TerminalPlugin;
    let rust_lsp = lsp::RustLspPlugin;
    let godot_lsp = godot::GodotLspPlugin;
    let go_lsp = go::GoLspPlugin;
    let ts_lsp = typescript::TypeScriptLspPlugin;
    let clang_lsp = clang::ClangLspPlugin;
    let python_lsp = python::PythonLspPlugin;
    for id in [
        plugin.id(),
        terminal.id(),
        rust_lsp.id(),
        godot_lsp.id(),
        go_lsp.id(),
        ts_lsp.id(),
        clang_lsp.id(),
        python_lsp.id(),
    ] {
        registry.plugins.push(id);
    }
    let mut host = PluginHost {
        registry: &mut registry,
    };
    plugin.register(&mut host);
    terminal.register(&mut host);
    rust_lsp.register(&mut host);
    godot_lsp.register(&mut host);
    go_lsp.register(&mut host);
    ts_lsp.register(&mut host);
    clang_lsp.register(&mut host);
    python_lsp.register(&mut host);
    registry
}
