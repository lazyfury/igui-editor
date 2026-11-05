//! Shared app state written by input callbacks and read by painters.
//!
//! The tree is rebuilt only when the shell *structure* changes (sidebar, find
//! bar, view mode); everything else is a value here. Painters borrow this state,
//! so hit rectangles are recorded during paint and consumed by the next input
//! event — the standard immediate-mode trade.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use igui_core::{Key, NodeId, Rect, Vec2, ViewportSize};
use igui_ui::{Clipboard, TextEdit};

use crate::keymap::{KeyChord, Keymap, KeymapId, Resolution, Scope};
use crate::model::brackets::match_bracket;
use crate::model::doc::Document;
use crate::model::find::Matcher;
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
use tooling::ToolingUi;

/// Deadline scheduling: the shared timing constants and the next wakeup.
pub(crate) mod schedule;

// Domain implementations of [`AppState`]. The struct and its constructors stay
// in this module; each child owns one concern so no single file becomes a god
// object. Fields remain private to `state`, which child modules may access.
mod actions;
mod dock;
mod files;
mod find;
mod format;
mod git;
mod keymap;
mod lsp;
mod modal;
mod new_file;
mod quick_open;
mod reliability;
mod rename;
mod search;
mod services;
mod settings;
mod tabs;
mod terminal;
mod tooling;
mod view;
mod watch;
mod workspace;

pub use dock::{DockId, Panel};

pub const MAX_RECENT: usize = 8;
/// Recent entries shown per group in the File menu (the session keeps 8 each).
const MENU_RECENT: usize = 5;

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
    FormatDocument,
    /// Smart line start (`primary+left`), extending the selection when true.
    MoveLineStart(bool),
    /// Line end (`primary+right`), extending the selection when true.
    MoveLineEnd(bool),
    /// Document start (`primary+up`), extending the selection when true.
    MoveDocStart(bool),
    /// Document end (`primary+down`), extending the selection when true.
    MoveDocEnd(bool),
    LspHover,
    LspCompletion,
    LspDefinition,
    /// Drops and restarts the active document's language server.
    LspRestart,
    ToggleTerminal,
    Settings,
    ToggleSidebar,
    ToggleOutline,
    /// Toggles one chrome panel (header icon buttons).
    ShowPanel(Panel),
    ToggleTheme,
    SetView(ViewMode),
    ToggleScrollSync,
    ToggleSoftWrap,
    SetLanguage(Option<String>),
    OpenRecentProject(PathBuf),
    OpenRecentFile(PathBuf),
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
            Action::OpenFolder => "Open Project…".into(),
            Action::CloseFolder => "Close Project".into(),
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
            Action::ProjectSearch => "Search in Project…".into(),
            Action::CycleView => "Cycle View".into(),
            Action::DeleteLine => "Delete Line".into(),
            Action::DuplicateLine => "Duplicate Line".into(),
            Action::MoveLineUp => "Move Line Up".into(),
            Action::MoveLineDown => "Move Line Down".into(),
            Action::DeleteWordBackward => "Delete Word Backward".into(),
            Action::DeleteWordForward => "Delete Word Forward".into(),
            Action::ToggleComment => "Toggle Comment".into(),
            Action::FormatDocument => "Format Document".into(),
            Action::MoveLineStart(_) => "Go to Line Start".into(),
            Action::MoveLineEnd(_) => "Go to Line End".into(),
            Action::MoveDocStart(_) => "Go to Document Start".into(),
            Action::MoveDocEnd(_) => "Go to Document End".into(),
            Action::LspHover => "Show Hover".into(),
            Action::LspCompletion => "Complete".into(),
            Action::LspDefinition => "Go to Definition".into(),
            Action::LspRestart => "Restart Language Server".into(),
            Action::ToggleTerminal => "Terminal".into(),
            Action::Settings => "Settings…".into(),
            Action::ToggleSidebar => "Toggle Sidebar".into(),
            Action::ToggleOutline => "Outline".into(),
            Action::ShowPanel(panel) => format!("Toggle {} Panel", panel.label()),
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
            Action::OpenRecentProject(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("Project: {name}")
            }
            Action::OpenRecentFile(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("File: {name}")
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
            | Action::ShowPanel(_)
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
            | Action::ToggleComment
            | Action::FormatDocument => doc.is_some_and(|doc| !doc.is_read_only()),
            Action::MoveLineStart(_)
            | Action::MoveLineEnd(_)
            | Action::MoveDocStart(_)
            | Action::MoveDocEnd(_) => doc.is_some(),
            Action::LspHover
            | Action::LspCompletion
            | Action::LspDefinition
            | Action::LspRestart => doc
                .and_then(|doc| doc.path())
                .is_some_and(|path| state.lsp.supports(path)),
            Action::SetView(_) => doc.is_some(),
            Action::ToggleSoftWrap => doc.is_some(),
            Action::SetLanguage(_) => doc.is_some(),
            Action::OpenRecentProject(path) => path.is_dir(),
            Action::OpenRecentFile(path) => path.is_file(),
            Action::ClearRecent => {
                !state.session.recent_projects.is_empty() || !state.session.recent_files.is_empty()
            }
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
            Action::ShowPanel(panel) => panel.is_open(state),
            Action::ToggleSoftWrap => state.session.soft_wrap,
            Action::ToggleTerminal => Panel::Terminal.is_open(state),
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
            if !state.session.recent_projects.is_empty() {
                items.push(None);
                for path in state.session.recent_projects.iter().take(MENU_RECENT) {
                    items.push(Some(Action::OpenRecentProject(path.clone())));
                }
            }
            if !state.session.recent_files.is_empty() {
                items.push(None);
                for path in state.session.recent_files.iter().take(MENU_RECENT) {
                    items.push(Some(Action::OpenRecentFile(path.clone())));
                }
            }
            if !state.session.recent_projects.is_empty() || !state.session.recent_files.is_empty() {
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
            Some(Action::FormatDocument),
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
            Some(Action::ShowPanel(Panel::Problems)),
            None,
            Some(Action::LspRestart),
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
    Explorer,
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
    /// The match list hit [`crate::model::find::MAX_MATCHES`].
    pub truncated: bool,
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
            truncated: false,
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
    /// Quit with dirty tabs: Cancel / Discard & Quit.
    Quit { dirty: usize },
    /// Close the project and all its open tabs: Cancel / Discard.
    CloseFolder { dirty: usize },
    /// Replace the open project after discarding dirty tabs.
    SwitchProject { dirty: usize },
    /// Permanently delete a file or folder from the explorer.
    DeletePath { is_dir: bool },
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
    /// Body rectangle (below the dock header); rows and scroll use it.
    pub rect: Rect,
    pub scroll: f32,
    pub hover: Option<usize>,
    pub selected: Option<PathBuf>,
    /// Empty-state actions (no project open).
    pub actions: Vec<(Action, Rect)>,
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

/// Bottom-dock PROBLEMS panel: a workspace-wide, click-to-jump diagnostic list.
#[derive(Clone, Debug, Default)]
pub struct ProblemsUi {
    /// Diagnostic snapshot from every live language server, refreshed on change.
    pub rows: Vec<crate::model::lsp::ProblemEntry>,
    /// Severity totals for the dock tab badge (cached with `rows`).
    pub errors: usize,
    pub warnings: usize,
    pub hover: Option<usize>,
    pub rect: Rect,
    /// Visible row index and its hit rectangle, recorded during paint.
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
/// Inclusive cell range selected in the terminal grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TerminalSelection {
    pub anchor: (u16, u16),
    pub caret: (u16, u16),
}

impl TerminalSelection {
    pub fn ordered(self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.caret {
            (self.anchor, self.caret)
        } else {
            (self.caret, self.anchor)
        }
    }

    pub fn contains(self, row: u16, col: u16) -> bool {
        let (start, end) = self.ordered();
        if row < start.0 || row > end.0 {
            return false;
        }
        if start.0 == end.0 {
            return col >= start.1 && col <= end.1;
        }
        if row == start.0 {
            return col >= start.1;
        }
        if row == end.0 {
            return col <= end.1;
        }
        true
    }
}

pub struct TerminalSession {
    pub session: Option<PtySession>,
    pub parser: Option<vt100::Parser>,
    pub cwd: PathBuf,
    /// Last grid handed to this parser/PTY.
    pub rows: u16,
    pub cols: u16,
    pub scrollback: usize,
    pub selection: Option<TerminalSelection>,
    pub selecting: bool,
    pub caret_rect: Rect,
}

impl Default for TerminalSession {
    fn default() -> Self {
        Self {
            session: None,
            parser: None,
            cwd: PathBuf::from("."),
            rows: 24,
            cols: 80,
            scrollback: 0,
            selection: None,
            selecting: false,
            caret_rect: Rect::ZERO,
        }
    }
}

pub struct TerminalState {
    pub open: bool,
    pub focused: bool,
    pub sessions: Vec<TerminalSession>,
    pub active: usize,
    pub rect: Rect,
    /// Header rectangle, used to route tab/new/close clicks before selection.
    pub header_rect: Rect,
    pub tab_hits: Vec<(usize, Rect)>,
    pub new_hit: Rect,
    pub close_hit: Rect,
    /// Fixed-pitch family resolved from [`crate::fonts::MONO_FAMILIES`].
    pub font: Option<String>,
    /// One column's advance in pixels, as last painted.
    pub cell_w: f32,
    /// Whether [`TerminalState::font`] has been probed (needs the backend
    /// measurer, which is only available during painting).
    pub font_checked: bool,
    /// Cached body height of one terminal row (paint geometry).
    pub line_h: f32,
    pub height: f32,
    pub node: Option<NodeId>,
    pub drag: Option<(f32, f32)>,
    pub handle_rect: Rect,
}

impl TerminalState {
    pub fn active(&self) -> Option<&TerminalSession> {
        self.sessions.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(self.active)
    }

    pub fn active_parser(&self) -> Option<&vt100::Parser> {
        self.active()?.parser.as_ref()
    }

    pub fn active_rows(&self) -> u16 {
        self.active().map_or(24, |session| session.rows)
    }

    pub fn active_cols(&self) -> u16 {
        self.active().map_or(80, |session| session.cols)
    }
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            open: false,
            focused: false,
            sessions: Vec::new(),
            active: 0,
            rect: Rect::ZERO,
            header_rect: Rect::ZERO,
            tab_hits: Vec::new(),
            new_hit: Rect::ZERO,
            close_hit: Rect::ZERO,
            font: None,
            font_checked: false,
            cell_w: 0.0,
            line_h: 0.0,
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
    Rename,
    NewFile,
    NewFolder,
    Delete,
    CopyPath,
    Reveal,
}

impl TreeMenuAction {
    pub const fn label(self) -> &'static str {
        match self {
            TreeMenuAction::Open => "Open",
            TreeMenuAction::Rename => "Rename…",
            TreeMenuAction::NewFile => "New File…",
            TreeMenuAction::NewFolder => "New Folder…",
            TreeMenuAction::Delete => "Delete",
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
    /// Right-click on empty Explorer space rather than a row.
    pub blank: bool,
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

/// What the inline create prompt will create.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NewFileKind {
    File,
    Folder,
    Rename,
}

impl NewFileKind {
    pub const fn title(self) -> &'static str {
        match self {
            NewFileKind::File => "New File",
            NewFileKind::Folder => "New Folder",
            NewFileKind::Rename => "Rename",
        }
    }
}

/// Quick-create prompt (`⇧⌘N`, tree context menu).
pub struct NewFileState {
    pub open: bool,
    pub kind: NewFileKind,
    pub dir: PathBuf,
    pub name: TextEdit,
    /// Existing path for a rename; `None` for create modes.
    pub target: Option<PathBuf>,
    pub error: Option<String>,
    pub preedit: Option<String>,
}

impl Default for NewFileState {
    fn default() -> Self {
        Self {
            open: false,
            kind: NewFileKind::File,
            dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            name: TextEdit::new("untitled.md"),
            target: None,
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
    /// Active bottom-dock tab (`Terminal` or `Problems`).
    pub bottom_tab: Panel,
    pub viewport: ViewportSize,
    pub sidebar: SidebarUi,
    pub outline: OutlineUi,
    pub problems: ProblemsUi,
    pub tooling: ToolingUi,
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
    /// Header panel buttons, recorded in paint and used for hit-testing.
    pub panel_hits: Vec<(Panel, Rect)>,
    /// Header panel button under the pointer.
    pub panel_hover: Option<Panel>,
    /// Dock tab strips: per-dock tab rectangles and close button, recorded in
    /// paint so paint and hit-testing share one layout.
    pub dock_hits: [Vec<(Panel, Rect)>; 3],
    pub dock_close: [Rect; 3],
    /// Dock tab / close button under the pointer.
    pub dock_hover: Option<Panel>,
    pub dock_close_hover: Option<DockId>,
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
    /// Project root waiting for the switch confirmation.
    pub pending_project: Option<PathBuf>,
    /// Path waiting for the delete confirmation.
    pub pending_delete: Option<PathBuf>,
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
    /// Status-bar theme toggle hit rect; clicking it toggles dark/light.
    pub theme_hit: Rect,
    pub theme_hover: bool,
    /// Status-bar LSP summary chip; clicking it opens the problems popup.
    pub lsp_hit: Rect,
    pub lsp_hover: bool,
    /// Status-bar LSP on/off dot next to the summary chip.
    pub lsp_toggle_hit: Rect,
    pub lsp_toggle_hover: bool,
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
    /// The quit confirmation was discarded: the shell terminates the app.
    pub quit_requested: bool,
    /// Per-path language overrides (persisted) and per-document ones (untitled).
    pub language_by_path: HashMap<PathBuf, String>,
    pub language_by_id: HashMap<u64, String>,
    /// Keep editor and preview scroll positions in sync (W3).
    pub sync_scroll: bool,
    /// Last seen disk identity per open path (`None` = missing on disk).
    external: HashMap<PathBuf, Option<DiskStamp>>,
    /// Directory listings last seen by the external-change poll.
    watched_dirs: HashMap<PathBuf, Vec<crate::model::watch::EntryStamp>>,
    /// File changes queued for the language servers.
    file_events: Vec<crate::model::watch::FileEvent>,
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
            bottom_tab: Panel::from_id(&session.bottom_tab)
                .filter(|panel| panel.dock() == DockId::Bottom)
                .unwrap_or(Panel::Terminal),
            viewport: ViewportSize::default(),
            sidebar: SidebarUi::default(),
            outline: OutlineUi::default(),
            problems: ProblemsUi::default(),
            tooling: ToolingUi::default(),
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
            panel_hits: Vec::new(),
            panel_hover: None,
            dock_hits: std::array::from_fn(|_| Vec::new()),
            dock_close: [Rect::ZERO; 3],
            dock_hover: None,
            dock_close_hover: None,
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
            pending_project: None,
            pending_delete: None,
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
            theme_hit: Rect::ZERO,
            theme_hover: false,
            lsp_hit: Rect::ZERO,
            lsp_hover: false,
            lsp_toggle_hit: Rect::ZERO,
            lsp_toggle_hover: false,
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
            quit_requested: false,
            language_by_path: languages,
            language_by_id: HashMap::new(),
            sync_scroll,
            external,
            watched_dirs: HashMap::new(),
            file_events: Vec::new(),
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
        state.apply_explorer_filter();
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

    /// Stops native child processes before an AppKit/OS quit that may not run
    /// Rust destructors. Also used when the quit confirmation is accepted.
    pub fn shutdown_children(&mut self) {
        self.lsp.shutdown();
        self.shutdown_terminal_sessions();
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
        indent_tabs: session.indent_tabs,
        line_numbers: session.line_numbers,
        indent_guides: session.indent_guides,
        syntax: session.syntax,
        autosave: session.autosave,
        lsp: session.lsp,
        trust_local_servers: session.trust_local_servers,
        sidebar_width: session.sidebar_width,
        show_hidden: session.show_hidden,
        ignored_files: crate::model::ignore::IgnoreMode::from_session(&session.ignored_files),
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
    fn unbinding_a_chord_is_not_resurrected_by_input() {
        let mut state = state_with_tabs(1);
        state.keymap.apply_overrides(&serde_json::json!({
            "bindings": { "global": { "primary+s": null } }
        }));
        let primary = if cfg!(target_os = "macos") {
            igui_core::Modifiers {
                meta: true,
                ..igui_core::Modifiers::NONE
            }
        } else {
            igui_core::Modifiers {
                ctrl: true,
                ..igui_core::Modifiers::NONE
            }
        };
        assert_eq!(
            crate::input::shortcut(&mut state, Key::Character('s'), primary),
            None
        );
    }

    #[test]
    fn terminal_focus_keeps_clipboard_commands_out_of_the_document() {
        let mut state = state_with_tabs(1);
        state.ws.active_doc_mut().unwrap().insert_text("editor");
        state.clipboard = Some(std::rc::Rc::new(std::cell::RefCell::new(
            igui_ui::MemoryClipboard::default(),
        )));
        state.set_clipboard("shell text");
        state.focus = Focus::Terminal;

        state.run(Action::Paste);
        assert_eq!(state.ws.active_doc().unwrap().text(), "editor");
        assert!(state
            .message()
            .is_some_and(|(text, _)| text.contains("terminal")));

        state.run(Action::Cut);
        state.run(Action::Copy);
        state.run(Action::SelectAll);
        assert_eq!(state.ws.active_doc().unwrap().text(), "editor");
        assert!(!state.ws.active_doc().unwrap().has_selection());
    }

    #[test]
    fn terminal_session_selection_and_close_keep_the_active_index_valid() {
        let mut state = state_with_tabs(1);
        state.terminal.open = true;
        state.terminal.sessions = vec![
            TerminalSession::default(),
            TerminalSession::default(),
            TerminalSession::default(),
        ];
        state.terminal.active = 1;
        state.terminal_select_session(2);
        assert_eq!(state.terminal.active, 2);
        state.terminal_close_session(0);
        assert_eq!(state.terminal.sessions.len(), 2);
        assert_eq!(state.terminal.active, 1);
        state.terminal_close_session(1);
        assert_eq!(state.terminal.sessions.len(), 1);
        assert_eq!(state.terminal.active, 0);
        state.terminal_close_session(0);
        assert!(!state.terminal.open);
        assert!(state.terminal.sessions.is_empty());
    }

    #[test]
    fn shutdown_children_clears_native_sessions() {
        let mut state = state_with_tabs(1);
        let parser = vt100::Parser::new(
            std::num::NonZeroU16::new(2).unwrap(),
            std::num::NonZeroU16::new(20).unwrap(),
            0,
        );
        state.terminal.sessions = vec![TerminalSession {
            parser: Some(parser),
            selection: Some(TerminalSelection {
                anchor: (0, 0),
                caret: (0, 2),
            }),
            scrollback: 3,
            ..TerminalSession::default()
        }];
        state.terminal.active = 0;
        state.shutdown_children();
        assert!(state.terminal.sessions.is_empty());
        assert_eq!(state.lsp.status(), &crate::model::lsp::LspStatus::Off);
    }

    #[test]
    fn terminal_copy_reads_the_selected_cells() {
        let mut state = state_with_tabs(1);
        state.ws.active_doc_mut().unwrap().insert_text("editor");
        state.clipboard = Some(std::rc::Rc::new(std::cell::RefCell::new(
            igui_ui::MemoryClipboard::default(),
        )));
        let mut parser = vt100::Parser::new(
            std::num::NonZeroU16::new(2).unwrap(),
            std::num::NonZeroU16::new(20).unwrap(),
            0,
        );
        parser.process(b"hello world");
        state.terminal.sessions = vec![TerminalSession {
            parser: Some(parser),
            rows: 2,
            cols: 20,
            selection: Some(TerminalSelection {
                anchor: (0, 0),
                caret: (0, 4),
            }),
            ..TerminalSession::default()
        }];
        state.terminal.active = 0;
        state.focus = Focus::Terminal;

        state.run(Action::Copy);
        assert_eq!(state.get_clipboard().as_deref(), Some("hello"));
        assert_eq!(state.ws.active_doc().unwrap().text(), "editor");
        assert!(state
            .message()
            .is_some_and(|(text, _)| text.contains("terminal selection")));
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
    fn switching_projects_closes_tabs_and_resets_project_ui() {
        let base =
            std::env::temp_dir().join(format!("igui-editor-project-switch-{}", std::process::id()));
        let first = base.join("first");
        let second = base.join("second");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut ws = Workspace::new(&first);
        ws.new_tab();
        let mut state = AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            base.join("session.conf"),
        );
        state.session.project = Some(first.clone());
        state.terminal.sessions.push(TerminalSession::default());
        state.outline.hover = Some(1);
        state.outline.scroll = 12.0;
        state.bottom_tab = Panel::Problems;
        state.terminal.open = true;

        state.open_project(second.clone());

        assert_eq!(state.ws.root, std::fs::canonicalize(&second).unwrap());
        assert!(state.ws.folder_open);
        assert!(state.ws.tabs.is_empty());
        assert_eq!(state.session.project, Some(state.ws.root.clone()));
        assert_eq!(state.session.recent_projects.first(), Some(&state.ws.root));
        assert!(state.terminal.sessions.is_empty());
        assert!(state.outline.hover.is_none());
        assert_eq!(state.outline.scroll, 0.0);
        assert!(state.problems.rows.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn switching_projects_confirms_dirty_tabs() {
        let base = std::env::temp_dir().join(format!(
            "igui-editor-project-confirm-{}",
            std::process::id()
        ));
        let first = base.join("first");
        let second = base.join("second");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut ws = Workspace::new(&first);
        ws.new_tab();
        let mut state = AppState::new(
            Theme::dark(),
            ws,
            Session::default(),
            base.join("session.conf"),
        );
        state.ws.active_doc_mut().unwrap().insert_text("dirty");

        state.open_project(second.clone());
        assert!(matches!(
            state.modal,
            Some(Modal::SwitchProject { dirty: 1 })
        ));
        assert_eq!(
            state.pending_project,
            Some(std::fs::canonicalize(&second).unwrap())
        );
        assert_eq!(state.ws.root, std::fs::canonicalize(&first).unwrap());
        state.resolve_modal(ModalButton::Cancel);
        assert!(state.modal.is_none());
        assert!(state.pending_project.is_none());
        assert_eq!(state.ws.root, std::fs::canonicalize(&first).unwrap());

        state.open_project(second.clone());
        state.resolve_modal(ModalButton::Discard);
        assert_eq!(state.ws.root, std::fs::canonicalize(&second).unwrap());
        assert!(state.ws.tabs.is_empty());
        assert!(state.pending_project.is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rename_updates_open_tabs_and_recent_files() {
        let base =
            std::env::temp_dir().join(format!("igui-editor-rename-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let old = base.join("old.rs");
        std::fs::write(&old, "fn old() {}\n").unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            Workspace::new(&base),
            Session::default(),
            base.join("session.conf"),
        );
        state.open_path(old.clone());
        let old = state
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(Path::to_path_buf)
            .unwrap();
        state.open_rename(old.clone());
        state.new_file.name.set_text("new.rs");
        state.submit_new_file();
        let new = std::fs::canonicalize(base.join("new.rs")).unwrap();
        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(
            state.ws.active_doc().and_then(|doc| doc.path()),
            Some(new.as_path())
        );
        assert_eq!(state.sidebar.selected.as_deref(), Some(new.as_path()));
        assert!(state.session.recent_files.iter().any(|path| path == &new));
        assert!(state.session.recent_files.iter().all(|path| path != &old));
        assert!(state.session.tabs.iter().any(|path| path == &new));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn new_folder_prompt_creates_a_directory() {
        let base =
            std::env::temp_dir().join(format!("igui-editor-new-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            Workspace::new(&base),
            Session::default(),
            base.join("session.conf"),
        );
        state.open_new_folder_in(base.clone());
        state.new_file.name.set_text("new-dir");
        state.submit_new_file();
        assert!(!state.new_file.open);
        assert!(base.join("new-dir").is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn deleting_an_open_file_closes_its_tab() {
        let base =
            std::env::temp_dir().join(format!("igui-editor-delete-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("gone.rs");
        std::fs::write(&file, "fn gone() {}\n").unwrap();
        let file = std::fs::canonicalize(&file).unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            Workspace::new(&base),
            Session::default(),
            base.join("session.conf"),
        );
        state.open_path(file.clone());
        state.request_delete(file.clone());
        assert!(matches!(
            state.modal,
            Some(Modal::DeletePath { is_dir: false })
        ));
        state.resolve_modal(ModalButton::Discard);
        assert!(!file.exists());
        assert!(state.modal.is_none());
        assert!(state.ws.tabs.is_empty());
        assert!(state.session.recent_files.iter().all(|path| path != &file));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dropped_files_are_recent_files_not_projects() {
        let base =
            std::env::temp_dir().join(format!("igui-editor-drop-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("dropped.rs");
        std::fs::write(&file, "fn dropped() {}\n").unwrap();
        let mut state = AppState::new(
            Theme::dark(),
            Workspace::detached(),
            Session::default(),
            base.join("session.conf"),
        );

        state.open_dropped(vec![file.clone()]);

        let canonical = std::fs::canonicalize(&file).unwrap();
        assert!(!state.ws.folder_open);
        assert_eq!(state.session.recent_projects, Vec::<PathBuf>::new());
        assert_eq!(state.session.recent_files.first(), Some(&canonical));
        assert_eq!(
            state.ws.active_doc().and_then(|doc| doc.path()),
            Some(canonical.as_path())
        );
        let _ = std::fs::remove_dir_all(&base);
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
        assert!(state.session.project.is_none());
        assert!(state.session.tabs.is_empty());
    }

    #[test]
    fn quit_request_opens_a_confirm_modal() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("dirty");
        state.request_quit(1);
        assert!(matches!(state.modal, Some(Modal::Quit { dirty: 1 })));
        assert!(!state.quit_requested);
        state.resolve_modal(ModalButton::Cancel);
        assert!(state.modal.is_none());
        assert!(!state.quit_requested);
        assert_eq!(state.ws.tabs.len(), 1);
    }

    #[test]
    fn discarding_the_quit_modal_asks_the_shell_to_terminate() {
        let mut state = state();
        state.request_quit(2);
        state.resolve_modal(ModalButton::Discard);
        assert!(state.modal.is_none());
        assert!(state.quit_requested);
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
