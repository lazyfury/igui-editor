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
use crate::drop::SharedDropTarget;
use crate::input;
use crate::kit;
use crate::model::session::{self, Session};
use crate::model::Workspace;
use crate::state::{Action, AppState, DialogKind, DialogRequest, Focus, Menu, ViewMode};
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
    /// Set while syntax highlighting is still catching up (drives `needs_frame`).
    analysis_behind: bool,
    /// File-drop state shared with the winit observer.
    drop: Option<SharedDropTarget>,
    /// Whether the saved window position was applied.
    window_restored: bool,
    /// The dialog currently open (native sheet / portal / scripted).
    pending: Option<PendingDialog>,
    /// Drop the committed text that follows the Alt+Z soft-wrap shortcut.
    suppress_text: bool,
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
        let mut state = AppState::new(
            Theme::new(theme_mode),
            workspace,
            session,
            config_path.clone(),
        );
        // Data-only language packs (`<config>/languages/*.json`) join the
        // registry after the built-ins, so a pack can never override one.
        let (providers, servers, errors) = crate::syntax::load_language_packs(
            &config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("languages"),
        );
        for provider in providers {
            state.syntax.register(provider);
        }
        for server in servers {
            if !state
                .services
                .language_servers
                .iter()
                .any(|known| known.id == server.id)
            {
                state.services.language_servers.push(server);
            }
        }
        if !errors.is_empty() {
            for error in &errors {
                eprintln!("language pack: {error}");
            }
            state.status(format!("{} language pack(s) skipped", errors.len()));
        }
        // Keep the session's folder in sync with what actually opened (CLI
        // folder/file, a restored folder, or none).
        let folder = state.ws.folder().map(Path::to_path_buf);
        if state.session.folder != folder {
            state.session.folder = folder;
            state.config_dirty = true;
        }
        let recovered = state.recovery.list();
        if !recovered.is_empty() {
            state.recovery_prompt = Some(recovered);
        }
        state.sidebar.selected = state
            .ws
            .active_doc()
            .and_then(|doc| doc.path().map(Path::to_path_buf));
        state.request_git_refresh();
        state.load_keymap_overrides();
        state.restore_terminal();
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
            suppress_text: false,
            analysis_behind: false,
            drop: None,
            window_restored: false,
        }
    }

    pub fn state(&self) -> Ref<'_, AppState> {
        self.state.borrow()
    }

    /// Mutable state for wiring tests (the self-check drives caret/settings
    /// through it when a pointer/keys would be overkill).
    #[doc(hidden)]
    pub fn state_mut(&mut self) -> std::cell::RefMut<'_, AppState> {
        self.state.borrow_mut()
    }

    /// Registers an additional syntax provider (the plugin seam). Re-registering
    /// a name keeps the existing provider; cached highlighting is dropped so a
    /// newly covered file re-tokenizes immediately.
    pub fn register_syntax(&mut self, provider: std::sync::Arc<dyn crate::syntax::SyntaxProvider>) {
        let mut state = self.state.borrow_mut();
        state.syntax.register(provider);
        state.highlight.reset();
        state.dirty = true;
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
        self.pump_native_requests();
        // Keep native Dock Recent / quit state in step with the session.
        {
            let state = self.state.borrow();
            crate::macos::set_recent(&state.session.recent);
            crate::macos::set_dirty_count(
                state.ws.tabs.iter().filter(|doc| doc.is_dirty()).count(),
            );
        }
        // A settings panel change (or a sidebar drag) may need the flex basis
        // updated without rebuilding the shell.
        let (pending, width, sidebar) = {
            let state = self.state.borrow();
            (
                state.pending_sidebar_sync,
                state.settings.sidebar_width,
                state.sidebar_node,
            )
        };
        if pending {
            if let Some(sidebar) = sidebar {
                kit::flex(&mut self.tree, sidebar, 0.0, Some(width), 0.0);
                kit::min_size(&mut self.tree, sidebar, width, 0.0);
                igui_ui::mark_dirty(&mut self.tree, sidebar);
            }
            self.state.borrow_mut().pending_sidebar_sync = false;
        }
        self.analysis_behind = self.state.borrow_mut().tick_analysis();
        self.state.borrow_mut().tick_external_changes();
        self.state.borrow_mut().tick_git();
        self.state.borrow_mut().tick_lsp();
        self.state.borrow_mut().tick_auto_completion();
        self.state.borrow_mut().tick_hover();
        self.state.borrow_mut().tick_terminal();
        self.state.borrow_mut().tick_reliability();
        self.state.borrow_mut().sync_scroll_positions();
        if !self.window_restored {
            self.restore_window_position();
        }
        self.persist_window_geometry();
        self.pump_drops();
        self.state.borrow_mut().search_tick();
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
        // Both drag handles share this vertical anchor, so their grip bars are
        // always the same height (including while the find bar is open).
        let node = self.state.borrow().content_node;
        let center = node
            .and_then(|id| self.tree.data::<Control>(id))
            .map(|control| control.data.rect.center().y);
        self.state.borrow_mut().content_center_y = center;
    }

    pub fn paint(&mut self, ctx: &mut PaintContext) {
        igui_ui::paint(&self.tree, ctx);
        // Git diff is an app overlay, above the tree but below menus/modals.
        let measurer = igui_ui::text_measurer(&self.tree);
        let mut state = self.state.borrow_mut();
        ui::git::paint_diff(&mut state, measurer.as_ref(), ctx);
        ui::git::paint_commit(&mut state, measurer.as_ref(), ctx);
        ui::git::paint_commit_detail(&mut state, measurer.as_ref(), ctx);
        drop(state);
        // Overlays, in stacking order: menu drop-down, settings, modal.
        let measurer = igui_ui::text_measurer(&self.tree);
        let mut state = self.state.borrow_mut();
        ui::menu::paint_popup(&mut state, measurer.as_ref(), ctx);
        ui::tab_menu::paint(&mut state, measurer.as_ref(), ctx);
        ui::tree_menu::paint(&mut state, measurer.as_ref(), ctx);
        ui::settings::paint(&mut state, measurer.as_ref(), ctx);
        ui::quick_open::paint(&mut state, measurer.as_ref(), ctx);
        ui::new_file::paint(&mut state, measurer.as_ref(), ctx);
        ui::project_search::paint(&mut state, measurer.as_ref(), ctx);
        ui::lsp::paint(&mut state, measurer.as_ref(), ctx);
        ui::problems::paint(&mut state, measurer.as_ref(), ctx);
        ui::drop::paint(&state, measurer.as_ref(), ctx);
        ui::modal::paint(&mut state, measurer.as_ref(), ctx);
        ui::recovery::paint(&state, measurer.as_ref(), ctx);
    }

    pub fn event(&mut self, event: &InputEvent) -> EventResult {
        let measurer = igui_ui::text_measurer(&self.tree);
        match event {
            InputEvent::PointerDown {
                position,
                button: PointerButton::Left,
            } => {
                let mut state = self.state.borrow_mut();
                if state.recovery_prompt.is_some() {
                    ui::recovery::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.modal.is_some() {
                    match ui::modal::hit(&state, *position) {
                        Some(button) => state.resolve_modal(button),
                        None => {
                            let (panel, _) =
                                ui::modal::layout(state.viewport.logical_rect(), state.modal);
                            if !panel.contains(*position) {
                                state.resolve_modal(crate::state::ModalButton::Cancel);
                            }
                        }
                    }
                    return EventResult::Handled;
                }
                if state.ws.active_doc().is_some_and(|doc| doc.is_docs_view())
                    && state.editor.text_rect.contains(*position)
                {
                    if let Some(byte) = ui::editor::byte_at(&state, *position, measurer.as_ref()) {
                        if let Some(link) = state.docs_link_at(byte) {
                            match link {
                                crate::model::bbcode::DocLink::Native { class, symbol } => {
                                    state.open_native_docs_at(class, symbol);
                                }
                                crate::model::bbcode::DocLink::Url(url) => {
                                    state.open_external_url(&url);
                                }
                            }
                            return EventResult::Handled;
                        }
                    }
                }
                if state.problems.open {
                    match ui::problems::click(&mut state, *position) {
                        ui::problems::Click::Jump(_) => return EventResult::Handled,
                        ui::problems::Click::Consumed => return EventResult::Handled,
                        ui::problems::Click::Outside => {}
                    }
                }
                if state.lsp.hover_doc_hit.contains(*position) {
                    if let Some(target) = state
                        .lsp
                        .hover
                        .as_ref()
                        .and_then(|hover| hover.native.clone())
                    {
                        state.lsp.dismiss_transient();
                        state.open_native_docs(&target);
                        return EventResult::Handled;
                    }
                }
                let definition_click = state.focus == Focus::Editor
                    && (igui_ui::modifiers(&self.tree).ctrl || igui_ui::modifiers(&self.tree).meta)
                    && state.editor.text_rect.contains(*position);
                if state.lsp.completion_doc_hit.contains(*position)
                    && state.lsp.completion.is_some()
                {
                    state.open_completion_docs();
                    return EventResult::Handled;
                }
                let lsp_chip = state.lsp_hit.contains(*position);
                if !lsp_chip && (state.lsp.hover.is_some() || state.lsp.completion.is_some()) {
                    state.lsp.dismiss_transient();
                    state.dirty = true;
                    if !definition_click {
                        return EventResult::Handled;
                    }
                }
                state.clear_hover_target();
                state.hover_leave_at = None;
                // The bar sits above the drop-down: a press on a bar item
                // toggles that menu (including closing the open one).
                if state.git.detail_visible {
                    ui::git::click_commit_detail(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.git.commit_open {
                    ui::git::commit_click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.git.diff_visible {
                    ui::git::click_diff(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.settings_open {
                    ui::settings::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.new_file.open {
                    ui::new_file::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.quick_open.open {
                    ui::quick_open::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.search.open {
                    ui::project_search::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.tab_menu.is_some() {
                    match ui::tab_menu::click(&mut state, *position) {
                        ui::tab_menu::Click::Outside => {
                            state.tab_menu = None;
                            state.dirty = true;
                        }
                        ui::tab_menu::Click::Consumed => return EventResult::Handled,
                        ui::tab_menu::Click::Run(action) => {
                            let tab = state.tab_menu.as_ref().map_or(0, |menu| menu.tab);
                            state.tab_menu = None;
                            state.run_tab_menu(action, tab);
                            return EventResult::Handled;
                        }
                    }
                }
                if state.tree_menu.is_some() {
                    match ui::tree_menu::click(&mut state, *position) {
                        ui::tree_menu::Click::Outside => {
                            state.tree_menu = None;
                            state.dirty = true;
                        }
                        ui::tree_menu::Click::Consumed => return EventResult::Handled,
                        ui::tree_menu::Click::Run(action) => {
                            let menu = state.tree_menu.clone();
                            state.tree_menu = None;
                            if let Some(menu) = menu {
                                state.run_tree_menu(menu.path, menu.is_dir, action);
                            }
                            return EventResult::Handled;
                        }
                    }
                }
                // Footer LSP chip: open the diagnostic message list.
                if state.lsp_hit.contains(*position) {
                    ui::problems::toggle(&mut state);
                    state.menu_open = None;
                    state.menu_hover = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
                // Footer language selector (bottom-right): a click toggles the
                // language popup, which opens above the status bar.
                if state.ws.active_doc().is_some() && state.language_hit.contains(*position) {
                    state.menu_open = if state.menu_open == Some(Menu::Language) {
                        None
                    } else {
                        Some(Menu::Language)
                    };
                    state.menu_hover = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
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
                if ui::outline::click(&mut state, *position) {
                    return EventResult::Handled;
                }
                let modifiers = igui_ui::modifiers(&self.tree);
                if state.focus == Focus::Editor
                    && (modifiers.ctrl || modifiers.meta)
                    && state.editor.text_rect.contains(*position)
                {
                    if let Some(byte) = ui::editor::byte_at(&state, *position, measurer.as_ref()) {
                        if let Some(class) = state.native_class_at(byte) {
                            let target = crate::model::lsp::NativeTarget {
                                class: class.clone(),
                                symbol: class,
                                kind: crate::model::lsp::NativeDocKind::Class,
                            };
                            state.open_native_docs(&target);
                            return EventResult::Handled;
                        }
                        let native_docs = state.request_lsp_native_docs_at(byte);
                        if state.request_lsp_definition_at(byte) || native_docs {
                            return EventResult::Handled;
                        }
                    }
                }
                drop(state);
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::PointerDown {
                position,
                button: PointerButton::Right,
            } => {
                let mut state = self.state.borrow_mut();
                if state.recovery_prompt.is_some() || state.modal.is_some() {
                    return EventResult::Handled;
                }
                if state.settings_open || state.quick_open.open || state.search.open {
                    return EventResult::Handled;
                }
                if let Some(index) = ui::sidebar::hover_at(&state, *position) {
                    if let Some(row) = state.ws.tree.rows().get(index).cloned() {
                        state.tab_drag = None;
                        state.tab_menu = None;
                        state.tree_menu = None;
                        state.menu_open = None;
                        state.menu_hover = None;
                        ui::tree_menu::open(&mut state, row.path, row.name, row.is_dir, *position);
                        return EventResult::Handled;
                    }
                }
                let tab = ui::tabs::hover_at(&state, *position);
                state.tab_drag = None;
                state.tab_menu = None;
                state.tree_menu = None;
                state.menu_open = None;
                state.menu_hover = None;
                if let Some(tab) = tab.filter(|index| *index != usize::MAX) {
                    ui::tab_menu::open(&mut state, tab, *position);
                }
                state.dirty = true;
                EventResult::Handled
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
                let language =
                    state.ws.active_doc().is_some() && state.language_hit.contains(*position);
                let problems = ui::problems::hover_at(&state, *position);
                let lsp_hover = state.lsp_hit.contains(*position);
                let git_hover = ui::git::hover_rect(&state, *position);
                if state.git.hover != git_hover {
                    state.git.hover = git_hover;
                    state.dirty = true;
                }
                let tab_menu = ui::tab_menu::hover_at(&state, *position);
                let tab_menu_changed = state.tab_menu.as_mut().is_some_and(|menu| {
                    if menu.hover == tab_menu {
                        false
                    } else {
                        menu.hover = tab_menu;
                        true
                    }
                });
                let tree_menu = ui::tree_menu::hover_at(&state, *position);
                let tree_menu_changed = state.tree_menu.as_mut().is_some_and(|menu| {
                    if menu.hover == tree_menu {
                        false
                    } else {
                        menu.hover = tree_menu;
                        true
                    }
                });
                if state.menubar_hover != menubar
                    || state.menu_hover != menu_hover
                    || state.tabs.hover != tabs
                    || state.sidebar.hover != sidebar
                    || state.language_hover != language
                    || state.lsp_hover != lsp_hover
                    || state.problems.hover != problems
                {
                    state.menubar_hover = menubar;
                    state.menu_hover = menu_hover;
                    state.tabs.hover = tabs;
                    state.sidebar.hover = sidebar;
                    state.language_hover = language;
                    state.lsp_hover = lsp_hover;
                    state.problems.hover = problems;
                    state.dirty = true;
                }
                if tab_menu_changed || tree_menu_changed {
                    state.dirty = true;
                }
                let over_hover =
                    state.lsp.hover.is_some() && state.lsp.hover_rect.contains(*position);
                if over_hover {
                    state.hover_leave_at = None;
                    state.clear_hover_target();
                } else if state.focus == Focus::Editor
                    && state.editor.text_rect.contains(*position)
                    && state.editor.drag == crate::state::Drag::None
                {
                    state.hover_leave_at = None;
                    if let Some(byte) = ui::editor::byte_at(&state, *position, measurer.as_ref()) {
                        if let Some(doc) = state.ws.active_doc() {
                            let doc_id = doc.id();
                            state.set_hover_target(doc_id, byte, *position);
                        }
                    } else {
                        state.clear_hover_target();
                    }
                } else {
                    state.clear_hover_target();
                    if state.lsp.hover.is_some() && state.hover_leave_at.is_none() {
                        state.hover_leave_at = Some(std::time::Instant::now());
                    }
                }
                ui::outline::hover_at(&mut state, *position);
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
            InputEvent::Wheel {
                position, delta, ..
            } => {
                let mut state = self.state.borrow_mut();
                if state.problems.open && state.problems.rect.contains(*position) {
                    ui::problems::wheel(&mut state, delta.y);
                    return EventResult::Handled;
                }
                if state.git.detail_visible {
                    let (panel, _) = ui::git::detail_layout(&state);
                    if panel.contains(*position) {
                        ui::git::wheel_commit_detail(&mut state, *delta);
                        return EventResult::Handled;
                    }
                }
                if state.git.diff_visible {
                    let (panel, _) = ui::git::diff_layout(&state);
                    if panel.contains(*position) {
                        ui::git::wheel_diff(&mut state, *delta);
                        return EventResult::Handled;
                    }
                }
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::KeyDown { key } => {
                let modifiers = igui_ui::modifiers(&self.tree);
                let mut state = self.state.borrow_mut();
                if state.recovery_prompt.is_some() {
                    ui::recovery::on_key(&mut state, *key, modifiers);
                    return EventResult::Handled;
                }
                if state.modal.is_some() {
                    return input::modal_key(&mut state, *key);
                }
                if state.settings_open && ui::settings::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.new_file.open && ui::new_file::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.quick_open.open && ui::quick_open::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.search.open && ui::project_search::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.problems.open && *key == Key::Escape {
                    ui::problems::close(&mut state);
                    return EventResult::Handled;
                }
                if state.lsp_completion_key(*key) {
                    return EventResult::Handled;
                }
                if state.lsp.hover.is_some() && *key == Key::Escape {
                    state.lsp.dismiss_transient();
                    state.dirty = true;
                    return EventResult::Handled;
                }
                if state.focus == Focus::Editor
                    && !modifiers.meta
                    && !modifiers.ctrl
                    && !modifiers.alt
                {
                    match key {
                        Key::F1 => {
                            state.request_lsp_hover();
                            return EventResult::Handled;
                        }
                        Key::F12 => {
                            state.request_lsp_definition();
                            return EventResult::Handled;
                        }
                        _ => {}
                    }
                }
                if state.focus == Focus::Editor
                    && modifiers.ctrl
                    && !modifiers.meta
                    && *key == Key::Space
                {
                    state.request_lsp_completion();
                    return EventResult::Handled;
                }
                if state.git.detail_visible && *key == Key::Escape {
                    state.close_git_commit_detail();
                    return EventResult::Handled;
                }
                if state.git.commit_open && ui::git::commit_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.git.diff_visible && *key == Key::Escape {
                    state.close_git_diff();
                    return EventResult::Handled;
                }
                if state.focus == Focus::Terminal
                    && modifiers.ctrl
                    && matches!(key, Key::Character('`'))
                {
                    state.run(Action::ToggleTerminal);
                    return EventResult::Handled;
                }
                if state.focus == Focus::Terminal && !modifiers.meta {
                    let application_cursor = state
                        .terminal
                        .parser
                        .as_ref()
                        .is_some_and(|parser| parser.screen().application_cursor());
                    if let Some(bytes) =
                        ui::terminal::encode_key(*key, modifiers, application_cursor)
                    {
                        state.terminal_write(&bytes);
                    }
                    return EventResult::Handled;
                }
                if state.tree_menu.is_some() && *key == Key::Escape {
                    state.tree_menu = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
                if state.tab_menu.is_some() && *key == Key::Escape {
                    state.tab_menu = None;
                    state.dirty = true;
                    return EventResult::Handled;
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
                let alt_z = modifiers.alt
                    && !modifiers.meta
                    && !modifiers.ctrl
                    && state.focus == Focus::Editor
                    && matches!(key, Key::Character(ch) if ch.eq_ignore_ascii_case(&'z'));
                if let Some(result) = input::shortcut(&mut state, *key, modifiers) {
                    if alt_z && result.is_handled() {
                        self.suppress_text = true;
                    }
                    return result;
                }
                if state.new_file.open {
                    return EventResult::Handled;
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
                if self.suppress_text {
                    self.suppress_text = false;
                    return EventResult::Handled;
                }
                // The host emits `TextInput` for character keys even while Cmd/Ctrl
                // is held (e.g. ⌘F also arrives as text "f"). Accelerator keys are
                // commands, never text. Ctrl+Alt is AltGr on Windows, so that stays
                // text.
                let modifiers = igui_ui::modifiers(&self.tree);
                if modifiers.meta || (modifiers.ctrl && !modifiers.alt) {
                    return EventResult::Handled;
                }
                let mut state = self.state.borrow_mut();
                if state.settings_open || state.recovery_prompt.is_some() {
                    return EventResult::Handled;
                }
                if state.lsp.hover.is_some() || state.lsp.completion.is_some() {
                    state.lsp.dismiss_transient();
                    state.dirty = true;
                }
                if state.vim.suppress_text {
                    state.vim.suppress_text = false;
                    return EventResult::Handled;
                }
                if state.focus == Focus::Editor
                    && state.settings.keymap == crate::keymap::KeymapId::Vim
                    && !state.vim.mode.is_insert()
                {
                    return EventResult::Handled;
                }
                if state.focus == Focus::Terminal {
                    state.terminal_write(text.as_bytes());
                    return EventResult::Handled;
                }
                if state.git.commit_open {
                    ui::git::commit_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.new_file.open {
                    ui::new_file::on_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.quick_open.open {
                    ui::quick_open::on_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.search.open {
                    ui::project_search::on_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.find.open && matches!(state.focus, Focus::Find | Focus::Replace) {
                    input::find_text(&mut state, text);
                    return EventResult::Handled;
                }
                state.schedule_auto_completion(text);
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::PointerLeave => {
                {
                    let mut state = self.state.borrow_mut();
                    state.clear_hover_target();
                    state.hover_leave_at = None;
                    state.lsp.hover = None;
                    state.lsp.hover_anchor = None;
                    state.lsp.hover_rect = igui_core::Rect::ZERO;
                }
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::KeyUp { .. } => {
                self.suppress_text = false;
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::Ime(ime) => {
                let mut state = self.state.borrow_mut();
                if state.settings_open || state.recovery_prompt.is_some() {
                    return EventResult::Handled;
                }
                if state.lsp.hover.is_some() || state.lsp.completion.is_some() {
                    state.lsp.dismiss_transient();
                    state.dirty = true;
                }
                if state.focus == Focus::Editor
                    && state.settings.keymap == crate::keymap::KeymapId::Vim
                    && !state.vim.mode.is_insert()
                {
                    state.vim.mode = crate::vim::VimMode::Normal;
                    return EventResult::Handled;
                }
                if state.focus == Focus::Terminal {
                    if let igui_core::ImeEvent::Commit(text) = ime {
                        state.terminal_write(text.as_bytes());
                    }
                    return EventResult::Handled;
                }
                if state.git.commit_open {
                    if let igui_core::ImeEvent::Commit(text) = ime {
                        ui::git::commit_text(&mut state, text);
                    }
                    return EventResult::Handled;
                }
                if state.new_file.open {
                    ui::new_file::on_ime(&mut state, ime);
                    return EventResult::Handled;
                }
                if state.quick_open.open {
                    ui::quick_open::on_ime(&mut state, ime);
                    return EventResult::Handled;
                }
                if state.search.open {
                    ui::project_search::on_ime(&mut state, ime);
                    return EventResult::Handled;
                }
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
        let state = self.state.borrow();
        if state.terminal.open && state.focus == Focus::Terminal {
            return Some(state.terminal.caret_rect);
        }
        drop(state);
        igui_ui::focused_caret(&self.tree)
    }

    pub fn cursor(&self) -> Cursor {
        igui_ui::hovered_cursor(&self.tree)
    }

    pub fn needs_frame(&self) -> bool {
        self.pending.is_some()
            || self.analysis_behind
            || self.state.borrow().search_busy()
            || self.state.borrow().terminal.open
            || self.state.borrow().lsp.is_busy()
            || self.state.borrow().auto_completion_pending()
            || self.state.borrow().hover_waiting()
            || self.state.borrow().hover_dismissing()
            || self.state.borrow().message.is_some()
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

    /// Drains Finder/Dock/quit commands from the macOS bridge.
    fn pump_native_requests(&mut self) {
        for request in crate::native::take() {
            match request {
                crate::native::NativeRequest::Open(paths) => {
                    self.state.borrow_mut().open_dropped(paths);
                }
                crate::native::NativeRequest::NewFile => {
                    self.state.borrow_mut().open_new_file();
                }
                crate::native::NativeRequest::ClearRecent => {
                    self.state.borrow_mut().clear_recent();
                }
                crate::native::NativeRequest::Quit => {
                    let dirty = self
                        .state
                        .borrow()
                        .ws
                        .tabs
                        .iter()
                        .filter(|doc| doc.is_dirty())
                        .count();
                    self.state.borrow_mut().request_quit(dirty);
                }
            }
        }
    }

    /// Drains file-drop state collected by the winit observer.
    fn pump_drops(&mut self) {
        let Some(drop) = self.drop.clone() else {
            return;
        };
        let (hovered, paths) = {
            let mut drop = drop.borrow_mut();
            (drop.hovered, std::mem::take(&mut drop.paths))
        };
        {
            let mut state = self.state.borrow_mut();
            if state.drop_hover != hovered {
                state.drop_hover = hovered;
                state.dirty = true;
            }
        }
        if !paths.is_empty() {
            self.drop_paths(paths);
        }
    }

    /// Handles dropped paths (also used by the self-check).
    pub fn drop_paths(&mut self, paths: Vec<PathBuf>) {
        self.state.borrow_mut().open_dropped(paths);
    }

    fn restore_window_position(&mut self) {
        if self.window_restored {
            return;
        }
        let (Some(shared), Some(geometry)) =
            (self.window.clone(), self.state.borrow().session.window)
        else {
            return;
        };
        let window = shared.borrow().as_ref().cloned();
        let Some(window) = window else {
            return;
        };
        let scale = window.scale_factor();
        let x = (geometry.position.0 * scale as f32) as i32;
        let y = (geometry.position.1 * scale as f32) as i32;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        self.window_restored = true;
    }

    /// Saves the live window size/position (logical px) when it changed.
    fn persist_window_geometry(&mut self) {
        let Some(shared) = self.window.clone() else {
            return;
        };
        let window = shared.borrow().as_ref().cloned();
        let Some(window) = window else {
            return;
        };
        if window.is_maximized() || window.fullscreen().is_some() {
            return;
        }
        let scale = window.scale_factor();
        if scale <= 0.0 {
            return;
        }
        let size = window.inner_size();
        let position = window.outer_position().ok();
        let size = (
            size.width as f32 / scale as f32,
            size.height as f32 / scale as f32,
        );
        let position = position
            .map(|position| {
                (
                    position.x as f32 / scale as f32,
                    position.y as f32 / scale as f32,
                )
            })
            .unwrap_or((0.0, 0.0));
        self.state.borrow_mut().set_window_geometry(size, position);
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
                None => state
                    .ws
                    .folder()
                    .and_then(|folder| folder.file_name())
                    .map(|name| format!("{} — igui-editor", name.to_string_lossy()))
                    .unwrap_or_else(|| "igui-editor".to_string()),
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

/// Session-aware startup.
///
/// - An explicit CLI **folder** opens it as the workspace (no tab).
/// - An explicit CLI **file** opens it, with its directory as the workspace.
/// - Otherwise the stored folder is restored (v1 sessions infer it from the
///   first tab), then the stored tabs are reopened.
/// - With neither, the app starts in the welcome state: no folder, no buffer.
fn workspace_for(open: Option<PathBuf>, session: &Session) -> Workspace {
    if let Some(path) = open {
        if path.is_dir() {
            return Workspace::new(path);
        }
        return Workspace::with_file(path).unwrap_or_else(|error| {
            eprintln!("igui-editor: {error}");
            Workspace::detached()
        });
    }
    let mut workspace = Workspace::detached();
    let folder = session
        .folder
        .clone()
        .filter(|path| path.is_dir())
        .or_else(|| {
            // v1 sessions have no `folder` key: keep the old behaviour.
            if session.version < 2 {
                session
                    .tabs
                    .iter()
                    .find(|path| path.is_file())
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            } else {
                None
            }
        });
    if let Some(folder) = folder {
        let _ = workspace.set_folder(folder);
    }
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

    let sidebar_width = state.borrow().settings.sidebar_width;
    let mut sidebar_node = None;
    if show_sidebar {
        let sidebar = ui::sidebar::build(tree, body, state.clone());
        kit::flex(tree, sidebar, 0.0, Some(sidebar_width), 0.0);
        kit::min_size(tree, sidebar, sidebar_width, 0.0);
        let divider = ui::sidebar::build_divider(tree, body, state.clone());
        kit::flex(tree, divider, 0.0, Some(ui::handle::HANDLE_WIDTH), 0.0);
        kit::min_size(tree, divider, ui::handle::HANDLE_WIDTH, 0.0);
        sidebar_node = Some(sidebar);
    }
    state.borrow_mut().sidebar_node = sidebar_node;

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
            {
                let mut state = state.borrow_mut();
                state.split_editor = None;
                state.split_preview = None;
                state.split_dragging = false;
            }
            let editor = ui::editor::build(tree, main, state.clone());
            kit::flex(tree, editor, 1.0, None, 1.0);
            kit::min_size(tree, editor, 0.0, 0.0);
            state.borrow_mut().content_node = Some(editor);
            Some(editor)
        }
        ViewMode::Preview => {
            {
                let mut state = state.borrow_mut();
                state.split_editor = None;
                state.split_preview = None;
                state.split_dragging = false;
            }
            let preview = ui::preview::build(tree, main, state.clone());
            kit::flex(tree, preview, 1.0, None, 1.0);
            kit::min_size(tree, preview, 0.0, 0.0);
            state.borrow_mut().content_node = Some(preview);
            None
        }
        ViewMode::Split => {
            let split = kit::node(tree, main, "split");
            kit::set_container(tree, split, kit::tight_row());
            kit::flex(tree, split, 1.0, None, 1.0);
            let ratio = state.borrow().split_of_active();
            let editor = ui::editor::build(tree, split, state.clone());
            kit::flex_percent(tree, editor, ratio);
            kit::min_size(tree, editor, 0.0, 0.0);
            let divider = ui::split::build(tree, split, state.clone());
            kit::flex(tree, divider, 0.0, Some(ui::handle::HANDLE_WIDTH), 0.0);
            kit::min_size(tree, divider, ui::handle::HANDLE_WIDTH, 0.0);
            let preview = ui::preview::build(tree, split, state.clone());
            kit::flex(tree, preview, 1.0, None, 1.0);
            kit::min_size(tree, preview, 0.0, 0.0);
            {
                let mut state = state.borrow_mut();
                state.content_node = Some(editor);
                state.split_editor = Some(editor);
                state.split_preview = Some(preview);
                state.split_dragging = false;
                state.split_divider = Rect::ZERO;
            }
            Some(editor)
        }
    };

    if state.borrow().terminal.open {
        let handle = ui::terminal::build_handle(tree, main, state.clone());
        kit::flex(tree, handle, 0.0, Some(ui::handle::HANDLE_WIDTH), 0.0);
        kit::min_size(tree, handle, 0.0, ui::handle::HANDLE_WIDTH);
        let terminal = ui::terminal::build(tree, main, state.clone());
        let height = state.borrow().terminal.height.clamp(120.0, 600.0);
        kit::flex(tree, terminal, 0.0, Some(height), 0.0);
        kit::min_size(tree, terminal, 0.0, height);
        state.borrow_mut().terminal.node = Some(terminal);
    } else {
        state.borrow_mut().terminal.node = None;
    }

    if state.borrow().show_outline {
        let width = ui::outline::clamp_width(state.borrow().session.outline_width);
        let divider = ui::outline::build_divider(tree, body, state.clone());
        kit::flex(tree, divider, 0.0, Some(ui::handle::HANDLE_WIDTH), 0.0);
        kit::min_size(tree, divider, ui::handle::HANDLE_WIDTH, 0.0);
        let outline = ui::outline::build(tree, body, state.clone());
        kit::flex(tree, outline, 0.0, Some(width), 0.0);
        kit::min_size(tree, outline, width, 0.0);
        state.borrow_mut().outline_node = Some(outline);
    } else {
        state.borrow_mut().outline_node = None;
    }

    let status = ui::status::build(tree, column, state.clone());
    kit::flex(tree, status, 0.0, Some(m.status_h), 0.0);
    kit::min_size(tree, status, 0.0, m.status_h);

    {
        let mut state = state.borrow_mut();
        state.built_view = view;
        if view == ViewMode::Split {
            state.built_split = state.split_of_active();
        }
    }
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
            self.restore_window_position();
        }
        if let Some(drop) = ctx.service::<SharedDropTarget>() {
            self.drop = Some(drop.clone());
        }
        crate::macos::install_dock();
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
