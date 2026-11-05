//! Built-in Git plugin descriptor. The service implementation lives in
//! `model::git`; this file declares what future extension hosts may call.

use super::{
    BuiltinPlugin, Capability, CommandSpec, PluginHost, ServiceManifest, SidebarContribution,
};

pub struct GitPlugin;

pub const CAPABILITIES: &[Capability] = &[Capability::GitReadStatus, Capability::GitReadDiff];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "git.refresh",
        title: "Refresh Git Status",
        capability: Some(Capability::GitReadStatus),
    },
    CommandSpec {
        id: "git.diff",
        title: "Show Diff",
        capability: Some(Capability::GitReadDiff),
    },
];

impl BuiltinPlugin for GitPlugin {
    fn id(&self) -> &'static str {
        "builtin.git"
    }

    fn manifest(&self) -> ServiceManifest {
        ServiceManifest {
            id: "git",
            version: "1",
            capabilities: CAPABILITIES,
        }
    }

    fn register(&self, host: &mut PluginHost) {
        host.service(self.manifest(), COMMANDS);
        host.sidebar_tab(SidebarContribution {
            id: "git",
            label: "GIT",
            icon: "git-branch",
        });
    }
}
