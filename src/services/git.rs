//! Built-in Git plugin descriptor. The service implementation lives in
//! `model::git`; this file declares what future extension hosts may call.

use super::{
    BuiltinPlugin, Capability, CommandSpec, PluginHost, ServiceManifest, SidebarContribution,
};

pub struct GitPlugin;

pub const CAPABILITIES: &[Capability] = &[
    Capability::GitReadStatus,
    Capability::GitReadDiff,
    Capability::GitReadLog,
    Capability::GitReadCommit,
    Capability::GitWriteStage,
    Capability::GitWriteCommit,
];

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
    CommandSpec {
        id: "git.log",
        title: "Show Commit History",
        capability: Some(Capability::GitReadLog),
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
