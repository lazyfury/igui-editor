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
use std::time::Instant;

use igui_core::{Rect, ViewportSize};
use igui_ui::{Clipboard, TextEdit};

use crate::model::find::{find_all, Matcher};
use crate::model::fs_tree::FsTree;
use crate::model::session::Session;
use crate::model::Workspace;
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
}

impl Menu {
    pub const ALL: [Menu; 3] = [Menu::File, Menu::Edit, Menu::View];

    pub const fn label(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
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

/// Every command the app can run, from a menu item or a shortcut.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    New,
    Open,
    OpenFolder,
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
    ToggleSidebar,
    ToggleTheme,
    SetView(ViewMode),
    OpenRecent(PathBuf),
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
            Action::Open => "Open…".into(),
            Action::OpenFolder => "Open Folder…".into(),
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
            Action::ToggleSidebar => "Toggle Sidebar".into(),
            Action::ToggleTheme => "Toggle Theme".into(),
            Action::SetView(mode) => mode.label().into(),
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
            Action::Open => match ACCEL {
                "⌘" => "⌘O",
                _ => "Ctrl+O",
            },
            Action::OpenFolder => match ACCEL {
                "⌘" => "⇧⌘O",
                _ => "Ctrl+Shift+O",
            },
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
            Action::OpenRecent(_) => "",
        }
    }

    pub fn enabled(&self, state: &AppState) -> bool {
        let doc = state.ws.active_doc();
        match self {
            Action::New
            | Action::Open
            | Action::OpenFolder
            | Action::ToggleSidebar
            | Action::ToggleTheme
            | Action::Find => true,
            Action::Save => doc.is_some(),
            Action::SaveAs => doc.is_some(),
            Action::CloseTab => doc.is_some(),
            Action::Undo => doc.is_some_and(|doc| doc.can_undo()),
            Action::Redo => doc.is_some_and(|doc| doc.can_redo()),
            Action::Cut | Action::Copy => doc.is_some_and(|doc| doc.has_selection()),
            Action::Paste => doc.is_some() && state.clipboard.is_some(),
            Action::SelectAll => doc.is_some(),
            Action::SetView(_) => doc.is_some(),
            Action::OpenRecent(path) => path.is_file(),
        }
    }

    pub fn checked(&self, state: &AppState) -> bool {
        match self {
            Action::SetView(mode) => *mode == state.view_of_active(),
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
                Some(Action::Open),
                Some(Action::OpenFolder),
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
        ],
        Menu::View => vec![
            Some(Action::ToggleSidebar),
            Some(Action::ToggleTheme),
            None,
            Some(Action::SetView(ViewMode::Edit)),
            Some(Action::SetView(ViewMode::Split)),
            Some(Action::SetView(ViewMode::Preview)),
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
}

/// Buttons of the confirmation modal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalButton {
    Cancel,
    Discard,
    Save,
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
}

/// Sidebar geometry captured by the last paint.
#[derive(Clone, Debug, Default)]
pub struct SidebarUi {
    pub rect: Rect,
    pub scroll: f32,
    pub hover: Option<usize>,
    pub selected: Option<PathBuf>,
}

/// Tab strip hit rectangles captured by the last paint.
#[derive(Clone, Debug, Default)]
pub struct TabsUi {
    /// `(tab index, whole tab, close button)`.
    pub hits: Vec<(usize, Rect, Rect)>,
    pub hover: Option<usize>,
    /// The `+` (new tab) button.
    pub plus: Rect,
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
    /// Set when the next frame should repaint (paint cache invalidation).
    pub dirty: bool,
    // -- M2 --------------------------------------------------------------
    pub focus: Focus,
    pub find: FindState,
    pub modal: Option<Modal>,
    pub modal_buttons: Vec<(ModalButton, Rect)>,
    pub pending_dialog: Option<DialogRequest>,
    /// Per-document view mode (keyed by `Document::id`).
    pub doc_views: HashMap<u64, ViewMode>,
    pub session: Session,
    pub config_path: PathBuf,
    pub config_dirty: bool,
    pub preview: PreviewUi,
    /// Window title last pushed to the platform.
    pub title: String,
}

impl AppState {
    pub fn new(theme: Theme, ws: Workspace, session: Session, config_path: PathBuf) -> Self {
        let title = String::new();
        Self {
            theme,
            ws,
            show_sidebar: session.sidebar,
            viewport: ViewportSize::default(),
            sidebar: SidebarUi::default(),
            editor: EditorUi {
                reveal: true,
                ..EditorUi::default()
            },
            tabs: TabsUi::default(),
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
        }
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

    // -- view mode -------------------------------------------------------

    pub fn view_of_active(&self) -> ViewMode {
        let Some(doc) = self.ws.active_doc() else {
            return ViewMode::Edit;
        };
        self.doc_views
            .get(&doc.id())
            .copied()
            .unwrap_or_else(|| ViewMode::for_path(doc.path()))
    }

    pub fn set_view(&mut self, mode: ViewMode) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let id = doc.id();
        self.doc_views.insert(id, mode);
        self.sync_layout();
        self.status(format!("view: {}", mode.label()));
    }

    /// Rebuild the shell when the pane structure the active document needs
    /// differs from the one currently mounted.
    pub fn sync_layout(&mut self) {
        let needs = self.view_of_active() != self.built_view;
        if needs {
            self.needs_rebuild = true;
        }
    }

    // -- document lifecycle ---------------------------------------------

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
            self.modal = Some(Modal::CloseDirtyTab { tab: index });
            self.menu_open = None;
            self.dirty = true;
        } else {
            self.close_tab_now(index);
        }
    }

    /// Closes `index` (bypassing the dirty check) and refreshes the session.
    pub fn close_tab_now(&mut self, index: usize) {
        self.ws.force_close(index);
        self.after_tabs_changed();
    }

    /// Re-opens the workspace after a tab change.
    pub fn after_tabs_changed(&mut self) {
        if self.ws.tabs.is_empty() {
            self.ws.new_tab();
        }
        self.ws.validate_active();
        self.sync_layout();
        self.sync_session_tabs();
        self.find.dirty = true;
        self.focus = Focus::Editor;
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
                self.session.push_recent(&path);
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
        if let Some(path) = self
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        {
            self.session.push_recent(&path);
            self.config_dirty = true;
            self.sidebar.selected = Some(path);
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
            .or_else(|| Some(self.ws.root.clone()));
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
        }
        self.dirty = true;
    }

    /// Points the sidebar at a new root directory (Open Folder), keeping tabs.
    pub fn set_root(&mut self, path: PathBuf) {
        let root = std::fs::canonicalize(&path).unwrap_or(path);
        if !root.is_dir() {
            self.error(format!("not a directory: {}", root.display()));
            return;
        }
        self.ws.root = root.clone();
        self.ws.tree = FsTree::new(&root);
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = None;
        self.status(format!("workspace: {}", root.display()));
        self.dirty = true;
    }

    // -- actions ---------------------------------------------------------

    /// Runs `action` and reports the outcome in the status bar.
    pub fn run(&mut self, action: Action) {
        match action {
            Action::New => {
                self.ws.new_tab();
                self.sync_layout();
                self.sync_session_tabs();
                self.find.dirty = true;
                self.focus = Focus::Editor;
                self.status("new buffer");
            }
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
            Action::OpenRecent(path) => self.open_path(path),
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
    fn discard_closes_and_leaves_an_untitled_buffer() {
        let mut state = state();
        state.ws.active_doc_mut().unwrap().insert_text("x");
        state.request_close_tab(0);
        state.resolve_modal(ModalButton::Discard);
        assert!(state.modal.is_none());
        assert_eq!(state.ws.tabs.len(), 1);
        let doc = state.ws.active_doc().unwrap();
        assert!(doc.name().starts_with("untitled"));
        assert!(doc.text().is_empty());
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
