//! Built-in terminal plugin descriptor. PTY implementation lives in `model::pty`.

use super::{BuiltinPlugin, Capability, CommandSpec, PluginHost, ServiceManifest};

pub struct TerminalPlugin;

pub const CAPABILITIES: &[Capability] = &[
    Capability::TerminalCreate,
    Capability::TerminalWrite,
    Capability::TerminalResize,
    Capability::TerminalClose,
];

pub const COMMANDS: &[CommandSpec] = &[CommandSpec {
    id: "terminal.toggle",
    title: "Toggle Terminal",
    capability: Some(Capability::TerminalCreate),
}];

impl BuiltinPlugin for TerminalPlugin {
    fn id(&self) -> &'static str {
        "builtin.terminal"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "terminal",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
    }
}
