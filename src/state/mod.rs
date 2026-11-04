//! Shared app state written by input callbacks and read by painters.
//!
//! The tree is rebuilt only when the shell *structure* changes (sidebar, find
//! bar, view mode); everything else is a value here. Painters borrow this state,
//! so hit rectangles are recorded during paint and consumed by the next input
//! event — the standard immediate-mode trade.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use igui_core::{Key, NodeId, Rect, Vec2, ViewportSize};
use igui_ui::{Clipboard, TextEdit};

use crate::keymap::{KeyChord, Keymap, KeymapId, Resolution, Scope};
use crate::model::brackets::match_bracket;
use crate::model::doc::Document;
use crate::model::find::{find_all, Matcher};
use crate::model::git::{
    GitCommit, GitCommitDetail, GitDiff, GitEvent, GitRequest, GitWorker, RepoStatus,
};
use crate::model::highlight::HighlightCache;
use crate::model::lsp::{self as model_lsp, LspController};
use crate::model::pty::PtySession;
use crate::model::recovery::{RecoveryEntry, RecoveryStore};
use crate::model::replace::{self, PlannedFile};
use crate::model::search::{SearchHit, SearchMessage, SearchWorker};
use crate::model::session::{Session, WindowGeometry};
use crate::model::workspace_index::{display_path, FileIndex};
use crate::model::Workspace;
use crate::services::{ServiceRegistry, SidebarContribution};
use crate::settings::{EditorMetrics, Row, Settings};
use crate::syntax::{SyntaxProvider, SyntaxRegistry, TokenKind};
use crate::theme::{Mode, Theme};
use crate::ui::preview::PreviewUi;

/// Deadline scheduling: the shared timing constants and the next wakeup.
pub(crate) mod schedule;

// Domain implementations of [`AppState`]. The struct and its constructors stay
// in this module; each child owns one concern so no single file becomes a god
// object. Fields remain private to `state`, which child modules may access.
mod actions;
mod files;
mod find;
mod git;
mod keymap;
mod lsp;
mod modal;
mod new_file;
mod quick_open;
mod reliability;
mod search;
mod services;
mod settings;
mod tabs;
mod terminal;
mod view;
mod workspace;

pub const MAX_RECENT: usize = 8;

// -- menus & actions ---------------------------------------------------------

/// Menus in the menu bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Menu {
    File,
    Edit,
    View,
    Language,
}

impl Menu {
    pub const ALL: [Menu; 3] = [Menu::File, Menu::Edit, Menu::View];

    pub const fn label(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Language => "Language",
        }
    }
}

/// Which custom macOS header traffic light is under the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderButton {
    Close,
    Minimize,
    Zoom,
}

/// How the active document is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    /// Text editor only.
    #[default]
    Edit,
    /// Editor left, rendered Markdown right.
    Split,
    /// Rendered Markdown only.
    Preview,
}

impl ViewMode {
    pub const fn label(self) -> &'static str {
        match self {
            ViewMode::Edit => "Editor",
            ViewMode::Split => "Split Preview",
            ViewMode::Preview => "Preview Only",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ViewMode::Edit => ViewMode::Split,
            ViewMode::Split => ViewMode::Preview,
            ViewMode::Preview => ViewMode::Edit,
        }
    }

    /// Markdown sources open in split preview; everything else in the editor.
    pub fn for_path(path: Option<&Path>) -> Self {
        let markdown = path
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "md" | "markdown" | "mdown" | "mkd"
                )
            });
        if markdown {
            ViewMode::Split
        } else {
            ViewMode::Edit
        }
    }
}

/// Per-document UI state: view mode and split ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocUi {
    pub view: ViewMode,
    pub split_ratio: f32,
}

/// Every command the app can run, from a menu item or a shortcut.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
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
    CycleView,
    DeleteLine,
    DuplicateLine,
    MoveLineUp,
    MoveLineDown,
    DeleteWordBackward,
    DeleteWordForward,
    ToggleComment,
    LspHover,
    LspCompletion,
    LspDefinition,
    ToggleTerminal,
    Settings,
    ToggleSidebar,
    ToggleOutline,
    ToggleTheme,
    SetView(ViewMode),
    ToggleScrollSync,
    ToggleSoftWrap,
    SetLanguage(Option<String>),
    OpenRecent(PathBuf),
    ClearRecent,
    NextTab,
    PreviousTab,
    /// Selects a tab by 1-based slot; slot 9 means "last tab".
    SelectTab(usize),
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Action::New => "New Buffer".into(),
            Action::NewFile => "New File…".into(),
            Action::Open => "Open…".into(),
            Action::OpenFolder => "Open Folder…".into(),
            Action::CloseFolder => "Close Folder".into(),
            Action::Save => "Save".into(),
            Action::SaveAs => "Save As…".into(),
            Action::CloseTab => "Close Tab".into(),
            Action::Undo => "Undo".into(),
            Action::Redo => "Redo".into(),
            Action::Cut => "Cut".into(),
            Action::Copy => "Copy".into(),
            Action::Paste => "Paste".into(),
            Action::SelectAll => "Select All".into(),
            Action::Find => "Find…".into(),
            Action::FindReplace => "Find and Replace…".into(),
            Action::FindNext => "Find Next".into(),
            Action::FindPrevious => "Find Previous".into(),
            Action::QuickOpen => "Quick Open…".into(),
            Action::ProjectSearch => "Search in Folder…".into(),
            Action::CycleView => "Cycle View".into(),
            Action::DeleteLine => "Delete Line".into(),
            Action::DuplicateLine => "Duplicate Line".into(),
            Action::MoveLineUp => "Move Line Up".into(),
            Action::MoveLineDown => "Move Line Down".into(),
            Action::DeleteWordBackward => "Delete Word Backward".into(),
            Action::DeleteWordForward => "Delete Word Forward".into(),
            Action::ToggleComment => "Toggle Comment".into(),
            Action::LspHover => "Show Hover".into(),
            Action::LspCompletion => "Complete".into(),
            Action::LspDefinition => "Go to Definition".into(),
            Action::ToggleTerminal => "Terminal".into(),
            Action::Settings => "Settings…".into(),
            Action::ToggleSidebar => "Toggle Sidebar".into(),
            Action::ToggleOutline => "Outline".into(),
            Action::ToggleTheme => "Toggle Theme".into(),
            Action::SetView(mode) => mode.label().into(),
            Action::ToggleScrollSync => "Sync Scroll".into(),
            Action::ToggleSoftWrap => "Soft Wrap".into(),
            Action::SetLanguage(None) => "Auto (by extension)".into(),
            Action::SetLanguage(Some(name)) => name.clone(),
            Action::ClearRecent => "Clear Recent".into(),
            Action::NextTab => "Next Tab".into(),
            Action::PreviousTab => "Previous Tab".into(),
            Action::SelectTab(slot) => format!("Tab {slot}"),
            Action::OpenRecent(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("Recent: {name}")
            }
        }
    }

    pub fn enabled(&self, state: &AppState) -> bool {
        let doc = state.ws.active_doc();
        match self {
            Action::New
            | Action::NewFile
            | Action::Open
            | Action::OpenFolder
            | Action::ToggleSidebar
            | Action::ToggleOutline
            | Action::ToggleTheme
            | Action::Find
            | Action::FindReplace
            | Action::Settings
            | Action::ToggleScrollSync
            | Action::ToggleTerminal => true,
            Action::CloseFolder => state.ws.folder_open,
            Action::QuickOpen | Action::ProjectSearch => state.ws.folder_open,
            Action::Save => doc.is_some_and(|doc| !doc.is_read_only()),
            Action::SaveAs => doc.is_some_and(|doc| !doc.is_read_only()),
            Action::CloseTab => doc.is_some(),
            Action::Undo => doc.is_some_and(|doc| !doc.is_read_only() && doc.can_undo()),
            Action::Redo => doc.is_some_and(|doc| !doc.is_read_only() && doc.can_redo()),
            Action::Cut => doc.is_some_and(|doc| !doc.is_read_only()),
            Action::Copy => doc.is_some(),
            Action::Paste => {
                doc.is_some_and(|doc| !doc.is_read_only()) && state.clipboard.is_some()
            }
            Action::SelectAll => doc.is_some(),
            Action::FindNext | Action::FindPrevious => doc.is_some(),
            Action::CycleView => doc.is_some(),
            Action::DeleteLine
            | Action::DuplicateLine
            | Action::MoveLineUp
            | Action::MoveLineDown
            | Action::DeleteWordBackward
            | Action::DeleteWordForward
            | Action::ToggleComment => doc.is_some_and(|doc| !doc.is_read_only()),
            Action::LspHover | Action::LspCompletion | Action::LspDefinition => doc
                .and_then(|doc| doc.path())
                .is_some_and(|path| state.lsp.supports(path)),
            Action::SetView(_) => doc.is_some(),
            Action::ToggleSoftWrap => doc.is_some(),
            Action::SetLanguage(_) => doc.is_some(),
            Action::OpenRecent(path) => path.is_file(),
            Action::ClearRecent => !state.session.recent.is_empty(),
            Action::NextTab | Action::PreviousTab => state.ws.tabs.len() > 1,
            Action::SelectTab(slot) => {
                *slot >= 1
                    && !state.ws.tabs.is_empty()
                    && (*slot >= 9 || *slot <= state.ws.tabs.len())
            }
        }
    }

    pub fn checked(&self, state: &AppState) -> bool {
        match self {
            Action::SetView(mode) => *mode == state.view_of_active(),
            Action::ToggleScrollSync => state.sync_scroll,
            Action::ToggleOutline => state.show_outline,
            Action::ToggleSoftWrap => state.session.soft_wrap,
            Action::ToggleTerminal => state.terminal.open,
            Action::SetLanguage(None) => state.language_override_of_active().is_none(),
            Action::SetLanguage(Some(name)) => {
                state.language_override_of_active() == Some(name.as_str())
            }
            _ => false,
        }
    }
}

/// Items of one menu; `None` is a separator.
pub fn items(menu: Menu, state: &AppState) -> Vec<Option<Action>> {
    match menu {
        Menu::File => {
            let mut items = vec![
                Some(Action::New),
                Some(Action::NewFile),
                Some(Action::Open),
                Some(Action::OpenFolder),
                Some(Action::CloseFolder),
                None,
                Some(Action::Save),
                Some(Action::SaveAs),
                Some(Action::CloseTab),
            ];
            if !state.session.recent.is_empty() {
                items.push(None);
                for path in state.session.recent.iter().take(MAX_RECENT) {
                    items.push(Some(Action::OpenRecent(path.clone())));
                }
                items.push(None);
                items.push(Some(Action::ClearRecent));
            }
            items
        }
        Menu::Edit => vec![
            Some(Action::Undo),
            Some(Action::Redo),
            None,
            Some(Action::Cut),
            Some(Action::Copy),
            Some(Action::Paste),
            None,
            Some(Action::SelectAll),
            None,
            Some(Action::Find),
            Some(Action::QuickOpen),
            Some(Action::ProjectSearch),
        ],
        Menu::Language => {
            let mut items = vec![Some(Action::SetLanguage(None))];
            for name in state.syntax.names() {
                items.push(Some(Action::SetLanguage(Some(name.to_string()))));
            }
            items
        }
        Menu::View => vec![
            Some(Action::ToggleSidebar),
            Some(Action::ToggleOutline),
            Some(Action::ToggleTheme),
            None,
            Some(Action::SetView(ViewMode::Edit)),
            Some(Action::SetView(ViewMode::Split)),
            Some(Action::SetView(ViewMode::Preview)),
            None,
            Some(Action::ToggleScrollSync),
            Some(Action::ToggleSoftWrap),
            Some(Action::ToggleTerminal),
            Some(Action::Settings),
        ],
    }
}

// -- input focus & overlays --------------------------------------------------

/// Who receives plain keys / text / IME.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Focus {
    #[default]
    Editor,
    Find,
    Replace,
    Terminal,
}

/// What the pointer is currently dragging inside the editor.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Drag {
    #[default]
    None,
    /// Selecting text; the pointer position sets the selection end.
    Select,
    /// Dragging the vertical scrollbar by `grab` pixels from the thumb top.
    VScroll { grab: f32 },
    /// Dragging the horizontal scrollbar.
    HScroll { grab: f32 },
}

/// Widgets of the find/replace bar (hit targets).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindHit {
    FindField,
    ReplaceField,
    Case,
    Word,
    Prev,
    Next,
    ReplaceOne,
    ReplaceAll,
    Close,
}

/// Find / replace bar state: queries are `TextEdit` (the engine's pure text
/// state), matches are recomputed from the active document.
pub struct FindState {
    pub open: bool,
    pub query: TextEdit,
    pub replace: TextEdit,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub matches: Vec<(usize, usize)>,
    pub current: Option<usize>,
    /// Set when the query or the document changed since the last match pass.
    pub dirty: bool,
    /// Revision the matches were computed from.
    pub rev: u64,
    /// Bar geometry and hit targets from the last paint.
    pub hits: Vec<(FindHit, Rect)>,
    pub preedit: Option<String>,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            open: false,
            query: TextEdit::new(""),
            replace: TextEdit::new(""),
            case_sensitive: false,
            whole_word: false,
            matches: Vec::new(),
            current: None,
            dirty: false,
            rev: u64::MAX,
            hits: Vec::new(),
            preedit: None,
        }
    }
}

/// What needs confirming before it happens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modal {
    /// Close a dirty tab: Save / Discard / Cancel.
    CloseDirtyTab { tab: usize },
    /// Close several tabs after a context-menu action: Cancel / Discard.
    CloseTabs { keep: Option<usize>, from: usize },
    /// The file changed on disk while the buffer is dirty: Keep / Reload.
    ExternalChange { tab: usize },
    /// macOS Cmd+Q with dirty tabs: Cancel / Discard & Quit.
    Quit { dirty: usize },
    /// Close the folder and all its open tabs: Cancel / Discard.
    CloseFolder { dirty: usize },
    /// Replace every match in the scanned files: Cancel / Replace all.
    ReplaceInFiles { files: usize, occurrences: usize },
}

/// Buttons of the confirmation modal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalButton {
    Cancel,
    Discard,
    Save,
    /// Confirms a replace-in-files plan.
    Apply,
    /// Reloads a dirty buffer after an external change.
    Reload,
}

/// A dialog request, executed in `update` (outside input dispatch, on the main
/// thread) so a blocking native dialog never runs inside engine callbacks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogKind {
    Open,
    OpenFolder,
    SaveAs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DialogRequest {
    pub kind: DialogKind,
    /// Close this tab after a successful Save As (confirm-modal flow).
    pub then_close: Option<usize>,
}

/// Pending mouse-hover request (delayed tooltip).
#[derive(Clone, Copy, Debug)]
pub struct HoverTarget {
    pub doc: u64,
    pub byte: usize,
    pub anchor: Vec2,
    pub since: Instant,
    pub requested: bool,
}

/// Editor geometry captured by the last paint.
#[derive(Clone, Debug, Default)]
pub struct EditorUi {
    /// Text area (excludes gutter and both scrollbars).
    pub text_rect: Rect,
    pub gutter_w: f32,
    /// Caret rectangle in viewport space (IME candidate placement).
    pub caret_rect: Rect,
    pub drag: Drag,
    pub vbar: Rect,
    pub vthumb: Rect,
    pub hbar: Rect,
    pub hthumb: Rect,
    /// Content extents used to clamp scrolling.
    pub content_h: f32,
    pub content_w: f32,
    /// Width of the visible text area.
    pub view_w: f32,
    pub view_h: f32,
    /// In-progress IME composition (text, optional cursor range).
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    /// Last word-selecting double click, to recognize a triple click.
    pub last_double: Option<(Instant, Vec2)>,
    /// Scroll the caret into view on the next paint (set by input, not by
    /// wheel scrolling).
    pub reveal: bool,
    /// Clickable rows of the welcome screen (no document open).
    pub welcome_actions: Vec<(Action, Rect)>,
    /// Soft-wrap rows for the current document/width (rebuilt by paint).
    pub wrap: crate::model::wrap::WrapLayout,
    /// Horizontal scroll saved when wrap turned on, restored when it turns off.
    pub wrap_scroll_x: f32,
    /// Fixed-pitch family resolved for code text (`None` before the probe or
    /// when no fixed-pitch face is installed).
    pub font: Option<String>,
    /// Whether [`EditorUi::font`] has been probed (needs the backend measurer).
    pub font_checked: bool,
}

/// Sidebar geometry captured by the last paint.
#[derive(Clone, Debug, Default)]
pub struct SidebarUi {
    pub rect: Rect,
    pub scroll: f32,
    pub hover: Option<usize>,
    pub selected: Option<PathBuf>,
    /// Empty-state actions (no folder open).
    pub actions: Vec<(Action, Rect)>,
    /// Sidebar tab hit rectangles (tab id, rect).
    pub tabs: Vec<(&'static str, Rect)>,
}

/// Right-side document outline (LSP document symbols).
#[derive(Clone, Debug, Default)]
pub struct OutlineUi {
    pub rect: Rect,
    pub handle_rect: Rect,
    pub scroll: f32,
    pub scroll_max: f32,
    pub hover: Option<usize>,
    /// Visible row index and its hit rectangle, recorded during paint.
    pub rows: Vec<(usize, Rect)>,
}

/// LSP problems popup, anchored above the status bar.
#[derive(Clone, Debug, Default)]
pub struct ProblemsUi {
    pub open: bool,
    pub hover: Option<usize>,
    pub rect: Rect,
    /// Visible diagnostic index and its hit rectangle, recorded during paint.
    pub hits: Vec<(usize, Rect)>,
    pub scroll: f32,
    pub scroll_max: f32,
}

/// State for the single synthetic Godot documentation tab.
#[derive(Clone, Debug, Default)]
pub struct DocsUi {
    pub class: String,
    pub symbol: String,
    /// Byte-range links keyed by document id.
    pub links: HashMap<u64, Vec<(usize, usize, crate::model::bbcode::DocLink)>>,
}

/// Geometry/scroll state for the dedicated documentation view control.
#[derive(Clone, Debug, Default)]
pub struct DocsViewUi {
    pub rect: Rect,
    pub body: Rect,
    pub scroll: f32,
    pub scroll_max: f32,
    pub wrap: crate::model::wrap::WrapLayout,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GitMode {
    Changes,
    History,
}

/// Built-in terminal panel/session.
pub struct TerminalState {
    pub open: bool,
    pub focused: bool,
    pub session: Option<PtySession>,
    pub parser: Option<vt100::Parser>,
    pub rect: Rect,
    /// Last grid handed to the parser/PTY; also the spawn size for the next
    /// session. Never treated as the source of truth (the parser's screen is).
    pub rows: u16,
    pub cols: u16,
    /// Fixed-pitch family resolved from [`crate::fonts::MONO_FAMILIES`].
    pub font: Option<String>,
    /// One column's advance in pixels, as last painted.
    pub cell_w: f32,
    /// Whether [`TerminalState::font`] has been probed (needs the backend
    /// measurer, which is only available during painting).
    pub font_checked: bool,
    pub cwd: PathBuf,
    pub caret_rect: Rect,
    pub height: f32,
    pub node: Option<NodeId>,
    pub drag: Option<(f32, f32)>,
    pub handle_rect: Rect,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            open: false,
            focused: false,
            session: None,
            parser: None,
            rect: Rect::ZERO,
            rows: 24,
            cols: 80,
            font: None,
            font_checked: false,
            cell_w: 0.0,
            cwd: PathBuf::from("."),
            caret_rect: Rect::ZERO,
            height: 220.0,
            node: None,
            drag: None,
            handle_rect: Rect::ZERO,
        }
    }
}

/// Git panel / diff state. The worker owns the child process; the UI owns only
/// immutable snapshots.
pub struct GitState {
    pub worker: Option<GitWorker>,
    pub status: RepoStatus,
    pub loading: bool,
    pub generation: u64,
    pub diff_generation: u64,
    pub cwd: Option<PathBuf>,
    pub pending: bool,
    pub selected: Option<PathBuf>,
    pub diff: Option<GitDiff>,
    pub diff_loading: bool,
    pub diff_visible: bool,
    pub diff_scroll: f32,
    pub rows: Vec<(PathBuf, bool, Rect, Rect)>,
    pub refresh_rect: Rect,
    pub stage_all_rect: Rect,
    pub commit_rect: Rect,
    pub scroll: f32,
    pub scroll_max: f32,
    pub error: Option<String>,
    pub mode: GitMode,
    pub commits: Vec<GitCommit>,
    pub history_generation: u64,
    /// A `git log` request is in flight (drives frame polling).
    pub history_loading: bool,
    pub history_rows: Vec<(String, Rect)>,
    pub history_rect: Rect,
    pub changes_rect: Rect,
    pub detail: Option<GitCommitDetail>,
    pub detail_generation: u64,
    pub detail_visible: bool,
    pub detail_loading: bool,
    pub detail_scroll: f32,
    pub hover: Option<Rect>,
    pub write_generation: u64,
    pub commit_open: bool,
    pub commit_message: TextEdit,
    pub commit_error: Option<String>,
}

impl Default for GitState {
    fn default() -> Self {
        Self {
            worker: None,
            status: RepoStatus::default(),
            loading: false,
            generation: 0,
            diff_generation: 0,
            cwd: None,
            pending: false,
            selected: None,
            diff: None,
            diff_loading: false,
            diff_visible: false,
            diff_scroll: 0.0,
            rows: Vec::new(),
            refresh_rect: Rect::ZERO,
            stage_all_rect: Rect::ZERO,
            commit_rect: Rect::ZERO,
            scroll: 0.0,
            scroll_max: 0.0,
            error: None,
            mode: GitMode::Changes,
            commits: Vec::new(),
            history_generation: 0,
            history_loading: false,
            history_rows: Vec::new(),
            history_rect: Rect::ZERO,
            changes_rect: Rect::ZERO,
            detail: None,
            detail_generation: 0,
            detail_visible: false,
            detail_loading: false,
            detail_scroll: 0.0,
            hover: None,
            write_generation: 0,
            commit_open: false,
            commit_message: TextEdit::new(""),
            commit_error: None,
        }
    }
}

/// Tab strip hit rectangles captured by the last paint.
#[derive(Clone, Debug, Default)]
pub struct TabsUi {
    /// `(tab index, whole tab, close button)`.
    pub hits: Vec<(usize, Rect, Rect)>,
    pub hover: Option<usize>,
    /// The `+` (new tab) button.
    pub plus: Rect,
    /// Horizontal scroll offset when the tabs do not fit.
    pub scroll: f32,
    /// The strip rectangle (scroll/hit diagnostics).
    pub strip: Rect,
    /// Scroll so the active tab is visible on the next paint (set by tab
    /// switches/new/close, not by manual wheel scrolling).
    pub reveal_active: bool,
}

/// In-progress tab drag (reorder).
#[derive(Clone, Copy, Debug)]
pub struct TabDrag {
    pub from: usize,
    pub start: Vec2,
    /// Insertion gap in the *current* layout (before removal).
    pub marker: usize,
    /// Final index after removal/insertion.
    pub insertion: usize,
    pub active: bool,
}

/// Disk identity used for external-change detection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DiskStamp {
    len: u64,
    modified: u128,
}

/// Actions in a tab's right-click menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabMenuAction {
    Close,
    CloseOthers,
    CloseRight,
    CopyPath,
    RevealInSidebar,
}

impl TabMenuAction {
    pub const ALL: [TabMenuAction; 5] = [
        TabMenuAction::Close,
        TabMenuAction::CloseOthers,
        TabMenuAction::CloseRight,
        TabMenuAction::CopyPath,
        TabMenuAction::RevealInSidebar,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            TabMenuAction::Close => "Close",
            TabMenuAction::CloseOthers => "Close Others",
            TabMenuAction::CloseRight => "Close to the Right",
            TabMenuAction::CopyPath => "Copy Path",
            TabMenuAction::RevealInSidebar => "Reveal in Sidebar",
        }
    }
}

/// Tab context-menu overlay state.
#[derive(Clone, Debug)]
pub struct TabMenuUi {
    pub tab: usize,
    /// Pointer position where the menu opened (anchor).
    pub at: Vec2,
    pub rect: Rect,
    pub hits: Vec<(TabMenuAction, Rect)>,
    pub hover: Option<usize>,
}

/// Actions in a file-tree row's right-click menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeMenuAction {
    Open,
    NewFile,
    CopyPath,
    Reveal,
}

impl TreeMenuAction {
    pub const ALL: [TreeMenuAction; 4] = [
        TreeMenuAction::Open,
        TreeMenuAction::NewFile,
        TreeMenuAction::CopyPath,
        TreeMenuAction::Reveal,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            TreeMenuAction::Open => "Open",
            TreeMenuAction::NewFile => "New File…",
            TreeMenuAction::CopyPath => "Copy Path",
            TreeMenuAction::Reveal => "Reveal in Finder",
        }
    }
}

/// File-tree context-menu overlay state.
#[derive(Clone, Debug)]
pub struct TreeMenuUi {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub at: Vec2,
    pub rect: Rect,
    pub hits: Vec<(TreeMenuAction, Rect)>,
    pub hover: Option<usize>,
}

/// A planned replace-in-files run (applied from the confirmation modal).
#[derive(Clone, Debug)]
pub struct ReplaceJob {
    pub plans: Vec<PlannedFile>,
    pub replacement: String,
    pub files: usize,
    pub occurrences: usize,
}

/// Quick Open palette (`⌘P`).
pub struct QuickOpenState {
    pub open: bool,
    pub query: TextEdit,
    pub selected: usize,
    pub scroll: usize,
    pub results: Vec<PathBuf>,
    pub index: Option<FileIndex>,
    pub preedit: Option<String>,
}

/// Quick-create-file prompt (`⇧⌘N`).
pub struct NewFileState {
    pub open: bool,
    pub dir: PathBuf,
    pub name: TextEdit,
    pub error: Option<String>,
    pub preedit: Option<String>,
}

impl Default for NewFileState {
    fn default() -> Self {
        Self {
            open: false,
            dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            name: TextEdit::new("untitled.md"),
            error: None,
            preedit: None,
        }
    }
}

impl Default for QuickOpenState {
    fn default() -> Self {
        Self {
            open: false,
            query: TextEdit::new(""),
            selected: 0,
            scroll: 0,
            results: Vec::new(),
            index: None,
            preedit: None,
        }
    }
}

/// Which search field owns typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchFocus {
    #[default]
    Query,
    Replace,
}

/// Project search (`⇧⌘F`): query/replace, streamed hits and the worker.
pub struct SearchState {
    pub open: bool,
    pub query: TextEdit,
    pub replace: TextEdit,
    pub focus: SearchFocus,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    pub scroll: usize,
    pub worker: Option<SearchWorker>,
    pub scanning: bool,
    pub files_scanned: usize,
    /// Debounce timer for the next (re)start.
    pub pending: Option<Instant>,
    pub preedit: Option<String>,
    /// Geometry of the replace row (recorded in paint for hit tests).
    pub replace_rect: Rect,
    pub replace_button: Rect,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            open: false,
            query: TextEdit::new(""),
            replace: TextEdit::new(""),
            focus: SearchFocus::Query,
            case_sensitive: false,
            whole_word: false,
            hits: Vec::new(),
            selected: 0,
            scroll: 0,
            worker: None,
            scanning: false,
            files_scanned: 0,
            pending: None,
            preedit: None,
            replace_rect: Rect::ZERO,
            replace_button: Rect::ZERO,
        }
    }
}

/// The single shared state. Callbacks hold an `Rc<RefCell<AppState>>`.
pub struct AppState {
    pub theme: Theme,
    pub ws: Workspace,
    pub show_sidebar: bool,
    pub show_outline: bool,
    pub viewport: ViewportSize,
    pub sidebar: SidebarUi,
    pub outline: OutlineUi,
    pub problems: ProblemsUi,
    pub docs: DocsUi,
    pub docs_view: DocsViewUi,
    pub outline_node: Option<NodeId>,
    pub outline_drag: Option<(f32, f32)>,
    pub hover_target: Option<HoverTarget>,
    pub hover_leave_at: Option<Instant>,
    /// Debounced auto-completion activation while typing.
    pub auto_completion_at: Option<Instant>,
    pub editor: EditorUi,
    pub tabs: TabsUi,
    pub tab_drag: Option<TabDrag>,
    pub tab_menu: Option<TabMenuUi>,
    pub tree_menu: Option<TreeMenuUi>,
    pub menu_open: Option<Menu>,
    pub menubar_hover: Option<Menu>,
    pub menubar_hits: Vec<(Menu, Rect)>,
    /// Native-style window chrome painted by the header.
    pub header_close: Rect,
    pub header_minimize: Rect,
    pub header_zoom: Rect,
    /// Traffic light under the pointer, for hover feedback.
    pub header_hover: Option<HeaderButton>,
    pub menu_rect: Rect,
    /// Popup item rectangles, in item order (separators have no rect).
    pub menu_items: Vec<(Action, Rect)>,
    pub menu_hover: Option<usize>,
    /// Status-bar message and whether it is an error.
    pub message: Option<(String, bool, Instant)>,
    pub clipboard: Option<Rc<RefCell<dyn Clipboard>>>,
    /// Cross-thread "draw a frame" handle (pty output, frame clock). `None` in
    /// headless runs, where the runner drives frames itself.
    pub wake: Option<crate::wake::WakeFn>,
    /// Set when the shell must be rebuilt (sidebar, find bar, view mode).
    pub needs_rebuild: bool,
    /// View mode the shell was last built for.
    pub built_view: ViewMode,
    /// Whether the shell currently mounts the documentation view.
    pub built_docs: bool,
    /// Split ratio the shell was last built for.
    pub built_split: f32,
    /// Set when the next frame should repaint (paint cache invalidation).
    pub dirty: bool,
    // -- M2 --------------------------------------------------------------
    pub focus: Focus,
    pub find: FindState,
    pub modal: Option<Modal>,
    pub modal_buttons: Vec<(ModalButton, Rect)>,
    pub pending_dialog: Option<DialogRequest>,
    /// Per-document UI state (keyed by `Document::id`).
    pub doc_views: HashMap<u64, DocUi>,
    pub session: Session,
    pub config_path: PathBuf,
    pub config_dirty: bool,
    pub preview: PreviewUi,
    /// Window title last pushed to the platform.
    pub title: String,
    // -- M3 --------------------------------------------------------------
    pub settings: Settings,
    pub services: ServiceRegistry,
    pub sidebar_tab: String,
    pub git: GitState,
    pub terminal: TerminalState,
    pub lsp: LspController,
    pub keymap: Keymap,
    keymap_pending: Vec<KeyChord>,
    keymap_pending_at: Instant,
    keymap_overrides: Option<serde_json::Value>,
    pub vim: crate::vim::VimState,
    pub syntax: SyntaxRegistry,
    pub highlight: HighlightCache,
    /// Matched bracket byte positions for the caret (`open`, `close`).
    pub bracket: Option<(usize, usize)>,
    bracket_key: Option<(u64, u64, usize, bool)>,
    pub settings_open: bool,
    pub settings_row: usize,
    /// Node ids captured by the shell for live layout tweaks.
    pub sidebar_node: Option<NodeId>,
    /// Sidebar resize drag: `(start_pointer_x, start_width)`.
    pub sidebar_drag: Option<(f32, f32)>,
    /// Status-bar language selector hit rect (footer, bottom right).
    pub language_hit: Rect,
    pub language_hover: bool,
    /// Status-bar LSP summary chip; clicking it opens the problems popup.
    pub lsp_hit: Rect,
    pub lsp_hover: bool,
    /// Set when a settings change needs the sidebar's flex basis updated.
    pub pending_sidebar_sync: bool,
    /// A file drag is hovering the window (paints the drop overlay).
    pub drop_hover: bool,
    pub quick_open: QuickOpenState,
    pub new_file: NewFileState,
    /// Split-view nodes (for drag resizing) and the divider rectangle.
    /// Editor/preview control for the active view (vertical band anchor).
    pub content_node: Option<NodeId>,
    /// Shared vertical centre of the content band, used by both drag handles.
    pub content_center_y: Option<f32>,
    pub split_editor: Option<NodeId>,
    pub split_preview: Option<NodeId>,
    pub split_dragging: bool,
    pub split_divider: Rect,
    pub recovery: RecoveryStore,
    pub recovery_prompt: Option<Vec<RecoveryEntry>>,
    /// Planned replace-in-files job waiting for confirmation.
    pub pending_replace: Option<ReplaceJob>,
    /// Per-path language overrides (persisted) and per-document ones (untitled).
    pub language_by_path: HashMap<PathBuf, String>,
    pub language_by_id: HashMap<u64, String>,
    /// Keep editor and preview scroll positions in sync (W3).
    pub sync_scroll: bool,
    /// Last seen disk identity per open path (`None` = missing on disk).
    external: HashMap<PathBuf, Option<DiskStamp>>,
    last_external_check: Instant,
    last_editor_scroll: f32,
    last_preview_scroll: f32,
    sync_rev: u64,
    last_activity: Instant,
    doc_revs: HashMap<u64, u64>,
    pub search: SearchState,
}

impl AppState {
    pub fn new(theme: Theme, ws: Workspace, session: Session, config_path: PathBuf) -> Self {
        let title = String::new();
        let settings = settings_from_session(&session);
        let recovery_dir = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("recovery");
        let languages: HashMap<PathBuf, String> = session.languages.iter().cloned().collect();
        let sync_scroll = session.sync_scroll;
        let terminal_open = session.terminal_open;
        let terminal_height = session.terminal_height.clamp(120.0, 600.0);
        let show_outline = session.outline;
        let sidebar_tab = if session.sidebar_tab.is_empty() {
            "explorer".into()
        } else {
            session.sidebar_tab.clone()
        };
        let services = crate::services::install_builtins();
        let external = ws
            .tabs
            .iter()
            .filter_map(|doc| {
                doc.path()
                    .map(|path| (path.to_path_buf(), disk_stamp(path)))
            })
            .collect();
        let mut state = Self {
            theme,
            ws,
            show_sidebar: session.sidebar,
            show_outline,
            viewport: ViewportSize::default(),
            sidebar: SidebarUi::default(),
            outline: OutlineUi::default(),
            problems: ProblemsUi::default(),
            docs: DocsUi::default(),
            docs_view: DocsViewUi::default(),
            outline_node: None,
            outline_drag: None,
            hover_target: None,
            hover_leave_at: None,
            auto_completion_at: None,
            editor: EditorUi {
                reveal: true,
                wrap: crate::model::wrap::WrapLayout::default(),
                wrap_scroll_x: 0.0,
                ..EditorUi::default()
            },
            tabs: TabsUi {
                reveal_active: true,
                ..TabsUi::default()
            },
            tab_drag: None,
            tab_menu: None,
            tree_menu: None,
            menu_open: None,
            menubar_hover: None,
            menubar_hits: Vec::new(),
            header_close: Rect::ZERO,
            header_minimize: Rect::ZERO,
            header_zoom: Rect::ZERO,
            header_hover: None,
            menu_rect: Rect::ZERO,
            menu_items: Vec::new(),
            menu_hover: None,
            message: None,
            clipboard: None,
            wake: None,
            needs_rebuild: false,
            built_view: ViewMode::Edit,
            built_docs: false,
            built_split: 0.5,
            dirty: true,
            focus: Focus::Editor,
            find: FindState::default(),
            modal: None,
            modal_buttons: Vec::new(),
            pending_dialog: None,
            doc_views: HashMap::new(),
            session,
            config_path,
            config_dirty: false,
            preview: PreviewUi::default(),
            title,
            settings,
            services,
            sidebar_tab,
            git: GitState::default(),
            terminal: TerminalState {
                open: terminal_open,
                height: terminal_height,
                ..TerminalState::default()
            },
            lsp: LspController::default(),
            keymap: Keymap::for_id(settings.keymap),
            keymap_pending: Vec::new(),
            keymap_pending_at: Instant::now(),
            keymap_overrides: None,
            vim: {
                let mut vim = crate::vim::VimState::default();
                vim.reset_for_keymap(settings.keymap == KeymapId::Vim);
                vim
            },
            syntax: SyntaxRegistry::with_builtins(),
            highlight: HighlightCache::new(),
            bracket: None,
            bracket_key: None,
            settings_open: false,
            settings_row: 0,
            sidebar_node: None,
            sidebar_drag: None,
            language_hit: Rect::ZERO,
            language_hover: false,
            lsp_hit: Rect::ZERO,
            lsp_hover: false,
            pending_sidebar_sync: false,
            drop_hover: false,
            quick_open: QuickOpenState::default(),
            new_file: NewFileState::default(),
            search: SearchState::default(),
            content_node: None,
            content_center_y: None,
            split_editor: None,
            split_preview: None,
            split_dragging: false,
            split_divider: Rect::ZERO,
            recovery: RecoveryStore::new(recovery_dir),
            recovery_prompt: None,
            pending_replace: None,
            language_by_path: languages,
            language_by_id: HashMap::new(),
            sync_scroll,
            external,
            last_external_check: Instant::now(),
            last_editor_scroll: 0.0,
            last_preview_scroll: 0.0,
            sync_rev: u64::MAX,
            last_activity: Instant::now(),
            doc_revs: HashMap::new(),
        };
        if !state
            .services
            .sidebar_tabs
            .iter()
            .any(|tab| tab.id == state.sidebar_tab)
        {
            state.sidebar_tab = "explorer".into();
        }
        state.git.cwd = state.git_cwd_for_tab();
        state.git.pending = true;
        state
    }

    fn git_cwd_for_tab(&self) -> Option<PathBuf> {
        self.ws
            .active_doc()
            .and_then(|doc| {
                doc.path()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
    }

    pub fn status(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), false, Instant::now()));
        self.dirty = true;
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), true, Instant::now()));
        self.dirty = true;
    }

    /// Message text plus severity, if one is still worth showing.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message
            .as_ref()
            .map(|(text, error, _)| (text.as_str(), *error))
    }
}

fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Some(DiskStamp {
        len: metadata.len(),
        modified,
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Preview scroll offset (content space) for a fractional source line.
fn preview_y_for_line(anchors: &[(usize, f32)], line: f32) -> f32 {
    let base = anchors[0].1;
    let line = line.max(0.0);
    if line <= anchors[0].0 as f32 {
        return 0.0;
    }
    for pair in anchors.windows(2) {
        let (from, y0) = pair[0];
        let (to, y1) = pair[1];
        if to == from {
            continue;
        }
        if line <= to as f32 {
            let t = (line - from as f32) / (to - from) as f32;
            return (y0 + (y1 - y0) * t - base).max(0.0);
        }
    }
    (anchors.last().map_or(0.0, |anchor| anchor.1) - base).max(0.0)
}

/// Fractional source line for a preview scroll offset.
fn line_for_preview_y(anchors: &[(usize, f32)], y: f32) -> f32 {
    let base = anchors[0].1;
    let y = y + base;
    for pair in anchors.windows(2) {
        let (from, y0) = pair[0];
        let (to, y1) = pair[1];
        if y1 <= y0 {
            continue;
        }
        if y <= y1 {
            let t = (y - y0) / (y1 - y0);
            return from as f32 + (to - from) as f32 * t.clamp(0.0, 1.0);
        }
    }
    anchors.last().map_or(0.0, |anchor| anchor.0 as f32)
}

fn settings_from_session(session: &Session) -> Settings {
    Settings {
        font_size: session.font,
        tab_width: session.tab,
        line_numbers: session.line_numbers,
        indent_guides: session.indent_guides,
        syntax: session.syntax,
        autosave: session.autosave,
        sidebar_width: session.sidebar_width,
        keymap: KeymapId::from_session(&session.keymap),
    }
    .normalized()
}

fn on_off(value: bool) -> String {
    if value {
        "ON".into()
    } else {
        "OFF".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let mut ws = Workspace::new(std::env::temp_dir());
        ws.new_tab();
        AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            std::env::temp_dir().join("igui-editor-test-session.conf"),
        )
    }

    fn state_with_tabs(count: usize) -> AppState {
        let mut state = state();
        while state.ws.tabs.len() < count {
            state.ws.new_tab();
        }
        state
    }

    #[test]
    fn tab_commands_select_by_slot_and_wrap() {
        let mut state = state_with_tabs(3);
        state.run(Action::SelectTab(2));
        assert_eq!(state.ws.active, 1);
        state.run(Action::SelectTab(9));
        assert_eq!(state.ws.active, 2, "slot 9 means the last tab");
        state.run(Action::NextTab);
        assert_eq!(state.ws.active, 0, "next wraps");
        state.run(Action::PreviousTab);
        assert_eq!(state.ws.active, 2, "previous wraps");
        state.run(Action::SelectTab(8));
        assert_eq!(state.ws.active, 2, "out-of-range slots are no-ops");
        state.run(Action::SelectTab(0));
        assert_eq!(state.ws.active, 2, "slot 0 is ignored, not an underflow");
    }

    #[test]
    fn activating_a_tab_returns_focus_to_the_editor() {
        let mut state = state_with_tabs(2);
        state.focus = Focus::Find;
        state.activate_tab(0);
        assert_eq!(state.ws.active, 0);
        assert_eq!(state.focus, Focus::Editor);
        assert!(state.tabs.reveal_active);
    }

    #[test]
    fn markdown_paths_default_to_split_preview() {
        assert_eq!(
            ViewMode::for_path(Some(Path::new("notes.md"))),
            ViewMode::Split
        );
        assert_eq!(
            ViewMode::for_path(Some(Path::new("README.MARKDOWN"))),
            ViewMode::Split
        );
        assert_eq!(
            ViewMode::for_path(Some(Path::new("main.rs"))),
            ViewMode::Edit
        );
        assert_eq!(ViewMode::for_path(None), ViewMode::Edit);
    }

    #[test]
    fn view_mode_cycles() {
        assert_eq!(ViewMode::Edit.next(), ViewMode::Split);
        assert_eq!(ViewMode::Split.next(), ViewMode::Preview);
        assert_eq!(ViewMode::Preview.next(), ViewMode::Edit);
    }

    #[test]
    fn find_steps_wrap_and_select_matches() {
        let mut state = state();
        state
            .ws
            .active_doc_mut()
            .unwrap()
            .insert_text("one two one");
        state.open_find(false);
        state.find.query.insert("one");
        state.find.dirty = true;
        state.refresh_find();
        assert_eq!(state.find.matches.len(), 2);

        state.find_step(1);
        assert_eq!(state.ws.active_doc().unwrap().selected_text(), Some("one"));
        state.find_step(1);
        let second = state.ws.active_doc().unwrap().caret();
        state.find_step(1);
        let wrapped = state.ws.active_doc().unwrap().caret();
        assert_ne!(second, wrapped, "navigation must wrap");
    }

    #[test]
    fn replace_all_rewrites_every_match() {
        let mut state = state();
        state
            .ws
            .active_doc_mut()
            .unwrap()
            .insert_text("one two one");
        state.open_find(true);
        state.find.query.insert("one");
        state.find.replace.insert("1");
        state.find.dirty = true;
        state.replace_all();
        assert_eq!(state.ws.active_doc().unwrap().text(), "1 two 1");
        assert!(state.find.matches.is_empty());
    }

    #[test]
    fn replace_all_skips_overlapping_matches() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("aaaa");
        state.open_find(true);
        state.find.query.insert("aa");
        state.find.replace.insert("b");
        state.find.dirty = true;
        state.replace_all();
        assert_eq!(state.ws.active_doc().unwrap().text(), "bb");
    }

    #[test]
    fn dirty_close_opens_the_modal_and_cancel_keeps_the_tab() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("x");
        state.request_close_tab(0);
        assert!(matches!(state.modal, Some(Modal::CloseDirtyTab { tab: 0 })));
        state.resolve_modal(ModalButton::Cancel);
        assert!(state.modal.is_none());
        assert_eq!(state.ws.tabs.len(), 1);
    }

    #[test]
    fn discard_closes_and_leaves_the_welcome_state() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("x");
        state.request_close_tab(0);
        state.resolve_modal(ModalButton::Discard);
        assert!(state.modal.is_none());
        assert!(state.ws.tabs.is_empty());
        assert!(state.ws.active_doc().is_none());
    }

    #[test]
    fn close_folder_clears_clean_tabs_and_confirms_dirty_ones() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("dirty");
        state.close_folder();
        assert!(matches!(state.modal, Some(Modal::CloseFolder { dirty: 1 })));
        state.resolve_modal(ModalButton::Cancel);
        assert_eq!(state.ws.tabs.len(), 1);
        assert!(state.ws.folder_open);

        state.close_folder();
        state.resolve_modal(ModalButton::Discard);
        assert!(!state.ws.folder_open);
        assert!(state.ws.tabs.is_empty());
        assert!(state.session.folder.is_none());
        assert!(state.session.tabs.is_empty());
    }

    #[test]
    fn quit_request_opens_a_confirm_modal() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("dirty");
        state.request_quit(1);
        assert!(matches!(state.modal, Some(Modal::Quit { dirty: 1 })));
        state.resolve_modal(ModalButton::Cancel);
        assert!(state.modal.is_none());
        assert_eq!(state.ws.tabs.len(), 1);
    }

    #[test]
    fn soft_wrap_refuses_files_over_the_line_cap() {
        let dir = std::env::temp_dir().join(format!("igui-editor-wrap-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("many.txt");
        let text = "line\n".repeat(crate::model::wrap::MAX_WRAP_LINES + 1);
        std::fs::write(&path, text).unwrap();
        let mut ws = Workspace::new(&dir);
        ws.open(&path).unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            dir.join("session.conf"),
        );
        state.set_soft_wrap(true);
        assert!(!state.session.soft_wrap, "over-cap file must refuse wrap");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn close_others_keeps_the_chosen_tab_and_confirms_dirty_targets() {
        let mut state = state();
        state.ws.new_tab();
        state.ws.new_tab();
        let keep = state.ws.tabs[1].id();
        state.ws.active = 1;
        state.request_close_others(1);
        assert!(state.modal.is_none());
        assert_eq!(state.ws.tabs.len(), 1);
        assert_eq!(state.ws.tabs[0].id(), keep);

        state.ws.new_tab();
        let keep = state.ws.tabs[1].id();
        state.ws.tabs[0].insert_text("dirty");
        state.ws.active = 1;
        state.request_close_others(1);
        assert!(matches!(
            state.modal,
            Some(Modal::CloseTabs { keep: Some(1), .. })
        ));
        state.resolve_modal(ModalButton::Cancel);
        assert_eq!(state.ws.tabs.len(), 2);
        state.request_close_others(1);
        state.resolve_modal(ModalButton::Discard);
        assert_eq!(state.ws.tabs.len(), 1);
        assert_eq!(state.ws.tabs[0].id(), keep);
    }

    #[test]
    fn close_right_only_targets_tabs_to_the_right() {
        let mut state = state();
        state.ws.new_tab();
        state.ws.new_tab();
        let first = state.ws.tabs[0].id();
        state.request_close_right(0);
        assert_eq!(state.ws.tabs.len(), 1);
        assert_eq!(state.ws.tabs[0].id(), first);
    }

    #[test]
    fn saving_an_untitled_tab_routes_through_save_as() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("x");
        state.request_close_tab(0);
        state.resolve_modal(ModalButton::Save);
        assert_eq!(
            state.pending_dialog,
            Some(DialogRequest {
                kind: DialogKind::SaveAs,
                then_close: Some(0)
            })
        );
        assert!(state.modal.is_none());
    }
}
