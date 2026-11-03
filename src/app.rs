//! The application: shell construction, input routing and the `igui_app` glue.
//!
//! The shell is rebuilt only when its structure changes (sidebar, find bar,
//! view mode); every other frame is a repaint over the same tree. All state
//! lives in [`AppState`], which input callbacks and painters share.

use std::cell::{Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use igui_app::{AppLogic, EventContext, FrameContext, InitContext};
use igui_core::{Cursor, EventResult, InputEvent, Key, NodeId, PointerButton, Rect, ViewportSize};
use igui_render::PaintContext;
use igui_scene::SceneTree;
use igui_ui::{Control, ControlData, TextMeasurer};
use igui_winit::SharedWindow;

use crate::dialogs::{poll_dialog, DialogFuture, Dialogs};
use crate::input;
use crate::kit;
use crate::model::session::{self, Session};
use crate::model::Workspace;
use crate::state::{AppState, DialogKind, DialogRequest, Focus, ViewMode};
use crate::theme::{Mode, Theme};
use crate::ui;

/// The editor application, host-agnostic: no window, no wgpu, no winit.
pub struct EditorApp {
    state: Rc<RefCell<AppState>>,
    tree: SceneTree,
    shell: NodeId,
    editor_id: Option<NodeId>,
    dialogs: Box<dyn Dialogs>,
    window: Option<SharedWindow>,
    /// The dialog currently open (native sheet / portal / scripted).
    pending: Option<PendingDialog>,
}

/// A dialog that has been started but not answered yet. The future is polled
/// once per frame; `needs_frame` keeps frames coming while it is open.
struct PendingDialog {
    kind: DialogKind,
    then_close: Option<usize>,
    suggested: String,
    future: DialogFuture,
}

impl EditorApp {
    /// Restores the last session (or opens `path`) with native dialogs
    /// installed by the host.
    pub fn with_dialogs(
        open: Option<PathBuf>,
        mode: Option<Mode>,
        dialogs: Box<dyn Dialogs>,
    ) -> Self {
        Self::with_config(open, mode, session::path(), dialogs)
    }

    /// Full constructor: explicit session path and dialog implementation
    /// (the self-check uses a temp config and scripted dialogs).
    pub fn with_config(
        open: Option<PathBuf>,
        mode: Option<Mode>,
        config_path: PathBuf,
        dialogs: Box<dyn Dialogs>,
    ) -> Self {
        let session = Session::load(&config_path);
        let theme_mode = mode.unwrap_or(if session.is_dark() {
            Mode::Dark
        } else {
            Mode::Light
        });
        let workspace = workspace_for(open, &session);
        let mut state = AppState::new(Theme::new(theme_mode), workspace, session, config_path);
        if state.ws.tabs.is_empty() {
            state.ws.new_tab();
        }
        state.sidebar.selected = state
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf));
        let state = Rc::new(RefCell::new(state));
        let mut tree = SceneTree::new();
        igui_ui::set_text_measurer(
            &mut tree,
            Rc::new(igui_ui::ApproxTextMeasurer) as Rc<dyn TextMeasurer>,
        );
        let (shell, editor) = build_shell(&mut tree, &state);
        igui_ui::invalidate_layout(&mut tree);
        Self {
            state,
            tree,
            shell,
            editor_id: editor,
            dialogs,
            window: None,
            pending: None,
        }
    }

    pub fn state(&self) -> Ref<'_, AppState> {
        self.state.borrow()
    }

    /// The built tree (diagnostics / self-check geometry dumps).
    pub fn tree(&self) -> &SceneTree {
        &self.tree
    }

    pub fn control_count(&self) -> usize {
        self.tree.iter().count()
    }

    /// Drains deferred work: rebuild the shell when its structure changed,
    /// refresh find matches, run queued dialogs, persist the session, expire the
    /// status message and sync the window title.
    pub fn update(&mut self, _delta: f32) {
        // Dialogs run first: opening a Markdown file can change the pane
        // structure, and the same frame must rebuild for it.
        self.pump_dialog();
        let needs_rebuild = self.state.borrow().needs_rebuild;
        if needs_rebuild {
            self.rebuild();
        }
        let find_open = self.state.borrow().find.open;
        if find_open {
            self.state.borrow_mut().refresh_find();
        }
        let expired = {
            let state = self.state.borrow();
            state
                .message
                .as_ref()
                .is_some_and(|(_, _, at)| at.elapsed().as_secs_f32() > 3.0)
        };
        if expired {
            let mut state = self.state.borrow_mut();
            state.message = None;
            state.dirty = true;
        }
        self.sync_title();
        self.state.borrow_mut().save_session_if_dirty();
    }

    pub fn layout(&mut self, viewport: ViewportSize) {
        self.state.borrow_mut().viewport = viewport;
        igui_ui::layout(&mut self.tree, viewport);
        self.tree.update();
    }

    pub fn paint(&mut self, ctx: &mut PaintContext) {
        igui_ui::paint(&self.tree, ctx);
        // Overlays, in stacking order: menu drop-down, then the modal.
        let measurer = igui_ui::text_measurer(&self.tree);
        let mut state = self.state.borrow_mut();
        ui::menu::paint_popup(&mut state, measurer.as_ref(), ctx);
        ui::modal::paint(&mut state, measurer.as_ref(), ctx);
    }

    pub fn event(&mut self, event: &InputEvent) -> EventResult {
        let measurer = igui_ui::text_measurer(&self.tree);
        match event {
            InputEvent::PointerDown {
                position,
                button: PointerButton::Left,
            } => {
                let mut state = self.state.borrow_mut();
                if state.modal.is_some() {
                    match ui::modal::hit(&state, *position) {
                        Some(button) => state.resolve_modal(button),
                        None => {
                            let (panel, _) = ui::modal::layout(state.viewport.logical_rect());
                            if !panel.contains(*position) {
                                state.resolve_modal(crate::state::ModalButton::Cancel);
                            }
                        }
                    }
                    return EventResult::Handled;
                }
                // The bar sits above the drop-down: a press on a bar item
                // toggles that menu (including closing the open one).
                if ui::menu::handle_bar_click(&mut state, *position) {
                    return EventResult::Handled;
                }
                if state.menu_open.is_some() {
                    match ui::menu::handle_click(&mut state, *position, measurer.as_ref()) {
                        ui::menu::Click::Run(action) => {
                            state.menu_open = None;
                            state.menu_hover = None;
                            state.run(action);
                            return EventResult::Handled;
                        }
                        ui::menu::Click::Consumed => return EventResult::Handled,
                        ui::menu::Click::Outside => {
                            state.menu_open = None;
                            state.menu_hover = None;
                            state.dirty = true;
                        }
                    }
                }
                drop(state);
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::PointerMove { position } => {
                let mut state = self.state.borrow_mut();
                let menubar = state
                    .menubar_hits
                    .iter()
                    .find(|(_, rect)| rect.contains(*position))
                    .map(|(menu, _)| *menu);
                let menu_hover = ui::menu::hover_at(&state, *position, measurer.as_ref());
                let tabs = ui::tabs::hover_at(&state, *position);
                let sidebar = ui::sidebar::hover_at(&state, *position);
                if state.menubar_hover != menubar
                    || state.menu_hover != menu_hover
                    || state.tabs.hover != tabs
                    || state.sidebar.hover != sidebar
                {
                    state.menubar_hover = menubar;
                    state.menu_hover = menu_hover;
                    state.tabs.hover = tabs;
                    state.sidebar.hover = sidebar;
                    state.dirty = true;
                }
                drop(state);
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::PointerUp { .. } => {
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::KeyDown { key } => {
                let modifiers = igui_ui::modifiers(&self.tree);
                let mut state = self.state.borrow_mut();
                if state.modal.is_some() {
                    return input::modal_key(&mut state, *key);
                }
                if state.menu_open.is_some() && *key == Key::Escape {
                    state.menu_open = None;
                    state.menu_hover = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
                if state.find.open && input::find_key(&mut state, *key, modifiers).is_handled() {
                    return EventResult::Handled;
                }
                if let Some(result) = input::shortcut(&mut state, *key, modifiers) {
                    return result;
                }
                if state.find.open && *key == Key::Escape {
                    state.close_find();
                    return EventResult::Handled;
                }
                // While a find field owns the focus, unclaimed keys must not
                // leak into the document.
                if matches!(state.focus, Focus::Find | Focus::Replace) {
                    return EventResult::Handled;
                }
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::TextInput { text } => {
                let mut state = self.state.borrow_mut();
                if state.find.open && matches!(state.focus, Focus::Find | Focus::Replace) {
                    input::find_text(&mut state, text);
                    return EventResult::Handled;
                }
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::Ime(ime) => {
                let mut state = self.state.borrow_mut();
                if state.find.open && matches!(state.focus, Focus::Find | Focus::Replace) {
                    input::find_ime(&mut state, ime);
                    return EventResult::Handled;
                }
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            _ => igui_ui::route_input(&mut self.tree, event),
        }
    }

    pub fn caret(&self) -> Option<Rect> {
        igui_ui::focused_caret(&self.tree)
    }

    pub fn cursor(&self) -> Cursor {
        igui_ui::hovered_cursor(&self.tree)
    }

    pub fn needs_frame(&self) -> bool {
        self.pending.is_some() || self.state.borrow().message.is_some()
    }

    /// Starts a requested dialog (one at a time) and polls the open one. The
    /// futures are non-blocking: a native sheet never runs a nested event loop,
    /// so the application handler is never re-entered.
    fn pump_dialog(&mut self) {
        if self.pending.is_none() {
            let request = self.state.borrow_mut().pending_dialog.take();
            if let Some(DialogRequest { kind, then_close }) = request {
                let (start, suggested) = self.state.borrow().dialog_context();
                let future = match kind {
                    DialogKind::Open => self.dialogs.open_file(start.as_deref()),
                    DialogKind::OpenFolder => self.dialogs.open_folder(start.as_deref()),
                    DialogKind::SaveAs => {
                        self.dialogs.save_file(start.as_deref(), suggested.as_str())
                    }
                };
                self.pending = Some(PendingDialog {
                    kind,
                    then_close,
                    suggested,
                    future,
                });
            }
        }
        let answer = self
            .pending
            .as_mut()
            .and_then(|pending| poll_dialog(&mut pending.future));
        if let Some(result) = answer {
            let pending = self.pending.take().expect("polled while pending");
            self.finish_dialog(pending, result);
        }
    }

    fn finish_dialog(&mut self, pending: PendingDialog, result: Option<PathBuf>) {
        let Some(mut path) = result else {
            return;
        };
        let mut state = self.state.borrow_mut();
        match pending.kind {
            DialogKind::Open => state.open_path(path),
            DialogKind::OpenFolder => state.set_root(path),
            DialogKind::SaveAs => {
                // Keep the suggested name's extension when the picker returns
                // a bare name and the suggestion has one.
                if path.extension().is_none() {
                    if let Some((_, extension)) = pending.suggested.rsplit_once('.') {
                        if !extension.is_empty() {
                            path.set_extension(extension);
                        }
                    }
                }
                state.save_as(path, pending.then_close);
            }
        }
    }

    fn focus_editor(&mut self) {
        if let Some(id) = self.editor_id {
            igui_ui::set_focus(&mut self.tree, id);
        }
    }

    fn sync_title(&mut self) {
        let title = {
            let state = self.state.borrow();
            match state.ws.active_doc() {
                Some(doc) => format!(
                    "{}{} — igui-editor",
                    doc.name(),
                    if doc.is_dirty() { " •" } else { "" }
                ),
                None => "igui-editor".to_string(),
            }
        };
        if self.state.borrow().title == title {
            return;
        }
        self.state.borrow_mut().title = title.clone();
        if let Some(window) = &self.window {
            let window = window.borrow().clone();
            if let Some(window) = window {
                window.set_title(&title);
            }
        }
    }

    fn rebuild(&mut self) {
        self.state.borrow_mut().needs_rebuild = false;
        self.state.borrow_mut().find.hits.clear();
        self.tree.remove(self.shell);
        let (shell, editor) = build_shell(&mut self.tree, &self.state);
        self.shell = shell;
        self.editor_id = editor;
        igui_ui::invalidate_layout(&mut self.tree);
        self.focus_editor();
    }
}

/// Session-aware startup: an explicit CLI path wins; otherwise the stored tabs
/// are reopened (missing files are skipped).
fn workspace_for(open: Option<PathBuf>, session: &Session) -> Workspace {
    if let Some(path) = open {
        return Workspace::with_file(path).unwrap_or_else(|error| {
            eprintln!("igui-editor: {error}");
            Workspace::new(std::env::current_dir().unwrap_or_default())
        });
    }
    let mut root = None;
    for path in &session.tabs {
        if path.is_file() {
            root = path.parent().map(Path::to_path_buf);
            break;
        }
    }
    let mut workspace =
        Workspace::new(root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default()));
    for path in &session.tabs {
        if path.is_file() {
            let _ = workspace.open(path);
        }
    }
    workspace
}

fn build_shell(tree: &mut SceneTree, state: &Rc<RefCell<AppState>>) -> (NodeId, Option<NodeId>) {
    let (theme, view) = {
        let state = state.borrow();
        (state.theme, state.view_of_active())
    };
    let (m, show_sidebar, find_open) = {
        let state = state.borrow();
        (theme.m, state.show_sidebar, state.find.open)
    };
    let root = tree.root();

    // The topmost control is the viewport root: `igui_ui` pins it to the
    // viewport and places its children by anchors, ignoring its container. The
    // flex shell therefore lives one level down, filling the root.
    let shell = kit::node(tree, root, "shell");
    let column = kit::node(tree, shell, "column");
    if let Some(control) = tree.data_mut::<Control>(column) {
        control.data = ControlData::fill_parent();
        control.container = kit::tight_column();
    }

    let menubar = ui::menu::build(tree, column, state.clone());
    kit::flex(tree, menubar, 0.0, Some(m.menubar_h), 0.0);
    kit::min_size(tree, menubar, 0.0, m.menubar_h);

    let body = kit::node(tree, column, "body");
    kit::set_container(tree, body, kit::tight_row());
    kit::flex(tree, body, 1.0, None, 1.0);

    if show_sidebar {
        let sidebar = ui::sidebar::build(tree, body, state.clone());
        kit::flex(tree, sidebar, 0.0, Some(m.sidebar_w), 0.0);
        kit::min_size(tree, sidebar, m.sidebar_w, 0.0);
    }

    let main = kit::node(tree, body, "main");
    kit::set_container(tree, main, kit::tight_column());
    kit::flex(tree, main, 1.0, None, 1.0);

    let tabs = ui::tabs::build(tree, main, state.clone());
    kit::flex(tree, tabs, 0.0, Some(m.tabbar_h), 0.0);
    kit::min_size(tree, tabs, 0.0, m.tabbar_h);

    if find_open {
        let bar = ui::find_bar::build(tree, main, state.clone());
        kit::flex(tree, bar, 0.0, Some(ui::find_bar::BAR_H), 0.0);
        kit::min_size(tree, bar, 0.0, ui::find_bar::BAR_H);
    }

    let editor = match view {
        ViewMode::Edit => {
            let editor = ui::editor::build(tree, main, state.clone());
            kit::flex(tree, editor, 1.0, None, 1.0);
            kit::min_size(tree, editor, 0.0, 0.0);
            Some(editor)
        }
        ViewMode::Preview => {
            let preview = ui::preview::build(tree, main, state.clone());
            kit::flex(tree, preview, 1.0, None, 1.0);
            kit::min_size(tree, preview, 0.0, 0.0);
            None
        }
        ViewMode::Split => {
            let split = kit::node(tree, main, "split");
            kit::set_container(tree, split, kit::tight_row());
            kit::flex(tree, split, 1.0, None, 1.0);
            let editor = ui::editor::build(tree, split, state.clone());
            kit::flex(tree, editor, 1.0, None, 1.0);
            kit::min_size(tree, editor, 0.0, 0.0);
            let preview = ui::preview::build(tree, split, state.clone());
            kit::flex(tree, preview, 1.0, None, 1.0);
            kit::min_size(tree, preview, 0.0, 0.0);
            Some(editor)
        }
    };

    let status = ui::status::build(tree, column, state.clone());
    kit::flex(tree, status, 0.0, Some(m.status_h), 0.0);
    kit::min_size(tree, status, 0.0, m.status_h);

    state.borrow_mut().built_view = view;
    if let Some(editor) = editor {
        igui_ui::set_focus(tree, editor);
    }
    (shell, editor)
}

impl AppLogic for EditorApp {
    fn init(&mut self, ctx: &InitContext<'_>) {
        if let Some(measurer) = ctx.service::<Rc<dyn TextMeasurer>>() {
            igui_ui::set_text_measurer(&mut self.tree, measurer.clone());
        }
        if let Some(clipboard) = ctx.service::<Rc<RefCell<dyn igui_ui::Clipboard>>>() {
            igui_ui::set_clipboard(&mut self.tree, clipboard.clone());
            self.state.borrow_mut().clipboard = Some(clipboard.clone());
        }
        if let Some(window) = ctx.service::<SharedWindow>() {
            self.window = Some(window.clone());
        }
        self.sync_title();
    }

    fn event(&mut self, _ctx: &EventContext<'_>, event: &InputEvent) -> EventResult {
        self.event(event)
    }

    fn update(&mut self, ctx: &FrameContext<'_>) {
        self.update(ctx.delta());
    }

    fn layout(&mut self, ctx: &FrameContext<'_>) {
        self.layout(ctx.viewport());
    }

    fn paint(&mut self, _ctx: &FrameContext<'_>, paint: &mut PaintContext) {
        self.paint(paint);
    }

    fn needs_frame(&self) -> bool {
        self.needs_frame()
    }

    fn cursor(&self) -> Option<Cursor> {
        Some(self.cursor())
    }

    fn caret(&self) -> Option<Rect> {
        self.caret()
    }
}
