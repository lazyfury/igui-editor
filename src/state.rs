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

use igui_core::{NodeId, Rect, Vec2, ViewportSize};
use igui_ui::{Clipboard, TextEdit};

use crate::model::brackets::match_bracket;
use crate::model::doc::Document;
use crate::model::find::{find_all, Matcher};
use crate::model::git::{GitDiff, GitEvent, GitRequest, GitWorker, RepoStatus};
use crate::model::highlight::HighlightCache;
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
    QuickOpen,
    ProjectSearch,
    Settings,
    ToggleSidebar,
    ToggleTheme,
    SetView(ViewMode),
    ToggleScrollSync,
    ToggleSoftWrap,
    SetLanguage(Option<String>),
    OpenRecent(PathBuf),
    ClearRecent,
}

/// Platform's primary accelerator label.
pub const ACCEL: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl+"
};

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
            Action::QuickOpen => "Quick Open…".into(),
            Action::ProjectSearch => "Search in Folder…".into(),
            Action::Settings => "Settings…".into(),
            Action::ToggleSidebar => "Toggle Sidebar".into(),
            Action::ToggleTheme => "Toggle Theme".into(),
            Action::SetView(mode) => mode.label().into(),
            Action::ToggleScrollSync => "Sync Scroll".into(),
            Action::ToggleSoftWrap => "Soft Wrap".into(),
            Action::SetLanguage(None) => "Auto (by extension)".into(),
            Action::SetLanguage(Some(name)) => name.clone(),
            Action::ClearRecent => "Clear Recent".into(),
            Action::OpenRecent(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("Recent: {name}")
            }
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            Action::New => match ACCEL {
                "⌘" => "⌘N",
                _ => "Ctrl+N",
            },
            Action::NewFile => match ACCEL {
                "⌘" => "⇧⌘N",
                _ => "Ctrl+Shift+N",
            },
            Action::Open => match ACCEL {
                "⌘" => "⌘O",
                _ => "Ctrl+O",
            },
            Action::OpenFolder => match ACCEL {
                "⌘" => "⇧⌘O",
                _ => "Ctrl+Shift+O",
            },
            Action::CloseFolder => "",
            Action::Save => match ACCEL {
                "⌘" => "⌘S",
                _ => "Ctrl+S",
            },
            Action::SaveAs => match ACCEL {
                "⌘" => "⇧⌘S",
                _ => "Ctrl+Shift+S",
            },
            Action::CloseTab => match ACCEL {
                "⌘" => "⌘W",
                _ => "Ctrl+W",
            },
            Action::Undo => match ACCEL {
                "⌘" => "⌘Z",
                _ => "Ctrl+Z",
            },
            Action::Redo => match ACCEL {
                "⌘" => "⇧⌘Z",
                _ => "Ctrl+Shift+Z",
            },
            Action::Cut => match ACCEL {
                "⌘" => "⌘X",
                _ => "Ctrl+X",
            },
            Action::Copy => match ACCEL {
                "⌘" => "⌘C",
                _ => "Ctrl+C",
            },
            Action::Paste => match ACCEL {
                "⌘" => "⌘V",
                _ => "Ctrl+V",
            },
            Action::SelectAll => match ACCEL {
                "⌘" => "⌘A",
                _ => "Ctrl+A",
            },
            Action::Find => match ACCEL {
                "⌘" => "⌘F",
                _ => "Ctrl+F",
            },
            Action::QuickOpen => match ACCEL {
                "⌘" => "⌘P",
                _ => "Ctrl+P",
            },
            Action::ProjectSearch => match ACCEL {
                "⌘" => "⇧⌘F",
                _ => "Ctrl+Shift+F",
            },
            Action::Settings => match ACCEL {
                "⌘" => "⌘,",
                _ => "Ctrl+,",
            },
            Action::ToggleSidebar => match ACCEL {
                "⌘" => "⌘B",
                _ => "Ctrl+B",
            },
            Action::ToggleTheme => match ACCEL {
                "⌘" => "⇧⌘L",
                _ => "Ctrl+Shift+L",
            },
            Action::SetView(_) => match ACCEL {
                "⌘" => "⇧⌘M",
                _ => "Ctrl+Shift+M",
            },
            Action::ToggleScrollSync => match ACCEL {
                "⌘" => "⇧⌘Y",
                _ => "Ctrl+Shift+Y",
            },
            Action::ToggleSoftWrap => match ACCEL {
                "⌘" => "⌥Z",
                _ => "Alt+Z",
            },
            Action::SetLanguage(_) => "",
            Action::OpenRecent(_) | Action::ClearRecent => "",
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
            | Action::ToggleTheme
            | Action::Find
            | Action::Settings
            | Action::ToggleScrollSync => true,
            Action::CloseFolder => state.ws.folder_open,
            Action::QuickOpen | Action::ProjectSearch => state.ws.folder_open,
            Action::Save => doc.is_some(),
            Action::SaveAs => doc.is_some(),
            Action::CloseTab => doc.is_some(),
            Action::Undo => doc.is_some_and(|doc| doc.can_undo()),
            Action::Redo => doc.is_some_and(|doc| doc.can_redo()),
            Action::Cut | Action::Copy => doc.is_some_and(|doc| doc.has_selection()),
            Action::Paste => doc.is_some() && state.clipboard.is_some(),
            Action::SelectAll => doc.is_some(),
            Action::SetView(_) => doc.is_some(),
            Action::ToggleSoftWrap => doc.is_some(),
            Action::SetLanguage(_) => doc.is_some(),
            Action::OpenRecent(path) => path.is_file(),
            Action::ClearRecent => !state.session.recent.is_empty(),
        }
    }

    pub fn checked(&self, state: &AppState) -> bool {
        match self {
            Action::SetView(mode) => *mode == state.view_of_active(),
            Action::ToggleScrollSync => state.sync_scroll,
            Action::ToggleSoftWrap => state.session.soft_wrap,
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
            Some(Action::ToggleTheme),
            None,
            Some(Action::SetView(ViewMode::Edit)),
            Some(Action::SetView(ViewMode::Split)),
            Some(Action::SetView(ViewMode::Preview)),
            None,
            Some(Action::ToggleScrollSync),
            Some(Action::ToggleSoftWrap),
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
    /// Scroll the caret into view on the next paint (set by input, not by
    /// wheel scrolling).
    pub reveal: bool,
    /// Clickable rows of the welcome screen (no document open).
    pub welcome_actions: Vec<(Action, Rect)>,
    /// Soft-wrap rows for the current document/width (rebuilt by paint).
    pub wrap: crate::model::wrap::WrapLayout,
    /// Horizontal scroll saved when wrap turned on, restored when it turns off.
    pub wrap_scroll_x: f32,
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
    pub rows: Vec<(PathBuf, bool, Rect)>,
    pub refresh_rect: Rect,
    pub scroll: f32,
    pub error: Option<String>,
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
            scroll: 0.0,
            error: None,
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
    pub viewport: ViewportSize,
    pub sidebar: SidebarUi,
    pub editor: EditorUi,
    pub tabs: TabsUi,
    pub tab_drag: Option<TabDrag>,
    pub tab_menu: Option<TabMenuUi>,
    pub menu_open: Option<Menu>,
    pub menubar_hover: Option<Menu>,
    pub menubar_hits: Vec<(Menu, Rect)>,
    pub menu_rect: Rect,
    /// Popup item rectangles, in item order (separators have no rect).
    pub menu_items: Vec<(Action, Rect)>,
    pub menu_hover: Option<usize>,
    /// Status-bar message and whether it is an error.
    pub message: Option<(String, bool, Instant)>,
    pub clipboard: Option<Rc<RefCell<dyn Clipboard>>>,
    /// Set when the shell must be rebuilt (sidebar, find bar, view mode).
    pub needs_rebuild: bool,
    /// View mode the shell was last built for.
    pub built_view: ViewMode,
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
            viewport: ViewportSize::default(),
            sidebar: SidebarUi::default(),
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
            menu_open: None,
            menubar_hover: None,
            menubar_hits: Vec::new(),
            menu_rect: Rect::ZERO,
            menu_items: Vec::new(),
            menu_hover: None,
            message: None,
            clipboard: None,
            needs_rebuild: false,
            built_view: ViewMode::Edit,
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
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), true, Instant::now()));
    }

    /// Message text plus severity, if one is still worth showing.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message
            .as_ref()
            .map(|(text, error, _)| (text.as_str(), *error))
    }

    pub fn sidebar_tabs(&self) -> &[SidebarContribution] {
        &self.services.sidebar_tabs
    }

    pub fn set_sidebar_tab(&mut self, id: &'static str) {
        if !self.services.sidebar_tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        if self.sidebar_tab != id {
            self.sidebar_tab = id.into();
            self.session.sidebar_tab = id.into();
            self.config_dirty = true;
            if id == "git" {
                self.request_git_refresh();
            }
            self.dirty = true;
        }
    }

    fn git_cwd(&self) -> Option<PathBuf> {
        self.ws
            .active_doc()
            .and_then(|doc| {
                doc.path()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
    }

    pub fn request_git_refresh(&mut self) {
        self.git.pending = true;
        self.git.cwd = self.git_cwd();
        self.dirty = true;
    }

    fn ensure_git_worker(&mut self) {
        if self.git.worker.is_none() {
            self.git.worker = Some(GitWorker::spawn());
        }
    }

    pub fn open_git_diff(&mut self, path: PathBuf, staged: bool) {
        self.ensure_git_worker();
        self.git.selected = Some(path.clone());
        self.git.diff = None;
        self.git.diff_loading = true;
        self.git.diff_visible = true;
        self.git.diff_scroll = 0.0;
        self.git.diff_generation += 1;
        let generation = self.git.diff_generation;
        let cwd = self
            .git
            .cwd
            .clone()
            .or_else(|| self.git_cwd())
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(worker) = &self.git.worker {
            worker.send(GitRequest::Diff {
                generation,
                cwd,
                path,
                staged,
            });
        }
        self.dirty = true;
    }

    pub fn close_git_diff(&mut self) {
        self.git.diff_visible = false;
        self.git.diff_loading = false;
        self.git.diff = None;
        self.git.diff_scroll = 0.0;
        self.dirty = true;
    }

    /// Drains Git worker events and issues a pending status request.
    pub fn tick_git(&mut self) {
        if let Some(worker) = &self.git.worker {
            while let Some(event) = worker.try_recv() {
                match event {
                    GitEvent::Status { generation, status }
                        if generation == self.git.generation =>
                    {
                        self.git.status = status;
                        self.git.loading = false;
                        self.git.error = None;
                        self.dirty = true;
                    }
                    GitEvent::Diff { generation, diff }
                        if generation == self.git.diff_generation =>
                    {
                        self.git.diff = Some(diff);
                        self.git.diff_loading = false;
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
        }
        if self.git.pending {
            self.git.pending = false;
            self.ensure_git_worker();
            self.git.generation += 1;
            let generation = self.git.generation;
            self.git.loading = true;
            self.git.error = None;
            let cwd = self.git.cwd.clone().or_else(|| self.git_cwd());
            if let Some(cwd) = cwd {
                if let Some(worker) = &self.git.worker {
                    worker.send(GitRequest::Status { generation, cwd });
                }
            } else {
                self.git.loading = false;
                self.git.status = RepoStatus::default();
            }
        }
        // Drop stale worker errors so the panel does not keep a previous repo.
        if let Some(status) = self.git.status.error.take() {
            if !status.is_empty() {
                self.git.error = Some(status);
                self.dirty = true;
            }
        }
    }

    /// Opens the quick-create-file prompt.
    pub fn open_new_file(&mut self) {
        if self.new_file.open || self.modal.is_some() || self.recovery_prompt.is_some() {
            return;
        }
        self.settings_open = false;
        self.quick_open.open = false;
        if self.search.open {
            self.close_project_search();
        }
        self.close_find();
        self.menu_open = None;
        self.tab_menu = None;
        let (dir, _) = self.dialog_context();
        self.new_file.dir =
            dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.new_file.name = TextEdit::new("untitled.md");
        self.new_file.name.select_all();
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.new_file.open = true;
        self.dirty = true;
    }

    pub fn close_new_file(&mut self) {
        self.new_file.open = false;
        self.new_file.error = None;
        self.new_file.preedit = None;
        self.dirty = true;
    }

    /// Creates and opens the entered file. Errors stay inline in the prompt.
    pub fn submit_new_file(&mut self) {
        let name = self.new_file.name.text().trim().to_string();
        if name.is_empty() {
            self.new_file.error = Some("enter a file name".into());
            self.dirty = true;
            return;
        }
        let relative = Path::new(&name);
        if relative.is_absolute()
            || relative.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            self.new_file.error = Some("use a relative name without ..".into());
            self.dirty = true;
            return;
        }
        let path = self.new_file.dir.join(relative);
        if let Some(parent) = path.parent() {
            if !parent.is_dir() {
                self.new_file.error = Some(format!("folder does not exist: {}", parent.display()));
                self.dirty = true;
                return;
            }
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.new_file.error = Some(format!("already exists: {name}"));
                self.dirty = true;
                return;
            }
            Err(error) => {
                self.new_file.error = Some(format!("create failed: {error}"));
                self.dirty = true;
                return;
            }
        }
        let name = name.clone();
        self.new_file.open = false;
        self.new_file.error = None;
        if self.ws.folder_open {
            self.ws.tree.reload();
            if let Some(parent) = path.parent() {
                self.ws.tree.expand_to(parent);
            }
        }
        self.open_path(path);
        self.status(format!("created {name}"));
        self.dirty = true;
    }

    /// Opens the quick-open palette (folder required).
    pub fn open_quick_open(&mut self) {
        if !self.ws.folder_open {
            self.error("open a folder first (⇧⌘O)");
            return;
        }
        if self.quick_open.open {
            self.close_quick_open();
            return;
        }
        self.settings_open = false;
        self.menu_open = None;
        if self.search.open {
            self.close_project_search();
        }
        self.quick_open.open = true;
        self.quick_open.index = Some(FileIndex::build(&self.ws.root));
        self.quick_open.selected = 0;
        self.quick_open.scroll = 0;
        self.quick_open.query.select_all();
        self.refresh_quick_open();
        self.dirty = true;
    }

    pub fn close_quick_open(&mut self) {
        self.quick_open.open = false;
        self.quick_open.preedit = None;
        self.quick_open.results.clear();
        self.quick_open.selected = 0;
        self.quick_open.scroll = 0;
        self.dirty = true;
    }

    /// Re-ranks the index for the current query.
    pub fn refresh_quick_open(&mut self) {
        if self.quick_open.index.is_none() && self.ws.folder_open {
            self.quick_open.index = Some(FileIndex::build(&self.ws.root));
        }
        let Some(index) = &self.quick_open.index else {
            self.quick_open.results.clear();
            return;
        };
        let query = self.quick_open.query.text().to_string();
        let recent = &self.session.recent;
        self.quick_open.results = index
            .query(&query, |path| recent.iter().any(|entry| entry == path))
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        self.quick_open.selected = self
            .quick_open
            .selected
            .min(self.quick_open.results.len().saturating_sub(1));
        self.dirty = true;
    }

    pub fn quick_open_move(&mut self, delta: isize) {
        if self.quick_open.results.is_empty() {
            return;
        }
        let last = self.quick_open.results.len() - 1;
        let selected = self.quick_open.selected as isize + delta;
        self.quick_open.selected = selected.clamp(0, last as isize) as usize;
        self.dirty = true;
    }

    /// Opens the selected result.
    pub fn quick_open_accept(&mut self) {
        let Some(path) = self
            .quick_open
            .results
            .get(self.quick_open.selected)
            .cloned()
        else {
            return;
        };
        self.close_quick_open();
        self.open_path(path);
    }

    /// Display path for a Quick Open row.
    pub fn quick_open_display(&self, path: &Path) -> String {
        display_path(&self.ws.root, path)
    }

    /// Opens (or closes) the project search panel (folder required).
    pub fn open_project_search(&mut self) {
        if self.search.open {
            self.close_project_search();
            return;
        }
        if !self.ws.folder_open {
            self.error("open a folder first (⇧⌘O)");
            return;
        }
        self.settings_open = false;
        self.quick_open.open = false;
        self.menu_open = None;
        self.search.open = true;
        self.search.query.select_all();
        self.search.focus = SearchFocus::Query;
        self.search.selected = 0;
        self.search.scroll = 0;
        self.search.hits.clear();
        self.search.files_scanned = 0;
        self.search.scanning = false;
        self.search.pending = None;
        if self.search.query.text().is_empty() {
            self.search.worker = None;
        } else {
            self.search_start();
        }
        self.dirty = true;
    }

    pub fn close_project_search(&mut self) {
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        self.search.open = false;
        self.search.pending = None;
        self.search.scanning = false;
        self.search.hits.clear();
        self.search.selected = 0;
        self.search.scroll = 0;
        self.dirty = true;
    }

    /// Debounces query/toggle edits (120 ms), then restarts the worker.
    pub fn search_query_changed(&mut self) {
        self.search.pending = Some(Instant::now());
        self.search.selected = 0;
        self.search.scroll = 0;
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        self.search.scanning = false;
        self.dirty = true;
    }

    fn search_start(&mut self) {
        self.search.pending = None;
        if let Some(worker) = self.search.worker.take() {
            worker.cancel();
        }
        let query = self.search.query.text().trim().to_string();
        if query.is_empty() {
            self.search.scanning = false;
            self.search.hits.clear();
            return;
        }
        self.search.hits.clear();
        self.search.files_scanned = 0;
        self.search.selected = 0;
        self.search.scroll = 0;
        self.search.scanning = true;
        let matcher = Matcher {
            query,
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
        };
        self.search.worker = Some(SearchWorker::spawn(self.ws.root.clone(), matcher));
        self.dirty = true;
    }

    /// Advances the debounce and drains worker messages. Called from `update`.
    pub fn search_tick(&mut self) {
        if !self.search.open {
            return;
        }
        if self
            .search
            .pending
            .is_some_and(|at| at.elapsed().as_millis() >= 120)
        {
            self.search_start();
        }
        let mut done = false;
        if let Some(worker) = &self.search.worker {
            for _ in 0..64 {
                match worker.try_recv() {
                    Some(SearchMessage::Hits(batch)) => {
                        self.search.hits.extend(batch);
                        if self.search.hits.len() >= crate::model::search::MAX_HITS {
                            self.search.hits.truncate(crate::model::search::MAX_HITS);
                            worker.cancel();
                        }
                        self.search.selected = self
                            .search
                            .selected
                            .min(self.search.hits.len().saturating_sub(1));
                        self.dirty = true;
                    }
                    Some(SearchMessage::Progress { files }) => {
                        self.search.files_scanned = files;
                        self.dirty = true;
                    }
                    Some(SearchMessage::Done { files }) => {
                        self.search.files_scanned = files;
                        done = true;
                        self.dirty = true;
                        break;
                    }
                    None => break,
                }
            }
        }
        if done {
            self.search.worker = None;
            self.search.scanning = false;
        }
    }

    pub fn search_busy(&self) -> bool {
        self.search.scanning || self.search.pending.is_some()
    }

    pub fn search_move(&mut self, delta: isize) {
        if self.search.hits.is_empty() {
            return;
        }
        let last = self.search.hits.len() - 1;
        let selected = self.search.selected as isize + delta;
        self.search.selected = selected.clamp(0, last as isize) as usize;
        self.dirty = true;
    }

    /// Opens the selected hit and selects the match.
    pub fn search_accept(&mut self) {
        let Some(hit) = self.search.hits.get(self.search.selected).cloned() else {
            return;
        };
        let file_name = hit
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = hit.line + 1;
        self.open_path(hit.path);
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_caret(hit.offset);
            doc.extend_to(hit.offset + (hit.end_in_line.saturating_sub(hit.start_in_line)));
        }
        self.editor.reveal = true;
        self.focus = Focus::Editor;
        self.status(format!("{file_name}:{line}"));
        self.dirty = true;
    }

    // -- reliability -----------------------------------------------------

    /// Autosave + recovery flush after the idle delay; tracks edits to reset it.
    pub fn tick_reliability(&mut self) {
        if self.recovery_prompt.is_some() || self.modal.is_some() {
            return;
        }
        let mut changed = false;
        for doc in &self.ws.tabs {
            let id = doc.id();
            let rev = doc.rev();
            match self.doc_revs.get(&id) {
                Some(known) if *known == rev => {}
                _ => {
                    self.doc_revs.insert(id, rev);
                    changed = true;
                }
            }
        }
        let live: std::collections::HashSet<u64> =
            self.ws.tabs.iter().map(|doc| doc.id()).collect();
        self.doc_revs.retain(|id, _| live.contains(id));
        if changed {
            self.last_activity = Instant::now();
        }
        if self.last_activity.elapsed().as_secs_f32() < 2.0 {
            return;
        }
        self.flush_reliability();
        self.last_activity = Instant::now();
    }

    /// Runs autosave + snapshots immediately (tests / shutdown paths).
    pub fn force_reliability_flush(&mut self) {
        self.flush_reliability();
    }

    fn flush_reliability(&mut self) {
        let mut saved = 0usize;
        let mut saved_paths = Vec::new();
        if self.settings.autosave {
            for doc in &mut self.ws.tabs {
                if doc.is_dirty() && doc.path().is_some() && doc.save().is_ok() {
                    saved += 1;
                    if let Some(path) = doc.path().map(Path::to_path_buf) {
                        saved_paths.push(path);
                    }
                }
            }
        }
        for path in saved_paths {
            self.remember_disk(&path);
        }
        let mut live = std::collections::HashSet::new();
        for doc in &self.ws.tabs {
            live.insert(doc.id());
            if doc.is_dirty() {
                let _ = self.recovery.save(
                    doc.id(),
                    doc.name(),
                    doc.path(),
                    doc.text(),
                    doc.caret(),
                    doc.anchor(),
                );
            } else {
                self.recovery.remove(doc.id());
            }
        }
        for entry in self.recovery.list() {
            if !live.contains(&entry.id) {
                self.recovery.remove(entry.id);
            }
        }
        if saved > 0 {
            self.status(format!("autosaved {saved} file(s)"));
            self.sync_session_tabs();
        }
    }

    /// Applies the startup recovery prompt.
    pub fn resolve_recovery(&mut self, restore: bool) {
        let Some(entries) = self.recovery_prompt.take() else {
            return;
        };
        if restore {
            let mut last = None;
            for entry in &entries {
                let existing = entry.path.as_ref().and_then(|path| {
                    self.ws
                        .tabs
                        .iter()
                        .position(|doc| doc.path() == Some(path.as_path()))
                });
                if let Some(index) = existing {
                    if let Some(doc) = self.ws.tabs.get_mut(index) {
                        doc.restore_snapshot(entry.text.clone(), entry.caret, entry.anchor);
                    }
                    last = Some(index);
                } else {
                    let index = self.ws.add_recovered(entry);
                    if let Some(doc) = self.ws.tabs.get(index) {
                        self.doc_revs.insert(doc.id(), doc.rev());
                    }
                    last = Some(index);
                }
            }
            if let Some(index) = last {
                self.ws.select(index);
            }
            self.recovery.clear();
            self.after_tabs_changed();
            self.status(format!("restored {} buffer(s)", entries.len()));
        } else {
            self.recovery.clear();
            self.status("discarded recovered buffers");
        }
        self.dirty = true;
    }

    /// Persists window size/position (logical pixels).
    pub fn set_window_geometry(&mut self, size: (f32, f32), position: (f32, f32)) {
        let geometry = WindowGeometry { size, position };
        if self.session.window != Some(geometry) {
            self.session.window = Some(geometry);
            self.config_dirty = true;
        }
    }

    // -- settings --------------------------------------------------------

    /// Editor typography derived from the current settings.
    pub fn editor_metrics(&self) -> EditorMetrics {
        self.settings.editor()
    }

    /// The override for a document (path first, then untitled id).
    pub fn language_override_for(&self, doc: &Document) -> Option<&str> {
        doc.path()
            .and_then(|path| self.language_by_path.get(path))
            .or_else(|| self.language_by_id.get(&doc.id()))
            .map(String::as_str)
    }

    /// The active document's override, if any.
    pub fn language_override_of_active(&self) -> Option<&str> {
        self.ws
            .active_doc()
            .and_then(|doc| self.language_override_for(doc))
    }

    /// Provider for the active document (override, then extension).
    pub fn language_of_active(&self) -> Option<Arc<dyn SyntaxProvider>> {
        let doc = self.ws.active_doc()?;
        self.language_override_for(doc)
            .and_then(|name| self.syntax.by_name(name))
            .or_else(|| self.syntax.for_path(doc.path()))
    }

    /// Provider name for the active document (shown in the status bar), even
    /// when highlighting is disabled by a size limit.
    pub fn language_name(&self) -> Option<&'static str> {
        self.language_of_active().map(|provider| provider.name())
    }

    /// Sets (or clears with `None`) the active document's language override.
    pub fn set_language(&mut self, name: Option<String>) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if let Some(name) = &name {
            if self.syntax.by_name(name).is_none() {
                self.error(format!("unknown language: {name}"));
                return;
            }
        }
        let id = doc.id();
        let path = doc.path().map(Path::to_path_buf);
        match (path, name) {
            (Some(path), Some(name)) => {
                self.language_by_path.insert(path, name.clone());
                self.status(format!("language: {name}"));
            }
            (Some(path), None) => {
                self.language_by_path.remove(&path);
                self.status("language: auto");
            }
            (None, Some(name)) => {
                self.language_by_id.insert(id, name.clone());
                self.status(format!("language: {name}"));
            }
            (None, None) => {
                self.language_by_id.remove(&id);
                self.status("language: auto");
            }
        }
        self.sync_languages_session();
        self.highlight.reset();
        self.find.dirty = true;
        self.dirty = true;
    }

    fn sync_languages_session(&mut self) {
        let mut entries: Vec<(PathBuf, String)> = self
            .language_by_path
            .iter()
            .map(|(path, name)| (path.clone(), name.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.session.languages = entries;
        self.config_dirty = true;
    }

    /// Keeps the editor and preview in step while in Split view. One side is
    /// authoritative per frame (whichever changed), so the pair never loops.
    pub fn sync_scroll_positions(&mut self) {
        let split = self.view_of_active() == ViewMode::Split;
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let rev = doc.rev();
        let line_h = self.editor_metrics().line_h.max(1.0);
        let editor_scroll = doc.scroll_y;
        let preview_scroll = self.preview.scroll;
        let anchors = self.preview.anchors.clone();
        if !split || !self.sync_scroll || rev != self.sync_rev || anchors.len() < 2 {
            self.last_editor_scroll = editor_scroll;
            self.last_preview_scroll = preview_scroll;
            self.sync_rev = rev;
            return;
        }
        let soft_wrap = self.session.soft_wrap
            && !self.editor.wrap.is_empty()
            && self.editor.wrap.key().is_some();
        let editor_line = if soft_wrap {
            let row = (editor_scroll / line_h).floor() as usize;
            self.editor
                .wrap
                .line_of_row(row.min(self.editor.wrap.len().saturating_sub(1)))
                .unwrap_or(0) as f32
        } else {
            editor_scroll / line_h
        };
        let editor_changed = (editor_scroll - self.last_editor_scroll).abs() > 0.5;
        let preview_changed = (preview_scroll - self.last_preview_scroll).abs() > 0.5;
        if editor_changed && !preview_changed {
            self.preview.scroll = preview_y_for_line(&anchors, editor_line);
        } else if preview_changed && !editor_changed {
            let line = line_for_preview_y(&anchors, preview_scroll);
            if let Some(doc) = self.ws.active_doc_mut() {
                if soft_wrap {
                    let row = self.editor.wrap.first_row_of_line(line.floor() as usize);
                    doc.scroll_y = row as f32 * line_h;
                } else {
                    doc.scroll_y = line * line_h;
                }
            }
        }
        self.last_editor_scroll = self
            .ws
            .active_doc()
            .map_or(editor_scroll, |doc| doc.scroll_y);
        self.last_preview_scroll = self.preview.scroll;
        self.sync_rev = rev;
        self.dirty = true;
    }

    /// Enables/disables soft wrap, refusing files outside the hard limits.
    pub fn set_soft_wrap(&mut self, on: bool) {
        let was_on = self.session.soft_wrap;
        if on && !was_on {
            if let Some(doc) = self.ws.active_doc() {
                self.editor.wrap_scroll_x = doc.scroll_x;
            }
        }
        if !on && was_on {
            if let Some(doc) = self.ws.active_doc_mut() {
                doc.scroll_x = self.editor.wrap_scroll_x;
            }
        }
        if on {
            if let Some(doc) = self.ws.active_doc() {
                if doc.text().len() > crate::model::wrap::MAX_WRAP_BYTES
                    || doc.line_count() > crate::model::wrap::MAX_WRAP_LINES
                {
                    self.error(format!(
                        "soft wrap is off above {} MB / {}k lines",
                        crate::model::wrap::MAX_WRAP_BYTES / (1024 * 1024),
                        crate::model::wrap::MAX_WRAP_LINES / 1000
                    ));
                    return;
                }
            }
        }
        if self.session.soft_wrap != on {
            self.session.soft_wrap = on;
            self.config_dirty = true;
            self.editor.reveal = on;
            self.status(if on { "soft wrap on" } else { "soft wrap off" });
            self.dirty = true;
        }
    }

    pub fn toggle_sync_scroll(&mut self) {
        self.sync_scroll = !self.sync_scroll;
        self.session.sync_scroll = self.sync_scroll;
        self.config_dirty = true;
        self.status(format!(
            "scroll sync: {}",
            if self.sync_scroll { "ON" } else { "OFF" }
        ));
        self.dirty = true;
    }

    pub fn open_settings(&mut self) {
        self.settings_open = true;
        self.quick_open.open = false;
        if self.search.open {
            self.close_project_search();
        }
        self.menu_open = None;
        self.settings_row = self.settings_row.min(Row::Reset as usize);
        self.dirty = true;
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
        self.dirty = true;
    }

    pub fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    /// Display value of a settings row.
    pub fn settings_value(&self, row: Row) -> String {
        match row {
            Row::Theme => self.theme.name().to_string(),
            Row::FontSize => format!("{} px", self.settings.font_size as i32),
            Row::TabWidth => format!("{} spaces", self.settings.tab_width),
            Row::LineNumbers => on_off(self.settings.line_numbers),
            Row::IndentGuides => on_off(self.settings.indent_guides),
            Row::Syntax => on_off(self.settings.syntax),
            Row::Autosave => on_off(self.settings.autosave),
            Row::SidebarWidth => format!("{} px", self.settings.sidebar_width as i32),
            Row::Reset => "DEFAULT".into(),
        }
    }

    /// Steps a row (`delta` is -1 or +1). Returns whether anything changed.
    pub fn adjust_setting(&mut self, row: Row, delta: i32) -> bool {
        match row {
            Row::Theme => {
                if delta != 0 {
                    self.theme.toggle();
                    self.settings_changed();
                }
            }
            Row::FontSize => {
                self.settings.step_font(delta);
                self.settings_changed();
            }
            Row::TabWidth => {
                self.settings.step_tab(delta);
                self.settings_changed();
            }
            Row::LineNumbers => {
                self.settings.line_numbers = !self.settings.line_numbers;
                self.settings_changed();
            }
            Row::IndentGuides => {
                self.settings.indent_guides = !self.settings.indent_guides;
                self.settings_changed();
            }
            Row::Syntax => {
                self.settings.syntax = !self.settings.syntax;
                self.settings_changed();
            }
            Row::Autosave => {
                self.settings.autosave = !self.settings.autosave;
                self.settings_changed();
            }
            Row::SidebarWidth => {
                self.settings.step_sidebar(delta as f32 * 8.0);
                self.settings_changed();
                self.pending_sidebar_sync = true;
            }
            Row::Reset => self.reset_settings(),
        }
        self.dirty = true;
        true
    }

    /// Enter/Space on a row: toggles booleans, resets, or steps other rows.
    pub fn activate_setting(&mut self, row: Row) {
        self.adjust_setting(row, 1);
    }

    pub fn reset_settings(&mut self) {
        self.settings = Settings::default();
        self.settings_changed();
        self.pending_sidebar_sync = true;
        self.status("settings reset");
    }

    /// Writes the current settings into the session (call after a live drag).
    pub fn persist_settings(&mut self) {
        self.settings_changed();
    }

    fn settings_changed(&mut self) {
        self.session.font = self.settings.font_size;
        self.session.tab = self.settings.tab_width;
        self.session.line_numbers = self.settings.line_numbers;
        self.session.indent_guides = self.settings.indent_guides;
        self.session.syntax = self.settings.syntax;
        self.session.autosave = self.settings.autosave;
        self.session.sidebar_width = self.settings.sidebar_width;
        self.session.theme = match self.theme.mode {
            Mode::Dark => "dark".into(),
            Mode::Light => "light".into(),
        };
        self.config_dirty = true;
        self.find.dirty = true;
        self.editor.reveal = true;
        self.dirty = true;
    }

    /// Applies settings-driven work for this frame: incremental highlighting and
    /// bracket matching. Returns whether more highlight work remains.
    pub fn tick_analysis(&mut self) -> bool {
        if self.pending_sidebar_sync {
            self.pending_sidebar_sync = false;
        }
        let language_override = self.language_override_of_active().map(str::to_string);
        let behind = {
            let AppState {
                ws,
                settings,
                syntax,
                highlight,
                ..
            } = self;
            match ws.active_doc() {
                Some(doc) => {
                    highlight.sync(
                        doc,
                        syntax,
                        settings,
                        crate::model::highlight::FRAME_LINE_BUDGET,
                        language_override.as_deref(),
                    );
                    highlight.is_behind()
                }
                None => {
                    highlight.reset();
                    false
                }
            }
        };

        let key = self.ws.active_doc().map(|doc| {
            (
                doc.id(),
                doc.rev(),
                doc.caret(),
                self.settings.syntax && self.highlight.is_active(),
            )
        });
        if key != self.bracket_key {
            self.bracket_key = key;
            self.bracket = self.ws.active_doc().and_then(|doc| {
                match_bracket(doc.text(), doc.caret(), |byte| {
                    let line = doc.line_of(byte);
                    let offset = byte - doc.line_start(line);
                    match self.highlight.tokens(line) {
                        Some(tokens) => !tokens.iter().any(|token| {
                            token.start <= offset
                                && offset < token.end
                                && matches!(token.kind, TokenKind::Comment | TokenKind::String)
                        }),
                        None => true,
                    }
                })
            });
        }
        behind
    }

    // -- view mode -------------------------------------------------------

    pub fn view_of_active(&self) -> ViewMode {
        let Some(doc) = self.ws.active_doc() else {
            return ViewMode::Edit;
        };
        self.doc_views
            .get(&doc.id())
            .map(|ui| ui.view)
            .unwrap_or_else(|| ViewMode::for_path(doc.path()))
    }

    /// Split ratio for the active document (clamped 0.2–0.8).
    pub fn split_of_active(&self) -> f32 {
        self.ws
            .active_doc()
            .and_then(|doc| self.doc_views.get(&doc.id()))
            .map(|ui| ui.split_ratio)
            .unwrap_or(self.session.split)
            .clamp(0.2, 0.8)
    }

    pub fn set_view(&mut self, mode: ViewMode) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let id = doc.id();
        let default_ratio = self.session.split.clamp(0.2, 0.8);
        self.doc_views
            .entry(id)
            .or_insert(DocUi {
                view: ViewMode::for_path(doc.path()),
                split_ratio: default_ratio,
            })
            .view = mode;
        self.sync_layout();
        self.status(format!("view: {}", mode.label()));
    }

    /// Sets the split ratio for the active document and remembers it globally.
    pub fn set_split_ratio(&mut self, ratio: f32) {
        let ratio = ratio.clamp(0.2, 0.8);
        if let Some(doc) = self.ws.active_doc() {
            let id = doc.id();
            let default_ratio = self.session.split.clamp(0.2, 0.8);
            let entry = self.doc_views.entry(id).or_insert(DocUi {
                view: ViewMode::for_path(doc.path()),
                split_ratio: default_ratio,
            });
            entry.split_ratio = ratio;
        }
        self.session.split = ratio;
        self.config_dirty = true;
        self.dirty = true;
    }

    /// Rebuild the shell when the pane structure the active document needs
    /// differs from the one currently mounted.
    pub fn sync_layout(&mut self) {
        let view = self.view_of_active();
        let ratio = self.split_of_active();
        let needs = view != self.built_view
            || (view == ViewMode::Split && (ratio - self.built_split).abs() > 0.001);
        if needs {
            self.needs_rebuild = true;
        }
    }

    // -- document lifecycle ---------------------------------------------

    /// Closes every tab except `keep` (context menu). Dirty targets require a
    /// single discard confirmation.
    pub fn request_close_others(&mut self, keep: usize) {
        if keep >= self.ws.tabs.len() || self.ws.tabs.len() <= 1 {
            return;
        }
        self.request_close_many(Some(keep), 0);
    }

    /// Closes every tab to the right of `from` (context menu).
    pub fn request_close_right(&mut self, from: usize) {
        if from + 1 >= self.ws.tabs.len() {
            return;
        }
        self.request_close_many(None, from + 1);
    }

    fn request_close_many(&mut self, keep: Option<usize>, from: usize) {
        let targets: Vec<usize> = (from..self.ws.tabs.len())
            .filter(|index| keep != Some(*index))
            .collect();
        if targets.is_empty() {
            return;
        }
        let dirty = targets.iter().any(|index| self.ws.tabs[*index].is_dirty());
        self.tab_menu = None;
        self.menu_open = None;
        if dirty {
            self.modal = Some(Modal::CloseTabs { keep, from });
        } else {
            self.close_tabs_now(keep, from);
        }
        self.dirty = true;
    }

    fn close_tabs_now(&mut self, keep: Option<usize>, from: usize) {
        let mut targets: Vec<usize> = (from..self.ws.tabs.len())
            .filter(|index| keep != Some(*index))
            .collect();
        let count = targets.len();
        targets.sort_unstable_by(|a, b| b.cmp(a));
        for index in targets {
            self.close_tab_now(index);
        }
        if count > 0 {
            self.status(format!(
                "closed {count} tab{}",
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    /// Runs a tab context-menu command.
    pub fn run_tab_menu(&mut self, action: TabMenuAction, tab: usize) {
        self.tab_menu = None;
        match action {
            TabMenuAction::Close => self.request_close_tab(tab),
            TabMenuAction::CloseOthers => self.request_close_others(tab),
            TabMenuAction::CloseRight => self.request_close_right(tab),
            TabMenuAction::CopyPath => {
                let Some(path) = self
                    .ws
                    .tabs
                    .get(tab)
                    .and_then(|doc| doc.path().map(Path::to_path_buf))
                else {
                    self.error("untitled buffer has no path");
                    return;
                };
                let text = path.display().to_string();
                self.set_clipboard(&text);
                self.status(format!("copied {text}"));
            }
            TabMenuAction::RevealInSidebar => self.reveal_tab(tab),
        }
        self.dirty = true;
    }

    fn reveal_tab(&mut self, tab: usize) {
        let Some(path) = self
            .ws
            .tabs
            .get(tab)
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        else {
            self.error("untitled buffer has no path");
            return;
        };
        if !self.ws.folder_open || !path.starts_with(&self.ws.root) {
            self.error("file is outside the open folder");
            return;
        }
        let Some(parent) = path.parent() else {
            return;
        };
        self.ws.tree.expand_to(parent);
        let row_h = self.theme.m.row_h;
        let rows = self.ws.tree.rows();
        if let Some(index) = rows.iter().position(|row| row.path == path) {
            let view_h = (self.sidebar.rect.size.height - crate::ui::sidebar::HEADER_H).max(0.0);
            self.sidebar.scroll = (index as f32 * row_h - view_h * 0.5).max(0.0);
        }
        self.sidebar.selected = Some(path.clone());
        if !self.show_sidebar {
            self.show_sidebar = true;
            self.session.sidebar = true;
            self.config_dirty = true;
            self.needs_rebuild = true;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.status(format!("revealed {name}"));
    }

    /// Called when macOS asks to quit with dirty tabs.
    pub fn request_quit(&mut self, dirty: usize) {
        self.tab_menu = None;
        self.tab_drag = None;
        self.menu_open = None;
        self.modal = Some(Modal::Quit { dirty });
        self.dirty = true;
    }

    /// Closes the active tab, asking for confirmation when it is dirty.
    pub fn request_close_active(&mut self) {
        self.request_close_tab(self.ws.active);
    }

    /// Closes tab `index`, asking for confirmation when it is dirty.
    pub fn request_close_tab(&mut self, index: usize) {
        let Some(doc) = self.ws.tabs.get(index) else {
            return;
        };
        if doc.is_dirty() {
            self.ws.select(index);
            self.tabs.reveal_active = true;
            self.modal = Some(Modal::CloseDirtyTab { tab: index });
            self.menu_open = None;
            self.dirty = true;
        } else {
            self.close_tab_now(index);
        }
    }

    /// Closes `index` (bypassing the dirty check) and refreshes the session.
    pub fn close_tab_now(&mut self, index: usize) {
        let id = self.ws.tabs.get(index).map(|doc| doc.id());
        self.ws.force_close(index);
        if let Some(id) = id {
            self.doc_revs.remove(&id);
            self.recovery.remove(id);
            self.language_by_id.remove(&id);
        }
        self.after_tabs_changed();
    }

    /// Re-opens the workspace after a tab change.
    pub fn after_tabs_changed(&mut self) {
        self.ws.validate_active();
        self.tabs.reveal_active = true;
        self.request_git_refresh();
        self.sync_layout();
        self.sync_session_tabs();
        self.find.dirty = true;
        self.focus = Focus::Editor;
        self.dirty = true;
    }

    pub fn clear_recent(&mut self) {
        if self.session.recent.is_empty() {
            return;
        }
        self.session.recent.clear();
        self.config_dirty = true;
        crate::macos::clear_system_recent();
        self.status("recent files cleared");
        self.dirty = true;
    }

    pub fn open_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        match self.ws.open(&path) {
            Ok(_) => {
                let name = self
                    .ws
                    .active_doc()
                    .map(|doc| doc.name().to_string())
                    .unwrap_or_default();
                self.sidebar.selected = Some(path.clone());
                self.tabs.reveal_active = true;
                self.request_git_refresh();
                self.remember_disk(&path);
                self.session.push_recent(&path);
                crate::macos::note_recent(&path);
                self.config_dirty = true;
                self.sync_layout();
                self.sync_session_tabs();
                self.find.dirty = true;
                self.focus = Focus::Editor;
                self.editor.preedit = None;
                self.editor.drag = Drag::None;
                self.status(format!("opened {name}"));
            }
            Err(message) => self.error(message),
        }
        self.dirty = true;
    }

    pub fn save_active(&mut self) {
        match self.ws.save_active() {
            Ok(message) => {
                self.note_saved();
                self.status(message);
            }
            Err(message) => {
                if self.ws.active_doc().is_some_and(|doc| doc.path().is_none()) {
                    self.pending_dialog = Some(DialogRequest {
                        kind: DialogKind::SaveAs,
                        then_close: None,
                    });
                } else {
                    self.error(message);
                }
            }
        }
        self.dirty = true;
    }

    /// Applies a Save As path to the active tab.
    pub fn save_as(&mut self, path: PathBuf, then_close: Option<usize>) {
        match self.ws.save_active_as(path) {
            Ok(message) => {
                self.note_saved();
                self.status(message);
                if let Some(index) = then_close {
                    self.close_tab_now(index);
                }
            }
            Err(message) => self.error(message),
        }
        self.dirty = true;
    }

    fn note_saved(&mut self) {
        if let Some(doc) = self.ws.active_doc() {
            self.recovery.remove(doc.id());
        }
        if let Some(path) = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        {
            self.session.push_recent(&path);
            self.config_dirty = true;
            self.sidebar.selected = Some(path.clone());
            self.request_git_refresh();
            self.remember_disk(&path);
            crate::macos::note_recent(&path);
        }
        self.sync_session_tabs();
    }

    /// Start directory and suggested name for the next file dialog.
    pub fn dialog_context(&self) -> (Option<PathBuf>, String) {
        let start = self
            .ws
            .active_doc()
            .and_then(|doc| {
                doc.path()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
            .or_else(home_dir);
        let suggested = self
            .ws
            .active_doc()
            .map(|doc| doc.name().to_string())
            .unwrap_or_else(|| "untitled.txt".into());
        (start, suggested)
    }

    /// Records the open, saved tabs so a restart can restore them.
    pub fn sync_session_tabs(&mut self) {
        let tabs: Vec<PathBuf> = self
            .ws
            .tabs
            .iter()
            .filter_map(|doc| doc.path().map(Path::to_path_buf))
            .collect();
        self.session.set_tabs(tabs);
        self.config_dirty = true;
    }

    // -- find / replace --------------------------------------------------

    pub fn open_find(&mut self, replace: bool) {
        self.settings_open = false;
        self.quick_open.open = false;
        if self.search.open {
            self.close_project_search();
        }
        let was_open = self.find.open;
        self.find.open = true;
        self.focus = if replace { Focus::Replace } else { Focus::Find };
        self.find.query.select_all();
        self.find.dirty = true;
        if !was_open {
            self.needs_rebuild = true;
        }
        self.dirty = true;
    }

    pub fn close_find(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.preedit = None;
        self.find.matches.clear();
        self.find.current = None;
        self.focus = Focus::Editor;
        self.needs_rebuild = true;
        self.dirty = true;
    }

    pub fn find_field_mut(&mut self) -> Option<&mut TextEdit> {
        match self.focus {
            Focus::Find => Some(&mut self.find.query),
            Focus::Replace => Some(&mut self.find.replace),
            Focus::Editor => None,
        }
    }

    /// Recomputes matches when the query or the document changed.
    pub fn refresh_find(&mut self) {
        if !self.find.open {
            return;
        }
        let rev = self.ws.active_doc().map_or(u64::MAX, |doc| doc.rev());
        if !self.find.dirty && rev == self.find.rev {
            return;
        }
        let matcher = Matcher {
            query: self.find.query.text().to_string(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
        };
        self.find.matches = self
            .ws
            .active_doc()
            .map(|doc| find_all(doc.text(), &matcher))
            .unwrap_or_default();
        self.find.rev = rev;
        self.find.dirty = false;
        if self.find.matches.is_empty() {
            self.find.current = None;
        } else {
            let index = self
                .find
                .current
                .unwrap_or(0)
                .min(self.find.matches.len() - 1);
            self.find.current = Some(index);
        }
    }

    /// Moves to the next (`dir > 0`) or previous match, wrapping around.
    pub fn find_step(&mut self, dir: isize) {
        self.refresh_find();
        if self.find.matches.is_empty() {
            self.status("no matches");
            return;
        }
        let count = self.find.matches.len();
        let current = self.find.current.unwrap_or(0) as isize;
        let next = if dir >= 0 {
            (current + 1) % count as isize
        } else {
            (current - 1 + count as isize) % count as isize
        };
        self.find.current = Some(next as usize);
        let (start, end) = self.find.matches[next as usize];
        if let Some(doc) = self.ws.active_doc_mut() {
            doc.set_caret(start);
            doc.extend_to(end);
        }
        self.editor.reveal = true;
        self.dirty = true;
    }

    /// Selects the current match (or the first one) without stepping.
    pub fn find_select_current(&mut self) {
        self.refresh_find();
        let Some(index) = self.find.current else {
            return;
        };
        if let Some(&(start, end)) = self.find.matches.get(index) {
            if let Some(doc) = self.ws.active_doc_mut() {
                doc.set_caret(start);
                doc.extend_to(end);
            }
            self.editor.reveal = true;
            self.dirty = true;
        }
    }

    /// Replaces the current match with the replace text.
    pub fn replace_one(&mut self) {
        self.refresh_find();
        let Some(index) = self.find.current else {
            self.status("no matches");
            return;
        };
        let Some(&(start, end)) = self.find.matches.get(index) else {
            return;
        };
        let replacement = self.find.replace.text().to_string();
        if let Some(doc) = self.ws.active_doc_mut() {
            if doc.replace_range(start, end, &replacement) {
                self.editor.reveal = true;
            }
        }
        self.find.dirty = true;
        self.refresh_find();
        self.find_select_current();
    }

    /// Replaces every match.
    pub fn replace_all(&mut self) {
        self.refresh_find();
        if self.find.matches.is_empty() {
            self.status("no matches");
            return;
        }
        let replacement = self.find.replace.text().to_string();
        // Overlapping matches (e.g. "aa" in "aaaa") share bytes; keep the first
        // non-overlapping set so the replacement cannot clobber itself.
        let mut chosen: Vec<(usize, usize)> = Vec::new();
        let mut last_end = 0;
        for &(start, end) in &self.find.matches {
            if start >= last_end {
                chosen.push((start, end));
                last_end = end;
            }
        }
        let Some(doc) = self.ws.active_doc_mut() else {
            return;
        };
        let mut replaced = 0;
        for (start, end) in chosen.iter().rev() {
            if doc.replace_range(*start, *end, &replacement) {
                replaced += 1;
            }
        }
        self.find.dirty = true;
        self.find.current = None;
        self.refresh_find();
        self.status(format!("replaced {replaced}"));
        self.dirty = true;
    }

    /// Applies a confirmation-modal choice.
    pub fn resolve_modal(&mut self, button: ModalButton) {
        let Some(modal) = self.modal else {
            return;
        };
        if let Modal::ReplaceInFiles { .. } = modal {
            match button {
                ModalButton::Cancel => {
                    self.pending_replace = None;
                    self.modal = None;
                }
                ModalButton::Apply => {
                    let Some(job) = self.pending_replace.take() else {
                        self.modal = None;
                        return;
                    };
                    let applied = replace::apply(job.plans);
                    self.modal = None;
                    let mut message = format!(
                        "replaced {} in {} file(s)",
                        applied.occurrences, applied.files
                    );
                    if !applied.errors.is_empty() {
                        message.push_str(&format!("; {} skipped", applied.errors.len()));
                        if let Some(first) = applied.errors.first() {
                            message.push_str(&format!(" ({first})"));
                        }
                    }
                    self.status(message);
                    self.search_query_changed();
                    self.dirty = true;
                }
                _ => {}
            }
            return;
        }
        if let Modal::Quit { .. } = modal {
            self.modal = None;
            crate::macos::reply_quit(button == ModalButton::Discard);
            self.dirty = true;
            return;
        }
        if let Modal::ExternalChange { tab } = modal {
            self.modal = None;
            if button == ModalButton::Reload {
                self.reload_tab(tab, true);
            } else {
                self.status("kept the buffer");
            }
            self.dirty = true;
            return;
        }
        if let Modal::CloseTabs { keep, from } = modal {
            self.modal = None;
            if button == ModalButton::Discard {
                self.close_tabs_now(keep, from);
            }
            self.dirty = true;
            return;
        }
        let Some(Modal::CloseDirtyTab { tab }) = self.modal else {
            return;
        };
        self.modal = None;
        match button {
            ModalButton::Cancel => {}
            ModalButton::Discard => self.close_tab_now(tab),
            ModalButton::Save => {
                let has_path = self
                    .ws
                    .tabs
                    .get(tab)
                    .is_some_and(|doc| doc.path().is_some());
                if !has_path {
                    self.ws.select(tab);
                    self.tabs.reveal_active = true;
                    self.pending_dialog = Some(DialogRequest {
                        kind: DialogKind::SaveAs,
                        then_close: Some(tab),
                    });
                    return;
                }
                match self.ws.save_tab(tab) {
                    Ok(message) => {
                        self.note_saved();
                        self.status(message);
                        self.close_tab_now(tab);
                    }
                    Err(message) => self.error(message),
                }
            }
            ModalButton::Apply | ModalButton::Reload => {}
        }
        self.dirty = true;
    }

    /// Opens a folder as the workspace (Open Folder / dropped folder).
    pub fn set_root(&mut self, path: PathBuf) {
        if let Err(message) = self.ws.set_folder(path) {
            self.error(message);
            return;
        }
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = None;
        self.settings_open = false;
        self.session.folder = Some(self.ws.root.clone());
        self.request_git_refresh();
        self.quick_open.index = None;
        if self.quick_open.open {
            self.close_quick_open();
        }
        if self.search.open {
            self.close_project_search();
        }
        self.config_dirty = true;
        let name = self
            .ws
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.ws.root.display().to_string());
        self.status(format!("folder: {name}"));
        self.dirty = true;
    }

    fn remember_disk(&mut self, path: &Path) {
        self.external.insert(path.to_path_buf(), disk_stamp(path));
    }

    /// Polls metadata for every open file and reacts to outside edits. Clean
    /// tabs reload automatically; dirty tabs ask before losing their changes.
    pub fn check_external_changes(&mut self) {
        if self.modal.is_some() || self.recovery_prompt.is_some() {
            return;
        }
        for index in 0..self.ws.tabs.len() {
            let Some(path) = self
                .ws
                .tabs
                .get(index)
                .and_then(|doc| doc.path().map(Path::to_path_buf))
            else {
                continue;
            };
            let current = disk_stamp(&path);
            let Some(previous) = self.external.get(&path).copied() else {
                self.external.insert(path, current);
                continue;
            };
            if previous == current {
                continue;
            }
            self.external.insert(path.clone(), current);
            if current.is_none() {
                let name = self.ws.tabs[index].name().to_string();
                self.error(format!("{name} was deleted on disk"));
                self.dirty = true;
                continue;
            }
            let dirty = self.ws.tabs.get(index).is_some_and(|doc| doc.is_dirty());
            if !dirty {
                self.reload_tab(index, false);
            } else {
                self.modal = Some(Modal::ExternalChange { tab: index });
                self.menu_open = None;
                self.tab_menu = None;
                self.dirty = true;
                return;
            }
        }
    }

    /// Runs the 1-second metadata poll.
    pub fn tick_external_changes(&mut self) {
        if self.last_external_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_external_check = Instant::now();
        self.check_external_changes();
    }

    fn reload_tab(&mut self, tab: usize, report: bool) {
        let Some(path) = self
            .ws
            .tabs
            .get(tab)
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(doc) = self.ws.tabs.get_mut(tab) else {
            return;
        };
        let name = doc.name().to_string();
        match doc.reload() {
            Ok(()) => {
                self.remember_disk(&path);
                self.highlight.reset();
                self.find.dirty = true;
                self.sync_layout();
                if report {
                    self.status(format!("reloaded {name} from disk"));
                } else {
                    self.status(format!("{name} changed on disk — reloaded"));
                }
                self.dirty = true;
            }
            Err(error) => self.error(format!("reload {name} failed: {error}")),
        }
    }

    /// Closes the sidebar folder, keeping the open tabs as standalone files.
    pub fn close_folder(&mut self) {
        if !self.ws.close_folder() {
            self.error("no folder open");
            return;
        }
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = None;
        self.sidebar.actions.clear();
        self.session.folder = None;
        self.request_git_refresh();
        self.config_dirty = true;
        if self.quick_open.open {
            self.close_quick_open();
        }
        self.quick_open.index = None;
        if self.search.open {
            self.close_project_search();
        }
        self.status("folder closed");
        self.dirty = true;
    }

    /// Opens dropped paths: a folder becomes the workspace, files become tabs.
    pub fn open_dropped(&mut self, paths: Vec<PathBuf>) {
        let mut folder = None;
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                if folder.is_none() {
                    folder = Some(path);
                }
            } else if path.is_file() {
                files.push(path);
            }
        }
        if let Some(folder) = folder {
            self.set_root(folder);
        }
        let had_files = !files.is_empty();
        let mut opened = 0usize;
        for file in files.into_iter().take(20) {
            self.open_path(file);
            opened += 1;
        }
        if opened > 0 {
            self.focus = Focus::Editor;
            self.status(format!(
                "opened {opened} dropped file{}",
                if opened == 1 { "" } else { "s" }
            ));
        } else if self.ws.folder_open {
            if !had_files {
                self.status("folder dropped");
            }
        } else if !had_files {
            self.status("nothing to open");
        }
        self.dirty = true;
    }

    /// Plans a replace-in-files run from the current hits and asks for
    /// confirmation. Open tabs are skipped.
    pub fn request_replace_in_files(&mut self, replacement: String) {
        if self.search.hits.is_empty() {
            self.error("nothing to replace");
            return;
        }
        let matcher = Matcher {
            query: self.search.query.text().trim().to_string(),
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
        };
        if matcher.query.is_empty() || replacement.is_empty() {
            self.error("enter a search term and a replacement");
            return;
        }
        let paths = self.search.hits.iter().map(|hit| hit.path.clone());
        let open: std::collections::HashSet<PathBuf> = self
            .ws
            .tabs
            .iter()
            .filter_map(|doc| doc.path().map(Path::to_path_buf))
            .collect();
        let (plans, errors) =
            replace::plan(paths, &matcher, &replacement, |path| open.contains(path));
        if plans.is_empty() {
            let detail = errors
                .first()
                .cloned()
                .unwrap_or_else(|| "no matches".into());
            self.error(format!("nothing replaced: {detail}"));
            return;
        }
        let files = plans.len();
        let occurrences: usize = plans.iter().map(|plan| plan.count).sum();
        self.pending_replace = Some(ReplaceJob {
            plans,
            replacement,
            files,
            occurrences,
        });
        self.modal = Some(Modal::ReplaceInFiles { files, occurrences });
        self.menu_open = None;
        self.dirty = true;
    }

    // -- actions ---------------------------------------------------------

    /// Runs `action` and reports the outcome in the status bar.
    pub fn run(&mut self, action: Action) {
        self.tab_menu = None;
        match action {
            Action::New => {
                self.ws.new_tab();
                self.tabs.reveal_active = true;
                self.sync_layout();
                self.sync_session_tabs();
                self.find.dirty = true;
                self.focus = Focus::Editor;
                self.status("new buffer");
            }
            Action::NewFile => self.open_new_file(),
            Action::Open => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::Open,
                    then_close: None,
                });
            }
            Action::OpenFolder => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::OpenFolder,
                    then_close: None,
                });
            }
            Action::CloseFolder => self.close_folder(),
            Action::Save => self.save_active(),
            Action::SaveAs => {
                self.pending_dialog = Some(DialogRequest {
                    kind: DialogKind::SaveAs,
                    then_close: None,
                });
            }
            Action::CloseTab => self.request_close_active(),
            Action::Undo => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.undo();
                }
                self.find.dirty = true;
            }
            Action::Redo => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.redo();
                }
                self.find.dirty = true;
            }
            Action::Cut => {
                if let Some(text) = self.selected_text() {
                    self.set_clipboard(&text);
                    if let Some(doc) = self.ws.active_doc_mut() {
                        doc.delete_selection();
                    }
                    self.find.dirty = true;
                }
            }
            Action::Copy => {
                if let Some(text) = self.selected_text() {
                    self.set_clipboard(&text);
                    self.status("copied");
                }
            }
            Action::Paste => match self.get_clipboard() {
                Some(text) => {
                    if let Some(doc) = self.ws.active_doc_mut() {
                        doc.insert_text(&text);
                    }
                    self.find.dirty = true;
                }
                None => self.error("clipboard is empty"),
            },
            Action::SelectAll => {
                if let Some(doc) = self.ws.active_doc_mut() {
                    doc.select_all();
                }
            }
            Action::Find => self.open_find(false),
            Action::QuickOpen => self.open_quick_open(),
            Action::ProjectSearch => self.open_project_search(),
            Action::Settings => self.toggle_settings(),
            Action::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                self.session.sidebar = self.show_sidebar;
                self.config_dirty = true;
                self.needs_rebuild = true;
            }
            Action::ToggleTheme => {
                self.theme.toggle();
                self.session.theme = match self.theme.mode {
                    Mode::Dark => "dark".into(),
                    Mode::Light => "light".into(),
                };
                self.config_dirty = true;
                self.status(format!("theme: {}", self.theme.name()));
            }
            Action::SetView(mode) => self.set_view(mode),
            Action::ToggleScrollSync => self.toggle_sync_scroll(),
            Action::ToggleSoftWrap => {
                let on = !self.session.soft_wrap;
                self.set_soft_wrap(on);
            }
            Action::SetLanguage(name) => self.set_language(name),
            Action::OpenRecent(path) => self.open_path(path),
            Action::ClearRecent => self.clear_recent(),
        }
        self.dirty = true;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.ws
            .active_doc()
            .and_then(|doc| doc.selected_text())
            .map(str::to_string)
    }

    pub fn get_clipboard(&self) -> Option<String> {
        self.clipboard.as_ref()?.borrow().get()
    }

    pub fn set_clipboard(&self, text: &str) {
        if let Some(clipboard) = &self.clipboard {
            clipboard.borrow_mut().set(text);
        }
    }

    /// Flushes the session when something persisted changed.
    pub fn save_session_if_dirty(&mut self) {
        if !self.config_dirty {
            return;
        }
        self.config_dirty = false;
        if let Err(error) = self.session.save(&self.config_path) {
            self.error(format!("session save failed: {error}"));
        }
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
