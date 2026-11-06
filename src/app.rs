//! The application: shell construction, input routing and the `igui_app` glue.
//!
//! The shell is rebuilt only when its structure changes (sidebar, find bar,
//! view mode); every other frame is a repaint over the same tree. All state
//! lives in [`AppState`], which input callbacks and painters share.

use std::cell::{Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use igui_app::{AppLogic, EventContext, FrameContext, InitContext};
use igui_core::{
    Cursor, EventResult, InputEvent, Key, NodeId, PointerButton, Rect, Vec2, ViewportSize,
};
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
use crate::wake::{FrameClock, WindowWake};

/// The editor application, host-agnostic: no window, no wgpu, no winit.
pub struct EditorApp {
    state: Rc<RefCell<AppState>>,
    tree: SceneTree,
    shell: NodeId,
    editor_id: Option<NodeId>,
    dialogs: Box<dyn Dialogs>,
    window: Option<SharedWindow>,
    #[cfg(feature = "profile")]
    profile: Option<Rc<RefCell<crate::profile::ProfileState>>>,
    /// Set while syntax highlighting is still catching up (drives `needs_frame`).
    analysis_behind: bool,
    /// File-drop state shared with the winit observer.
    drop: Option<SharedDropTarget>,
    /// Whether the saved window position was applied.
    window_restored: bool,
    /// Cross-thread wakeups (PTY output, frame clock).
    wake: WindowWake,
    /// Deadline driver; `None` in headless runs and on non-windowed hosts.
    clock: Option<FrameClock>,
    /// Last UI `DrawList`, reused while only the app overlays change.
    paint_cache: igui_ui::UiPaintCache,
    /// Whether the last frame spliced the cached tree list (diagnostics).
    last_paint_reused: bool,
    /// Mirrors of the native Dock/menu state, so idle frames do not clone or
    /// rebuild it.
    recent: Vec<PathBuf>,
    /// Active-document identity last pushed to the window title.
    title_key: Option<(u64, bool)>,
    title_folder: Option<PathBuf>,
    /// The dialog currently open (native sheet / portal / scripted).
    pending: Option<PendingDialog>,
    /// Drop the committed text that follows the Alt+Z soft-wrap shortcut.
    suppress_text: bool,
    /// A custom header drag is in flight (past the click threshold); pointer
    /// moves must not leak into editor selection.
    window_dragging: bool,
    /// Header press that may become a drag. Starting only after a small move
    /// keeps plain clicks and double-clicks from nudging the window.
    window_drag_pending: Option<Vec2>,
    /// The current left press started in the custom header; consume its
    /// release so a traffic-light click cannot fall through to the UI below.
    header_pressed: bool,
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
        let cli_file = open.as_ref().filter(|path| path.is_file()).cloned();
        let workspace = workspace_for(open, &session);
        // A lazily bound wake handle: the PTY is restored below, before the
        // window exists, and must still be able to ask for frames later.
        let wake = WindowWake::default();
        let mut state = AppState::new(
            Theme::new(theme_mode),
            workspace,
            session,
            config_path.clone(),
        );
        state.wake = Some(wake.function());
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
        // Keep the session's project in sync with what actually opened (CLI
        // folder, a restored project, or none).
        let project = state.ws.folder().map(Path::to_path_buf);
        if state.session.project != project {
            state.session.project = project;
            state.config_dirty = true;
        }
        if let Some(project) = state.ws.folder() {
            state.session.push_recent_project(project);
        }
        if let Some(file) = cli_file {
            state.session.push_recent_file(&file);
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
            window_dragging: false,
            window_drag_pending: None,
            header_pressed: false,
            analysis_behind: false,
            drop: None,
            window_restored: false,
            wake,
            clock: None,
            paint_cache: igui_ui::UiPaintCache::new(),
            last_paint_reused: false,
            recent: Vec::new(),
            title_key: None,
            title_folder: None,
            #[cfg(feature = "profile")]
            profile: None,
        }
    }

    /// Attaches the optional runtime profiler (feature `profile`).
    #[cfg(feature = "profile")]
    pub fn set_profile(&mut self, profile: Rc<RefCell<crate::profile::ProfileState>>) {
        self.profile = Some(profile);
    }

    pub fn state(&self) -> Ref<'_, AppState> {
        self.state.borrow()
    }

    /// Mutable state for wiring tests (the self-check drives caret/settings
    /// through it when a pointer/keys would be overkill).
    #[doc(hidden)]
    pub fn state_mut(&mut self) -> std::cell::RefMut<'_, AppState> {
        let mut state = self.state.borrow_mut();
        // Wiring tests mutate visible state directly; keep the paint cache
        // honest without requiring every test to know about it.
        state.dirty = true;
        state
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

    /// Whether the last frame reused the cached tree `DrawList` (self-check).
    #[doc(hidden)]
    pub fn paint_cache_reused(&self) -> bool {
        self.last_paint_reused
    }

    /// Why the frame loop is still awake (self-check diagnostics).
    #[doc(hidden)]
    pub fn frame_blocker(&self) -> &'static str {
        if self.pending.is_some() {
            "dialog"
        } else if self.window_dragging {
            "window-drag"
        } else if self.analysis_behind {
            "highlight"
        } else {
            let state = self.state.borrow();
            if state.dirty {
                "dirty"
            } else if state.search_busy() {
                "search"
            } else if state.quick_open_busy() {
                "quick-open"
            } else if state.git.pending {
                "git-pending"
            } else if state.git.loading {
                "git-loading"
            } else if state.git.diff_loading {
                "git-diff"
            } else if state.lsp.is_busy() {
                "lsp"
            } else {
                ""
            }
        }
    }

    /// Drains deferred work: rebuild the shell when its structure changed,
    /// refresh find matches, run queued dialogs, persist the session, expire the
    /// status message and sync the window title.
    pub fn update(&mut self, _delta: f32) {
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            profile.borrow_mut().begin_frame();
        }
        // Moving the frame from the clock (rather than a nested AppKit drag
        // loop) keeps winit in charge of the event loop. `needs_frame` keeps
        // frames coming while the button is held.
        if self.window_dragging {
            if crate::macos::window_drag_button_down() {
                crate::macos::continue_window_drag();
            } else {
                crate::macos::end_window_drag();
                self.window_dragging = false;
            }
        }
        // Dialogs run first: opening a Markdown file can change the pane
        // structure, and the same frame must rebuild for it.
        self.pump_dialog();
        self.pump_native_requests();
        // Keep native Dock Recent / quit state in step with the session. Both
        // are Objective-C calls; only touch them when the value changed.
        let recent_changed = {
            let state = self.state.borrow();
            let projects = &state.session.recent_projects;
            let files = &state.session.recent_files;
            let project_count = projects.len().min(crate::state::MAX_RECENT);
            let file_count = files
                .len()
                .min(crate::state::MAX_RECENT.saturating_sub(project_count));
            self.recent.len() != project_count + file_count
                || self.recent[..project_count] != projects[..project_count]
                || self.recent[project_count..] != files[..file_count]
        };
        if recent_changed {
            let recent = {
                let state = self.state.borrow();
                let mut recent = state.session.recent_projects.clone();
                recent.extend(state.session.recent_files.iter().cloned());
                recent.truncate(crate::state::MAX_RECENT);
                recent
            };
            crate::macos::set_recent(&recent);
            self.recent = recent;
        }
        // The quit confirmation's "Discard & Quit" is a state flag; the shell
        // owns the native terminate call.
        if self.state.borrow_mut().quit_requested {
            self.quit_now();
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
        self.state.borrow_mut().tick_preview_debounce();
        self.state.borrow_mut().tick_external_changes();
        self.state.borrow_mut().tick_git();
        self.state.borrow_mut().tick_lsp();
        self.state.borrow_mut().tick_auto_completion();
        self.state.borrow_mut().tick_signature_help();
        self.state.borrow_mut().tick_occurrences();
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
        self.state.borrow_mut().quick_open_tick();
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
                .is_some_and(|(_, _, at)| at.elapsed() > crate::state::schedule::STATUS_TTL)
        };
        if expired {
            let mut state = self.state.borrow_mut();
            state.message = None;
            state.dirty = true;
        }
        // A coalesced preview rebuild is only meaningful while the preview
        // pane is mounted; drop the deadline instead of waking for nothing.
        {
            let mut state = self.state.borrow_mut();
            if state.preview.rebuild_pending()
                && (state.ws.active_doc().is_none() || state.view_of_active() == ViewMode::Edit)
            {
                state.preview.cancel_rebuild();
            }
        }
        self.sync_title();
        self.state.borrow_mut().save_session_if_dirty();
        self.schedule_wake();
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            profile.borrow_mut().end_update();
        }
    }

    /// Hands the earliest time-based deadline to the frame clock. Time-based
    /// work must never return `true` from `needs_frame` (that asks for the next
    /// frame immediately and recreates the 60 fps idle loop).
    fn schedule_wake(&self) {
        let Some(clock) = self.clock.as_ref() else {
            return;
        };
        clock.set_deadline(self.state.borrow().next_wake());
    }

    pub fn layout(&mut self, viewport: ViewportSize) {
        {
            let mut state = self.state.borrow_mut();
            if state.viewport != viewport {
                state.viewport = viewport;
                // Menus/modals anchor to the viewport; their cached geometry
                // must not survive a resize.
                state.dirty = true;
            }
        }
        igui_ui::layout(&mut self.tree, viewport);
        self.tree.update();
        // Both drag handles share this vertical anchor, so their grip bars are
        // always the same height (including while the find bar is open).
        let node = self.state.borrow().content_node;
        let center = node
            .and_then(|id| self.tree.data::<Control>(id))
            .map(|control| control.data.rect.center().y);
        self.state.borrow_mut().content_center_y = center;
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            profile.borrow_mut().end_layout();
        }
    }

    pub fn paint(&mut self, ctx: &mut PaintContext) {
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            profile.borrow_mut().begin_paint();
        }
        // Consume the dirty flag at entry: anything a painter sets during this
        // frame survives for the next one (`needs_frame` also reads it).
        let dirty = std::mem::take(&mut self.state.borrow_mut().dirty);
        if dirty {
            self.paint_cache.clear();
        }
        // Rebuild the tree's `DrawList` only when the tree or the app changed;
        // an idle frame replays the cached commands.
        self.last_paint_reused =
            igui_ui::paint_cached(&self.tree, &mut self.paint_cache, ctx).is_reused();
        // Git diff is an app overlay, above the tree but below menus/modals.
        let measurer = igui_ui::text_measurer(&self.tree);
        let mut state = self.state.borrow_mut();
        ui::git::paint_diff(&mut state, measurer.as_ref(), ctx);
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
        ui::goto_line::paint(&mut state, measurer.as_ref(), ctx);
        ui::project_search::paint(&mut state, measurer.as_ref(), ctx);
        ui::references::paint(&mut state, measurer.as_ref(), ctx);
        ui::rename_symbol::paint(&mut state, measurer.as_ref(), ctx);
        ui::lsp::paint(&mut state, measurer.as_ref(), ctx);
        ui::tooling::paint(&mut state, measurer.as_ref(), ctx);
        ui::drop::paint(&state, measurer.as_ref(), ctx);
        ui::modal::paint(&mut state, measurer.as_ref(), ctx);
        ui::recovery::paint(&state, measurer.as_ref(), ctx);
        drop(state);
        // The preview debounce deadline is chosen while painting.
        self.schedule_wake();
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            let controls = self.control_count();
            let nodes = self.tree.node_count();
            profile.borrow_mut().end_paint(controls, nodes);
            let viewport = self.state.borrow().viewport;
            profile.borrow_mut().paint(ctx, viewport);
        }
    }

    pub fn event(&mut self, event: &InputEvent) -> EventResult {
        let measurer = igui_ui::text_measurer(&self.tree);
        #[cfg(feature = "profile")]
        if let Some(profile) = self.profile.clone() {
            if matches!(event, InputEvent::KeyDown { key: Key::F4 }) {
                profile.borrow_mut().toggle();
                return EventResult::Handled;
            }
            if profile.borrow_mut().handle_input(event).is_handled() {
                return EventResult::Handled;
            }
        }
        match event {
            InputEvent::PointerDown {
                position,
                button: PointerButton::Left,
            } => {
                self.header_pressed = false;
                if self.window_drag_pending.take().is_some() || self.window_dragging {
                    self.window_dragging = false;
                    crate::macos::end_window_drag();
                }
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
                #[cfg(target_os = "macos")]
                if ui::menu::panel_at(&state, *position).is_none() {
                    if let Some(hit) = ui::menu::header_hit(&state, *position) {
                        // AppKit can re-enter the event loop; never keep the state
                        // RefCell borrowed across the native call.
                        drop(state);
                        self.header_pressed = true;
                        match hit {
                            ui::menu::HeaderHit::Close => self.request_quit(),
                            ui::menu::HeaderHit::Minimize => crate::macos::minimize_window(),
                            ui::menu::HeaderHit::Zoom => {
                                crate::macos::toggle_zoom();
                                self.save_window_state();
                            }
                            ui::menu::HeaderHit::Drag => {
                                // Capture the press-to-frame offset now; movement
                                // only starts after the click threshold.
                                crate::macos::begin_window_drag();
                                self.window_drag_pending = Some(*position);
                            }
                        }
                        return EventResult::Handled;
                    }
                }
                if state.tooling.prompt.is_some() {
                    ui::tooling::click(&mut state, *position);
                    return EventResult::Handled;
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
                if !lsp_chip
                    && (state.lsp.hover.is_some()
                        || state.lsp.completion.is_some()
                        || state.lsp.signature_help.is_some())
                {
                    state.lsp.dismiss_transient();
                    state.clear_signature_help();
                    state.dirty = true;
                    if !definition_click {
                        return EventResult::Handled;
                    }
                }
                state.clear_hover_target();
                state.hover_leave_at = None;
                // The bar sits above the drop-down: a press on a bar item
                // toggles that menu (including closing the open one).
                if state.git.diff_visible {
                    ui::git::click_diff(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.rename_symbol.open {
                    ui::rename_symbol::click(&mut state, *position);
                    return EventResult::Handled;
                }
                if state.references.open {
                    ui::references::click(&mut state, *position);
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
                if state.goto_line.open {
                    ui::goto_line::click(&mut state, *position);
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
                // Footer LSP dot: start/stop every language server.
                if state.lsp_toggle_hit.contains(*position) {
                    let enabled = !state.settings.lsp;
                    state.set_lsp_enabled(enabled);
                    state.menu_open = None;
                    state.menu_hover = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
                // Footer LSP chip: show the bottom dock's PROBLEMS tab.
                if state.lsp_hit.contains(*position) {
                    state.run(Action::ShowPanel(crate::state::Panel::Problems));
                    state.menu_open = None;
                    state.menu_hover = None;
                    state.dirty = true;
                    return EventResult::Handled;
                }
                // Footer theme toggle.
                if state.theme_hit.contains(*position) {
                    state.run(Action::ToggleTheme);
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
                if ui::menu::handle_panel_click(&mut state, *position) {
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
                        ui::tree_menu::open(&mut state, row.path, row.is_dir, *position);
                        return EventResult::Handled;
                    }
                }
                if state.show_sidebar
                    && state.sidebar_tab == "explorer"
                    && state.ws.folder_open
                    && state.sidebar.rect.contains(*position)
                {
                    state.tab_drag = None;
                    state.tab_menu = None;
                    state.tree_menu = None;
                    state.menu_open = None;
                    state.menu_hover = None;
                    ui::tree_menu::open_blank(&mut state, *position);
                    return EventResult::Handled;
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
                if self.window_dragging {
                    if crate::macos::window_drag_button_down() {
                        crate::macos::continue_window_drag();
                    } else {
                        crate::macos::end_window_drag();
                        self.window_dragging = false;
                    }
                    return EventResult::Handled;
                }
                if let Some(start) = self.window_drag_pending {
                    if !crate::macos::window_drag_button_down() {
                        self.window_drag_pending = None;
                        crate::macos::end_window_drag();
                    } else if start.distance_squared(*position) > 9.0 {
                        self.window_drag_pending = None;
                        self.window_dragging = true;
                        crate::macos::continue_window_drag();
                    }
                    return EventResult::Handled;
                }
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
                let theme = state.theme_hit.contains(*position);
                let problems = ui::problems::hover_at(&state, *position);
                let lsp_hover = state.lsp_hit.contains(*position);
                let lsp_toggle_hover = state.lsp_toggle_hit.contains(*position);
                let header_hover = ui::menu::header_button_at(&state, *position);
                let panel_hover = ui::menu::panel_at(&state, *position);
                let dock_hover = ui::dock::tab_at_any(&state, *position);
                let dock_close_hover = ui::dock::close_at(&state, *position);
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
                    || state.theme_hover != theme
                    || state.lsp_hover != lsp_hover
                    || state.lsp_toggle_hover != lsp_toggle_hover
                    || state.problems.hover != problems
                    || state.header_hover != header_hover
                    || state.panel_hover != panel_hover
                    || state.dock_hover != dock_hover
                    || state.dock_close_hover != dock_close_hover
                {
                    state.menubar_hover = menubar;
                    state.menu_hover = menu_hover;
                    state.tabs.hover = tabs;
                    state.sidebar.hover = sidebar;
                    state.language_hover = language;
                    state.theme_hover = theme;
                    state.lsp_hover = lsp_hover;
                    state.lsp_toggle_hover = lsp_toggle_hover;
                    state.problems.hover = problems;
                    state.header_hover = header_hover;
                    state.panel_hover = panel_hover;
                    state.dock_hover = dock_hover;
                    state.dock_close_hover = dock_close_hover;
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
                let header_pressed = std::mem::take(&mut self.header_pressed);
                if header_pressed
                    || self.window_drag_pending.take().is_some()
                    || self.window_dragging
                {
                    self.window_dragging = false;
                    crate::macos::end_window_drag();
                    return EventResult::Handled;
                }
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::DoubleClick { position } => {
                #[cfg(target_os = "macos")]
                {
                    let on_panel = {
                        let state = self.state.borrow();
                        ui::menu::panel_at(&state, *position).is_some()
                    };
                    if on_panel {
                        return EventResult::Handled;
                    }
                    let hit = {
                        let state = self.state.borrow();
                        ui::menu::header_hit(&state, *position)
                    };
                    if let Some(hit) = hit {
                        if self.window_drag_pending.take().is_some() || self.window_dragging {
                            self.window_dragging = false;
                            crate::macos::end_window_drag();
                        }
                        if hit == ui::menu::HeaderHit::Drag {
                            crate::macos::toggle_zoom();
                            self.save_window_state();
                        }
                        return EventResult::Handled;
                    }
                }
                let result = igui_ui::route_input(&mut self.tree, event);
                self.focus_editor();
                result
            }
            InputEvent::Wheel {
                position, delta, ..
            } => {
                let mut state = self.state.borrow_mut();
                if state.tooling.prompt.is_some() {
                    return EventResult::Handled;
                }
                if state.rename_symbol.open {
                    return EventResult::Handled;
                }
                if state.references.open {
                    let panel = ui::references::layout(&state).panel;
                    if panel.contains(*position) {
                        ui::references::scroll(&mut state, delta.y);
                    }
                    return EventResult::Handled;
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
                if state.tooling.prompt.is_some() {
                    if *key == Key::Escape {
                        state.close_tool_prompt();
                    }
                    return EventResult::Handled;
                }
                if state.references.open && ui::references::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.rename_symbol.open
                    && ui::rename_symbol::on_key(&mut state, *key, modifiers)
                {
                    return EventResult::Handled;
                }
                if state.settings_open && ui::settings::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.new_file.open && ui::new_file::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.goto_line.open && ui::goto_line::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.quick_open.open && ui::quick_open::on_key(&mut state, *key, modifiers) {
                    return EventResult::Handled;
                }
                if state.search.open && ui::project_search::on_key(&mut state, *key, modifiers) {
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
                if *key == Key::Escape && state.lsp.signature_help.is_some() {
                    state.clear_signature_help();
                    return EventResult::Handled;
                }
                if state.focus == Focus::Editor
                    && modifiers.ctrl
                    && !modifiers.meta
                    && *key == Key::Space
                {
                    state.request_lsp_completion();
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
                    // Conventional terminal clipboard chords must reach the
                    // keymap instead of being encoded as shell control bytes.
                    let clipboard_chord = modifiers.ctrl
                        && modifiers.shift
                        && matches!(
                            key,
                            Key::Character(ch)
                                if matches!(ch.to_ascii_lowercase(), 'c' | 'v' | 'a')
                        );
                    if !clipboard_chord {
                        let application_cursor = state
                            .terminal
                            .active_parser()
                            .is_some_and(|parser| parser.screen().application_cursor());
                        if let Some(bytes) =
                            ui::terminal::encode_key(*key, modifiers, application_cursor)
                        {
                            state.terminal_write(&bytes);
                        }
                        return EventResult::Handled;
                    }
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
                if state.find.open && input::find_key(&mut state, *key, modifiers).is_handled() {
                    return EventResult::Handled;
                }
                if state.focus == Focus::Explorer
                    && !modifiers.meta
                    && !modifiers.ctrl
                    && !modifiers.alt
                {
                    match key {
                        Key::Enter => {
                            state.rename_selected();
                            return EventResult::Handled;
                        }
                        Key::Escape => {
                            state.focus = Focus::Editor;
                            state.dirty = true;
                            return EventResult::Handled;
                        }
                        _ => state.focus = Focus::Editor,
                    }
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
                if state.goto_line.open {
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
                if state.rename_symbol.open {
                    ui::rename_symbol::on_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.settings_open
                    || state.recovery_prompt.is_some()
                    || state.menu_open.is_some()
                    || state.tooling.prompt.is_some()
                    || state.references.open
                {
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
                if state.new_file.open {
                    ui::new_file::on_text(&mut state, text);
                    return EventResult::Handled;
                }
                if state.goto_line.open {
                    ui::goto_line::on_text(&mut state, text);
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
                if state.focus == Focus::Explorer {
                    state.focus = Focus::Editor;
                }
                state.schedule_auto_completion(text);
                state.schedule_signature_help(text);
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
                    // Leaving the window must drop every visual hover, not
                    // just the editor's symbol hover.
                    state.menubar_hover = None;
                    state.menu_hover = None;
                    state.tabs.hover = None;
                    state.sidebar.hover = None;
                    state.language_hover = false;
                    state.theme_hover = false;
                    state.lsp_hover = false;
                    state.lsp_toggle_hover = false;
                    state.lsp_toggle_hover = false;
                    state.problems.hover = None;
                    state.dock_hover = None;
                    state.dock_close_hover = None;
                    state.git.hover = None;
                    state.outline.hover = None;
                    state.header_hover = None;
                    state.panel_hover = None;
                    if let Some(menu) = state.tab_menu.as_mut() {
                        menu.hover = None;
                    }
                    if let Some(menu) = state.tree_menu.as_mut() {
                        menu.hover = None;
                    }
                    state.dirty = true;
                }
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::KeyUp { .. } => {
                self.suppress_text = false;
                igui_ui::route_input(&mut self.tree, event)
            }
            InputEvent::Ime(ime) => {
                let mut state = self.state.borrow_mut();
                if state.settings_open
                    || state.recovery_prompt.is_some()
                    || state.tooling.prompt.is_some()
                {
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
                if state.rename_symbol.open {
                    ui::rename_symbol::on_ime(&mut state, ime);
                    return EventResult::Handled;
                }
                if state.new_file.open {
                    ui::new_file::on_ime(&mut state, ime);
                    return EventResult::Handled;
                }
                if state.goto_line.open {
                    ui::goto_line::on_ime(&mut state, ime);
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
                if state.focus == Focus::Explorer {
                    state.focus = Focus::Editor;
                }
                drop(state);
                igui_ui::route_input(&mut self.tree, event)
            }
            _ => igui_ui::route_input(&mut self.tree, event),
        }
    }

    pub fn caret(&self) -> Option<Rect> {
        let state = self.state.borrow();
        if state.terminal.open
            && state.bottom_tab == crate::state::Panel::Terminal
            && state.focus == Focus::Terminal
        {
            return Some(
                state
                    .terminal
                    .active()
                    .map_or(Rect::ZERO, |session| session.caret_rect),
            );
        }
        drop(state);
        igui_ui::focused_caret(&self.tree)
    }

    pub fn cursor(&self) -> Cursor {
        igui_ui::hovered_cursor(&self.tree)
    }

    pub fn needs_frame(&self) -> bool {
        if self.pending.is_some() || self.window_dragging || self.analysis_behind {
            return true;
        }
        let state = self.state.borrow();
        // `dirty` catches a painter that changed state mid-frame; the rest are
        // "keep polling until the work lands" states. Timed work lives in
        // `AppState::next_wake`, not here.
        state.dirty
            || state.search_busy()
            || state.git_busy()
            || state.lsp.is_busy()
            || state.quick_open_busy()
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
            DialogKind::OpenFolder => state.open_project(path),
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
                crate::native::NativeRequest::Command(command) => {
                    self.run_native_command(command);
                }
            }
        }
    }

    /// Runs a command selected from the native macOS menu.
    fn run_native_command(&mut self, command: crate::native::NativeCommand) {
        use crate::native::NativeCommand as C;
        use crate::state::ViewMode;
        match command {
            C::Quit => self.request_quit(),
            C::Minimize => crate::macos::minimize_window(),
            C::Zoom => {
                crate::macos::toggle_zoom();
                self.save_window_state();
            }
            C::New => self.state.borrow_mut().run(Action::New),
            C::NewFile => self.state.borrow_mut().run(Action::NewFile),
            C::Open => self.state.borrow_mut().run(Action::Open),
            C::OpenFolder => self.state.borrow_mut().run(Action::OpenFolder),
            C::CloseFolder => self.state.borrow_mut().run(Action::CloseFolder),
            C::Save => self.state.borrow_mut().run(Action::Save),
            C::SaveAs => self.state.borrow_mut().run(Action::SaveAs),
            C::CloseTab => self.state.borrow_mut().run(Action::CloseTab),
            C::Undo => self.state.borrow_mut().run(Action::Undo),
            C::Redo => self.state.borrow_mut().run(Action::Redo),
            C::Cut => self.run_edit_command('x', Action::Cut),
            C::Copy => self.run_edit_command('c', Action::Copy),
            C::Paste => self.run_edit_command('v', Action::Paste),
            C::SelectAll => self.run_edit_command('a', Action::SelectAll),
            C::Find => self.state.borrow_mut().run(Action::Find),
            C::GotoLine => self.state.borrow_mut().run(Action::GotoLine),
            C::FindReplace => self.state.borrow_mut().run(Action::FindReplace),
            C::FindNext => self.state.borrow_mut().run(Action::FindNext),
            C::FindPrevious => self.state.borrow_mut().run(Action::FindPrevious),
            C::QuickOpen => self.state.borrow_mut().run(Action::QuickOpen),
            C::ProjectSearch => self.state.borrow_mut().run(Action::ProjectSearch),
            C::ToggleSidebar => self.state.borrow_mut().run(Action::ToggleSidebar),
            C::ToggleOutline => self.state.borrow_mut().run(Action::ToggleOutline),
            C::ToggleTheme => self.state.borrow_mut().run(Action::ToggleTheme),
            C::SetViewEdit => self.state.borrow_mut().run(Action::SetView(ViewMode::Edit)),
            C::SetViewSplit => self
                .state
                .borrow_mut()
                .run(Action::SetView(ViewMode::Split)),
            C::SetViewPreview => self
                .state
                .borrow_mut()
                .run(Action::SetView(ViewMode::Preview)),
            C::ToggleScrollSync => self.state.borrow_mut().run(Action::ToggleScrollSync),
            C::ToggleSoftWrap => self.state.borrow_mut().run(Action::ToggleSoftWrap),
            C::ToggleMinimap => self.state.borrow_mut().run(Action::ToggleMinimap),
            C::ToggleTerminal => self.state.borrow_mut().run(Action::ToggleTerminal),
            C::Settings => self.state.borrow_mut().run(Action::Settings),
        }
    }

    /// Native Edit-menu commands must follow the focused find/replace field,
    /// just like the keyboard path.
    fn run_edit_command(&mut self, key: char, action: Action) {
        let mut state = self.state.borrow_mut();
        if matches!(state.focus, Focus::Find | Focus::Replace)
            && input::find_key(
                &mut state,
                Key::Character(key),
                igui_core::Modifiers {
                    meta: true,
                    ..igui_core::Modifiers::NONE
                },
            )
            .is_handled()
        {
            return;
        }
        state.run(action);
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
        let maximized = self.state.borrow().session.window_maximized;
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
        if maximized {
            #[cfg(target_os = "macos")]
            crate::macos::toggle_zoom();
            #[cfg(not(target_os = "macos"))]
            window.set_maximized(true);
        }
        self.window_restored = true;
    }

    /// Persists the current window geometry and maximize state. Called after
    /// green-button/double-click zoom and before quitting, so closing the app
    /// while maximized still restores that state.
    fn save_window_state(&mut self) {
        self.persist_window_geometry();
        self.state.borrow_mut().save_session_if_dirty();
    }

    /// The red close button, `⌘Q` and the Dock's Quit: confirm when a buffer is
    /// dirty, otherwise quit immediately.
    fn request_quit(&mut self) {
        let dirty = self
            .state
            .borrow()
            .ws
            .tabs
            .iter()
            .filter(|doc| doc.is_dirty())
            .count();
        if dirty > 0 {
            self.state.borrow_mut().request_quit(dirty);
            return;
        }
        self.quit_now();
    }

    /// Terminates the app. The dirty check already ran in the app, so no
    /// deferred `NSTerminateLater` reply is involved: persist the window state
    /// and ask AppKit to terminate. Native children are SIGKILLed by the
    /// `applicationShouldTerminate:` hook, so nothing here can block the UI
    /// thread on language-server/PTY teardown.
    fn quit_now(&mut self) {
        self.state.borrow_mut().quit_requested = false;
        self.save_window_state();
        crate::macos::request_quit();
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
        #[cfg(target_os = "macos")]
        let maximized = crate::macos::window_is_zoomed();
        #[cfg(not(target_os = "macos"))]
        let maximized = window.is_maximized();
        if window.fullscreen().is_some() {
            return;
        }
        if maximized {
            self.state.borrow_mut().set_window_maximized(true);
            return;
        }
        self.state.borrow_mut().set_window_maximized(false);
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
        let doc_key = self
            .state
            .borrow()
            .ws
            .active_doc()
            .map(|doc| (doc.id(), doc.is_dirty()));
        let folder_changed = {
            let state = self.state.borrow();
            state.ws.folder() != self.title_folder.as_deref()
        };
        if self.title_key == doc_key && !folder_changed {
            return;
        }
        self.title_key = doc_key;
        let title = {
            let state = self.state.borrow();
            if folder_changed {
                self.title_folder = state.ws.folder().map(Path::to_path_buf);
            }
            match state.ws.active_doc() {
                Some(doc) => format!(
                    "{}{} — igui-editor",
                    doc.name(),
                    if doc.is_dirty() { " •" } else { "" }
                ),
                None => self
                    .title_folder
                    .as_deref()
                    .and_then(|folder| folder.file_name())
                    .map(|name| format!("{} — igui-editor", name.to_string_lossy()))
                    .unwrap_or_else(|| "igui-editor".to_string()),
            }
        };
        self.state.borrow_mut().title = title.clone();
        if let Some(window) = &self.window {
            let window = window.borrow().clone();
            if let Some(window) = window {
                window.set_title(&title);
            }
        }
    }

    fn rebuild(&mut self) {
        {
            let mut state = self.state.borrow_mut();
            state.needs_rebuild = false;
            state.dirty = true;
        }
        self.paint_cache.clear();
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
/// - An explicit CLI **folder** opens it as the project (no tab).
/// - An explicit CLI **file** opens with its directory as the project.
/// - Otherwise the stored project is restored, then the stored tabs reopen.
/// - With neither, the app starts in the welcome state: no project, no buffer.
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
    let project = session
        .project
        .clone()
        .filter(|path| path.is_dir())
        .or_else(|| {
            // v1 sessions have no project key: keep the old behaviour.
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
    if let Some(project) = project {
        let _ = workspace.set_folder(project);
    }
    for path in &session.tabs {
        if path.is_file() {
            let _ = workspace.open(path);
        }
    }
    // `open` leaves the last tab active, so restore the recorded tab explicitly.
    // Missing paths simply fall back to that last successfully opened tab.
    if let Some(active) = session.active_tab.as_deref() {
        if let Some(index) = workspace
            .tabs
            .iter()
            .position(|doc| doc.path() == Some(active))
        {
            workspace.select(index);
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

    let docs_view = state
        .borrow()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.is_docs_view());
    let editor = if docs_view {
        {
            let mut state = state.borrow_mut();
            state.split_editor = None;
            state.split_preview = None;
            state.split_dragging = false;
        }
        let docs = ui::docs_view::build(tree, main, state.clone());
        kit::flex(tree, docs, 1.0, None, 1.0);
        kit::min_size(tree, docs, 0.0, 0.0);
        state.borrow_mut().content_node = Some(docs);
        Some(docs)
    } else {
        match view {
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
        }
    };

    // Bottom dock: one seam, one shared header (tabs + shell instance tabs),
    // and the body of the active tab. It stays inside `main`, so it spans the
    // editor column instead of the whole window.
    if state.borrow().terminal.open {
        let handle = ui::terminal::build_handle(tree, main, state.clone());
        kit::flex(tree, handle, 0.0, Some(ui::handle::HANDLE_WIDTH), 0.0);
        kit::min_size(tree, handle, 0.0, ui::handle::HANDLE_WIDTH);
        let dock = kit::node(tree, main, "bottom-dock");
        kit::set_container(tree, dock, kit::tight_column());
        let height = state.borrow().terminal.height.clamp(120.0, 600.0);
        kit::flex(tree, dock, 0.0, Some(height), 0.0);
        kit::min_size(tree, dock, 0.0, height);
        let header = ui::dock::build_bottom(tree, dock, state.clone());
        kit::flex(tree, header, 0.0, Some(ui::dock::HEADER_H), 0.0);
        kit::min_size(tree, header, 0.0, ui::dock::HEADER_H);
        let bottom_tab = state.borrow().bottom_tab;
        let body = if bottom_tab == crate::state::Panel::Terminal {
            ui::terminal::build(tree, dock, state.clone())
        } else {
            ui::problems::build(tree, dock, state.clone())
        };
        kit::flex(tree, body, 1.0, None, 1.0);
        kit::min_size(tree, body, 0.0, 0.0);
        state.borrow_mut().terminal.node = Some(dock);
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
        state.built_docs = docs_view;
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
            if let Some(handle) = window.borrow().as_ref().cloned() {
                self.wake.set(handle);
                if self.clock.is_none() {
                    self.clock = Some(FrameClock::start(self.wake.function()));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_workspace_selects_the_recorded_active_tab() {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-active-tab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = ["a.rs", "b.rs", "c.rs"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                std::fs::write(&path, "fn main() {}\n").unwrap();
                std::fs::canonicalize(&path).unwrap()
            })
            .collect();
        let session = Session {
            project: Some(std::fs::canonicalize(&dir).unwrap()),
            tabs: paths.clone(),
            active_tab: Some(paths[1].clone()),
            ..Session::default()
        };
        let workspace = workspace_for(None, &session);
        assert_eq!(
            workspace.active_doc().and_then(|doc| doc.path()),
            Some(paths[1].as_path())
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
