//! Commands from the native macOS shell (Finder open, Dock menu, quit) into
//! the app loop. Native callbacks only enqueue; `EditorApp::update` drains the
//! queue on the main thread, so they never touch UI state directly.

use std::cell::RefCell;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCommand {
    New,
    NewFile,
    Open,
    OpenFolder,
    CloseFolder,
    Save,
    SaveAs,
    CloseTab,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindReplace,
    FindNext,
    FindPrevious,
    QuickOpen,
    ProjectSearch,
    ToggleSidebar,
    ToggleOutline,
    ToggleTheme,
    SetViewEdit,
    SetViewSplit,
    SetViewPreview,
    ToggleScrollSync,
    ToggleSoftWrap,
    ToggleTerminal,
    Settings,
    Quit,
    Minimize,
    Zoom,
    GotoLine,
}

impl NativeCommand {
    /// Stable tags for the AppKit menu target.
    pub const fn tag(self) -> isize {
        self as isize
    }

    pub fn from_tag(tag: isize) -> Option<Self> {
        Some(match tag {
            0 => Self::New,
            1 => Self::NewFile,
            2 => Self::Open,
            3 => Self::OpenFolder,
            4 => Self::CloseFolder,
            5 => Self::Save,
            6 => Self::SaveAs,
            7 => Self::CloseTab,
            8 => Self::Undo,
            9 => Self::Redo,
            10 => Self::Cut,
            11 => Self::Copy,
            12 => Self::Paste,
            13 => Self::SelectAll,
            14 => Self::Find,
            15 => Self::FindReplace,
            16 => Self::FindNext,
            17 => Self::FindPrevious,
            18 => Self::QuickOpen,
            19 => Self::ProjectSearch,
            20 => Self::ToggleSidebar,
            21 => Self::ToggleOutline,
            22 => Self::ToggleTheme,
            23 => Self::SetViewEdit,
            24 => Self::SetViewSplit,
            25 => Self::SetViewPreview,
            26 => Self::ToggleScrollSync,
            27 => Self::ToggleSoftWrap,
            28 => Self::ToggleTerminal,
            29 => Self::Settings,
            30 => Self::Quit,
            31 => Self::Minimize,
            32 => Self::Zoom,
            33 => Self::GotoLine,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeRequest {
    /// Files/folders from Finder, `open -a`, or the Dock recent menu.
    Open(Vec<PathBuf>),
    NewFile,
    ClearRecent,
    /// A command selected from the native macOS menu.
    Command(NativeCommand),
}

thread_local! {
    static QUEUE: RefCell<Vec<NativeRequest>> = const { RefCell::new(Vec::new()) };
}

pub fn push(request: NativeRequest) {
    QUEUE.with(|queue| queue.borrow_mut().push(request));
}

pub fn take() -> Vec<NativeRequest> {
    QUEUE.with(|queue| std::mem::take(&mut *queue.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_command_tags_round_trip() {
        for command in [
            NativeCommand::New,
            NativeCommand::Open,
            NativeCommand::Save,
            NativeCommand::Find,
            NativeCommand::GotoLine,
            NativeCommand::SetViewSplit,
            NativeCommand::Settings,
            NativeCommand::Quit,
            NativeCommand::Zoom,
        ] {
            assert_eq!(NativeCommand::from_tag(command.tag()), Some(command));
        }
        assert_eq!(NativeCommand::from_tag(999), None);
    }
}
