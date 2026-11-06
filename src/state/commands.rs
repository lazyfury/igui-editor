//! The canonical command-id table.
//!
//! [`COMMANDS`] is the single source of truth for stable command ids: the
//! keymap resolves overrides through [`Action::from_id`], and [`Action::id`]
//! gives menus and a future command palette the same name. The table carries
//! only what the ids need today; labels, categories and enabled predicates stay
//! on [`Action`] until the metadata slice lands.
//!
//! Dynamic actions have no stable id and are deliberately absent: recent paths,
//! named languages and tab slots outside 1..=9 are data, not commands.

use super::*;

/// One stable command id and the action it names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CommandSpec {
    /// Canonical id written to docs and `keymaps.json`.
    pub(crate) id: &'static str,
    /// Short or historical spellings accepted by `keymaps.json`.
    pub(crate) aliases: &'static [&'static str],
    pub(crate) action: Action,
}

/// Every action with a stable id. Ids are unique; an alias never repeats a
/// canonical id from another entry.
pub(crate) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "file.new",
        aliases: &["new"],
        action: Action::New,
    },
    CommandSpec {
        id: "file.newFile",
        aliases: &[],
        action: Action::NewFile,
    },
    CommandSpec {
        id: "file.open",
        aliases: &["open"],
        action: Action::Open,
    },
    CommandSpec {
        id: "file.openFolder",
        aliases: &["file.openProject"],
        action: Action::OpenFolder,
    },
    CommandSpec {
        id: "file.closeFolder",
        aliases: &["file.closeProject"],
        action: Action::CloseFolder,
    },
    CommandSpec {
        id: "file.save",
        aliases: &["save"],
        action: Action::Save,
    },
    CommandSpec {
        id: "file.saveAs",
        aliases: &[],
        action: Action::SaveAs,
    },
    CommandSpec {
        id: "file.closeTab",
        aliases: &[],
        action: Action::CloseTab,
    },
    CommandSpec {
        id: "file.clearRecent",
        aliases: &[],
        action: Action::ClearRecent,
    },
    CommandSpec {
        id: "edit.undo",
        aliases: &["undo"],
        action: Action::Undo,
    },
    CommandSpec {
        id: "edit.redo",
        aliases: &["redo"],
        action: Action::Redo,
    },
    CommandSpec {
        id: "edit.cut",
        aliases: &["cut"],
        action: Action::Cut,
    },
    CommandSpec {
        id: "edit.copy",
        aliases: &["copy"],
        action: Action::Copy,
    },
    CommandSpec {
        id: "edit.paste",
        aliases: &["paste"],
        action: Action::Paste,
    },
    CommandSpec {
        id: "edit.selectAll",
        aliases: &["selectAll"],
        action: Action::SelectAll,
    },
    CommandSpec {
        id: "edit.selectNextOccurrence",
        aliases: &["selectNextOccurrence"],
        action: Action::SelectNextOccurrence,
    },
    CommandSpec {
        id: "edit.deleteLine",
        aliases: &["deleteLine"],
        action: Action::DeleteLine,
    },
    CommandSpec {
        id: "edit.duplicateLine",
        aliases: &["duplicateLine"],
        action: Action::DuplicateLine,
    },
    CommandSpec {
        id: "edit.moveLineUp",
        aliases: &["moveLineUp"],
        action: Action::MoveLineUp,
    },
    CommandSpec {
        id: "edit.moveLineDown",
        aliases: &["moveLineDown"],
        action: Action::MoveLineDown,
    },
    CommandSpec {
        id: "edit.deleteWordBackward",
        aliases: &["deleteWordBackward"],
        action: Action::DeleteWordBackward,
    },
    CommandSpec {
        id: "edit.deleteWordForward",
        aliases: &["deleteWordForward"],
        action: Action::DeleteWordForward,
    },
    CommandSpec {
        id: "edit.toggleComment",
        aliases: &["toggleComment"],
        action: Action::ToggleComment,
    },
    CommandSpec {
        id: "edit.gotoLine",
        aliases: &[],
        action: Action::GotoLine,
    },
    CommandSpec {
        id: "motion.lineStart",
        aliases: &[],
        action: Action::MoveLineStart(false),
    },
    CommandSpec {
        id: "motion.lineEnd",
        aliases: &[],
        action: Action::MoveLineEnd(false),
    },
    CommandSpec {
        id: "motion.docStart",
        aliases: &[],
        action: Action::MoveDocStart(false),
    },
    CommandSpec {
        id: "motion.docEnd",
        aliases: &[],
        action: Action::MoveDocEnd(false),
    },
    CommandSpec {
        id: "editor.format",
        aliases: &[],
        action: Action::FormatDocument,
    },
    CommandSpec {
        id: "find.find",
        aliases: &[],
        action: Action::Find,
    },
    CommandSpec {
        id: "find.replace",
        aliases: &[],
        action: Action::FindReplace,
    },
    CommandSpec {
        id: "find.next",
        aliases: &[],
        action: Action::FindNext,
    },
    CommandSpec {
        id: "find.previous",
        aliases: &[],
        action: Action::FindPrevious,
    },
    CommandSpec {
        id: "quickOpen",
        aliases: &[],
        action: Action::QuickOpen,
    },
    CommandSpec {
        id: "projectSearch",
        aliases: &[],
        action: Action::ProjectSearch,
    },
    CommandSpec {
        id: "view.cycle",
        aliases: &[],
        action: Action::CycleView,
    },
    CommandSpec {
        id: "view.sidebar",
        aliases: &[],
        action: Action::ToggleSidebar,
    },
    CommandSpec {
        id: "view.explorer",
        aliases: &[],
        action: Action::ShowPanel(Panel::Explorer),
    },
    CommandSpec {
        id: "view.git",
        aliases: &[],
        action: Action::ShowPanel(Panel::Git),
    },
    CommandSpec {
        // The header Outline button toggles; `view.outline` is that same
        // command, so the panel id below stays distinct.
        id: "view.outline",
        aliases: &[],
        action: Action::ToggleOutline,
    },
    CommandSpec {
        id: "panel.outline",
        aliases: &[],
        action: Action::ShowPanel(Panel::Outline),
    },
    CommandSpec {
        id: "view.edit",
        aliases: &[],
        action: Action::SetView(ViewMode::Edit),
    },
    CommandSpec {
        id: "view.split",
        aliases: &[],
        action: Action::SetView(ViewMode::Split),
    },
    CommandSpec {
        id: "view.preview",
        aliases: &[],
        action: Action::SetView(ViewMode::Preview),
    },
    CommandSpec {
        id: "view.theme",
        aliases: &[],
        action: Action::ToggleTheme,
    },
    CommandSpec {
        id: "view.scrollSync",
        aliases: &[],
        action: Action::ToggleScrollSync,
    },
    CommandSpec {
        id: "view.softWrap",
        aliases: &[],
        action: Action::ToggleSoftWrap,
    },
    CommandSpec {
        id: "view.minimap",
        aliases: &["minimap"],
        action: Action::ToggleMinimap,
    },
    CommandSpec {
        id: "view.terminal",
        aliases: &[],
        action: Action::ToggleTerminal,
    },
    CommandSpec {
        id: "panel.terminal",
        aliases: &[],
        action: Action::ShowPanel(Panel::Terminal),
    },
    CommandSpec {
        id: "view.problems",
        aliases: &[],
        action: Action::ShowPanel(Panel::Problems),
    },
    CommandSpec {
        id: "language.auto",
        aliases: &[],
        action: Action::SetLanguage(None),
    },
    CommandSpec {
        id: "settings",
        aliases: &[],
        action: Action::Settings,
    },
    CommandSpec {
        id: "lsp.hover",
        aliases: &[],
        action: Action::LspHover,
    },
    CommandSpec {
        id: "lsp.completion",
        aliases: &[],
        action: Action::LspCompletion,
    },
    CommandSpec {
        id: "lsp.definition",
        aliases: &[],
        action: Action::LspDefinition,
    },
    CommandSpec {
        id: "lsp.references",
        aliases: &[],
        action: Action::LspReferences,
    },
    CommandSpec {
        id: "lsp.rename",
        aliases: &[],
        action: Action::LspRename,
    },
    CommandSpec {
        id: "lsp.restart",
        aliases: &[],
        action: Action::LspRestart,
    },
    CommandSpec {
        id: "tabs.next",
        aliases: &[],
        action: Action::NextTab,
    },
    CommandSpec {
        id: "tabs.previous",
        aliases: &[],
        action: Action::PreviousTab,
    },
    CommandSpec {
        id: "tabs.select1",
        aliases: &[],
        action: Action::SelectTab(1),
    },
    CommandSpec {
        id: "tabs.select2",
        aliases: &[],
        action: Action::SelectTab(2),
    },
    CommandSpec {
        id: "tabs.select3",
        aliases: &[],
        action: Action::SelectTab(3),
    },
    CommandSpec {
        id: "tabs.select4",
        aliases: &[],
        action: Action::SelectTab(4),
    },
    CommandSpec {
        id: "tabs.select5",
        aliases: &[],
        action: Action::SelectTab(5),
    },
    CommandSpec {
        id: "tabs.select6",
        aliases: &[],
        action: Action::SelectTab(6),
    },
    CommandSpec {
        id: "tabs.select7",
        aliases: &[],
        action: Action::SelectTab(7),
    },
    CommandSpec {
        id: "tabs.select8",
        aliases: &[],
        action: Action::SelectTab(8),
    },
    CommandSpec {
        id: "tabs.select9",
        aliases: &[],
        action: Action::SelectTab(9),
    },
];

impl Action {
    /// Canonical id, or `None` for actions whose identity is data (recent
    /// paths, named languages) or outside the table (shift-extending motions,
    /// tab slots past 9).
    pub fn id(&self) -> Option<&'static str> {
        COMMANDS
            .iter()
            .find(|spec| spec.action == *self)
            .map(|spec| spec.id)
    }

    /// Resolves a canonical id or alias. Unknown ids stay `None`; overrides
    /// depend on that so a typo cannot bind a random command.
    pub fn from_id(id: &str) -> Option<Action> {
        COMMANDS
            .iter()
            .find(|spec| spec.id == id)
            .or_else(|| COMMANDS.iter().find(|spec| spec.aliases.contains(&id)))
            .map(|spec| spec.action.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        for (index, spec) in COMMANDS.iter().enumerate() {
            assert!(
                !COMMANDS[..index].iter().any(|other| other.id == spec.id),
                "duplicate id {}",
                spec.id
            );
            assert_eq!(
                Action::from_id(spec.id),
                Some(spec.action.clone()),
                "{}",
                spec.id
            );
            assert_eq!(spec.action.id(), Some(spec.id), "{}", spec.id);
        }
    }

    #[test]
    fn goto_line_id_round_trips() {
        assert_eq!(Action::GotoLine.id(), Some("edit.gotoLine"));
        assert_eq!(Action::from_id("edit.gotoLine"), Some(Action::GotoLine));
    }

    #[test]
    fn aliases_resolve_without_shadowing_ids() {
        for spec in COMMANDS {
            for alias in spec.aliases {
                assert!(
                    !COMMANDS.iter().any(|other| other.id == *alias),
                    "alias {alias} shadows a canonical id"
                );
                assert_eq!(
                    Action::from_id(alias),
                    Some(spec.action.clone()),
                    "alias {alias}"
                );
            }
        }
    }

    #[test]
    fn legacy_keymap_ids_still_resolve() {
        // The exact table `keymap::action_from_id` accepted before the move.
        let cases: &[(&str, Action)] = &[
            ("file.new", Action::New),
            ("new", Action::New),
            ("file.newFile", Action::NewFile),
            ("file.open", Action::Open),
            ("open", Action::Open),
            ("file.openFolder", Action::OpenFolder),
            ("file.openProject", Action::OpenFolder),
            ("file.closeFolder", Action::CloseFolder),
            ("file.closeProject", Action::CloseFolder),
            ("file.save", Action::Save),
            ("save", Action::Save),
            ("file.saveAs", Action::SaveAs),
            ("file.closeTab", Action::CloseTab),
            ("edit.undo", Action::Undo),
            ("undo", Action::Undo),
            ("edit.redo", Action::Redo),
            ("redo", Action::Redo),
            ("edit.cut", Action::Cut),
            ("cut", Action::Cut),
            ("edit.copy", Action::Copy),
            ("copy", Action::Copy),
            ("edit.paste", Action::Paste),
            ("paste", Action::Paste),
            ("edit.selectAll", Action::SelectAll),
            ("selectAll", Action::SelectAll),
            ("edit.selectNextOccurrence", Action::SelectNextOccurrence),
            ("selectNextOccurrence", Action::SelectNextOccurrence),
            ("edit.deleteLine", Action::DeleteLine),
            ("deleteLine", Action::DeleteLine),
            ("edit.duplicateLine", Action::DuplicateLine),
            ("duplicateLine", Action::DuplicateLine),
            ("edit.moveLineUp", Action::MoveLineUp),
            ("moveLineUp", Action::MoveLineUp),
            ("edit.moveLineDown", Action::MoveLineDown),
            ("moveLineDown", Action::MoveLineDown),
            ("edit.deleteWordBackward", Action::DeleteWordBackward),
            ("deleteWordBackward", Action::DeleteWordBackward),
            ("edit.deleteWordForward", Action::DeleteWordForward),
            ("deleteWordForward", Action::DeleteWordForward),
            ("edit.toggleComment", Action::ToggleComment),
            ("toggleComment", Action::ToggleComment),
            ("motion.lineStart", Action::MoveLineStart(false)),
            ("motion.lineEnd", Action::MoveLineEnd(false)),
            ("motion.docStart", Action::MoveDocStart(false)),
            ("motion.docEnd", Action::MoveDocEnd(false)),
            ("editor.format", Action::FormatDocument),
            ("find.find", Action::Find),
            ("find.replace", Action::FindReplace),
            ("find.next", Action::FindNext),
            ("find.previous", Action::FindPrevious),
            ("quickOpen", Action::QuickOpen),
            ("projectSearch", Action::ProjectSearch),
            ("view.cycle", Action::CycleView),
            ("view.sidebar", Action::ToggleSidebar),
            ("view.explorer", Action::ShowPanel(Panel::Explorer)),
            ("view.git", Action::ShowPanel(Panel::Git)),
            ("view.outline", Action::ToggleOutline),
            ("view.theme", Action::ToggleTheme),
            ("view.scrollSync", Action::ToggleScrollSync),
            ("view.softWrap", Action::ToggleSoftWrap),
            ("view.minimap", Action::ToggleMinimap),
            ("view.terminal", Action::ToggleTerminal),
            ("view.problems", Action::ShowPanel(Panel::Problems)),
            ("settings", Action::Settings),
            ("lsp.hover", Action::LspHover),
            ("lsp.completion", Action::LspCompletion),
            ("lsp.definition", Action::LspDefinition),
            ("lsp.references", Action::LspReferences),
            ("lsp.rename", Action::LspRename),
            ("lsp.restart", Action::LspRestart),
            ("tabs.next", Action::NextTab),
            ("tabs.previous", Action::PreviousTab),
            ("tabs.select1", Action::SelectTab(1)),
            ("tabs.select2", Action::SelectTab(2)),
            ("tabs.select3", Action::SelectTab(3)),
            ("tabs.select4", Action::SelectTab(4)),
            ("tabs.select5", Action::SelectTab(5)),
            ("tabs.select6", Action::SelectTab(6)),
            ("tabs.select7", Action::SelectTab(7)),
            ("tabs.select8", Action::SelectTab(8)),
            ("tabs.select9", Action::SelectTab(9)),
        ];
        for (id, action) in cases {
            assert_eq!(Action::from_id(id), Some(action.clone()), "{id}");
        }
    }

    #[test]
    fn header_panel_ids_stay_distinct_from_view_toggles() {
        assert_eq!(
            Action::from_id("view.terminal"),
            Some(Action::ToggleTerminal)
        );
        assert_eq!(
            Action::from_id("panel.terminal"),
            Some(Action::ShowPanel(Panel::Terminal))
        );
        assert_eq!(Action::from_id("view.outline"), Some(Action::ToggleOutline));
        assert_eq!(
            Action::from_id("panel.outline"),
            Some(Action::ShowPanel(Panel::Outline))
        );
        assert_eq!(
            Action::from_id("view.problems"),
            Some(Action::ShowPanel(Panel::Problems))
        );
    }

    #[test]
    fn dynamic_actions_have_no_stable_id() {
        let dynamic = [
            Action::OpenRecentProject("/tmp/project".into()),
            Action::OpenRecentFile("/tmp/file.md".into()),
            Action::SetLanguage(Some("Rust".into())),
            Action::SelectTab(0),
            Action::SelectTab(10),
            Action::MoveLineStart(true),
        ];
        for action in &dynamic {
            assert_eq!(action.id(), None, "{action:?}");
        }
        // No table id or alias may resolve back to a data-carrying action.
        for spec in COMMANDS {
            for id in std::iter::once(spec.id).chain(spec.aliases.iter().copied()) {
                let resolved = Action::from_id(id).expect("table id resolves");
                for action in &dynamic {
                    assert_ne!(&resolved, action, "{id}");
                }
            }
        }
        assert_eq!(Action::SetLanguage(None).id(), Some("language.auto"));
    }
}
