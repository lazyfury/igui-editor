//! `--selfcheck`: the same view, the same events, no window.
//!
//! The app is driven directly (update -> layout -> paint -> record) into
//! [`RecordingBackend`], then the recorded `DrawList` and the model are
//! asserted on. Native dialogs are replaced by [`ScriptedDialogs`], and the
//! session file lives in the fixture directory, so the check never opens an OS
//! window and never touches the user's real configuration.
//!
//! Covered: startup, typing, save, undo/redo, the File menu, find/replace,
//! Save As, opening through a dialog, Markdown split preview, mode cycling, the
//! dirty-close modal, sidebar opening + focus return, drag selection, wheel
//! scrolling, theme switching, opening projects and session restore.
//!
//! No OS-level capture is used anywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use igui_backend_recording::{RecordedFrame, RecordingBackend};
use igui_core::{InputEvent, Key, Modifiers, PointerButton, Rect, Size, Vec2, ViewportSize};
use igui_profile::{inspect, FrameCounters, FrameStats, Severity};
use igui_render::{DrawCommand, PaintContext, RenderBackend};

use crate::app::EditorApp;
use crate::dialogs::{Call, ScriptedDialogs};
use crate::keymap::KeymapId;
use crate::kit::RectExt;
use crate::model::doc::LineEnding;
use crate::model::recovery::RecoveryStore;
use crate::model::session::WindowGeometry;
use crate::settings::Row;
use crate::state::{Action, Focus, HeaderButton, Menu, ModalButton, Panel, ViewMode};
use crate::theme::Mode;

const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 800.0;

const MAIN_RS: &str =
    "// header comment\nfn main() {\n    let s = \"hello\";\n    println!(s);\n}\n";
const NOTES_MD: &str = "# Notes\n\n## Notes\n\n- square corners\n- compact rows\n\n\
                        | Key | Meaning |\n|:----|--------:|\n| a   | alpha   |\n| b   | **beta** |\n\n\
                        ```rust\nfn preview() {}\n```\n";

pub fn run() -> Result<(), String> {
    let dir = fixture()?;
    let result = check(&dir)
        .and_then(|()| check_workspace_and_recovery(&dir))
        .and_then(|()| check_explorer_filters(&dir))
        .and_then(|()| check_typing(&dir))
        .and_then(|()| check_lsp(&dir))
        .and_then(|()| check_keymaps(&dir))
        .and_then(|()| check_goto_line(&dir))
        .and_then(|()| check_overlay_pointer(&dir))
        .and_then(|()| check_markdown_code_wrap(&dir))
        .and_then(|()| check_ime_caret(&dir))
        .and_then(|()| check_file_icons(&dir))
        .and_then(|()| check_emoji(&dir))
        .and_then(|()| check_markdown_outline(&dir))
        .and_then(|()| check_markdown_preview_outline_jump(&dir))
        .and_then(|()| check_godot_outline(&dir))
        .and_then(|()| check_minimap(&dir));
    let _ = fs::remove_dir_all(&dir);
    if result.is_ok() {
        println!(
            "selfcheck ok: main flow + workspace/recovery + explorer filters + file icons + typing + LSP + keymap + go-to-line + overlay pointer + code wrap + IME caret + emoji + Markdown/GDScript outline + preview outline jump + minimap checks"
        );
    }
    result
}

struct Harness {
    app: EditorApp,
    dialogs: ScriptedDialogs,
    viewport: ViewportSize,
}

impl Harness {
    fn new(open: Option<PathBuf>, config: PathBuf, dialogs: ScriptedDialogs) -> Self {
        let app = EditorApp::with_config(open, Some(Mode::Dark), config, Box::new(dialogs.clone()));
        let mut harness = Self {
            app,
            dialogs,
            viewport: ViewportSize::new(Size::new(WIDTH, HEIGHT)),
        };
        harness.frame().expect("startup frame");
        harness
    }

    fn frame(&mut self) -> Result<RecordedFrame, String> {
        self.app.update(0.0);
        self.app.layout(self.viewport);
        record(&mut self.app, self.viewport)
    }

    fn event(&mut self, event: InputEvent) {
        let _ = self.app.event(&event);
    }

    fn click(&mut self, position: Vec2) {
        self.event(InputEvent::PointerDown {
            position,
            button: PointerButton::Left,
        });
        self.event(InputEvent::PointerUp {
            position,
            button: PointerButton::Left,
        });
    }

    fn shortcut(&mut self, key: Key, modifiers: Modifiers) {
        self.event(InputEvent::ModifiersChanged(modifiers));
        self.event(InputEvent::KeyDown { key });
        self.event(InputEvent::KeyUp { key });
        self.event(InputEvent::ModifiersChanged(Modifiers::NONE));
    }

    fn key(&mut self, key: Key) {
        self.shortcut(key, Modifiers::NONE);
    }

    fn type_text(&mut self, text: &str) {
        self.event(InputEvent::TextInput { text: text.into() });
    }

    fn drag(&mut self, from: Vec2, to: Vec2) {
        self.event(InputEvent::PointerDown {
            position: from,
            button: PointerButton::Left,
        });
        self.event(InputEvent::PointerMove { position: to });
        self.event(InputEvent::PointerUp {
            position: to,
            button: PointerButton::Left,
        });
    }

    fn double_click(&mut self, position: Vec2) {
        self.event(InputEvent::PointerDown {
            position,
            button: PointerButton::Left,
        });
        self.event(InputEvent::DoubleClick { position });
        self.event(InputEvent::PointerUp {
            position,
            button: PointerButton::Left,
        });
    }

    fn right_click(&mut self, position: Vec2) {
        self.event(InputEvent::PointerDown {
            position,
            button: PointerButton::Right,
        });
        self.event(InputEvent::PointerUp {
            position,
            button: PointerButton::Right,
        });
    }

    fn find_hit(&self, hit: crate::state::FindHit) -> Vec2 {
        self.app
            .state()
            .find
            .hits
            .iter()
            .find(|(candidate, _)| *candidate == hit)
            .map(|(_, rect)| rect.center())
            .expect("find bar hit rect")
    }

    fn find_layout_ready(&mut self) {
        // The bar records its hit rectangles during paint.
        let _ = self.frame();
    }
}

/// Restores a process-wide env var when the check returns (including errors).
struct EnvGuard(&'static str);

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

/// Header panel group: one recorded hit per panel, icon strokes in the header
/// band, and a click toggles the dock without starting a window drag.
fn check_header_panels(harness: &mut Harness, frame: &RecordedFrame) -> Result<(), String> {
    let (buttons, menubar_h, viewport_right) = {
        let state = harness.app.state();
        (
            state.panel_hits.clone(),
            state.theme.m.menubar_h,
            state.viewport.logical_rect().right(),
        )
    };
    if buttons.len() != Panel::ALL.len() {
        return Err(format!(
            "expected {} header panel buttons, got {}",
            Panel::ALL.len(),
            buttons.len()
        ));
    }
    for (index, (panel, _)) in buttons.iter().enumerate() {
        if *panel != Panel::ALL[index] {
            return Err(format!(
                "header panel {:?} painted out of order (expected {:?})",
                panel,
                Panel::ALL[index]
            ));
        }
    }
    let project = {
        let state = harness.app.state();
        state.ws.folder_open.then(|| {
            state
                .ws
                .root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| state.ws.root.display().to_string())
        })
    };
    if let Some(project) = project {
        let mut painted = false;
        for command in frame.commands() {
            if let DrawCommand::DrawText { text, position, .. } = command {
                if *text == project {
                    if position.y > menubar_h + 1.0 {
                        return Err(format!(
                            "the project folder name is painted outside the header at {position:?}"
                        ));
                    }
                    painted = true;
                }
            }
        }
        if !painted {
            return Err("the header did not paint the project folder name".into());
        }
    }
    for (panel, rect) in &buttons {
        if rect.height() < 20.0 || rect.left() < 0.0 || rect.right() > viewport_right + 0.5 {
            return Err(format!(
                "{panel:?} panel button has invalid geometry {rect:?}"
            ));
        }
        if rect.top() < 0.0 || rect.bottom() > menubar_h + 0.5 {
            return Err(format!("{panel:?} panel button escapes the header band"));
        }
    }
    for pair in buttons.windows(2) {
        if pair[0].1.right() > pair[1].1.left() + 0.5 {
            return Err(format!(
                "panel buttons overlap: {:?} then {:?}",
                pair[0].0, pair[1].0
            ));
        }
    }
    let header_stroke = frame.commands().iter().any(|command| match command {
        DrawCommand::Line { from, to, .. } => from.y <= menubar_h && to.y <= menubar_h,
        DrawCommand::FillCircle { center, .. } => center.y <= menubar_h,
        _ => false,
    });
    if !header_stroke {
        return Err("header panel icons painted no strokes".into());
    }

    let center = |panel: Panel| -> Result<Vec2, String> {
        buttons
            .iter()
            .find(|(candidate, _)| *candidate == panel)
            .map(|(_, rect)| rect.center())
            .ok_or_else(|| format!("no header button for {panel:?}"))
    };
    let explorer = center(Panel::Explorer)?;
    let initially_open = Panel::Explorer.is_open(&harness.app.state());
    harness.click(explorer);
    harness.frame()?;
    if Panel::Explorer.is_open(&harness.app.state()) == initially_open {
        return Err("clicking the header Explorer toggle did not change the sidebar".into());
    }
    harness.click(explorer);
    harness.frame()?;
    if Panel::Explorer.is_open(&harness.app.state()) != initially_open {
        return Err("the Explorer header toggle did not restore the sidebar".into());
    }

    let git = center(Panel::Git)?;
    harness.click(git);
    harness.frame()?;
    if !Panel::Git.is_open(&harness.app.state()) {
        return Err("clicking the header Git toggle did not open the Git dock".into());
    }
    harness.click(explorer);
    harness.frame()?;
    if !Panel::Explorer.is_open(&harness.app.state()) {
        return Err("the Explorer header toggle did not switch back from Git".into());
    }

    let outline = center(Panel::Outline)?;
    let outline_before = harness.app.state().show_outline;
    harness.click(outline);
    harness.frame()?;
    if harness.app.state().show_outline == outline_before {
        return Err("clicking the header Outline toggle did not change the outline".into());
    }
    harness.click(outline);
    harness.frame()?;
    if harness.app.state().show_outline != outline_before {
        return Err("the Outline header toggle did not restore the outline".into());
    }

    // The bottom dock's two tabs share one header button group: each button
    // opens its own tab and a second click closes the dock.
    let problems = center(Panel::Problems)?;
    harness.click(problems);
    harness.frame()?;
    if !Panel::Problems.is_open(&harness.app.state()) {
        return Err("clicking the header Problems toggle did not open its dock tab".into());
    }
    harness.click(problems);
    harness.frame()?;
    if Panel::Problems.is_open(&harness.app.state()) {
        return Err("a second Problems click did not close the bottom dock".into());
    }

    // Hover feedback is state-only (no domain change).
    harness.event(InputEvent::PointerMove { position: explorer });
    if harness.app.state().panel_hover != Some(Panel::Explorer) {
        return Err("moving over a header panel button did not set its hover".into());
    }
    harness.event(InputEvent::PointerMove {
        position: Vec2::new(400.0, 400.0),
    });
    if harness.app.state().panel_hover.is_some() {
        return Err("leaving the header did not clear the panel-button hover".into());
    }
    harness.frame()?;
    Ok(())
}

fn check_footer_controls(harness: &mut Harness, frame: &RecordedFrame) -> Result<(), String> {
    let baseline = frame
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::DrawText { text, position, .. } if text == "UTF-8" => Some(position.y),
            _ => None,
        })
        .ok_or("the footer painted no UTF-8 fact")?;
    for command in frame.commands() {
        if let DrawCommand::DrawText { text, position, .. } = command {
            if (position.y - baseline).abs() < 0.01 && (text == "EDITOR" || text == "TERMINAL") {
                return Err(format!(
                    "the footer still shows a focus indicator: {text:?}"
                ));
            }
        }
    }

    let theme_hit = {
        let state = harness.app.state();
        if state.theme_hit.width() <= 0.0 {
            return Err("the footer theme chip has no hit target".into());
        }
        state.theme_hit
    };
    harness.event(InputEvent::PointerMove {
        position: theme_hit.center(),
    });
    if !harness.app.state().theme_hover {
        return Err("moving over the footer theme chip did not set its hover".into());
    }

    let before = harness.app.state().theme.mode;
    let hit = harness.app.state().theme_hit.center();
    harness.click(hit);
    harness.frame()?;
    if harness.app.state().theme.mode == before {
        return Err("clicking the footer theme chip did not toggle the theme".into());
    }
    let hit = harness.app.state().theme_hit.center();
    harness.click(hit);
    harness.frame()?;
    if harness.app.state().theme.mode != before {
        return Err("clicking the footer theme chip did not toggle back".into());
    }
    harness.event(InputEvent::PointerMove {
        position: Vec2::new(400.0, 400.0),
    });
    if harness.app.state().theme_hover {
        return Err("leaving the footer theme chip did not clear its hover".into());
    }
    harness.frame()?;
    Ok(())
}

/// Every glyph in the editor text area must be painted exactly once.
///
/// Antialiased glyphs composite, so a token painted over base text (or a
/// semantic token over a syntax token) renders heavier and muddier than the
/// same text in one pass. Two overlapping runs on one baseline is that bug.
fn check_editor_text_painted_once(harness: &Harness, frame: &RecordedFrame) -> Result<(), String> {
    let state = harness.app.state();
    // The find bar and the IME preedit intentionally paint over the text.
    if state.find.open || state.editor.preedit.is_some() {
        return Ok(());
    }
    let rect = state.editor.text_rect;
    let measurer = igui_ui::text_measurer(harness.app.tree());
    let mut runs: Vec<(f32, f32, f32, String)> = Vec::new();
    for command in frame.commands() {
        let DrawCommand::DrawText {
            text,
            position,
            font_size,
            family,
            ..
        } = command
        else {
            continue;
        };
        if text.trim().is_empty()
            || position.x >= rect.right() + 1.0
            || position.y < rect.top() - 0.5
            || position.y > rect.bottom() + font_size
        {
            continue;
        }
        let width = measurer.measure_run_weighted_in(
            text,
            *font_size,
            igui_core::FontWeight::NORMAL,
            family.as_deref(),
        );
        runs.push((position.y, position.x, position.x + width, text.clone()));
    }
    if runs.is_empty() {
        return Err("the editor text area painted no text to check".into());
    }
    for (index, a) in runs.iter().enumerate() {
        for b in &runs[index + 1..] {
            let same_baseline = (a.0 - b.0).abs() < 0.01;
            if same_baseline && a.1 < b.2 - 0.5 && b.1 < a.2 - 0.5 {
                return Err(format!(
                    "editor text painted twice on one baseline: {:?} at x={:.1} overlaps {:?} at x={:.1}",
                    a.3, a.1, b.3, b.1
                ));
            }
        }
    }
    Ok(())
}

fn check(dir: &Path) -> Result<(), String> {
    let config = dir.join("session.conf");
    // Data-only language pack loaded at startup.
    fs::create_dir_all(dir.join("languages")).map_err(|error| format!("fixture: {error}"))?;
    fs::write(
        dir.join("languages").join("lua.json"),
        r#"{
            "name": "lua",
            "extensions": ["lua"],
            "line_comment": ["--"],
            "block_comment": ["--[[", "]]"],
            "keywords": ["local", "function", "end", "return"],
            "constants": ["nil", "true", "false"],
            "strings": [{"quote": "\"", "escapes": true}]
        }"#,
    )
    .map_err(|error| format!("fixture: {error}"))?;
    let lua_text = "local function hello()\n    return nil -- note\nend\n";
    fs::write(dir.join("replace.txt"), "hello alpha\nhello beta\n")
        .map_err(|error| format!("fixture: {error}"))?;
    fs::write(dir.join("sample.lua"), lua_text).map_err(|error| format!("fixture: {error}"))?;
    let canonical_lua = canonical_target(dir.join("sample.lua"));
    let main_path = dir.join("main.rs");
    let notes_path = dir.join("notes.md");
    let out_path = dir.join("out.txt");
    let canonical_out = canonical_target(out_path.clone());
    let canonical_notes = canonical_target(notes_path.clone());

    let mut harness = Harness::new(
        Some(main_path.clone()),
        config.clone(),
        ScriptedDialogs::new(vec![]),
    );

    // -- plugin seam: a provider can be registered at runtime -------------
    {
        struct NoopProvider;
        impl crate::syntax::SyntaxProvider for NoopProvider {
            fn name(&self) -> &'static str {
                "selfcheck-noop"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &["noop"]
            }
            fn highlight_line(
                &self,
                _line: &str,
                state: u32,
            ) -> (Vec<crate::syntax::Highlight>, u32) {
                (Vec::new(), state)
            }
        }
        harness
            .app
            .register_syntax(std::sync::Arc::new(NoopProvider));
        if harness
            .app
            .state()
            .syntax
            .by_name("selfcheck-noop")
            .is_none()
        {
            return Err("register_syntax did not add the provider".into());
        }
    }

    // -- startup ---------------------------------------------------------
    let frame = harness.frame()?;
    if harness.app.state().syntax.by_name("lua").is_none() {
        return Err("the declarative lua language pack was not registered".into());
    }
    let painted = texts(&frame);
    // Highlighted lines are painted per token, so a token — not the whole
    // line — is the painted unit.
    expect(&painted, "main", "the opened file's first line")?;
    expect(&painted, "println!", "the opened file's second line")?;
    check_editor_text_painted_once(&harness, &frame)?;
    if cfg!(target_os = "macos") {
        if harness.app.state().header_close.width() == 0.0 {
            return Err("custom macOS traffic lights were not painted".into());
        }
        let (close, minimize, zoom, drag) = {
            let state = harness.app.state();
            (
                state.header_close.center(),
                state.header_minimize.center(),
                state.header_zoom.center(),
                Vec2::new(
                    state.header_zoom.right() + 30.0,
                    state.theme.m.menubar_h * 0.5,
                ),
            )
        };
        let state = harness.app.state();
        for (position, expected) in [
            (close, crate::ui::menu::HeaderHit::Close),
            (minimize, crate::ui::menu::HeaderHit::Minimize),
            (zoom, crate::ui::menu::HeaderHit::Zoom),
            (drag, crate::ui::menu::HeaderHit::Drag),
        ] {
            if crate::ui::menu::header_hit(&state, position) != Some(expected) {
                return Err(format!(
                    "custom header misclassified {position:?} (expected {expected:?})"
                ));
            }
        }
        drop(state);
        // Hovering a traffic light darkens it and reveals the glyph group.
        let base_color =
            circle_color(&frame, close).ok_or("the red traffic light did not paint a circle")?;
        harness.event(InputEvent::PointerMove { position: close });
        if harness.app.state().header_hover != Some(HeaderButton::Close) {
            return Err("moving over the red light did not set its hover state".into());
        }
        let hovered = harness.frame()?;
        let hover_color = circle_color(&hovered, close)
            .ok_or("the hovered red traffic light did not paint a circle")?;
        if hover_color == base_color {
            return Err("the red traffic light did not change color on hover".into());
        }
        if !hovered.commands().iter().any(|command| {
            matches!(
                command,
                DrawCommand::Line { from, to, .. }
                    if close.x - 4.0 <= from.x
                        && from.x <= close.x + 4.0
                        && close.y - 4.0 <= from.y
                        && from.y <= close.y + 4.0
                        && close.x - 4.0 <= to.x
                        && to.x <= close.x + 4.0
                        && close.y - 4.0 <= to.y
                        && to.y <= close.y + 4.0
            )
        }) {
            return Err("the hovered traffic light did not reveal its glyph".into());
        }
        harness.event(InputEvent::PointerLeave);
        if harness.app.state().header_hover.is_some() {
            return Err("leaving the window did not clear the traffic-light hover".into());
        }
        // A header double-click is ours (zoom/maximize), never an editor word
        // selection that happens to sit under the pointer.
        if !harness
            .app
            .event(&InputEvent::DoubleClick { position: drag })
            .is_handled()
        {
            return Err("the custom header did not consume a double-click".into());
        }
    } else {
        expect(&painted, "File", "menu bar labels")?;
    }
    expect(&painted, "UTF-8", "status bar facts")?;
    expect(&painted, "main.rs", "active tab / status file")?;
    expect(&painted, "EXPLORER", "sidebar header")?;
    expect(&painted, "notes.md", "sidebar rows")?;
    check_header_panels(&mut harness, &frame)?;
    check_footer_controls(&mut harness, &frame)?;
    if !frame
        .commands()
        .iter()
        .any(|command| matches!(command, DrawCommand::ClipRect { .. }))
    {
        return Err("expected the editor to be clipped".into());
    }
    inspect_frame(&harness.app, &frame)?;
    {
        let state = harness.app.state();
        if state.built_view != ViewMode::Edit {
            return Err(format!("expected Edit view, got {:?}", state.built_view));
        }
        if !state.title.contains("main.rs") {
            return Err(format!("window title missing the file: {}", state.title));
        }
        let (_, _, close) = state
            .tabs
            .hits
            .first()
            .ok_or("the active tab has no hit rect")?;
        if close.width() < 24.0 || close.height() < 24.0 {
            return Err(format!(
                "tab close target too small: {}×{}",
                close.width(),
                close.height()
            ));
        }
    }
    // Space is a named key: the host does not send it as text, so the editor
    // must insert it itself.
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }
    harness.key(Key::Space);
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no document after space")?;
        if !doc.text().starts_with(' ') {
            return Err("Space did not insert a space into the editor".into());
        }
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.undo();
        doc.set_caret(0);
    }

    // -- file-tree context menu ------------------------------------------
    let tree_pos = sidebar_click_position(&harness.app, "main.rs")?;
    harness.right_click(tree_pos);
    harness.frame()?;
    if harness.app.state().tree_menu.is_none() {
        return Err("right-clicking a file-tree row did not open its menu".into());
    }
    let copy_item = {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        hits.iter()
            .find(|(action, _, _)| *action == crate::state::TreeMenuAction::CopyPath)
            .map(|(_, rect, _)| rect.center())
            .ok_or("tree context menu has no Copy Path item")?
    };
    harness.click(copy_item);
    harness.frame()?;
    if harness.app.state().tree_menu.is_some() {
        return Err("tree context menu did not close after Copy Path".into());
    }
    harness.right_click(tree_pos);
    harness.frame()?;
    {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        if hits.iter().any(|(action, _, _)| {
            matches!(
                action,
                crate::state::TreeMenuAction::NewFile | crate::state::TreeMenuAction::NewFolder
            )
        }) {
            return Err("a file row must not offer child-create actions".into());
        }
        if !hits
            .iter()
            .any(|(action, _, _)| *action == crate::state::TreeMenuAction::Delete)
        {
            return Err("a file row must offer Delete".into());
        }
    }

    // Folder rows add New File / New Folder; blank Explorer space creates at
    // the project root and has no Delete/Open row actions.
    let folder_pos = sidebar_click_position(&harness.app, "src")?;
    harness.right_click(folder_pos);
    harness.frame()?;
    let new_folder_item = {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        if !hits.iter().any(|(action, _, _)| {
            matches!(
                action,
                crate::state::TreeMenuAction::NewFile | crate::state::TreeMenuAction::NewFolder
            )
        }) {
            return Err("a folder row must offer New File / New Folder".into());
        }
        hits.iter()
            .find(|(action, _, _)| *action == crate::state::TreeMenuAction::NewFolder)
            .map(|(_, rect, _)| rect.center())
            .ok_or("folder context menu has no New Folder item")?
    };
    harness.click(new_folder_item);
    harness.frame()?;
    if !harness.app.state().new_file.open
        || harness.app.state().new_file.kind != crate::state::NewFileKind::Folder
    {
        return Err("New Folder did not open the folder-create prompt".into());
    }
    harness.type_text("new-dir");
    harness.key(Key::Enter);
    harness.frame()?;
    if !dir.join("src").join("new-dir").is_dir() {
        return Err("New Folder did not create the directory".into());
    }

    // Rename from the folder context menu, then Enter on the focused Explorer.
    let created_dir_pos = sidebar_click_position(&harness.app, "new-dir")?;
    harness.right_click(created_dir_pos);
    harness.frame()?;
    let rename_item = {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        hits.iter()
            .find(|(action, _, _)| *action == crate::state::TreeMenuAction::Rename)
            .map(|(_, rect, _)| rect.center())
            .ok_or("folder context menu has no Rename item")?
    };
    harness.click(rename_item);
    harness.frame()?;
    if !harness.app.state().new_file.open
        || harness.app.state().new_file.kind != crate::state::NewFileKind::Rename
    {
        return Err("Rename did not open the rename prompt".into());
    }
    harness.type_text("renamed-dir");
    harness.key(Key::Enter);
    harness.frame()?;
    let renamed_dir = dir.join("src").join("renamed-dir");
    if dir.join("src").join("new-dir").exists() || !renamed_dir.is_dir() {
        return Err("Rename did not move the folder".into());
    }
    harness.key(Key::Enter);
    harness.frame()?;
    if harness.app.state().new_file.kind != crate::state::NewFileKind::Rename {
        return Err("Enter in the focused Explorer did not start a rename".into());
    }
    harness.type_text("renamed-again");
    harness.key(Key::Enter);
    harness.frame()?;
    if renamed_dir.exists() || !dir.join("src").join("renamed-again").is_dir() {
        return Err("the Enter rename did not move the selected folder".into());
    }

    let blank = {
        let state = harness.app.state();
        Vec2::new(
            state.sidebar.rect.left() + 60.0,
            state.sidebar.rect.bottom() - 20.0,
        )
    };
    harness.right_click(blank);
    harness.frame()?;
    let new_blank_file = {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        if hits.iter().any(|(action, _, _)| {
            matches!(
                action,
                crate::state::TreeMenuAction::Open | crate::state::TreeMenuAction::Delete
            )
        }) {
            return Err("the blank Explorer menu must not offer Open/Delete".into());
        }
        hits.iter()
            .find(|(action, _, _)| *action == crate::state::TreeMenuAction::NewFile)
            .map(|(_, rect, _)| rect.center())
            .ok_or("blank Explorer menu has no New File item")?
    };
    harness.click(new_blank_file);
    harness.frame()?;
    harness.type_text("blank-created.txt");
    harness.key(Key::Enter);
    harness.frame()?;
    let blank_path = canonical_target(dir.join("blank-created.txt"));
    if !blank_path.is_file() {
        return Err("New File from blank Explorer space did not create the file".into());
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .and_then(|doc| doc.path())
        != Some(blank_path.as_path())
    {
        return Err("the blank-area New File was not opened".into());
    }

    let blank_pos = sidebar_click_position(&harness.app, "blank-created.txt")?;
    harness.click(blank_pos);
    harness.frame()?;
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness.app.state().new_file.open
        || harness.app.state().new_file.kind != crate::state::NewFileKind::Rename
    {
        return Err("Enter after selecting a file did not start a rename".into());
    }
    harness.type_text("blank-renamed.txt");
    harness.key(Key::Enter);
    harness.frame()?;
    let blank_path = canonical_target(dir.join("blank-renamed.txt"));
    if !blank_path.is_file() || dir.join("blank-created.txt").exists() {
        return Err("the quick rename did not move the file".into());
    }

    let blank_pos = sidebar_click_position(&harness.app, "blank-renamed.txt")?;
    harness.right_click(blank_pos);
    harness.frame()?;
    let delete_item = {
        let state = harness.app.state();
        let (_, hits) = crate::ui::tree_menu::layout(&state);
        hits.iter()
            .find(|(action, _, _)| *action == crate::state::TreeMenuAction::Delete)
            .map(|(_, rect, _)| rect.center())
            .ok_or("file context menu has no Delete item")?
    };
    harness.click(delete_item);
    harness.frame()?;
    if !matches!(
        harness.app.state().modal,
        Some(crate::state::Modal::DeletePath { is_dir: false })
    ) {
        return Err("Delete did not open the delete confirmation".into());
    }
    harness
        .app
        .state_mut()
        .resolve_modal(crate::state::ModalButton::Discard);
    harness.frame()?;
    if blank_path.exists() {
        return Err("confirming Delete did not remove the file".into());
    }

    // -- double-click selects the word under the pointer, without snapping
    // the selection back to the clicked caret.
    let main_at = draw_position(&frame, "main").ok_or("main token was not painted")?;
    let main_click = Vec2::new(main_at.x + 2.0, main_at.y - 4.0);
    harness.click(main_click);
    harness.double_click(main_click);
    harness.event(InputEvent::PointerMove {
        position: Vec2::new(main_click.x + 120.0, main_click.y),
    });
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document after double-click")?;
        if doc.selected_text() != Some("main") {
            return Err(format!(
                "double-click selected {:?} at caret {:?}, expected main",
                doc.selected_text(),
                doc.line_col()
            ));
        }
    }
    harness.double_click(main_click);
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document after triple-click")?;
        if doc.selected_text() != Some("fn main() {\n") {
            return Err(format!(
                "triple-click selected {:?}, expected the whole line",
                doc.selected_text()
            ));
        }
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }

    // VS Code / JetBrains empty-selection cut: the current line is removed.
    {
        let mut state = harness.app.state_mut();
        let doc = state
            .ws
            .active_doc_mut()
            .ok_or("no document for line cut")?;
        doc.set_caret(doc.byte_at(3, 2));
    }
    harness.shortcut(
        Key::Character('x'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no document after line cut")?;
        if doc.text().contains("println!") {
            return Err("⌘X with an empty selection did not cut the current line".into());
        }
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.undo();
        doc.set_caret(0);
    }

    // The close target reaches the tab's top edge, not just the painted glyph.
    let tabs_before = harness.app.state().ws.tabs.len();
    harness.shortcut(
        Key::Character('n'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if harness.app.state().ws.tabs.len() != tabs_before + 1 {
        return Err("⌘N did not add a tab".into());
    }
    {
        let state = harness.app.state();
        let active = state.ws.active;
        let (_, tab, close) = state
            .tabs
            .hits
            .iter()
            .find(|(index, _, _)| *index == active)
            .ok_or("the new tab has no hit rect")?;
        let edge = Vec2::new(close.left() + 2.0, tab.top() + 2.0);
        if !close.contains(edge) {
            return Err("the close target does not start at the tab's top edge".into());
        }
        drop(state);
        harness.click(edge);
    }
    harness.frame()?;
    if harness.app.state().ws.tabs.len() != tabs_before {
        return Err("clicking the tab's top-right corner did not close it".into());
    }

    // -- tab drag reorder + context menu --------------------------------
    for _ in 0..2 {
        harness.shortcut(
            Key::Character('n'),
            Modifiers {
                meta: true,
                ..Modifiers::NONE
            },
        );
    }
    harness.frame()?;
    let dragged_id = harness
        .app
        .state()
        .ws
        .active_doc()
        .map(|doc| doc.id())
        .ok_or("no active tab for the drag test")?;
    let (from, to) = {
        let state = harness.app.state();
        let active = state.ws.active;
        let Some((_, tab, _)) = state
            .tabs
            .hits
            .iter()
            .find(|(index, _, _)| *index == active)
        else {
            return Err("the dragged tab has no rect".into());
        };
        let Some((_, first, _)) = state.tabs.hits.first() else {
            return Err("no first tab rect".into());
        };
        (
            tab.center(),
            Vec2::new(first.left() + 2.0, first.center().y),
        )
    };
    harness.drag(from, to);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.active != 0 || state.ws.tabs.first().map(|doc| doc.id()) != Some(dragged_id) {
            return Err(format!(
                "dragging a tab to the front did not reorder it (active={} first={:?})",
                state.ws.active,
                state.ws.tabs.first().map(|doc| doc.id())
            ));
        }
    }
    // Right-click opens the tab menu; Esc dismisses it.
    let tab_center = harness
        .app
        .state()
        .tabs
        .hits
        .first()
        .map(|(_, tab, _)| tab.center())
        .ok_or("no tab rect for the context menu")?;
    harness.right_click(tab_center);
    if harness.app.state().tab_menu.is_none() {
        return Err("right-clicking a tab did not open its context menu".into());
    }
    harness.frame()?;
    harness.key(Key::Escape);
    if harness.app.state().tab_menu.is_some() {
        return Err("Esc did not close the tab context menu".into());
    }
    // Close the two dragged/new untitled tabs through the menu (never main.rs).
    for _ in 0..2 {
        let tab_center = {
            let state = harness.app.state();
            let index = state
                .ws
                .tabs
                .iter()
                .position(|doc| doc.name().starts_with("untitled"))
                .ok_or("no untitled tab left to close")?;
            state
                .tabs
                .hits
                .iter()
                .find(|(hit, _, _)| *hit == index)
                .map(|(_, tab, _)| tab.center())
                .ok_or("the untitled tab has no rect")?
        };
        harness.right_click(tab_center);
        harness.frame()?;
        let close_item = harness
            .app
            .state()
            .tab_menu
            .as_ref()
            .and_then(|menu| {
                menu.hits
                    .iter()
                    .find(|(action, _)| *action == crate::state::TabMenuAction::Close)
                    .map(|(_, rect)| rect.center())
            })
            .ok_or("the tab context menu has no Close item")?;
        harness.click(close_item);
        harness.frame()?;
    }
    if harness.app.state().ws.tabs.len() != tabs_before {
        return Err("the tab context menu did not close the untitled tabs".into());
    }
    // Reveal the remaining file through the context menu.
    let main_index = harness
        .app
        .state()
        .ws
        .tabs
        .iter()
        .position(|doc| doc.name() == "main.rs")
        .ok_or("main.rs is not open after the tab menu test")?;
    let main_center = harness
        .app
        .state()
        .tabs
        .hits
        .iter()
        .find(|(index, _, _)| *index == main_index)
        .map(|(_, tab, _)| tab.center())
        .ok_or("main.rs tab has no rect")?;
    harness.right_click(main_center);
    harness.frame()?;
    let reveal_item = harness
        .app
        .state()
        .tab_menu
        .as_ref()
        .and_then(|menu| {
            menu.hits
                .iter()
                .find(|(action, _)| *action == crate::state::TabMenuAction::RevealInSidebar)
                .map(|(_, rect)| rect.center())
        })
        .ok_or("the tab context menu has no Reveal item")?;
    harness.click(reveal_item);
    harness.frame()?;
    if harness
        .app
        .state()
        .sidebar
        .selected
        .as_deref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        != Some("main.rs")
    {
        return Err("Reveal in Sidebar did not select the tab's file".into());
    }

    // -- quick create file ----------------------------------------------
    harness.shortcut(
        Key::Character('n'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if !harness.app.state().new_file.open {
        return Err("⇧⌘N did not open the New File prompt".into());
    }
    harness.type_text("made.txt");
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.new_file.open {
            return Err(format!(
                "creating a file left the prompt open: {:?}",
                state.new_file.error
            ));
        }
        let path = canonical_target(main_path.with_file_name("made.txt"));
        if !path.is_file() {
            return Err("the New File prompt did not create the file".into());
        }
        if state.ws.active_doc().and_then(|doc| doc.path()) != Some(path.as_path()) {
            return Err("the created file was not opened".into());
        }
    }
    // A second create with the same name is an inline error, not an overwrite.
    harness.shortcut(
        Key::Character('n'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    harness.type_text("made.txt");
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness.app.state().new_file.open || harness.app.state().new_file.error.is_none() {
        return Err("creating an existing file must stay in the prompt with an error".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;
    let created_tab = harness.app.state().ws.active;
    harness.app.state_mut().close_tab_now(created_tab);
    harness.frame()?;

    // Native macOS queue: Finder/Dock open + New File commands.
    let native_path = dir.join("native-open.txt");
    fs::write(&native_path, "native open\n").map_err(|error| format!("fixture: {error}"))?;
    crate::native::push(crate::native::NativeRequest::Open(
        vec![native_path.clone()],
    ));
    harness.frame()?;
    {
        let state = harness.app.state();
        if state
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            != Some("native-open.txt")
        {
            return Err("the native Open request did not open the file".into());
        }
    }
    let native_tab = harness.app.state().ws.active;
    harness.app.state_mut().close_tab_now(native_tab);
    crate::native::push(crate::native::NativeRequest::NewFile);
    harness.frame()?;
    if !harness.app.state().new_file.open {
        return Err("the native New File request did not open the prompt".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;

    // -- typing, save, undo/redo ----------------------------------------
    harness.type_text("// typed by selfcheck\n");
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "// typed by selfcheck",
        "typed text is painted",
    )?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if !doc.text().starts_with("// typed by selfcheck") {
            return Err("typed text was not inserted into the model".into());
        }
        if !doc.is_dirty() {
            return Err("typing must mark the document dirty".into());
        }
        if !state.title.contains('•') {
            return Err("the title must mark unsaved changes".into());
        }
    }
    harness.shortcut(
        Key::Character('s'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    let measurer = igui_ui::text_measurer(harness.app.tree());
    check_status_overlap(&frame, measurer.as_ref())?;
    let saved = fs::read_to_string(&main_path).map_err(|error| format!("read back: {error}"))?;
    if !saved.starts_with("// typed by selfcheck") {
        return Err("⌘S did not write the buffer to disk".into());
    }
    {
        let state = harness.app.state();
        if state.ws.active_doc().is_some_and(|doc| doc.is_dirty()) {
            return Err("saving must clear the dirty flag".into());
        }
        if state.title.contains('•') {
            return Err("the title still shows unsaved changes after ⌘S".into());
        }
    }
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.text().starts_with("// typed by selfcheck"))
    {
        return Err("⌘Z did not undo the typing".into());
    }
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    if !harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.text().starts_with("// typed by selfcheck"))
    {
        return Err("⇧⌘Z did not redo the typing".into());
    }

    // -- File -> Save ---------------------------------------------------
    harness.type_text(" ");
    harness.frame()?;
    if cfg!(target_os = "macos") {
        // The macOS File menu is native; exercise the same command directly.
        harness.shortcut(
            Key::Character('s'),
            Modifiers {
                meta: true,
                ..Modifiers::NONE
            },
        );
    } else {
        let file_item = harness
            .app
            .state()
            .menubar_hits
            .iter()
            .find(|(menu, _)| *menu == Menu::File)
            .map(|(_, rect)| rect.center())
            .ok_or("no File menu hit rect")?;
        harness.click(file_item);
        if harness.app.state().menu_open != Some(Menu::File) {
            return Err("clicking File did not open its menu".into());
        }
        harness.frame()?;
        let measurer = igui_ui::text_measurer(harness.app.tree());
        let save_item = {
            let state = harness.app.state();
            let (_, items) = crate::ui::menu::popup_layout(&state, measurer.as_ref());
            items
                .iter()
                .find(|(action, _)| *action == Some(Action::Save))
                .map(|(_, rect)| rect.center())
                .ok_or("Save item has no rect")?
        };
        harness.click(save_item);
    }
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.is_dirty())
    {
        return Err("File > Save did not save the document".into());
    }

    // -- find / replace --------------------------------------------------
    harness.shortcut(
        Key::Character('f'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.find_layout_ready();
    {
        let state = harness.app.state();
        if !state.find.open || state.focus != Focus::Find {
            return Err("⌘F did not open the find bar with the query focused".into());
        }
    }
    // The host also emits committed text for ⌘F ("f") before the modifiers are
    // released; that must not land in the freshly focused query field.
    harness.event(InputEvent::ModifiersChanged(Modifiers {
        meta: true,
        ..Modifiers::NONE
    }));
    harness.type_text("f");
    harness.event(InputEvent::ModifiersChanged(Modifiers::NONE));
    if !harness.app.state().find.query.text().is_empty() {
        return Err(format!(
            "⌘F leaked the accelerator key into the query: {:?}",
            harness.app.state().find.query.text()
        ));
    }
    harness.type_text("typed");
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.find.matches.len() != 1 {
            return Err(format!(
                "expected 1 match for `typed`, got {}",
                state.find.matches.len()
            ));
        }
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.selected_text() != Some("typed") {
            return Err("the current match must be selected in the document".into());
        }
    }
    expect(&texts(&frame), "1 / 1", "match counter")?;

    // Click the replace field, type a replacement, run Replace All.
    let replace_field = harness.find_hit(crate::state::FindHit::ReplaceField);
    harness.click(replace_field);
    harness.type_text("MARKED");
    harness.find_layout_ready();
    let replace_all = harness.find_hit(crate::state::FindHit::ReplaceAll);
    harness.click(replace_all);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.text().contains("typed by selfcheck") || !doc.text().contains("MARKED by selfcheck")
        {
            return Err(format!(
                "Replace All did not rewrite the document (query={:?} replace={:?} matches={} hits={:?} text={:?})",
                state.find.query.text(),
                state.find.replace.text(),
                state.find.matches.len(),
                state.find.hits,
                doc.text()
            ));
        }
        if !state.find.matches.is_empty() {
            return Err("the old query should have no matches after Replace All".into());
        }
    }
    expect(&texts(&frame), "no matches", "empty match counter")?;
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().find.open {
        return Err("Esc did not close the find bar".into());
    }
    let before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len());
    harness.type_text("x");
    let after = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len());
    if after != before + 1 {
        return Err("typing after closing find must reach the editor".into());
    }
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );

    // -- Save As through a scripted dialog -------------------------------
    harness.dialogs.push(Some(out_path.clone()));
    harness.shortcut(
        Key::Character('s'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if !harness.dialogs.calls().contains(&Call::SaveFile) {
        return Err("⇧⌘S did not ask the dialog layer for a path".into());
    }
    if !out_path.is_file() {
        return Err("Save As did not write the new file".into());
    }
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.path() != Some(canonical_out.as_path()) {
            return Err(format!(
                "Save As did not retarget the document (path={:?})",
                doc.path()
            ));
        }
        if state.session.recent_files.first() != Some(&canonical_out) {
            return Err("Save As did not update the recent list".into());
        }
    }

    // -- Open a Markdown file through a dialog -> split preview ----------
    harness.dialogs.push(Some(notes_path.clone()));
    harness.shortcut(
        Key::Character('o'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    if !harness.dialogs.calls().contains(&Call::OpenFile) {
        return Err("⌘O did not ask the dialog layer for a file".into());
    }
    {
        let state = harness.app.state();
        if state.built_view != ViewMode::Split {
            return Err(format!(
                "Markdown must open in split preview, got {:?}",
                state.built_view
            ));
        }
    }
    let painted = texts(&frame);
    expect(&painted, "## Notes", "editor shows the raw Markdown")?;
    expect(&painted, "Notes", "preview renders the heading")?;
    expect(&painted, "square corners", "preview renders list items")?;
    expect(&painted, "RUST", "preview labels the fenced code block")?;
    let (editor_right, preview_left) = {
        let state = harness.app.state();
        (state.editor.text_rect.right(), state.preview.rect.left())
    };
    if preview_left < editor_right {
        return Err("split preview must sit right of the editor".into());
    }
    if harness.app.state().preview.rect.width() < 300.0 {
        return Err("split preview is implausibly narrow".into());
    }
    // The editor also paints the raw Markdown source, so table assertions must
    // only look at runs inside the preview pane.
    let in_preview = |needle: &str| -> Option<Vec2> {
        frame.commands().iter().find_map(|command| match command {
            DrawCommand::DrawText { text, position, .. }
                if text == needle && position.x >= preview_left =>
            {
                Some(*position)
            }
            _ => None,
        })
    };
    // Table: header cells share a baseline, columns order left → right, the
    // body row sits below the header, and the right column is right-aligned.
    let key = in_preview("Key").ok_or("table header cell `Key` is missing")?;
    let meaning = in_preview("Meaning").ok_or("table header cell `Meaning` is missing")?;
    let alpha = in_preview("alpha").ok_or("table cell `alpha` is missing")?;
    let beta = in_preview("beta").ok_or("table cell `beta` is missing")?;
    if (key.y - meaning.y).abs() > 0.5 {
        return Err(format!(
            "table header cells must share a baseline (Key y={}, Meaning y={})",
            key.y, meaning.y
        ));
    }
    if key.x >= meaning.x {
        return Err("table columns are not laid out left to right".into());
    }
    if alpha.y <= key.y {
        return Err("the table body must render below the header".into());
    }
    if beta.x <= alpha.x {
        return Err("the right-aligned column must start right of the left column".into());
    }
    inspect_frame(&harness.app, &frame)?;

    // Preview scroll direction: wheel down (positive y) advances the content.
    let preview_center = harness.app.state().preview.rect.center();
    harness.event(InputEvent::Wheel {
        position: preview_center,
        delta: Vec2::new(0.0, 40.0),
        phase: igui_core::TouchPhase::Moved,
        device: igui_core::ScrollDevice::Wheel,
    });
    harness.frame()?;
    let scrolled = harness.app.state().preview.scroll;
    if scrolled <= 0.0 {
        return Err(format!(
            "wheel down must increase the preview scroll (got {scrolled})"
        ));
    }
    harness.event(InputEvent::Wheel {
        position: preview_center,
        delta: Vec2::new(0.0, -40.0),
        phase: igui_core::TouchPhase::Moved,
        device: igui_core::ScrollDevice::Wheel,
    });
    harness.frame()?;
    if harness.app.state().preview.scroll >= scrolled {
        return Err("wheel up must decrease the preview scroll".into());
    }

    // -- view cycling ----------------------------------------------------
    harness.shortcut(
        Key::Character('m'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.built_view != ViewMode::Preview {
            return Err("⇧⌘M did not switch to preview-only".into());
        }
    }
    expect(
        &texts(&frame),
        "square corners",
        "preview-only still renders",
    )?;
    if harness.app.caret().is_some() {
        return Err("preview-only mode must not expose the editor caret".into());
    }
    harness.shortcut(
        Key::Character('m'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.built_view != ViewMode::Edit {
            return Err("⇧⌘M did not return to the editor".into());
        }
    }
    expect(&texts(&frame), "## Notes", "editor is back")?;

    // -- dirty close: modal, cancel, discard ----------------------------
    harness.type_text("!");
    harness.frame()?;
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    let painted = texts(&frame);
    expect(&painted, "Unsaved changes", "modal title")?;
    expect(&painted, "Discard", "modal discard button")?;
    if harness.app.state().modal.is_none() {
        return Err("⌘W on a dirty tab did not open the confirmation modal".into());
    }
    let tabs_before = harness.app.state().ws.tabs.len();
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().modal.is_some() || harness.app.state().ws.tabs.len() != tabs_before {
        return Err("Esc must cancel the modal and keep the tab".into());
    }
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let discard = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Discard)
            .map(|(_, rect)| rect.center())
            .ok_or("no Discard button")?
    };
    harness.click(discard);
    harness.frame()?;
    if harness.app.state().modal.is_some() {
        return Err("Discard must close the modal".into());
    }
    if harness.app.state().ws.tabs.len() != tabs_before - 1 {
        return Err("Discard must close the tab".into());
    }

    // -- quit guard: dirty quit asks, Discard & Quit requests termination --
    harness.app.state_mut().request_quit(1);
    let frame = harness.frame()?;
    expect(&texts(&frame), "Unsaved changes", "quit modal title")?;
    let quit = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Discard)
            .map(|(_, rect)| rect.center())
            .ok_or("no Discard & Quit button")?
    };
    harness.click(quit);
    {
        let mut state = harness.app.state_mut();
        if state.modal.is_some() {
            return Err("Discard & Quit must close the modal".into());
        }
        if !state.quit_requested {
            return Err("Discard & Quit must ask the shell to terminate".into());
        }
        // A headless run must survive the check: consume the request here.
        state.quit_requested = false;
    }

    // -- dirty close: Save writes and closes ----------------------------
    harness.type_text("SAVED");
    harness.frame()?;
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let save = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Save)
            .map(|(_, rect)| rect.center())
            .ok_or("no Save button")?
    };
    harness.click(save);
    harness.frame()?;
    let saved = fs::read_to_string(&out_path).map_err(|error| format!("read back: {error}"))?;
    if !saved.contains("SAVED") {
        return Err("the modal Save path did not write the file".into());
    }
    {
        let state = harness.app.state();
        if state
            .ws
            .tabs
            .iter()
            .any(|doc| doc.path() == Some(canonical_out.as_path()))
        {
            return Err("the modal Save path did not close the tab".into());
        }
    }

    // -- sidebar opens a file and returns keyboard focus ----------------
    let notes_row = {
        let state = harness.app.state();
        let rows = state.ws.tree.rows();
        let index = rows
            .iter()
            .position(|row| row.name == "notes.md")
            .ok_or("notes.md is not in the sidebar")?;
        let m = state.theme.m;
        Vec2::new(
            state.sidebar.rect.left() + 60.0,
            state.sidebar.rect.top() + index as f32 * m.row_h + m.row_h * 0.5,
        )
    };
    harness.click(notes_row);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.active_doc().map(|doc| doc.name()) != Some("notes.md") {
            return Err("sidebar click did not open notes.md".into());
        }
        if state.built_view != ViewMode::Split {
            return Err("sidebar opened Markdown without the split layout".into());
        }
    }
    let before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len());
    harness.type_text("~");
    let after = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len());
    if after != before + 1 {
        return Err("clicking the sidebar must return focus to the editor".into());
    }
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );

    // -- drag selection and wheel ---------------------------------------
    let (start, end) = {
        let state = harness.app.state();
        let rect = state.editor.text_rect;
        let m = state.theme.m;
        let line_h = state.editor_metrics().line_h;
        (
            Vec2::new(rect.left() + m.text_pad + 2.0, rect.top() + line_h * 0.5),
            Vec2::new(rect.left() + m.text_pad + 120.0, rect.top() + line_h * 1.5),
        )
    };
    harness.event(InputEvent::PointerDown {
        position: start,
        button: PointerButton::Left,
    });
    harness.event(InputEvent::PointerMove { position: end });
    harness.event(InputEvent::PointerUp {
        position: end,
        button: PointerButton::Left,
    });
    if !harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.has_selection())
    {
        return Err("drag selection did not extend the selection".into());
    }
    harness.event(InputEvent::Wheel {
        position: end,
        delta: Vec2::new(0.0, 40.0),
        phase: igui_core::TouchPhase::Moved,
        device: igui_core::ScrollDevice::Wheel,
    });
    harness.frame()?;
    if harness.app.state().ws.active_doc().is_none() {
        return Err("wheel scrolling lost the document".into());
    }

    // -- modal Save on an untitled tab goes through Save As ------------
    harness.dialogs.push(Some(dir.join("scratch.txt")));
    harness.shortcut(
        Key::Character('n'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    harness.type_text("scratch buffer");
    harness.frame()?;
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let save = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Save)
            .map(|(_, rect)| rect.center())
            .ok_or("no Save button for the untitled tab")?
    };
    harness.click(save);
    harness.frame()?;
    let scratch = dir.join("scratch.txt");
    let saved = fs::read_to_string(&scratch).map_err(|error| format!("read back: {error}"))?;
    if !saved.contains("scratch buffer") {
        return Err("modal Save on an untitled tab did not write through Save As".into());
    }
    if harness.app.state().ws.tabs.iter().any(|doc| {
        doc.path()
            .is_some_and(|path| path.file_name() == Some("scratch.txt".as_ref()))
    }) {
        return Err("modal Save should close the untitled tab after saving".into());
    }

    // ================= M3: syntax, guides, brackets, settings =========
    harness.dialogs.push(Some(main_path.clone()));
    harness.shortcut(
        Key::Character('o'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.active_doc().map(|doc| doc.name()) != Some("main.rs") {
            return Err("syntax fixture did not open".into());
        }
        if !state.highlight.is_active() {
            return Err("highlighting is inactive for a Rust file".into());
        }
    }
    let theme = crate::theme::Theme::new(Mode::Dark);
    let keyword = theme
        .token_color(crate::syntax::TokenKind::Keyword)
        .to_rgba8();
    let function = theme
        .token_color(crate::syntax::TokenKind::Function)
        .to_rgba8();
    let string_color = theme
        .token_color(crate::syntax::TokenKind::String)
        .to_rgba8();
    let comment = theme
        .token_color(crate::syntax::TokenKind::Comment)
        .to_rgba8();
    if !draw_text_has_color(&frame, "fn", keyword) {
        return Err("keyword `fn` is not painted with the keyword color".into());
    }
    if !draw_text_has_color(&frame, "main", function) {
        return Err("function `main` is not painted with the function color".into());
    }
    if !draw_text_has_color(&frame, "\"hello\"", string_color) {
        return Err("the string literal is not painted with the string color".into());
    }
    if !draw_text_has_color(&frame, "// header comment", comment) {
        return Err("the comment is not painted with the comment color".into());
    }
    let guides_on = count_guides(&frame, harness.app.state().editor_metrics().line_h);
    if guides_on == 0 {
        return Err("indent guides were not painted for the indented lines".into());
    }

    // Incremental highlighting: type a new comment at the end of the file.
    harness.shortcut(
        Key::ArrowDown,
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.key(Key::Enter);
    harness.type_text("// tail comment");
    let frame = harness.frame()?;
    if !draw_text_has_color(&frame, "// tail comment", comment) {
        return Err("the edited line was not re-highlighted as a comment".into());
    }
    {
        let state = harness.app.state();
        let line = state.ws.active_doc().map_or(0, |doc| doc.line_count() - 1);
        if state.highlight.tokens(line).is_none() {
            return Err("the highlight cache did not catch up after the edit".into());
        }
    }

    // Bracket matching for the caret on `{`.
    {
        let brace = harness
            .app
            .state()
            .ws
            .active_doc()
            .and_then(|doc| doc.text().find('{'))
            .ok_or("no brace in the fixture")?;
        let close = harness
            .app
            .state()
            .ws
            .active_doc()
            .and_then(|doc| doc.text().rfind('}'))
            .ok_or("no closing brace in the fixture")?;
        harness
            .app
            .state_mut()
            .ws
            .active_doc_mut()
            .ok_or("no active document")?
            .set_caret(brace);
        harness.frame()?;
        if harness.app.state().bracket != Some((brace, close)) {
            return Err(format!(
                "bracket matching failed (got {:?})",
                harness.app.state().bracket
            ));
        }
    }

    // Declarative language pack + per-document override.
    harness.app.state_mut().open_path(canonical_lua.clone());
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.language_name() != Some("lua") {
            return Err(format!(
                "the lua pack was not selected by extension (got {:?})",
                state.language_name()
            ));
        }
    }
    expect(&texts(&frame), "LUA", "pack language in the status bar")?;
    let keyword = crate::theme::Theme::new(Mode::Dark)
        .token_color(crate::syntax::TokenKind::Keyword)
        .to_rgba8();
    if !draw_text_has_color(&frame, "local", keyword) {
        return Err("the lua pack did not highlight `local` as a keyword".into());
    }
    let language_chip = {
        let state = harness.app.state();
        if state
            .menubar_hits
            .iter()
            .any(|(menu, _)| *menu == Menu::Language)
        {
            return Err("the language menu is still in the menu bar".into());
        }
        if state.language_hit.width() <= 0.0 || state.language_hit.right() < WIDTH - 24.0 {
            return Err(format!(
                "the language chip is not in the footer's bottom-right: {:?}",
                state.language_hit
            ));
        }
        state.language_hit.center()
    };
    harness.click(language_chip);
    if harness.app.state().menu_open != Some(Menu::Language) {
        return Err("clicking the footer language type did not open the language popup".into());
    }
    let _ = harness.frame()?;
    {
        let state = harness.app.state();
        if state.menu_rect.bottom() > state.language_hit.top() + 0.5
            || state.menu_rect.left() < 0.0
            || state.menu_rect.right() > WIDTH
        {
            return Err(format!(
                "the language popup is not above the footer / inside the window: {:?} vs {:?}",
                state.menu_rect, state.language_hit
            ));
        }
    }
    let measurer = igui_ui::text_measurer(harness.app.tree());
    let rust_item = {
        let state = harness.app.state();
        let (_, items) = crate::ui::menu::popup_layout(&state, measurer.as_ref());
        items
            .iter()
            .find(|(action, _)| {
                matches!(action, Some(Action::SetLanguage(Some(name))) if name == "rust")
            })
            .map(|(_, rect)| rect.center())
            .ok_or("the footer language popup has no rust item")?
    };
    harness.click(rust_item);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.menu_open.is_some() {
            return Err("choosing a language did not close the popup".into());
        }
        if state.language_name() != Some("rust") {
            return Err("the language override to rust did not apply".into());
        }
    }
    expect(
        &texts(&frame),
        "RUST",
        "overridden language in the status bar",
    )?;
    if draw_text_has_color(&frame, "local", keyword) {
        return Err("the rust override kept the lua keyword color".into());
    }
    let language_chip = harness.app.state().language_hit.center();
    harness.click(language_chip);
    let _ = harness.frame();
    let auto_item = {
        let state = harness.app.state();
        let (_, items) = crate::ui::menu::popup_layout(&state, measurer.as_ref());
        items
            .iter()
            .find(|(action, _)| *action == Some(Action::SetLanguage(None)))
            .map(|(_, rect)| rect.center())
            .ok_or("the footer language popup has no Auto item")?
    };
    harness.click(auto_item);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.language_name() != Some("lua") {
            return Err("clearing the override did not return to the lua pack".into());
        }
    }
    expect(
        &texts(&frame),
        "LUA",
        "auto language after clearing the override",
    )?;

    // Settings panel: fonts, tabs, gutter, guides, syntax, sidebar width.
    harness.shortcut(
        Key::Character(','),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    expect(&texts(&frame), "Settings", "settings panel title")?;
    expect(&texts(&frame), "Font Size", "settings panel rows")?;
    expect(&texts(&frame), "13 px", "current font value")?;
    harness.key(Key::ArrowDown); // Theme -> Font Size
    harness.key(Key::ArrowRight);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.settings.font_size != 14.0 || state.session.font != 14.0 {
            return Err("the settings panel did not change the font size".into());
        }
        if state.editor_metrics().font != 14.0 {
            return Err("the editor did not pick up the new font size".into());
        }
    }
    expect(&texts(&frame), "14 px", "updated font value")?;
    // Force an editor code face: the headless measurer is approximate and
    // cannot resolve one on its own. Every narrow editor draw must then name
    // it, just like the terminal does.
    {
        let mut state = harness.app.state_mut();
        state.editor.font = Some("Menlo".into());
        state.editor.font_checked = true;
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        let body = state.editor.text_rect;
        let mut mono = 0;
        for command in frame.commands() {
            let DrawCommand::DrawText {
                family, position, ..
            } = command
            else {
                continue;
            };
            if body.contains(*position) && family.as_deref() == Some("Menlo") {
                mono += 1;
            }
        }
        if mono == 0 {
            return Err("the editor did not draw its code face".into());
        }
    }
    harness.key(Key::ArrowDown); // Font Size -> Indent Style
    harness.key(Key::Enter);
    if !harness.app.state().settings.indent_tabs || !harness.app.state().session.indent_tabs {
        return Err("the settings panel did not switch to tab indentation".into());
    }
    harness.key(Key::Enter);
    harness.key(Key::ArrowDown); // Indent Width
    harness.key(Key::ArrowRight);
    if harness.app.state().settings.tab_width != 8 {
        return Err("the settings panel did not cycle the indent width".into());
    }
    // Back to 4 spaces for the indent-guide check (the fixture indents by 4).
    harness.key(Key::ArrowLeft);
    harness.key(Key::ArrowDown); // Line Numbers
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.settings.line_numbers || state.editor.gutter_w != 0.0 {
            return Err("turning line numbers off did not collapse the gutter".into());
        }
    }
    let frame_before = harness.frame()?;
    harness.key(Key::ArrowDown); // Indent Guides
    harness.key(Key::Enter);
    let frame_after = harness.frame()?;
    let line_h = harness.app.state().editor_metrics().line_h;
    let guides_before = count_guides(&frame_before, line_h);
    let guides_after = count_guides(&frame_after, line_h);
    if guides_before == 0 || guides_after != 0 {
        return Err(format!(
            "the indent-guide toggle did not take effect (before={guides_before} after={guides_after} line_h={line_h} on={})",
            harness.app.state().settings.indent_guides
        ));
    }
    harness.key(Key::ArrowDown); // Auto Pairs
    harness.key(Key::Enter);
    if harness.app.state().settings.auto_pairs || harness.app.state().session.auto_pairs {
        return Err("the auto-pair toggle did not take effect".into());
    }
    harness.key(Key::Enter);
    if !harness.app.state().settings.auto_pairs || !harness.app.state().session.auto_pairs {
        return Err("turning auto pairs back on failed".into());
    }
    harness.key(Key::ArrowDown); // Syntax
    harness.key(Key::Enter);
    harness.frame()?;
    if harness.app.state().highlight.is_active() {
        return Err("turning syntax highlighting off did not deactivate it".into());
    }
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness.app.state().highlight.is_active() {
        return Err("turning syntax highlighting back on failed".into());
    }
    harness.key(Key::ArrowDown); // Autosave
    harness.key(Key::Enter);
    harness.frame()?;
    if harness.app.state().settings.autosave || harness.app.state().session.autosave {
        return Err("the autosave toggle did not take effect".into());
    }
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness.app.state().settings.autosave {
        return Err("turning autosave back on failed".into());
    }
    // Grouped rows + search: typing a label jumps to its row.
    harness.type_text("sidebar");
    let frame = harness.frame()?;
    expect(&texts(&frame), "Sidebar Width", "settings search result")?;
    if texts(&frame).iter().any(|text| text == "Minimap") {
        return Err("the settings search did not filter the rows".into());
    }
    harness.key(Key::Escape); // clears the query, keeps the panel open
    if !harness.app.state().settings_open {
        return Err("Esc closed the panel instead of clearing the search".into());
    }
    harness.key(Key::ArrowRight);
    harness.frame()?;
    {
        let state = harness.app.state();
        if (state.sidebar.rect.width() - 240.0).abs() > 1.5 {
            return Err(format!(
                "sidebar width setting did not resize the sidebar (got {})",
                state.sidebar.rect.width()
            ));
        }
    }
    // The Minimap row and the View menu command share one state and one
    // persistence path.
    harness.type_text("minimap");
    harness.frame()?;
    if !harness.app.state().session.minimap {
        return Err("the minimap is not on by default".into());
    }
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.session.minimap || state.editor.minimap.rect.width() != 0.0 {
            return Err("the Minimap settings row did not hide the strip".into());
        }
    }
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness.app.state().session.minimap {
        return Err("turning the minimap back on from the panel failed".into());
    }
    // Persist an 8-space tab width for the session-restore assertion.
    harness.key(Key::Escape); // clear the minimap query
    harness.type_text("indent width");
    harness.frame()?;
    harness.key(Key::Escape);
    harness.key(Key::ArrowRight);
    harness.frame()?;
    if harness.app.state().settings.tab_width != 8 {
        return Err("the Indent Width row did not cycle to 8".into());
    }
    harness.key(Key::Escape);
    if harness.app.state().settings_open {
        return Err("Esc did not close the settings panel".into());
    }

    // Discoverability: View > Settings… opens the same panel.
    if cfg!(target_os = "macos") {
        harness.app.state_mut().run(Action::Settings);
    } else {
        let view_item = harness
            .app
            .state()
            .menubar_hits
            .iter()
            .find(|(menu, _)| *menu == Menu::View)
            .map(|(_, rect)| rect.center())
            .ok_or("no View menu hit rect")?;
        harness.click(view_item);
        harness.frame()?;
        let measurer = igui_ui::text_measurer(harness.app.tree());
        let settings_item = {
            let state = harness.app.state();
            let (_, items) = crate::ui::menu::popup_layout(&state, measurer.as_ref());
            items
                .iter()
                .find(|(action, _)| *action == Some(Action::Settings))
                .map(|(_, rect)| rect.center())
                .ok_or("View menu has no Settings item")?
        };
        harness.click(settings_item);
    }
    harness.frame()?;
    if !harness.app.state().settings_open {
        return Err("View > Settings… did not open the settings panel".into());
    }
    // The panel is a left group navigation plus one scrolling pane; the group
    // click replaces the pane contents.
    let (nav, panel_height, viewport_height) = {
        let state = harness.app.state();
        let layout = crate::ui::settings::layout(&state);
        let nav = layout
            .nav_items
            .iter()
            .find(|item| item.group == crate::settings::Group::View)
            .map(|item| item.rect.center())
            .ok_or("the settings panel has no View group")?;
        (
            nav,
            layout.panel.height(),
            state.viewport.logical_rect().height(),
        )
    };
    if panel_height > viewport_height - 40.0 {
        return Err(format!(
            "the settings panel is {panel_height} px tall in a {viewport_height} px window"
        ));
    }
    harness.click(nav);
    let frame = harness.frame()?;
    expect(&texts(&frame), "Minimap", "View group after a nav click")?;
    if texts(&frame).iter().any(|text| text == "Font Size") {
        return Err("the settings pane kept another group's rows".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;

    // The View menu owns the explicit LSP hard reset; it must stay reachable
    // (disabled only when the active document has no language server).
    if !crate::state::items(Menu::View, &harness.app.state()).contains(&Some(Action::LspRestart)) {
        return Err("View menu lost the Restart Language Server item".into());
    }

    // Sidebar drag: one centered divider line. The sidebar no longer draws its
    // own border, so there is exactly one line.
    let dark = crate::theme::Theme::new(Mode::Dark);
    let (drag_from, drag_to, width_before, divider_height) = {
        let state = harness.app.state();
        let rect = state.sidebar.rect;
        (
            Vec2::new(rect.right() + 2.0, rect.center().y),
            Vec2::new(rect.right() + 42.0, rect.center().y),
            state.settings.sidebar_width,
            // `sidebar.rect` is the tree body; the handle covers the whole dock.
            rect.height() + crate::ui::dock::HEADER_H,
        )
    };
    let frame = harness.frame()?;
    let idle_line = find_fill(&frame, dark.p.border.to_rgba8(), 1.0, divider_height)
        .ok_or("the sidebar handle has no idle divider line")?;
    if (idle_line.center().x - drag_from.x).abs() > 3.0 {
        return Err("the divider line is not centered on the handle".into());
    }
    find_fill_near_x(
        &frame,
        dark.p.faint.to_rgba8(),
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        crate::ui::handle::grip_center_x(drag_from.x, crate::ui::handle::GripSide::Right),
        0.6,
    )
    .ok_or("the sidebar handle has no idle grip")?;
    if count_vertical_lines(&frame, drag_from.x - 4.0, drag_from.x + 5.0, divider_height) != 1 {
        return Err("the sidebar shows more than one divider line".into());
    }
    harness.event(InputEvent::PointerDown {
        position: drag_from,
        button: PointerButton::Left,
    });
    if harness.app.state().sidebar_drag.is_none() {
        return Err("pressing the divider did not start a drag".into());
    }
    let frame = harness.frame()?;
    if find_fill(&frame, dark.p.accent.to_rgba8(), 1.0, divider_height).is_none() {
        return Err("the divider line did not turn accent while dragging".into());
    }
    find_fill_near_x(
        &frame,
        dark.p.accent.to_rgba8(),
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        crate::ui::handle::grip_center_x(drag_from.x, crate::ui::handle::GripSide::Right),
        0.6,
    )
    .ok_or("the sidebar grip did not turn accent while dragging")?;
    if find_fill(
        &frame,
        dark.p.accent.alpha_mul(0.16).to_rgba8(),
        4.0,
        divider_height,
    )
    .is_none()
    {
        return Err("the divider strip is not tinted while dragging".into());
    }
    harness.event(InputEvent::PointerMove { position: drag_to });
    harness.event(InputEvent::PointerUp {
        position: drag_to,
        button: PointerButton::Left,
    });
    harness.frame()?;
    {
        let state = harness.app.state();
        let expected = crate::settings::clamp_sidebar(width_before + 40.0);
        if (state.settings.sidebar_width - expected).abs() > 1.0 {
            return Err(format!(
                "sidebar drag did not apply (expected {expected}, got {})",
                state.settings.sidebar_width
            ));
        }
        if (state.sidebar.rect.width() - expected).abs() > 1.5 {
            return Err("the sidebar did not relayout during/after the drag".into());
        }
        if (state.session.sidebar_width - expected).abs() > 1.0 {
            return Err("the sidebar width was not persisted".into());
        }
    }
    // Hovering brightens the same single line.
    let (hover, hover_height) = {
        let state = harness.app.state();
        (
            Vec2::new(
                state.sidebar.rect.right() + 2.0,
                state.sidebar.rect.center().y,
            ),
            state.sidebar.rect.height() + crate::ui::dock::HEADER_H,
        )
    };
    harness.event(InputEvent::PointerMove { position: hover });
    let frame = harness.frame()?;
    if find_fill(&frame, dark.p.dim.to_rgba8(), 1.0, hover_height).is_none() {
        return Err("hovering the divider did not brighten the line".into());
    }
    find_fill_near_x(
        &frame,
        dark.p.text.to_rgba8(),
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        crate::ui::handle::grip_center_x(hover.x, crate::ui::handle::GripSide::Right),
        0.6,
    )
    .ok_or("hovering the divider did not brighten the grip")?;

    // Large-file behavior: preview truncation and the open guard.
    harness.dialogs.push(Some(dir.join("large.md")));
    harness.shortcut(
        Key::Character('o'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    // The banner sits at the end of the truncated content; culling only paints
    // the visible window, so scroll to the bottom before asserting it.
    {
        let mut state = harness.app.state_mut();
        state.preview.scroll = f32::MAX;
        state.dirty = true;
    }
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "preview truncated at 512 KB",
        "large preview banner",
    )?;
    if harness.app.state().ws.active_doc().map(|doc| doc.name()) != Some("large.md") {
        return Err("the large Markdown fixture did not open".into());
    }
    // A large Markdown edit coalesces the expensive parse + layout.
    {
        let mut state = harness.app.state_mut();
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.insert_text("x");
        }
    }
    harness.frame()?;
    if !harness.app.state().preview.rebuild_pending() {
        return Err("large Markdown edit did not schedule a coalesced preview rebuild".into());
    }
    std::thread::sleep(Duration::from_millis(150));
    harness.frame()?;
    if harness.app.state().preview.rebuild_pending() {
        return Err("coalesced preview rebuild did not run after the debounce".into());
    }
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    // The edit above made the tab dirty; discard it before the next open.
    if harness.app.state().modal.is_some() {
        harness.app.state_mut().resolve_modal(ModalButton::Discard);
        harness.frame()?;
    }
    harness.dialogs.push(Some(dir.join("huge.txt")));
    harness.shortcut(
        Key::Character('o'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.tabs.iter().any(|doc| doc.name() == "huge.txt") {
            return Err("a >32 MB file must be refused".into());
        }
        if !state
            .message()
            .is_some_and(|(message, _)| message.contains("too large"))
        {
            return Err("the oversize-open refusal was not reported".into());
        }
    }

    // Tab overflow: many tabs force a scroll offset and keep the active visible.
    for _ in 0..12 {
        harness.shortcut(
            Key::Character('n'),
            Modifiers {
                meta: true,
                ..Modifiers::NONE
            },
        );
    }
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.tabs.scroll <= 0.0 {
            return Err("many tabs did not scroll the strip".into());
        }
        let active = state.ws.active;
        let Some((_, tab, _)) = state
            .tabs
            .hits
            .iter()
            .find(|(index, _, _)| *index == active)
        else {
            return Err("the active tab is not visible in the strip".into());
        };
        if tab.left() < 0.0 || tab.right() > WIDTH {
            return Err("the active tab escaped the window".into());
        }
        let strip = state.tabs.strip;
        let target = state
            .tabs
            .hits
            .iter()
            .find(|(index, tab, _)| {
                *index != active
                    && tab.left() >= strip.left()
                    && tab.right() <= strip.right() - 28.0
            })
            .map(|(index, tab, _)| (*index, tab.center()));
        if let Some((index, center)) = target {
            drop(state);
            harness.click(center);
            if harness.app.state().ws.active != index {
                return Err("clicking an overflowed tab did not select it".into());
            }
        }
    }
    // Wheel-scrolling the strip must move even when the active tab is at the
    // right edge (no scrollbar is drawn; the strip itself is the target).
    let strip_center = harness.app.state().tabs.strip.center();
    let before_scroll = harness.app.state().tabs.scroll;
    harness.event(InputEvent::Wheel {
        position: strip_center,
        delta: Vec2::new(0.0, -60.0),
        phase: igui_core::TouchPhase::Moved,
        device: igui_core::ScrollDevice::Wheel,
    });
    harness.frame()?;
    if harness.app.state().tabs.scroll >= before_scroll {
        return Err("wheel scrolling did not move the overflowing tab strip".into());
    }
    // Restore the active tab for the checks that follow.
    harness.app.state_mut().tabs.reveal_active = true;
    harness.frame()?;

    // ================= M4: quick open, search, split, reliability =====

    // Quick Open over the workspace.
    harness.shortcut(
        Key::Character('p'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    if !harness.app.state().quick_open.open {
        return Err("⌘P did not open Quick Open".into());
    }
    expect(&texts(&frame), "Type a file name", "Quick Open placeholder")?;
    harness.type_text("main");
    let mut frames = 0;
    loop {
        std::thread::sleep(Duration::from_millis(15));
        harness.frame()?;
        frames += 1;
        let busy = harness.app.state().quick_open_busy();
        if !busy || frames > 300 {
            break;
        }
    }
    if harness.app.state().quick_open_busy() {
        return Err("Quick Open indexing did not finish before the deadline".into());
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state
            .quick_open
            .results
            .first()
            .is_some_and(|path| path.ends_with("main.rs"))
        {
            return Err(format!(
                "Quick Open did not find main.rs (results={:?})",
                state.quick_open.results
            ));
        }
    }
    expect(&texts(&frame), "main.rs", "Quick Open result row")?;
    harness.key(Key::Enter);
    harness.frame()?;
    if harness.app.state().quick_open.open {
        return Err("Enter did not close Quick Open".into());
    }
    if harness.app.state().ws.active_doc().map(|doc| doc.name()) != Some("main.rs") {
        return Err("Quick Open did not open the selected file".into());
    }

    // Project search: debounce, worker, results, jump.
    harness.shortcut(
        Key::Character('f'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if !harness.app.state().search.open {
        return Err("⇧⌘F did not open project search".into());
    }
    harness.type_text("hello");
    let mut frames = 0;
    loop {
        std::thread::sleep(Duration::from_millis(15));
        harness.frame()?;
        frames += 1;
        let state = harness.app.state();
        let done = !state.search.scanning
            && state.search.pending.is_none()
            && !state.search.hits.is_empty();
        drop(state);
        if done || frames > 300 {
            break;
        }
    }
    {
        let state = harness.app.state();
        if state.search.hits.is_empty() {
            return Err("project search returned no hits for `hello`".into());
        }
        if !state
            .search
            .hits
            .iter()
            .any(|hit| hit.preview.contains("hello"))
        {
            return Err("project search hits have no matching preview".into());
        }
    }
    let frame = harness.frame()?;
    expect(&texts(&frame), "results", "project search footer")?;
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        let selected = state.ws.active_doc().and_then(|doc| doc.selected_text());
        if selected != Some("hello") {
            return Err(format!(
                "project search jump did not select the match (got {selected:?})"
            ));
        }
    }
    // Replace in files, through the confirmation modal.
    harness.frame()?;
    let replace_path = dir.join("replace.txt");
    let replace_original =
        fs::read_to_string(&replace_path).map_err(|error| format!("read replace.txt: {error}"))?;
    let (replace_rect, replace_button) = {
        let state = harness.app.state();
        (state.search.replace_rect, state.search.replace_button)
    };
    if replace_rect.width() < 10.0 {
        return Err("the replace field has no geometry".into());
    }
    harness.click(replace_rect.center());
    harness.type_text("bonjour");
    harness.frame()?;
    harness.click(replace_button.center());
    let frame = harness.frame()?;
    expect(&texts(&frame), "Replace in files", "replace confirmation")?;
    let cancel = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Cancel)
            .map(|(_, rect)| rect.center())
            .ok_or("no Cancel button on the replace modal")?
    };
    harness.click(cancel);
    harness.frame()?;
    if fs::read_to_string(&replace_path).map_err(|e| format!("read: {e}"))? != replace_original {
        return Err("cancelling the replace modal changed the file".into());
    }
    harness.click(replace_button.center());
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "Replace in files",
        "replace confirmation (second)",
    )?;
    if !harness
        .app
        .state()
        .modal_buttons
        .iter()
        .any(|(button, _)| *button == ModalButton::Apply)
    {
        return Err("no Replace all button on the replace modal".into());
    }
    // Enter confirms the replace modal (keyboard path).
    harness.key(Key::Enter);
    harness.frame()?;
    let replaced =
        fs::read_to_string(&replace_path).map_err(|error| format!("read back: {error}"))?;
    if !replaced.contains("bonjour alpha") || !replaced.contains("bonjour beta") {
        return Err(format!(
            "replace in files did not rewrite replace.txt (got {replaced:?})"
        ));
    }
    let main_after =
        fs::read_to_string(&main_path).map_err(|error| format!("read back: {error}"))?;
    if !main_after.contains("\"hello\"") {
        return Err("replace in files overwrote an open tab's file".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().search.open {
        return Err("Esc did not close project search".into());
    }

    // Split drag: divider ratio, persistence, double-click reset.
    let notes_click = sidebar_click_position(&harness.app, "notes.md")?;
    harness.click(notes_click);
    harness.frame()?;
    {
        let mut state = harness.app.state_mut();
        state.set_view(crate::state::ViewMode::Split);
    }
    harness.frame()?;
    let (divider, ratio_before) = {
        let state = harness.app.state();
        if state.built_view != crate::state::ViewMode::Split {
            return Err("notes.md did not open in split view".into());
        }
        (state.split_divider, state.split_of_active())
    };
    if divider.width() < 1.0 {
        return Err("the split divider has no geometry".into());
    }
    // The split handle follows the same drag-handle spec as the sidebar.
    let dark = crate::theme::Theme::new(Mode::Dark);
    let frame = harness.frame()?;
    find_fill_near_x(
        &frame,
        dark.p.border.to_rgba8(),
        1.0,
        divider.height(),
        divider.center().x,
        3.0,
    )
    .ok_or("the split handle has no idle line")?;
    find_fill_near_x(
        &frame,
        dark.p.faint.to_rgba8(),
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        crate::ui::handle::grip_center_x(divider.center().x, crate::ui::handle::GripSide::Left),
        0.6,
    )
    .ok_or("the split handle has no idle grip")?;
    if count_vertical_lines(
        &frame,
        divider.left() - 6.0,
        divider.right() + 6.0,
        divider.height(),
    ) != 1
    {
        return Err("the split preview shows more than one divider line".into());
    }
    // Both grips share the content-band centre, so their y matches exactly.
    let sidebar_x = crate::ui::handle::grip_center_x(
        harness.app.state().sidebar.rect.right() + crate::ui::handle::HANDLE_WIDTH * 0.5,
        crate::ui::handle::GripSide::Right,
    );
    let split_x =
        crate::ui::handle::grip_center_x(divider.center().x, crate::ui::handle::GripSide::Left);
    let sidebar_y = grip_center_y(&frame, dark.p.faint.to_rgba8(), sidebar_x)
        .ok_or("the sidebar grip is missing in split view")?;
    let split_y = grip_center_y(&frame, dark.p.faint.to_rgba8(), split_x)
        .ok_or("the split grip is missing in split view")?;
    if (sidebar_y - split_y).abs() > 0.6 {
        return Err(format!(
            "the two grips are not level (sidebar {sidebar_y}, split {split_y})"
        ));
    }
    if (split_y - divider.center().y).abs() > 0.6 {
        return Err(format!(
            "grips do not follow the content-band centre (grip {split_y}, divider {})",
            divider.center().y
        ));
    }
    let idle_grip_y = split_y;

    // Scroll sync: editor -> preview, preview -> editor, and the toggle.
    let line_h = harness.app.state().editor_metrics().line_h;
    {
        let mut state = harness.app.state_mut();
        state
            .ws
            .active_doc_mut()
            .ok_or("no active document")?
            .scroll_y = line_h * 6.0;
    }
    harness.frame()?;
    let preview_synced = harness.app.state().preview.scroll;
    if preview_synced <= 0.0 {
        return Err("scrolling the editor did not move the preview".into());
    }
    {
        let mut state = harness.app.state_mut();
        let target = (state.preview.scroll + 200.0).min(state.preview.total_h.max(0.0));
        state.preview.scroll = target;
    }
    harness.frame()?;
    let editor_synced = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0.0, |doc| doc.scroll_y);
    if (editor_synced - line_h * 6.0).abs() < 0.5 {
        return Err("scrolling the preview did not move the editor".into());
    }
    {
        let mut state = harness.app.state_mut();
        state.sync_scroll = false;
    }
    let preview_mark = harness.app.state().preview.scroll;
    {
        let mut state = harness.app.state_mut();
        state
            .ws
            .active_doc_mut()
            .ok_or("no active document")?
            .scroll_y = line_h * 2.0;
    }
    harness.frame()?;
    if (harness.app.state().preview.scroll - preview_mark).abs() > 0.5 {
        return Err("scroll sync stayed active after being disabled".into());
    }
    {
        let mut state = harness.app.state_mut();
        state.sync_scroll = true;
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.scroll_y = 0.0;
        }
        state.preview.scroll = 0.0;
    }
    harness.frame()?;

    let drag_to = Vec2::new(divider.center().x + 60.0, divider.center().y);
    harness.event(InputEvent::PointerDown {
        position: divider.center(),
        button: PointerButton::Left,
    });
    let frame = harness.frame()?;
    find_fill_near_x(
        &frame,
        dark.p.accent.to_rgba8(),
        1.0,
        divider.height(),
        divider.center().x,
        3.0,
    )
    .ok_or("the split handle line did not turn accent while dragging")?;
    find_fill_near_x(
        &frame,
        dark.p.accent.to_rgba8(),
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        crate::ui::handle::grip_center_x(divider.center().x, crate::ui::handle::GripSide::Left),
        0.6,
    )
    .ok_or("the split handle grip did not turn accent while dragging")?;
    harness.event(InputEvent::PointerMove { position: drag_to });
    harness.event(InputEvent::PointerUp {
        position: drag_to,
        button: PointerButton::Left,
    });
    harness.frame()?;
    {
        let state = harness.app.state();
        let ratio = state.split_of_active();
        if ratio <= ratio_before + 0.05 {
            return Err(format!(
                "dragging the split divider did not change the ratio ({ratio_before} -> {ratio})"
            ));
        }
        if (state.session.split - ratio).abs() > 0.001 {
            return Err("the split ratio was not stored in the session".into());
        }
    }
    let divider = harness.app.state().split_divider;
    harness.event(InputEvent::PointerDown {
        position: divider.center(),
        button: PointerButton::Left,
    });
    harness.event(InputEvent::DoubleClick {
        position: divider.center(),
    });
    harness.event(InputEvent::PointerUp {
        position: divider.center(),
        button: PointerButton::Left,
    });
    harness.frame()?;
    if (harness.app.state().split_of_active() - 0.5).abs() > 0.01 {
        return Err("double-clicking the split divider did not reset it to 50%".into());
    }

    // Opening the find bar shifts the content band down: both grips follow it
    // together (and stay level).
    harness.shortcut(
        Key::Character('f'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    let divider = harness.app.state().split_divider;
    let sidebar_x = crate::ui::handle::grip_center_x(
        harness.app.state().sidebar.rect.right() + crate::ui::handle::HANDLE_WIDTH * 0.5,
        crate::ui::handle::GripSide::Right,
    );
    let split_x =
        crate::ui::handle::grip_center_x(divider.center().x, crate::ui::handle::GripSide::Left);
    let sidebar_find = grip_center_y(&frame, dark.p.faint.to_rgba8(), sidebar_x)
        .ok_or("the sidebar grip disappeared with the find bar open")?;
    let split_find = grip_center_y(&frame, dark.p.faint.to_rgba8(), split_x)
        .ok_or("the split grip disappeared with the find bar open")?;
    if (sidebar_find - split_find).abs() > 0.6 {
        return Err(format!(
            "grips are not level with the find bar open (sidebar {sidebar_find}, split {split_find})"
        ));
    }
    if split_find <= idle_grip_y + 10.0 {
        return Err("the find bar did not move the grips down together".into());
    }
    harness.key(Key::Escape);
    let frame = harness.frame()?;
    let divider = harness.app.state().split_divider;
    let split_x =
        crate::ui::handle::grip_center_x(divider.center().x, crate::ui::handle::GripSide::Left);
    let split_after = grip_center_y(&frame, dark.p.faint.to_rgba8(), split_x)
        .ok_or("the split grip disappeared after closing the find bar")?;
    if (split_after - idle_grip_y).abs() > 0.6 {
        return Err("closing the find bar did not restore the grip height".into());
    }

    // Autosave: a dirty file is written after an idle flush.
    harness.type_text(" autosave-marker");
    harness.app.state_mut().force_reliability_flush();
    harness.frame()?;
    let notes_saved =
        fs::read_to_string(&notes_path).map_err(|error| format!("read back: {error}"))?;
    if !notes_saved.contains("autosave-marker") {
        return Err("autosave did not write the dirty buffer".into());
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.is_dirty())
    {
        return Err("autosave did not clear the dirty flag".into());
    }

    // Recovery snapshot for an untitled buffer, removed when discarded.
    harness.shortcut(
        Key::Character('n'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.type_text("recover me");
    harness.app.state_mut().force_reliability_flush();
    let untitled_name = harness
        .app
        .state()
        .ws
        .active_doc()
        .map(|doc| doc.name().to_string())
        .unwrap_or_default();
    if harness.app.state().recovery.list().is_empty() {
        return Err("a dirty untitled buffer was not snapshotted".into());
    }
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let discard = {
        let state = harness.app.state();
        state
            .modal_buttons
            .iter()
            .find(|(button, _)| *button == ModalButton::Discard)
            .map(|(_, rect)| rect.center())
            .ok_or("no Discard button for the recovered buffer")?
    };
    harness.click(discard);
    harness.frame()?;
    if harness
        .app
        .state()
        .recovery
        .list()
        .iter()
        .any(|entry| entry.name == untitled_name)
    {
        return Err("discarding a buffer left its recovery snapshot behind".into());
    }

    // Window geometry is persisted.
    harness
        .app
        .state_mut()
        .set_window_geometry((1111.0, 777.0), (33.0, 44.0));
    harness.app.state_mut().set_window_maximized(true);
    harness.frame()?;

    // -- theme toggle (persisted) ---------------------------------------
    harness.shortcut(
        Key::Character('l'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    let light_bg = crate::theme::Theme::new(Mode::Light).p.bg.to_rgba8();
    if !frame.commands().iter().any(|command| {
        matches!(command, DrawCommand::FillRect { paint, .. } if paint.color.to_rgba8() == light_bg)
    }) {
        return Err("⇧⌘L did not repaint with the light palette".into());
    }

    // -- Open Project through a dialog -----------------------------------
    harness.dialogs.push(Some(dir.join("src")));
    harness.shortcut(
        Key::Character('o'),
        Modifiers {
            meta: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    let frame = harness.frame()?;
    if !harness.dialogs.calls().contains(&Call::OpenFolder) {
        return Err("⇧⌘O did not ask the dialog layer for a project".into());
    }
    if harness
        .app
        .state()
        .ws
        .root
        .file_name()
        .and_then(|name| name.to_str())
        != Some("src")
    {
        return Err("Open Project did not retarget the sidebar root".into());
    }
    expect(&texts(&frame), "lib.rs", "sidebar now lists the new root")?;

    // -- File > Close Project clears tabs and the session project ----------
    if cfg!(target_os = "macos") {
        harness.app.state_mut().run(Action::CloseFolder);
    } else {
        let file_item = harness
            .app
            .state()
            .menubar_hits
            .iter()
            .find(|(menu, _)| *menu == Menu::File)
            .map(|(_, rect)| rect.center())
            .ok_or("no File menu hit rect after opening a project")?;
        harness.click(file_item);
        harness.frame()?;
        let close_folder_item = {
            let state = harness.app.state();
            if !Action::CloseFolder.enabled(&state) {
                return Err("Close Project is disabled while a project is open".into());
            }
            let (_, items) = crate::ui::menu::popup_layout(&state, measurer.as_ref());
            items
                .iter()
                .find(|(action, _)| *action == Some(Action::CloseFolder))
                .map(|(_, rect)| rect.center())
                .ok_or("the File menu has no Close Project item")?
        };
        harness.click(close_folder_item);
    }
    harness.frame()?;
    // A dirty tab opens the confirmation; discard for this flow.
    if harness.app.state().modal.is_some() {
        harness
            .app
            .state_mut()
            .resolve_modal(crate::state::ModalButton::Discard);
        harness.frame()?;
    }
    {
        let state = harness.app.state();
        if state.ws.folder_open || state.ws.folder().is_some() {
            return Err("Close Project left the project open".into());
        }
        if !state.ws.tabs.is_empty() || state.ws.active_doc().is_some() {
            return Err("Close Project must clear the open tabs".into());
        }
        if state.session.project.is_some() {
            return Err("Close Project did not clear the persisted session project".into());
        }
        if !state.session.tabs.is_empty() {
            return Err("Close Project did not clear the persisted tabs".into());
        }
        if Action::CloseFolder.enabled(&state) {
            return Err("Close Project is still enabled with no project".into());
        }
    }
    // Reopen it and put a file back for the session-restore assertion below.
    harness.app.state_mut().open_project(dir.join("src"));
    harness.frame()?;
    harness.app.state_mut().open_path(canonical_notes.clone());
    harness.frame()?;
    if !harness.app.state().ws.folder_open {
        return Err("reopening the project after Close Project failed".into());
    }

    // -- session restore -------------------------------------------------
    harness.frame()?; // flush the session file
    if !config.is_file() {
        return Err("the session file was not written".into());
    }
    {
        let mut state = harness.app.state_mut();
        if let Some(index) = state
            .ws
            .tabs
            .iter()
            .position(|doc| doc.path() == Some(canonical_lua.as_path()))
        {
            state.ws.select(index);
            state.set_language(Some("rust".into()));
        }
    }
    harness.frame()?;
    let final_settings = harness.app.state().settings;
    let final_languages = harness.app.state().language_by_path.clone();
    let final_sync = harness.app.state().sync_scroll;
    let restored_dialogs = ScriptedDialogs::new(vec![]);
    let restored = EditorApp::with_config(
        None,
        None,
        config.clone(),
        Box::new(restored_dialogs.clone()),
    );
    {
        let state = restored.state();
        if state.theme.mode != Mode::Light {
            return Err("session restore lost the light theme".into());
        }
        if !state
            .ws
            .tabs
            .iter()
            .any(|doc| doc.path() == Some(canonical_notes.as_path()))
        {
            return Err(format!(
                "session restore lost the open tab (config={:?} tabs={:?})",
                fs::read_to_string(&config).unwrap_or_default(),
                state
                    .ws
                    .tabs
                    .iter()
                    .map(|doc| doc.path().map(|p| p.to_path_buf()))
                    .collect::<Vec<_>>()
            ));
        }
        if state
            .session
            .recent_files
            .iter()
            .all(|path| path != &canonical_out)
        {
            return Err("session restore lost the recent files".into());
        }
        if state.sync_scroll != final_sync {
            return Err("session restore lost the scroll-sync setting".into());
        }
        if state.language_by_path != final_languages {
            return Err(format!(
                "session restore lost the language overrides ({:?} vs {:?})",
                state.language_by_path, final_languages
            ));
        }
        if state.settings != final_settings {
            return Err(format!(
                "session restore lost the editor settings (restored {:?}, expected {:?})",
                state.settings, final_settings
            ));
        }
        if state.session.window
            != Some(WindowGeometry {
                size: (1111.0, 777.0),
                position: (33.0, 44.0),
            })
        {
            return Err(format!(
                "session restore lost the window geometry ({:?})",
                state.session.window
            ));
        }
        if !state.session.window_maximized {
            return Err("session restore lost the maximized window state".into());
        }
        if (state.session.split - 0.5).abs() > 0.01 {
            return Err(format!(
                "session restore lost the split ratio ({})",
                state.session.split
            ));
        }
        if !state.ws.folder_open
            || state.ws.root.file_name().and_then(|name| name.to_str()) != Some("src")
        {
            return Err(format!(
                "session restore lost the workspace project ({:?})",
                state.ws.root
            ));
        }
    }

    // Frame loop: an idle app must not request frames. A terminal panel is the
    // classic offender (being open used to be enough to keep a 60 fps loop
    // alive), and a second unchanged frame must reuse the cached draw list.
    {
        let mut state = harness.app.state_mut();
        state.terminal.open = true;
        state.preview.cancel_rebuild();
        state.dirty = true;
    }
    // GDScript indent wiring: a tab file stays tabs on Enter, and Format
    // converts it to the configured four spaces.
    {
        let mut state = harness.app.state_mut();
        state.settings.indent_tabs = true;
        state.settings_changed();
        state.ws.new_tab();
        state.set_language(Some("gdscript".into()));
        state.focus = Focus::Editor;
    }
    harness.type_text("func _ready():");
    harness.key(Key::Enter);
    harness.type_text("pass");
    {
        let state = harness.app.state();
        let text = state.ws.active_doc().map(|doc| doc.text().to_string());
        if text.as_deref() != Some("func _ready():\n\tpass") {
            return Err(format!("GDScript Enter did not add a tab indent: {text:?}"));
        }
    }
    {
        let mut state = harness.app.state_mut();
        state.settings.indent_tabs = false;
        state.settings.tab_width = 4;
        state.settings_changed();
        state.run(Action::FormatDocument);
    }
    harness.frame()?;
    {
        let state = harness.app.state();
        let text = state.ws.active_doc().map(|doc| doc.text().to_string());
        if text.as_deref() != Some("func _ready():\n    pass") {
            return Err(format!(
                "Format Document did not convert the indent to four spaces: {text:?}"
            ));
        }
    }

    // A GDScript docstring line ending in `:` is content: Enter must not add a
    // block indent (the syntax layer keeps the string open until `"""`).
    {
        let mut state = harness.app.state_mut();
        let doc = state.ws.active_doc_mut().expect("active GDScript tab");
        let end = doc.text().len();
        doc.set_caret(end);
    }
    harness.type_text("\n\"\"\"");
    harness.key(Key::Enter);
    harness.type_text("Usage:");
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        let text = state
            .ws
            .active_doc()
            .map(|doc| doc.text().to_string())
            .unwrap_or_default();
        if !text.ends_with("\"\"\"\nUsage:\n") {
            return Err(format!(
                "Enter inside a GDScript docstring added an indent: {text:?}"
            ));
        }
    }
    harness.type_text("\"\"\"");
    harness.key(Key::Enter);
    harness.type_text("func later():");
    harness.key(Key::Enter);
    harness.type_text("pass");
    {
        let state = harness.app.state();
        let text = state
            .ws
            .active_doc()
            .map(|doc| doc.text().to_string())
            .unwrap_or_default();
        if !text.ends_with("func later():\n    pass") {
            return Err(format!(
                "Enter after a real GDScript block opener lost its indent: {text:?}"
            ));
        }
    }

    // Async workers (git) answer on their own thread; give them a moment.
    let settle = std::time::Instant::now() + Duration::from_secs(2);
    while harness.app.needs_frame() && std::time::Instant::now() < settle {
        harness.frame()?;
        std::thread::sleep(Duration::from_millis(2));
    }
    if harness.app.needs_frame() {
        return Err(format!(
            "an idle app still requests frames with the terminal open ({})",
            harness.app.frame_blocker()
        ));
    }
    let _ = harness.frame()?;
    if harness.app.needs_frame() {
        return Err(format!(
            "a settled frame still requests another ({})",
            harness.app.frame_blocker()
        ));
    }
    if !harness.app.paint_cache_reused() {
        return Err("an unchanged frame did not reuse the cached draw list".into());
    }
    harness.app.state_mut().terminal.open = false;

    let _ = restored_dialogs;
    println!(
        "selfcheck: main flow ok ({} commands in the final frame, {} controls)",
        frame.command_count(),
        harness.app.control_count()
    );
    Ok(())
}

/// Auto-pairing and Markdown list continuation through the real input path.
///
/// The unit tests cover the rules in `model::typing`; this covers the wiring
/// (input event -> editor -> document) and that IME text never grows a pair.
fn check_typing(dir: &Path) -> Result<(), String> {
    let config = dir.join("typing.conf");
    let mut harness = Harness::new(None, config, ScriptedDialogs::new(vec![]));

    fn expect_doc(harness: &Harness, text: &str, caret: usize) -> Result<(), String> {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.text() != text || doc.caret() != caret {
            return Err(format!(
                "typing: expected {text:?} with the caret at {caret}, got {:?} at {}",
                doc.text(),
                doc.caret()
            ));
        }
        Ok(())
    }

    fn fresh(harness: &mut Harness) -> Result<(), String> {
        {
            let mut state = harness.app.state_mut();
            state.ws.new_tab();
            state.set_language(Some("markdown".into()));
            state.focus = Focus::Editor;
        }
        harness.frame()?;
        Ok(())
    }

    // A bracket pairs with the caret inside and its closer is stepped over.
    fresh(&mut harness)?;
    harness.type_text("(");
    expect_doc(&harness, "()", 1)?;
    harness.type_text("a");
    harness.type_text(")");
    expect_doc(&harness, "(a)", 3)?;

    // Backspace between the halves removes the whole pair.
    fresh(&mut harness)?;
    harness.type_text("[");
    harness.key(Key::Backspace);
    expect_doc(&harness, "", 0)?;

    // Enter inside an empty pair opens an indented block.
    fresh(&mut harness)?;
    harness.type_text("(");
    harness.key(Key::Enter);
    expect_doc(&harness, "(\n    \n)", 6)?;

    // A selection is wrapped and stays selected.
    fresh(&mut harness)?;
    harness.type_text("word");
    harness
        .app
        .state_mut()
        .ws
        .active_doc_mut()
        .ok_or("no active document")?
        .select_all();
    harness.type_text("(");
    expect_doc(&harness, "(word)", 5)?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .and_then(|doc| doc.selected_text())
        != Some("word")
    {
        return Err("wrapping a selection lost the selection".into());
    }

    // Markdown list continuation, and an empty item ending the list.
    fresh(&mut harness)?;
    harness.type_text("- item");
    harness.key(Key::Enter);
    expect_doc(&harness, "- item\n- ", 9)?;
    harness.key(Key::Enter);
    expect_doc(&harness, "- item\n", 7)?;

    // Select Next Occurrence (Edit menu / ⌘D): the word, then each further
    // occurrence, with every site edited by one keystroke and one undo step.
    fresh(&mut harness)?;
    harness.type_text("one two one\none");
    {
        let mut state = harness.app.state_mut();
        state
            .ws
            .active_doc_mut()
            .ok_or("no active document")?
            .set_caret(1);
        state.run(Action::SelectNextOccurrence);
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .and_then(|doc| doc.selected_text())
        != Some("one")
    {
        return Err("the first Select Next Occurrence did not select the word".into());
    }
    {
        let mut state = harness.app.state_mut();
        state.run(Action::SelectNextOccurrence);
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.extra_selections() != [(8, 11)] {
            return Err(format!(
                "the second occurrence was not selected: {:?}",
                doc.extra_selections()
            ));
        }
    }
    // Both sites are painted as selections in this frame.
    let frame = harness.frame()?;
    let selection = harness.app.state().theme.p.selection.to_rgba8();
    let painted = frame
        .commands()
        .iter()
        .filter(|command| {
            matches!(command, DrawCommand::FillRect { paint, .. } if paint.color.to_rgba8() == selection)
        })
        .count();
    if painted < 2 {
        return Err(format!(
            "only {painted} occurrence selection(s) were painted"
        ));
    }
    harness.type_text("x");
    expect_doc(&harness, "x two x\none", 1)?;
    {
        let mut state = harness.app.state_mut();
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.extra_selections() != [(7, 7)] {
            return Err(format!(
                "the second site lost its caret after typing: {:?}",
                doc.extra_selections()
            ));
        }
        state.run(Action::Undo);
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.text() != "one two one\none" {
            return Err(format!(
                "undoing a multiple edit did not restore the text: {:?}",
                doc.text()
            ));
        }
    }
    // Esc drops the extra carets.
    {
        let mut state = harness.app.state_mut();
        state.run(Action::SelectNextOccurrence);
        state.run(Action::SelectNextOccurrence);
    }
    harness.type_text("y");
    harness.key(Key::Escape);
    if !harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.extra_selections().is_empty())
    {
        return Err("Esc did not collapse the extra selections".into());
    }

    // Committed IME text is content: it must never grow a pair.
    fresh(&mut harness)?;
    harness.event(InputEvent::Ime(igui_core::ImeEvent::Commit("你".into())));
    expect_doc(&harness, "你", 3)?;

    // The setting turns the pairing off again.
    fresh(&mut harness)?;
    {
        let mut state = harness.app.state_mut();
        state.settings.auto_pairs = false;
        state.settings_changed();
    }
    harness.type_text("(");
    harness.type_text(")");
    expect_doc(&harness, "()", 2)?;
    {
        let mut state = harness.app.state_mut();
        state.settings.auto_pairs = true;
        state.settings_changed();
    }

    // Saving tidies the file through the real ⌘S path: trailing whitespace is
    // removed and the file ends with a newline (Markdown hard breaks are kept,
    // see `model::doc`).
    let tidy_path = dir.join("tidy.rs");
    fs::write(&tidy_path, "fn main() {   \n    let x = 1;   ")
        .map_err(|error| format!("fixture: {error}"))?;
    harness.app.state_mut().open_path(tidy_path.clone());
    harness.frame()?;
    harness.shortcut(
        Key::Character('s'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let saved = fs::read_to_string(&tidy_path).map_err(|error| format!("read back: {error}"))?;
    if saved != "fn main() {\n    let x = 1;\n" {
        return Err(format!("⌘S did not tidy the file: {saved:?}"));
    }

    println!("selfcheck: typing ok (auto pairs, list continuation, next occurrence, tidy save)");
    Ok(())
}

/// Explorer visibility: dot entries, dimmed tool files and the hide mode,
/// driven through the settings path and asserted on the painted sidebar.
fn check_explorer_filters(dir: &Path) -> Result<(), String> {
    let root = dir.join("explorer-filters");
    fs::create_dir_all(root.join("src")).map_err(|error| format!("fixture: {error}"))?;
    fs::write(root.join("main.rs"), "fn main() {}\n")
        .map_err(|error| format!("fixture: {error}"))?;
    fs::write(root.join("src").join("inner.rs"), "fn inner() {}\n")
        .map_err(|error| format!("fixture: {error}"))?;
    fs::write(root.join(".env"), "SECRET=1\n").map_err(|error| format!("fixture: {error}"))?;
    fs::write(root.join("player.gd"), "extends Node\n")
        .map_err(|error| format!("fixture: {error}"))?;
    fs::write(root.join("player.gd.uid"), "uid://abc\n")
        .map_err(|error| format!("fixture: {error}"))?;

    let mut harness = Harness::new(
        Some(root.join("main.rs")),
        root.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    // A changed filter must not collapse the folders the user expanded.
    let src = fs::canonicalize(root.join("src")).map_err(|error| format!("fixture: {error}"))?;
    {
        let mut state = harness.app.state_mut();
        assert!(state.ws.tree.toggle(&src));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        let rows = state.ws.tree.rows();
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        if names.contains(&".env") {
            return Err("the Explorer lists dot entries before Hidden Files is enabled".into());
        }
        let uid = rows
            .iter()
            .find(|row| row.name == "player.gd.uid")
            .ok_or("the default ignore mode must keep the .uid sidecar visible")?;
        if !uid.dim {
            return Err("the .uid sidecar is not marked as ignored (dim)".into());
        }
        let dim = state.theme.p.dim.to_rgba8();
        if !draw_text_has_color(&frame, "player.gd.uid", dim) {
            return Err("the ignored .uid row did not paint in the secondary color".into());
        }
    }

    // Hidden Files ON reveals dot entries, still dimmed, without collapsing.
    harness.app.state_mut().activate_setting(Row::HiddenFiles);
    let frame = harness.frame()?;
    let painted = texts(&frame);
    expect(
        &painted,
        ".env",
        "the Hidden Files setting reveals dot entries",
    )?;
    expect(&painted, "inner.rs", "an expanded folder stays open")?;
    {
        let state = harness.app.state();
        if !state.settings.show_hidden || !state.session.show_hidden {
            return Err("Hidden Files did not persist to the session".into());
        }
    }

    // Ignored Files DIM -> HIDE removes the sidecar; -> OFF shows it normally.
    harness.app.state_mut().activate_setting(Row::IgnoredFiles);
    let frame = harness.frame()?;
    if texts(&frame).iter().any(|text| text == "player.gd.uid") {
        return Err("HIDE still painted the ignored .uid row".into());
    }
    {
        let state = harness.app.state();
        if state.session.ignored_files != "hide" {
            return Err(format!(
                "the ignore mode did not persist to the session ({})",
                state.session.ignored_files
            ));
        }
    }
    harness.app.state_mut().activate_setting(Row::IgnoredFiles);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        let text_color = state.theme.p.text.to_rgba8();
        if !draw_text_has_color(&frame, "player.gd.uid", text_color) {
            return Err("OFF did not paint the ignored row as a normal file".into());
        }
    }

    // Reset restores the hidden/dim defaults.
    harness.app.state_mut().activate_setting(Row::Reset);
    let frame = harness.frame()?;
    if texts(&frame).iter().any(|text| text == ".env") {
        return Err("Reset did not hide dot entries again".into());
    }
    Ok(())
}

/// Welcome/no-folder state, drag & drop, CLI folder and the recovery prompt.
fn check_workspace_and_recovery(dir: &Path) -> Result<(), String> {
    // -- no project, no document: welcome + empty sidebar ----------------
    let welcome_dir = dir.join("welcome");
    fs::create_dir_all(&welcome_dir).map_err(|error| format!("fixture: {error}"))?;
    let mut harness = Harness::new(
        None,
        welcome_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    let painted = texts(&frame);
    expect(&painted, "igui-editor", "welcome title")?;
    expect(&painted, "no project open", "welcome subtitle")?;
    expect(&painted, "Open Project…", "welcome actions")?;
    expect(&painted, "NO PROJECT OPEN", "sidebar empty state")?;
    expect(
        &painted,
        "or drop files or a project here",
        "sidebar drop hint",
    )?;
    {
        let state = harness.app.state();
        if state.ws.folder_open || !state.ws.tabs.is_empty() {
            return Err("a project-less startup must show the welcome state".into());
        }
    }

    // The theme toggle message must sit beside, not on top of, the project hint.
    let theme_hit = harness.app.state().theme_hit.center();
    harness.click(theme_hit);
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "theme: LIGHT",
        "theme toggle footer message",
    )?;
    let measurer = igui_ui::text_measurer(harness.app.tree());
    check_status_overlap(&frame, measurer.as_ref())?;
    let theme_hit = harness.app.state().theme_hit.center();
    harness.click(theme_hit);
    harness.frame()?;

    // Drop hover overlay.
    harness.app.state_mut().drop_hover = true;
    let frame = harness.frame()?;
    expect(&texts(&frame), "Drop to open", "drop overlay")?;
    harness.app.state_mut().drop_hover = false;
    harness.frame()?;

    // Dropping a file opens a tab without opening a project.
    harness.app.drop_paths(vec![dir.join("notes.md")]);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.folder_open {
            return Err("dropping a file must not open a project".into());
        }
        if state.ws.active_doc().map(|doc| doc.name()) != Some("notes.md") {
            return Err("dropping a file did not open it".into());
        }
        if state
            .session
            .recent_projects
            .iter()
            .any(|path| path.ends_with("notes.md"))
            || !state
                .session
                .recent_files
                .iter()
                .any(|path| path.ends_with("notes.md"))
        {
            return Err("a dropped file must be recorded as a recent file, not a project".into());
        }
    }
    expect(&texts(&frame), "# Notes", "dropped file content")?;

    // Dropping a folder opens it as the project.
    harness.app.drop_paths(vec![dir.join("src")]);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.ws.folder_open {
            return Err("dropping a folder did not open the project".into());
        }
        if state.ws.root.file_name().and_then(|name| name.to_str()) != Some("src") {
            return Err(format!(
                "dropped project root is wrong: {:?}",
                state.ws.root
            ));
        }
        if !state
            .session
            .recent_projects
            .iter()
            .any(|path| path.ends_with("src"))
        {
            return Err("a dropped project must be recorded in recent projects".into());
        }
    }
    expect(&texts(&frame), "lib.rs", "dropped project sidebar")?;

    // Switching projects closes the old tabs and reinitializes project UI.
    harness
        .app
        .state_mut()
        .open_path(dir.join("src").join("lib.rs"));
    harness.frame()?;
    {
        let mut state = harness.app.state_mut();
        state
            .terminal
            .sessions
            .push(crate::state::TerminalSession::default());
        state.outline.hover = Some(3);
        state.outline.scroll = 9.0;
        state.bottom_tab = crate::state::Panel::Problems;
        state.terminal.open = true;
        let root = state.ws.root.clone();
        state.problems.rows.push(crate::model::lsp::ProblemEntry {
            path: root.join("lib.rs"),
            diagnostic: crate::model::lsp::Diagnostic {
                start_line: 0,
                start_char: 0,
                end_line: 0,
                end_char: 1,
                severity: 1,
                message: "boom".into(),
            },
        });
        state.dirty = true;
    }
    harness.app.state_mut().open_project(welcome_dir.clone());
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.root.file_name().and_then(|name| name.to_str()) != Some("welcome") {
            return Err(format!(
                "switching projects did not retarget the root: {:?}",
                state.ws.root
            ));
        }
        if !state.ws.tabs.is_empty() || state.ws.active_doc().is_some() {
            return Err("switching projects must close the old tabs".into());
        }
        if !state.terminal.sessions.is_empty() {
            return Err("switching projects must reset terminal sessions".into());
        }
        if state.outline.hover.is_some() || state.outline.scroll != 0.0 {
            return Err("switching projects must reset the outline".into());
        }
        if !state.problems.rows.is_empty() {
            return Err("switching projects must clear the PROBLEMS list".into());
        }
        if !state
            .session
            .recent_projects
            .iter()
            .any(|path| path.ends_with("welcome"))
        {
            return Err("switching projects must update the recent-project list".into());
        }
    }
    expect(
        &texts(&frame),
        "no document open",
        "switched project welcome",
    )?;
    expect(&texts(&frame), "Project: src", "recent project on welcome")?;
    drop(harness);

    // -- CLI folder argument --------------------------------------------
    let cli_dir = dir.join("cli");
    fs::create_dir_all(&cli_dir).map_err(|error| format!("fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(dir.join("src")),
        cli_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.ws.folder_open || !state.ws.tabs.is_empty() {
            return Err("a CLI folder must open the project without a tab".into());
        }
    }
    expect(&texts(&frame), "igui-editor", "CLI project welcome")?;
    expect(&texts(&frame), "lib.rs", "CLI project sidebar")?;
    drop(harness);

    // -- recovery prompt: restore ---------------------------------------
    let restore_dir = dir.join("recovery-restore");
    fs::create_dir_all(&restore_dir).map_err(|error| format!("fixture: {error}"))?;
    let store = RecoveryStore::new(restore_dir.join("recovery"));
    store
        .save(
            7,
            crate::model::recovery::Snapshot {
                name: "recovered.txt",
                path: Some(Path::new("/tmp/original.txt")),
                text: "recovered body\n",
                caret: 3,
                anchor: 1,
                eol: LineEnding::Crlf,
            },
        )
        .map_err(|error| format!("recovery fixture: {error}"))?;
    let mut harness = Harness::new(
        None,
        restore_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "Recover unsaved changes?",
        "recovery prompt",
    )?;
    if harness.app.state().recovery_prompt.is_none() {
        return Err("the recovery prompt was not loaded at startup".into());
    }
    let restore_button = {
        let state = harness.app.state();
        let count = state
            .recovery_prompt
            .as_ref()
            .map_or(0, |entries| entries.len());
        crate::ui::recovery::layout(state.viewport.logical_rect(), count)
            .restore
            .center()
    };
    harness.click(restore_button);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.recovery_prompt.is_some() {
            return Err("Restore All did not close the recovery prompt".into());
        }
        let Some(doc) = state.ws.active_doc() else {
            return Err("Restore All did not add a buffer".into());
        };
        if doc.text() != "recovered body\n" {
            return Err("Restore All lost the snapshot text".into());
        }
        if doc.path() != Some(Path::new("/tmp/original.txt")) {
            return Err("Restore All lost the original path".into());
        }
    }
    expect(
        &texts(&frame),
        "recovered body",
        "restored buffer is painted",
    )?;
    if !store.list().is_empty() {
        return Err("Restore All did not clear the snapshots".into());
    }
    drop(harness);

    // -- recovery prompt: discard ---------------------------------------
    let discard_dir = dir.join("recovery-discard");
    fs::create_dir_all(&discard_dir).map_err(|error| format!("fixture: {error}"))?;
    let store = RecoveryStore::new(discard_dir.join("recovery"));
    store
        .save(
            9,
            crate::model::recovery::Snapshot {
                name: "discard.txt",
                path: None,
                text: "discard me",
                caret: 0,
                anchor: 0,
                eol: LineEnding::Lf,
            },
        )
        .map_err(|error| format!("recovery fixture: {error}"))?;
    let mut harness = Harness::new(
        None,
        discard_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    harness.frame()?;
    harness.key(Key::Escape);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.recovery_prompt.is_some() || !state.ws.tabs.is_empty() {
            return Err("Discard All did not clear the prompt and buffers".into());
        }
    }
    if !store.list().is_empty() {
        return Err("Discard All did not delete the snapshots".into());
    }

    // -- external changes: clean reload, dirty Keep/Reload ----------------
    let external_dir = dir.join("external");
    fs::create_dir_all(&external_dir).map_err(|error| format!("fixture: {error}"))?;
    let external_path = external_dir.join("external.txt");
    fs::write(&external_path, "remote one\n").map_err(|error| format!("fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(external_path.clone()),
        external_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    harness.frame()?;
    fs::write(&external_path, "remote one changed\n")
        .map_err(|error| format!("fixture: {error}"))?;
    harness.app.state_mut().check_external_changes();
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document after the external reload")?;
        if doc.text() != "remote one changed\n" || doc.is_dirty() {
            return Err("a clean tab did not reload after an external change".into());
        }
    }
    {
        let mut state = harness.app.state_mut();
        state
            .ws
            .active_doc_mut()
            .ok_or("no active document")?
            .insert_text("local edit\n");
    }
    harness.frame()?;
    fs::write(&external_path, "remote two changed\n")
        .map_err(|error| format!("fixture: {error}"))?;
    harness.app.state_mut().check_external_changes();
    harness.frame()?;
    if !matches!(
        harness.app.state().modal,
        Some(crate::state::Modal::ExternalChange { .. })
    ) {
        return Err("a dirty tab did not prompt for an external change".into());
    }
    let keep = harness
        .app
        .state()
        .modal_buttons
        .iter()
        .find(|(button, _)| *button == ModalButton::Cancel)
        .map(|(_, rect)| rect.center())
        .ok_or("the external-change modal has no Keep button")?;
    harness.click(keep);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.modal.is_some() {
            return Err("Keep did not close the external-change modal".into());
        }
        if !state
            .ws
            .active_doc()
            .is_some_and(|doc| doc.text().contains("local edit"))
        {
            return Err("Keep lost the dirty buffer".into());
        }
    }
    fs::write(&external_path, "remote three changed\n")
        .map_err(|error| format!("fixture: {error}"))?;
    harness.app.state_mut().check_external_changes();
    harness.frame()?;
    let reload = harness
        .app
        .state()
        .modal_buttons
        .iter()
        .find(|(button, _)| *button == ModalButton::Reload)
        .map(|(_, rect)| rect.center())
        .ok_or("the external-change modal has no Reload button")?;
    harness.click(reload);
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no document after Reload")?;
        if doc.text() != "remote three changed\n" || doc.is_dirty() {
            return Err("Reload did not take the disk text".into());
        }
    }

    // -- soft wrap: layout, click mapping, vertical movement, toggle ---------
    let wrap_dir = dir.join("wrap");
    fs::create_dir_all(&wrap_dir).map_err(|error| format!("fixture: {error}"))?;
    let wrap_path = wrap_dir.join("wrap.txt");
    let long_line = format!("{} END\n", "word ".repeat(90));
    fs::write(&wrap_path, &long_line).map_err(|error| format!("fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(wrap_path.clone()),
        wrap_dir.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    harness.frame()?;
    {
        let mut state = harness.app.state_mut();
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.scroll_x = 80.0;
        }
    }
    harness.frame()?;
    // Simulate the macOS event order: KeyDown, the Option+Z character as
    // TextInput, then KeyUp. The text must not reach the document.
    harness.event(InputEvent::ModifiersChanged(Modifiers {
        alt: true,
        ..Modifiers::NONE
    }));
    harness.event(InputEvent::KeyDown {
        key: Key::Character('z'),
    });
    harness.type_text("Ω");
    harness.event(InputEvent::KeyUp {
        key: Key::Character('z'),
    });
    harness.event(InputEvent::ModifiersChanged(Modifiers::NONE));
    let frame = harness.frame()?;
    if !harness.app.state().session.soft_wrap {
        return Err("Alt+Z did not enable soft wrap".into());
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.text().contains('Ω'))
    {
        return Err("Alt+Z leaked its character into the document".into());
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.scroll_x != 0.0)
    {
        return Err("soft wrap did not suppress horizontal scrolling".into());
    }
    let (text_rect, line_h, rows, line_count) = {
        let state = harness.app.state();
        (
            state.editor.text_rect,
            state.editor_metrics().line_h,
            state.editor.wrap.len(),
            state.ws.active_doc().map_or(0, |doc| doc.line_count()),
        )
    };
    if rows <= line_count {
        return Err(format!(
            "soft wrap did not split the long line (rows={rows} lines={line_count})"
        ));
    }
    if !texts(&frame).iter().any(|text| text.contains("word")) {
        return Err("soft-wrapped text was not painted".into());
    }
    // Click the second visual row: same logical line, non-zero column.
    let click_at = Vec2::new(text_rect.left() + 24.0, text_rect.top() + line_h * 1.5);
    harness.click(click_at);
    {
        let state = harness.app.state();
        let (line, column) = state
            .ws
            .active_doc()
            .ok_or("no document after wrapped click")?
            .line_col();
        if line != 0 || column == 0 {
            return Err(format!(
                "wrapped click mapped to line {line}, column {column}"
            ));
        }
    }
    {
        let mut state = harness.app.state_mut();
        state
            .ws
            .active_doc_mut()
            .ok_or("no document for wrap navigation")?
            .click(0, 0, false);
    }
    harness.frame()?;
    harness.key(Key::ArrowDown);
    {
        let state = harness.app.state();
        let (line, column) = state
            .ws
            .active_doc()
            .ok_or("no document after wrapped down")?
            .line_col();
        if line != 0 || column == 0 {
            return Err(format!(
                "ArrowDown did not move to the next visual row (line {line}, column {column})"
            ));
        }
    }
    harness.key(Key::ArrowUp);
    {
        let state = harness.app.state();
        let (line, column) = state
            .ws
            .active_doc()
            .ok_or("no document after wrapped up")?
            .line_col();
        if line != 0 || column != 0 {
            return Err(format!(
                "ArrowUp did not return to the first visual row (line {line}, column {column})"
            ));
        }
    }
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.session.soft_wrap {
            return Err("Alt+Z did not disable soft wrap".into());
        }
        if !state.editor.wrap.is_empty()
            || (state.editor.content_h - line_count as f32 * line_h).abs() > 0.5
        {
            return Err("soft wrap off did not restore logical line heights".into());
        }
        if state
            .ws
            .active_doc()
            .is_some_and(|doc| (doc.scroll_x - 80.0).abs() > 0.5)
        {
            return Err("soft wrap did not restore the horizontal scroll offset".into());
        }
    }
    // The setting is persisted; the session file restores it.
    harness.shortcut(
        Key::Character('z'),
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    let restored = EditorApp::with_config(
        None,
        None,
        wrap_dir.join("session.conf"),
        Box::new(ScriptedDialogs::new(vec![])),
    );
    if !restored.state().session.soft_wrap {
        return Err("soft wrap was not restored from the session".into());
    }

    // -- built-in Git plugin: status panel + read-only diff ----------------
    let repo = dir.join("git-repo");
    fs::create_dir_all(&repo).map_err(|error| format!("fixture: {error}"))?;
    let git = |args: &[&str]| -> Result<(), String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .map_err(|error| format!("git fixture: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    };
    git(&["init", "-q"])?;
    git(&["config", "user.email", "selfcheck@example.com"])?;
    git(&["config", "user.name", "selfcheck"])?;
    let tracked = repo.join("tracked.txt");
    let original: String = (0..5).map(|index| format!("line {index}\n")).collect();
    fs::write(&tracked, original).map_err(|error| format!("fixture: {error}"))?;
    git(&["add", "tracked.txt"])?;
    git(&["commit", "-qm", "initial"])?;
    let changed: String = (0..5)
        .map(|index| format!("changed line {index}\n"))
        .collect();
    fs::write(&tracked, changed).map_err(|error| format!("fixture: {error}"))?;
    fs::write(repo.join("new.txt"), "new\n").map_err(|error| format!("fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(tracked.clone()),
        repo.join("session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    for _ in 0..50 {
        harness.frame()?;
        if harness.app.state().git.status.is_repo() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let state = harness.app.state();
        if !state.git.status.is_repo() {
            return Err(format!("Git status did not arrive: {:?}", state.git.error));
        }
        let git_tab = state.dock_hits[crate::state::DockId::Left.index()]
            .iter()
            .find(|(panel, _)| *panel == crate::state::Panel::Git)
            .map(|(_, rect)| rect.center())
            .ok_or("no GIT sidebar tab")?;
        drop(state);
        harness.click(git_tab);
    }
    let git_frame = harness.frame()?;
    {
        let state = harness.app.state();
        let branch = format!(
            "GIT {} ↑{} ↓{}",
            state.git.status.branch, state.git.status.ahead, state.git.status.behind
        );
        let position = draw_position(&git_frame, &branch)
            .ok_or_else(|| format!("the footer did not paint the Git branch: {branch:?}"))?;
        if position.x > WIDTH * 0.5 {
            return Err(format!(
                "the Git branch is not on the footer's left side: {position:?}"
            ));
        }
    }
    {
        let state = harness.app.state();
        if state.sidebar_tab != "git" {
            return Err("clicking GIT did not switch the sidebar".into());
        }
        if !state
            .git
            .status
            .changes
            .iter()
            .any(|entry| entry.path == Path::new("tracked.txt"))
            || !state
                .git
                .status
                .changes
                .iter()
                .any(|entry| entry.path == Path::new("new.txt"))
        {
            return Err(format!(
                "Git panel status is missing files: {:?}",
                state.git.status
            ));
        }
    }
    let row = harness
        .app
        .state()
        .git
        .rows
        .iter()
        .find(|(path, _, _)| path == Path::new("tracked.txt"))
        .map(|(_, _, rect)| rect.center())
        .ok_or("Git panel has no tracked.txt row")?;
    harness.click(row);
    for _ in 0..50 {
        harness.frame()?;
        if !harness.app.state().git.diff_loading {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc().ok_or("no Git diff view")?;
        if !doc.is_read_only() || !doc.text().contains("+changed line 0") {
            return Err("Git diff did not open as a read-only editor view".into());
        }
        if !doc
            .view_key()
            .is_some_and(|key| key.starts_with("git:diff:"))
        {
            return Err("Git diff view has the wrong view key".into());
        }
        if !doc.text().contains("file preview") {
            return Err("Git diff view has no file preview".into());
        }
    }
    let frame = harness.frame()?;
    let diff_accent = crate::theme::Theme::new(Mode::Dark).p.accent.to_rgba8();
    if !draw_text_has_color(&frame, "+changed line 0", diff_accent) {
        return Err("Git diff additions are not highlighted".into());
    }
    let before_edit = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len());
    harness.type_text("x");
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.text().len())
        != before_edit
    {
        return Err("Git diff view accepted text input".into());
    }
    // Hover follows the painted refresh button exactly.
    harness.frame()?;
    let refresh = harness.app.state().git.refresh_rect;
    harness.event(InputEvent::PointerMove {
        position: refresh.center(),
    });
    harness.frame()?;
    if harness.app.state().git.hover != Some(refresh) {
        return Err("Git refresh hover rect is offset from the pointer".into());
    }
    harness.app.state_mut().close_git_diff();

    // Terminal service: PTY, vt100 screen, panel and close.
    harness.shortcut(
        Key::Character('`'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if !harness.app.state().terminal.open
        || harness
            .app
            .state()
            .terminal
            .active()
            .is_none_or(|session| session.session.is_none())
    {
        return Err("Ctrl+` did not open a PTY terminal".into());
    }
    harness
        .app
        .state_mut()
        .terminal_write(b"printf 'terminal-selfcheck \xe4\xb8\xad\xe6\x96\x87\n'");
    let mut seen = String::new();
    for _ in 0..100 {
        harness.frame()?;
        if let Some(parser) = harness.app.state().terminal.active_parser() {
            seen = parser.screen().contents();
        }
        if seen.contains("terminal-selfcheck") && seen.contains("中文") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !seen.contains("terminal-selfcheck") || !seen.contains("中文") {
        return Err(format!("terminal output was not parsed: {seen:?}"));
    }
    // Emoji output: a wide cell paints a sprite instead of a glyph.
    harness
        .app
        .state_mut()
        .terminal_write("printf '😀\\n'\n".as_bytes());
    let mut seen_emoji = false;
    for _ in 0..100 {
        harness.frame()?;
        if let Some(parser) = harness.app.state().terminal.active_parser() {
            if parser.screen().contents().contains('😀') {
                seen_emoji = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !seen_emoji {
        return Err("terminal emoji output was not parsed".into());
    }
    if harness.app.state().emoji.available() {
        let frame = harness.frame()?;
        let body = harness.app.state().terminal.rect;
        let images = frame
            .commands()
            .iter()
            .filter(|command| match command {
                DrawCommand::DrawImage { destination, .. } => destination.intersects(body),
                _ => false,
            })
            .count();
        if images == 0 {
            return Err("the terminal painted no emoji sprite".into());
        }
    }
    let frame = harness.frame()?;
    let accent = crate::theme::Theme::new(Mode::Dark).p.accent.to_rgba8();
    let faint = crate::theme::Theme::new(Mode::Dark).p.faint.to_rgba8();
    if !draw_text_has_color(&frame, "TERMINAL", accent) {
        return Err("focused terminal header was not accented".into());
    }
    // The app header owns the project name; the terminal header must not paint
    // the session cwd over the session tabs.
    {
        let state = harness.app.state();
        let header = state.terminal.header_rect;
        let folder = state.ws.root.display().to_string();
        let name = state
            .ws
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for command in frame.commands() {
            if let DrawCommand::DrawText { text, position, .. } = command {
                if header.contains(*position)
                    && (text.contains(&folder) || (!name.is_empty() && text.contains(&name)))
                {
                    return Err(format!(
                        "terminal header repeats the project folder name: {text:?}"
                    ));
                }
            }
        }
    }
    // The parser grid is the source of truth, and every cell is painted on a
    // fixed column grid (mixing per-glyph advances with the vt100 columns was
    // the visible terminal misalignment).
    {
        let state = harness.app.state();
        let parser_size = state.terminal.active_parser().map(|p| p.screen().size());
        if parser_size.map(|(rows, cols)| (rows.get(), cols.get()))
            != Some((state.terminal.active_rows(), state.terminal.active_cols()))
        {
            return Err(format!(
                "terminal grid mismatch after open: parser={parser_size:?} cached=({}, {})",
                state.terminal.active_rows(),
                state.terminal.active_cols()
            ));
        }
        let cell_w = state.terminal.cell_w;
        if cell_w < 1.0 {
            return Err("terminal cell width was not recorded".into());
        }
        let body = state.terminal.rect;
        let terminal_font = state.editor_metrics().font;
        let mut cells = 0;
        let mut wide = 0;
        for command in frame.commands() {
            let DrawCommand::DrawText {
                text,
                position,
                family,
                font_size,
                ..
            } = command
            else {
                continue;
            };
            if !body.contains(*position) {
                continue;
            }
            cells += 1;
            // Narrow cells start exactly on a column. Wide cells are centered
            // inside their two columns, so their origin sits at most half a
            // column past the grid line.
            let columns = (position.x - body.left()) / cell_w;
            let on_grid = (columns - columns.round()).abs() <= 0.5;
            if !on_grid {
                return Err(format!(
                    "terminal text is outside its cell: x={} cell_w={cell_w} left={}",
                    position.x,
                    body.left()
                ));
            }
            // Narrow cells use the resolved fixed-pitch family; wide cells
            // name the UI family (`None`) so CJK comes from the right script.
            // Without a resolved fixed-pitch family (headless measurer) every
            // cell is `None`, so only the family check is meaningful here.
            if let Some(mono) = state.terminal.font.as_deref() {
                if family.is_none() {
                    wide += 1;
                    if (*font_size - terminal_font).abs() > 0.01 {
                        return Err(format!(
                            "wide terminal glyph {text:?} changed size: {font_size} (base {terminal_font})"
                        ));
                    }
                } else if family.as_deref() != Some(mono) {
                    return Err(format!(
                        "terminal text family {family:?} is neither {mono:?} nor the UI family"
                    ));
                }
            }
        }
        if cells == 0 {
            return Err("terminal output produced no text cells".into());
        }
        if state.terminal.font.is_some() && wide == 0 {
            return Err("terminal output produced no wide cells".into());
        }
    }
    // Force a fixed-pitch family: the headless measurer is approximate (its
    // space advance differs), so it cannot resolve one on its own. This proves
    // the resolved family reaches every terminal draw.
    {
        let mut state = harness.app.state_mut();
        state.terminal.font = Some("Menlo".into());
        state.terminal.font_checked = true;
    }
    let forced = harness.frame()?;
    {
        let state = harness.app.state();
        let body = state.terminal.rect;
        let terminal_font = state.editor_metrics().font;
        let mut cells = 0;
        let mut mono = 0;
        let mut wide = 0;
        for command in forced.commands() {
            let DrawCommand::DrawText {
                text,
                position,
                family,
                font_size,
                ..
            } = command
            else {
                continue;
            };
            if !body.contains(*position) {
                continue;
            }
            cells += 1;
            match family.as_deref() {
                Some("Menlo") => mono += 1,
                // Wide cells deliberately use the UI family (`None`) at the
                // same size as the Latin cells.
                None => {
                    if (*font_size - terminal_font).abs() > 0.01 {
                        return Err(format!(
                            "wide terminal glyph {text:?} changed size: {font_size} (base {terminal_font})"
                        ));
                    }
                    wide += 1;
                }
                other => {
                    return Err(format!("terminal draw lost its family: {other:?}"));
                }
            }
        }
        if cells == 0 || mono == 0 || wide == 0 {
            return Err(format!(
                "terminal output produced no monospace/wide cells: cells={cells} mono={mono} wide={wide}"
            ));
        }
    }
    let handle = harness.app.state().terminal.handle_rect.center();
    let height_before = harness.app.state().terminal.height;
    harness.drag(handle, Vec2::new(handle.x, handle.y - 30.0));
    harness.frame()?;
    if harness.app.state().terminal.height <= height_before {
        return Err("terminal handle drag did not increase its height".into());
    }
    if let Some(caret) = harness.app.caret() {
        if !harness.app.state().terminal.rect.contains(caret.center()) {
            return Err(format!(
                "terminal IME caret is not at the terminal cursor: caret={caret:?} body={:?}",
                harness.app.state().terminal.rect
            ));
        }
    } else {
        return Err("terminal has no IME caret rect".into());
    }
    let editor_center = harness.app.state().editor.text_rect.center();
    harness.click(editor_center);
    let frame = harness.frame()?;
    if harness.app.state().focus != Focus::Editor || !draw_text_has_color(&frame, "TERMINAL", faint)
    {
        return Err("clicking the editor did not dim the terminal focus".into());
    }
    let terminal_center = harness.app.state().terminal.rect.center();
    harness.click(terminal_center);
    let frame = harness.frame()?;
    if harness.app.state().focus != Focus::Terminal
        || !draw_text_has_color(&frame, "TERMINAL", accent)
    {
        return Err("clicking the terminal did not restore its accent".into());
    }
    harness.shortcut(
        Key::Character('`'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    if harness.app.state().terminal.open {
        return Err("Ctrl+` did not close the terminal".into());
    }
    // Reopening the dock restores the hidden shell. The first paint must
    // re-sync the parser/PTY with the current panel instead of trusting the
    // cached rows/cols (that left the prompt outside the drawn area).
    harness.shortcut(
        Key::Character('`'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state.terminal.open
            || state
                .terminal
                .active()
                .is_none_or(|session| session.session.is_none())
        {
            return Err("reopening the terminal did not spawn a session".into());
        }
        let parser_size = state.terminal.active_parser().map(|p| p.screen().size());
        if parser_size.map(|(rows, cols)| (rows.get(), cols.get()))
            != Some((state.terminal.active_rows(), state.terminal.active_cols()))
        {
            return Err(format!(
                "terminal grid mismatch after reopen: parser={parser_size:?} cached=({}, {})",
                state.terminal.active_rows(),
                state.terminal.active_cols()
            ));
        }
    }

    // Multiple shell sessions: + adds a tab, tab 1 switches back, × closes.
    harness.frame()?;
    let new_hit = harness.app.state().terminal.new_hit.center();
    let first_tab = harness
        .app
        .state()
        .terminal
        .tab_hits
        .first()
        .map(|(_, rect)| rect.center())
        .ok_or_else(|| "terminal session tab was not painted".to_string())?;
    harness.click(new_hit);
    harness.frame()?;
    if harness.app.state().terminal.sessions.len() != 2 || harness.app.state().terminal.active != 1
    {
        return Err("terminal + did not open a second session".into());
    }
    harness.click(first_tab);
    harness.frame()?;
    if harness.app.state().terminal.active != 0 {
        return Err("terminal tab click did not select the first session".into());
    }
    let close_hit = harness.app.state().terminal.close_hit.center();
    harness.click(close_hit);
    harness.frame()?;
    if harness.app.state().terminal.sessions.len() != 1 {
        return Err("terminal × did not close the active session".into());
    }

    // Bottom dock: PROBLEMS and TERMINAL share one dock, and neither switching
    // tabs nor hiding the dock may kill a shell.
    let session_count = harness.app.state().terminal.sessions.len();
    let bottom_tab = |harness: &Harness, panel: crate::state::Panel| -> Result<Vec2, String> {
        harness.app.state().dock_hits[crate::state::DockId::Bottom.index()]
            .iter()
            .find(|(candidate, _)| *candidate == panel)
            .map(|(_, rect)| rect.center())
            .ok_or_else(|| format!("the bottom dock has no {panel:?} tab"))
    };
    let problems_tab = bottom_tab(&harness, crate::state::Panel::Problems)?;
    let terminal_tab = bottom_tab(&harness, crate::state::Panel::Terminal)?;
    if problems_tab.x >= terminal_tab.x {
        return Err("PROBLEMS must be the bottom dock's first tab".into());
    }
    harness.click(problems_tab);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.bottom_tab != crate::state::Panel::Problems || !state.terminal.open {
            return Err("clicking PROBLEMS did not switch the bottom dock".into());
        }
        if state.terminal.sessions.len() != session_count {
            return Err("switching to PROBLEMS killed a shell".into());
        }
    }
    {
        // The dock body must have swapped, not just the tab label.
        let state = harness.app.state();
        if state.problems.rect.width() <= 0.0 {
            return Err("the PROBLEMS dock body was not painted".into());
        }
        if !texts(&frame)
            .iter()
            .any(|text| text == "no problems detected")
        {
            return Err("the PROBLEMS dock body is empty for the wrong reason".into());
        }
    }
    let terminal_tab = bottom_tab(&harness, crate::state::Panel::Terminal)?;
    harness.click(terminal_tab);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.bottom_tab != crate::state::Panel::Terminal {
            return Err("clicking TERMINAL did not switch back".into());
        }
        if state.terminal.sessions.len() != session_count {
            return Err("switching back to TERMINAL killed a shell".into());
        }
    }
    let dock_close = harness.app.state().dock_close[crate::state::DockId::Bottom.index()].center();
    harness.click(dock_close);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.terminal.open {
            return Err("the dock close button did not hide the bottom dock".into());
        }
        if state.terminal.sessions.len() != session_count {
            return Err("hiding the bottom dock killed its shells".into());
        }
        if state.session.terminal_open {
            return Err("hiding the bottom dock must persist terminal_open=false".into());
        }
    }
    harness.shortcut(
        Key::Character('`'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state.terminal.open || state.bottom_tab != crate::state::Panel::Terminal {
            return Err("Ctrl+` did not reopen the Terminal dock tab".into());
        }
        if state.terminal.sessions.len() != session_count {
            return Err("reopening the dock spawned a second shell".into());
        }
    }
    Ok(())
}

fn check_keymaps(dir: &Path) -> Result<(), String> {
    let path = dir.join("keymap.txt");
    fs::write(&path, "one\ntwo\nthree\n").map_err(|error| format!("keymap fixture: {error}"))?;
    let config = dir.join("keymap-session.conf");
    let mut harness = Harness::new(Some(path), config, ScriptedDialogs::new(vec![]));

    let set_keymap = |harness: &mut Harness, id: KeymapId| {
        let mut state = harness.app.state_mut();
        state.settings.keymap = id;
        state.keymap = crate::keymap::Keymap::for_id(id);
        state.vim.reset_for_keymap(id == KeymapId::Vim);
    };
    let primary = if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    };
    let set_caret = |harness: &mut Harness, line: usize, column: usize| {
        if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
            doc.set_caret(doc.byte_at(line, column));
        }
    };

    // VS Code: Ctrl/Cmd+Shift+K deletes the current line.
    set_keymap(&mut harness, KeymapId::VsCode);
    set_caret(&mut harness, 1, 1);
    harness.shortcut(
        Key::Character('k'),
        Modifiers {
            shift: true,
            ..primary
        },
    );
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text().contains("two"))
    {
        return Err("VS Code Cmd+Shift+K did not delete the line".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.undo();
        doc.set_caret(0);
    }

    // JetBrains: Cmd+D duplicates the line, Cmd+Backspace deletes it.
    set_keymap(&mut harness, KeymapId::JetBrains);
    set_caret(&mut harness, 0, 1);
    harness.shortcut(Key::Character('d'), primary);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text() != "one\none\ntwo\nthree\n")
    {
        return Err("JetBrains Cmd+D did not duplicate the line".into());
    }
    harness.shortcut(Key::Backspace, primary);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text() != "one\ntwo\nthree\n")
    {
        return Err("JetBrains Cmd+Backspace did not delete the line".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.undo();
        doc.set_caret(0);
    }

    // Vim: Normal is the initial mode; `i` enters Insert, Esc returns, `dd`
    // removes a line, `u` restores it, and plain text is ignored in Normal.
    set_keymap(&mut harness, KeymapId::Vim);
    set_caret(&mut harness, 0, 0);
    harness.key(Key::Character('i'));
    harness.type_text("i"); // the host's committed text for the command key
    harness.type_text("X");
    harness.key(Key::Escape);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| !doc.text().starts_with("Xone"))
    {
        return Err("Vim i/Insert did not insert text".into());
    }
    let before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map(|doc| doc.text().to_string())
        .unwrap_or_default();
    harness.type_text("Z");
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text() != before)
    {
        return Err("Vim Normal mode let text through".into());
    }
    set_caret(&mut harness, 0, 0);
    harness.key(Key::Character('d'));
    harness.key(Key::Character('d'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text().starts_with("Xone"))
    {
        return Err("Vim dd did not delete the line".into());
    }
    harness.key(Key::Character('u'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text() != before)
    {
        return Err("Vim u did not undo the line delete".into());
    }

    // Vim motions and VisualLine delete.
    harness.key(Key::Character('G'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 + 1 != doc.line_count())
    {
        return Err("Vim G did not move to the last line".into());
    }
    harness.key(Key::Character('g'));
    harness.key(Key::Character('g'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 != 0)
    {
        return Err("Vim gg did not move to the first line".into());
    }
    harness.key(Key::Character('V'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.selected_text() != Some("Xone\n"))
    {
        return Err("Vim V did not select the line".into());
    }
    harness.key(Key::Character('d'));
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.text().starts_with("Xone"))
    {
        return Err("Vim VisualLine d did not delete the line".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.undo();
    }

    // Direct tab access and cycling.
    harness.shortcut(Key::Character('n'), primary);
    harness.shortcut(Key::Character('n'), primary);
    harness.frame()?;
    let tabs = harness.app.state().ws.tabs.len();
    harness.shortcut(Key::Character('2'), primary);
    harness.frame()?;
    if harness.app.state().ws.active != 1 {
        return Err("Cmd/Ctrl+2 did not select the second tab".into());
    }
    harness.shortcut(Key::Character('9'), primary);
    harness.frame()?;
    if harness.app.state().ws.active != tabs - 1 {
        return Err("Cmd/Ctrl+9 did not select the last tab".into());
    }
    let tab_modifiers = if cfg!(target_os = "macos") {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    } else {
        primary
    };
    harness.shortcut(Key::Tab, tab_modifiers);
    harness.frame()?;
    if harness.app.state().ws.active != 0 {
        return Err("Next Tab did not wrap to the first tab".into());
    }
    harness.shortcut(
        Key::Tab,
        Modifiers {
            shift: true,
            ..tab_modifiers
        },
    );
    harness.frame()?;
    if harness.app.state().ws.active != tabs - 1 {
        return Err("Previous Tab did not wrap to the last tab".into());
    }

    // Settings row cycles to the next profile and persists its session value.
    let mut state = harness.app.state_mut();
    let before = state.settings.keymap;
    state.adjust_setting(Row::Keymap, 1);
    if state.settings.keymap == before
        || state.session.keymap != state.settings.keymap.session_value()
    {
        return Err("Keymap setting did not cycle/persist".into());
    }
    Ok(())
}

/// Go to Line through the real shortcut: prefill, digit typing, Enter jumps,
/// Esc leaves the caret unchanged, and invalid input stays inline.
fn check_goto_line(dir: &Path) -> Result<(), String> {
    let path = dir.join("goto.txt");
    fs::write(&path, "alpha\nbeta\ngamma\ndelta")
        .map_err(|error| format!("goto fixture: {error}"))?;
    let config = dir.join("goto-session.conf");
    let mut harness = Harness::new(Some(path), config, ScriptedDialogs::new(vec![]));

    let (key, modifiers) = if cfg!(target_os = "macos") {
        (
            Key::Character('g'),
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        )
    } else {
        (
            Key::Character('l'),
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        )
    };
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        let start = doc.line_start(1);
        doc.set_caret(start);
    }
    harness.shortcut(key, modifiers);
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state.goto_line.open {
            return Err(format!("{key:?} did not open the Go to Line prompt"));
        }
        if state.goto_line.field.text() != "2" {
            return Err(format!(
                "Go to Line did not prefill the caret line: {:?}",
                state.goto_line.field.text()
            ));
        }
    }
    harness.type_text("4");
    harness.key(Key::Enter);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.goto_line.open {
            return Err(format!(
                "Go to Line stayed open: {:?}",
                state.goto_line.error
            ));
        }
        let doc = state.ws.active_doc().ok_or("no active document")?;
        if doc.line_col().0 != 3 || doc.caret() != doc.line_start(3) {
            return Err(format!(
                "Go to Line did not move to line 4 (line {}, caret {})",
                doc.line_col().0 + 1,
                doc.caret()
            ));
        }
    }
    expect(&texts(&frame), "line 4", "Go to Line status")?;

    // Esc closes without moving the caret.
    let before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map(|doc| doc.caret())
        .ok_or("no active document")?;
    harness.shortcut(key, modifiers);
    harness.frame()?;
    harness.key(Key::Escape);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.goto_line.open {
            return Err("Esc did not close the Go to Line prompt".into());
        }
        if state.ws.active_doc().map(|doc| doc.caret()) != Some(before) {
            return Err("Esc moved the caret".into());
        }
    }

    // Invalid input stays open with an inline error.
    harness.shortcut(key, modifiers);
    harness.frame()?;
    harness.type_text("99");
    harness.key(Key::Enter);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.goto_line.open || state.goto_line.error.as_deref() != Some("line must be 1..=4") {
            return Err(format!(
                "out-of-range Go to Line did not stay inline (error {:?})",
                state.goto_line.error
            ));
        }
        if state.ws.active_doc().map(|doc| doc.caret()) != Some(before) {
            return Err("an invalid Go to Line moved the caret".into());
        }
    }
    expect(
        &texts(&frame),
        "line must be 1..=4",
        "Go to Line inline error",
    )?;
    harness.key(Key::Escape);
    harness.frame()?;
    Ok(())
}

/// App overlays are painted above the tree but are not scene nodes. While one
/// is open it must own the pointer: no hover on the editor/tabs/sidebar/footer
/// underneath, no delayed LSP tooltip on top of it, no wheel/right-click leak.
fn check_overlay_pointer(dir: &Path) -> Result<(), String> {
    let path = dir.join("pointer.txt");
    let text: String = (0..200).map(|line| format!("line {line}\n")).collect();
    fs::write(&path, text).map_err(|error| format!("pointer fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(path),
        dir.join("pointer-session.conf"),
        ScriptedDialogs::new(vec![]),
    );

    let (text_pos, tab_pos, theme_pos) = {
        let state = harness.app.state();
        let text_rect = state.editor.text_rect;
        let tab = state
            .tabs
            .hits
            .first()
            .ok_or("the fixture painted no tab hit")?
            .1
            .center();
        (
            Vec2::new(text_rect.left() + 20.0, text_rect.bottom() - 20.0),
            tab,
            state.theme_hit.center(),
        )
    };

    // Baseline: tab and editor hover work when nothing is above them.
    harness.event(InputEvent::PointerMove { position: tab_pos });
    if harness.app.state().tabs.hover != Some(0) {
        return Err("the baseline tab hover was not set".into());
    }
    harness.event(InputEvent::PointerMove { position: text_pos });
    {
        let state = harness.app.state();
        if state.tabs.hover.is_some() {
            return Err("leaving a tab did not clear its hover".into());
        }
        if state.hover_target.is_none() {
            return Err("the baseline editor hover target was not set".into());
        }
    }
    if harness.app.cursor() != igui_core::Cursor::Text {
        return Err("the editor did not ask for a text cursor".into());
    }

    // Quick Open floats over the editor: it owns the whole window.
    harness.app.state_mut().open_quick_open();
    harness.frame()?;
    harness.event(InputEvent::PointerMove { position: text_pos });
    harness.event(InputEvent::PointerMove {
        position: theme_pos,
    });
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state.quick_open.open {
            return Err("the pointer move closed Quick Open".into());
        }
        if state.hover_target.is_some() {
            return Err("the editor kept a hover target under Quick Open".into());
        }
        if state.tabs.hover.is_some() || state.sidebar.hover.is_some() {
            return Err("tab/sidebar hover updated under Quick Open".into());
        }
        if state.theme_hover || state.panel_hover.is_some() {
            return Err("footer/header hover updated under Quick Open".into());
        }
    }
    if harness.app.cursor() != igui_core::Cursor::Default {
        return Err("the cursor over an app overlay is not the default arrow".into());
    }

    // The wheel belongs to the prompt, not the editor behind it.
    let scroll_before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0.0, |doc| doc.scroll_y);
    harness.event(InputEvent::Wheel {
        position: text_pos,
        delta: Vec2::new(0.0, 120.0),
        phase: igui_core::TouchPhase::Moved,
        device: igui_core::ScrollDevice::Wheel,
    });
    let scroll_after = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0.0, |doc| doc.scroll_y);
    if scroll_after != scroll_before {
        return Err(format!(
            "the wheel scrolled the editor under Quick Open ({scroll_before} -> {scroll_after})"
        ));
    }

    // A right click on something under the prompt must not open a menu.
    harness.right_click(tab_pos);
    if harness.app.state().tab_menu.is_some() {
        return Err("a right click under Quick Open opened the tab menu".into());
    }

    // Closing the prompt restores normal hover ownership.
    harness.key(Key::Escape);
    harness.frame()?;
    harness.event(InputEvent::PointerMove { position: tab_pos });
    if harness.app.state().tabs.hover != Some(0) {
        return Err("tab hover did not resume after closing Quick Open".into());
    }

    // The full-window Settings panel behaves the same way.
    harness.app.state_mut().open_settings();
    harness.frame()?;
    harness.event(InputEvent::PointerMove { position: text_pos });
    {
        let state = harness.app.state();
        if state.hover_target.is_some() || state.theme_hover || state.tabs.hover.is_some() {
            return Err("hover updated through the settings panel".into());
        }
    }
    if harness.app.cursor() != igui_core::Cursor::Default {
        return Err("the cursor over settings is not the default arrow".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().settings_open {
        return Err("Esc did not close the settings panel".into());
    }
    Ok(())
}

/// Long fenced-code lines have no horizontal scroll in the preview: they must
/// wrap instead of being clipped at the pane edge (missing tail characters).
fn check_markdown_code_wrap(dir: &Path) -> Result<(), String> {
    let path = dir.join("wrap.md");
    let long: String = "x".repeat(240);
    fs::write(&path, format!("# Wrap\n\n```text\n{long}\n```\n"))
        .map_err(|error| format!("code-wrap fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(path),
        dir.join("wrap-session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    let preview_left = harness.app.state().preview.rect.left();
    let preview_right = harness.app.state().preview.rect.right();
    let measurer = igui_ui::text_measurer(harness.app.tree());
    let mut runs = 0usize;
    let mut joined = String::new();
    for command in frame.commands() {
        let DrawCommand::DrawText {
            text,
            position,
            font_size,
            family,
            ..
        } = command
        else {
            continue;
        };
        if position.x < preview_left || !text.chars().all(|ch| ch == 'x') || text.is_empty() {
            continue;
        }
        runs += 1;
        joined.push_str(text);
        let width = measurer.measure_run_weighted_in(
            text,
            *font_size,
            igui_core::FontWeight::NORMAL,
            family.as_deref(),
        );
        if position.x + width > preview_right - 16.0 {
            return Err(format!(
                "the preview clipped a {:.0}px code run at x={:.1}",
                width, position.x
            ));
        }
    }
    if runs < 2 {
        return Err("a long preview code line was not wrapped".into());
    }
    if joined != long {
        return Err(format!(
            "wrapping lost code bytes ({} of {} characters)",
            joined.chars().count(),
            long.chars().count()
        ));
    }
    Ok(())
}

fn check_file_icons(dir: &Path) -> Result<(), String> {
    // A deterministic theme: each definition is a solid colour, so the paint
    // output can be attributed to one row without depending on the installed
    // extension version.
    let theme_dir = dir.join("icon-theme");
    fs::create_dir_all(theme_dir.join("dist")).map_err(|error| format!("icon theme: {error}"))?;
    fs::create_dir_all(theme_dir.join("icons")).map_err(|error| format!("icon theme: {error}"))?;
    let svg = |color: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="{color}" d="M0 0h16v16H0z"/></svg>"#
        )
    };
    for (name, color) in [
        ("file.svg", "#808080"),
        ("rust.svg", "#ff0000"),
        ("markdown.svg", "#00ff00"),
        ("folder.svg", "#0000ff"),
        ("folder-open.svg", "#0000ff"),
    ] {
        fs::write(theme_dir.join("icons").join(name), svg(color))
            .map_err(|error| format!("icon fixture: {error}"))?;
    }
    fs::write(
        theme_dir.join("dist/material-icons.json"),
        r##"{
            "iconDefinitions": {
                "file": {"iconPath": "../icons/file.svg"},
                "rust": {"iconPath": "../icons/rust.svg"},
                "markdown": {"iconPath": "../icons/markdown.svg"},
                "folder": {"iconPath": "../icons/folder.svg"},
                "folder-open": {"iconPath": "../icons/folder-open.svg"}
            },
            "file": "file",
            "folder": "folder",
            "folderExpanded": "folder-open",
            "fileExtensions": {"rs": "rust", "md": "markdown"}
        }"##,
    )
    .map_err(|error| format!("icon theme fixture: {error}"))?;

    let project = dir.join("icons-project");
    fs::create_dir_all(project.join("src")).map_err(|error| format!("icons project: {error}"))?;
    fs::write(project.join("main.rs"), "fn main() {}\n")
        .map_err(|error| format!("icons project: {error}"))?;
    fs::write(project.join("notes.md"), "# Notes\n")
        .map_err(|error| format!("icons project: {error}"))?;

    let previous = std::env::var_os(crate::model::icon_theme::THEME_ENV);
    std::env::set_var(
        crate::model::icon_theme::THEME_ENV,
        theme_dir.join("dist/material-icons.json"),
    );
    let result = (|| -> Result<(), String> {
        let mut harness = Harness::new(
            None,
            dir.join("icons-session.conf"),
            ScriptedDialogs::new(vec![]),
        );
        harness.app.state_mut().open_project(project.clone());
        harness.frame()?;
        let frame = harness.frame()?;
        let sidebar = harness.app.state().sidebar.rect;
        let icon_colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [128, 128, 128, 255],
        ];
        let count_icons = |frame: &RecordedFrame| {
            frame
                .commands()
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        DrawCommand::FillRect { rect, paint }
                            if sidebar.intersects(*rect)
                                && rect.size.width <= crate::ui::fileicon::SIZE + 0.01
                                && icon_colors.contains(&paint.color.to_rgba8())
                    )
                })
                .count()
        };
        let painted = count_icons(&frame);
        // One merged span per solid square: main.rs (red), notes.md (green)
        // and the closed src folder (blue).
        let icon_rect = |frame: &RecordedFrame, color: [u8; 4]| {
            frame.commands().iter().find_map(|command| match command {
                DrawCommand::FillRect { rect, paint }
                    if sidebar.intersects(*rect)
                        && rect.size.width <= crate::ui::fileicon::SIZE + 0.01
                        && paint.color.to_rgba8() == color =>
                {
                    Some(*rect)
                }
                _ => None,
            })
        };
        let mut rects = Vec::new();
        for color in [&icon_colors[0], &icon_colors[1], &icon_colors[2]] {
            let Some(rect) = icon_rect(&frame, *color) else {
                return Err(format!(
                    "the Explorer did not paint the {color:?} icon (spans {painted}, theme {:?}, error {:?})",
                    harness.app.state().file_icons.source(),
                    harness.app.state().file_icons.error()
                ));
            };
            rects.push(rect);
        }
        let (file_icon, folder_icon) = (rects[0], rects[2]);
        // The fixed arrow slot means the icon column is the same for files and
        // folders: same left edge, same size, same vertical centre.
        if (file_icon.left() - folder_icon.left()).abs() > 0.01 {
            return Err(format!(
                "file icon left {} is not aligned with folder icon left {}",
                file_icon.left(),
                folder_icon.left()
            ));
        }
        // Labels follow the icon column with a gap and share the same x.
        let label_position = |frame: &RecordedFrame, needle: &str| {
            frame.commands().iter().find_map(|command| match command {
                DrawCommand::DrawText { text, position, .. }
                    if text == needle && sidebar.contains(*position) =>
                {
                    Some(*position)
                }
                _ => None,
            })
        };
        let Some(file_label) = label_position(&frame, "main.rs") else {
            return Err("main.rs label was not painted".into());
        };
        let Some(folder_label) = label_position(&frame, "src") else {
            return Err("src label was not painted".into());
        };
        if (file_label.x - folder_label.x).abs() > 0.01 {
            return Err(format!(
                "file label x {} is not aligned with folder label x {}",
                file_label.x, folder_label.x
            ));
        }
        if file_label.x < file_icon.right() + 5.5 {
            return Err(format!(
                "the label gap collapsed: icon right {}, label x {}",
                file_icon.right(),
                file_label.x
            ));
        }
        // The arrow keeps its own fixed slot for folders, and files leave it
        // empty (no line at that x on a file row).
        let arrow_lines = |frame: &RecordedFrame, row_y: f32| {
            frame
                .commands()
                .iter()
                .filter(|command| match command {
                    DrawCommand::Line { from, to, .. } => {
                        sidebar.contains(*from)
                            && sidebar.contains(*to)
                            && (from.y - row_y).abs() < 10.0
                            && (to.y - row_y).abs() < 10.0
                    }
                    _ => false,
                })
                .count()
        };
        if arrow_lines(&frame, folder_icon.center().y) == 0 {
            return Err("the folder row lost its disclosure arrow".into());
        }
        if arrow_lines(&frame, file_icon.center().y) != 0 {
            return Err("a file row painted a disclosure arrow".into());
        }
        // With icons off the tree returns to the compact chevron layout, but
        // the folder arrow stays in the same fixed slot.
        harness.app.state_mut().toggle_setting(Row::FileIcons);
        let off = harness.frame()?;
        if count_icons(&off) != 0 {
            return Err("File Icons off still painted theme artwork".into());
        }
        let Some(compact_label) = label_position(&off, "main.rs") else {
            return Err("main.rs label disappeared when icons were disabled".into());
        };
        if compact_label.x >= file_label.x - 10.0 {
            return Err(format!(
                "labels did not return to the compact layout ({} -> {})",
                file_label.x, compact_label.x
            ));
        }
        Ok(())
    })();
    match previous {
        Some(value) => std::env::set_var(crate::model::icon_theme::THEME_ENV, value),
        None => std::env::remove_var(crate::model::icon_theme::THEME_ENV),
    }
    result
}

fn check_emoji(dir: &Path) -> Result<(), String> {
    let path = dir.join("emoji.md");
    fs::write(&path, "# Smile 😀\n\nParty 🎉 time ❤️\n")
        .map_err(|error| format!("emoji fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(path),
        dir.join("emoji-session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    let (available, font) = {
        let state = harness.app.state();
        (
            state.emoji.available(),
            state
                .emoji
                .font_path()
                .map(|path| path.display().to_string()),
        )
    };
    if !available {
        println!("selfcheck: no color emoji font installed; emoji paint skipped");
        return Ok(());
    }
    let (text_rect, preview_rect) = {
        let state = harness.app.state();
        (state.editor.text_rect, state.preview.rect)
    };
    if preview_rect.width() < 1.0 {
        return Err("the Markdown fixture did not open in Split view".into());
    }
    let mut editor_images = 0usize;
    let mut preview_images = 0usize;
    for command in frame.commands() {
        match command {
            DrawCommand::DrawImage { destination, .. } => {
                if destination.intersects(text_rect) {
                    editor_images += 1;
                }
                if destination.intersects(preview_rect) {
                    preview_images += 1;
                }
            }
            DrawCommand::DrawText { text, position, .. }
                if (text_rect.contains(*position) || preview_rect.contains(*position))
                    && text
                        .chars()
                        .any(|ch| crate::model::emoji::is_emoji(ch, None)) =>
            {
                return Err(format!(
                    "the editor painted emoji as text instead of an image: {text:?}"
                ));
            }
            _ => {}
        }
    }
    if editor_images < 2 {
        return Err(format!(
            "emoji in the editor painted {editor_images} images (face: {})",
            font.unwrap_or_else(|| "unknown".into())
        ));
    }
    if preview_images < 2 {
        return Err(format!(
            "emoji in the preview painted {preview_images} images (face: {})",
            font.unwrap_or_else(|| "unknown".into())
        ));
    }
    Ok(())
}

fn check_markdown_outline(dir: &Path) -> Result<(), String> {
    let path = dir.join("outline.md");
    fs::write(
        &path,
        "# First heading\n\n## Second heading\n\n### Third heading\n",
    )
    .map_err(|error| format!("Markdown outline fixture: {error}"))?;
    let config = dir.join("outline-session.conf");
    let mut harness = Harness::new(Some(path), config, ScriptedDialogs::new(vec![]));

    let frame = harness.frame()?;
    let target = {
        let state = harness.app.state();
        if state.outline.rows.len() != 3 {
            return Err(format!(
                "Markdown outline painted {} heading rows",
                state.outline.rows.len()
            ));
        }
        state.outline.rows[1].1.center()
    };
    let keyword = crate::theme::Theme::new(Mode::Dark)
        .p
        .syntax
        .keyword
        .to_rgba8();
    if !draw_text_has_color(&frame, "H", keyword) {
        return Err("Markdown outline did not paint its heading glyph".into());
    }
    for heading in ["First heading", "Second heading", "Third heading"] {
        expect(&texts(&frame), heading, "Markdown outline row")?;
    }
    // The right dock's shared tab strip must name the panel, including at the
    // narrow window sizes a restored session can produce.
    expect(&texts(&frame), "OUTLINE", "outline dock tab")?;
    harness.viewport = ViewportSize::new(Size::new(945.0, 568.0));
    let narrow = harness.frame()?;
    expect(
        &texts(&narrow),
        "OUTLINE",
        "outline dock tab (narrow window)",
    )?;
    harness.viewport = ViewportSize::new(Size::new(WIDTH, HEIGHT));
    harness.frame()?;

    harness.click(target);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 != 2)
    {
        return Err("clicking a Markdown outline row did not jump to its heading".into());
    }
    Ok(())
}

/// Preview-only mode has no editor pane, so an outline click must scroll the
/// rendered Markdown to the heading instead of moving an invisible caret.
fn check_markdown_preview_outline_jump(dir: &Path) -> Result<(), String> {
    let path = dir.join("preview-outline.md");
    let mut text = String::from("# Top heading\n\n");
    for index in 0..40 {
        text.push_str(&format!(
            "Filler {index} keeps the heading below the fold.\n\n"
        ));
    }
    text.push_str("## Middle heading\n\n");
    for index in 0..40 {
        text.push_str(&format!("Tail {index} keeps the preview scrollable.\n\n"));
    }
    fs::write(&path, &text).map_err(|error| format!("preview outline fixture: {error}"))?;
    let config = dir.join("preview-outline-session.conf");
    let mut harness = Harness::new(Some(path), config, ScriptedDialogs::new(vec![]));

    harness.frame()?;
    {
        let mut state = harness.app.state_mut();
        state.set_view(ViewMode::Preview);
        state.preview.scroll = 0.0;
    }
    let before = harness.frame()?;
    let (target, pane) = {
        let state = harness.app.state();
        if state.built_view != ViewMode::Preview {
            return Err("switching to Preview Only did not mount the preview pane".into());
        }
        if state.preview.total_h <= state.preview.rect.height() {
            return Err(format!(
                "preview outline fixture does not scroll ({} <= {})",
                state.preview.total_h,
                state.preview.rect.height()
            ));
        }
        let row = state
            .outline
            .rows
            .iter()
            .find(|(index, _)| *index == 1)
            .map(|(_, rect)| rect.center())
            .ok_or("the preview outline painted no second heading row")?;
        (row, state.preview.rect)
    };
    if text_y_in(&before, "Middle", pane).is_some() {
        return Err("the fixture's second heading is already visible in the pane".into());
    }

    harness.click(target);
    let after = harness.frame()?;
    let scroll = harness.app.state().preview.scroll;
    if scroll <= 0.0 {
        return Err(format!(
            "clicking a preview outline row did not scroll the pane (scroll {scroll})"
        ));
    }
    let y = text_y_in(&after, "Middle", pane)
        .ok_or("the outline jump did not paint the heading inside the pane")?;
    if y > pane.top() + pane.height() * 0.25 {
        return Err(format!(
            "the outline jump must put the heading near the pane top (y={y}, top={})",
            pane.top()
        ));
    }
    Ok(())
}

/// Minimap: recorded geometry, painted ink, click/drag mapping, soft wrap,
/// auto-hide and the persisted toggle./// Every text surface must tell the host where its caret is: the app overlays
/// are not scene nodes, so the platform IME window can only follow a rectangle
/// each painter records.
fn check_ime_caret(dir: &Path) -> Result<(), String> {
    let path = dir.join("ime-caret.txt");
    fs::write(&path, "alpha\nbeta\ngamma\n")
        .map_err(|error| format!("ime caret fixture: {error}"))?;
    let config = dir.join("ime-caret-session.conf");
    let mut harness = Harness::new(Some(path), config, ScriptedDialogs::new(vec![]));
    harness.frame()?;

    let editor_caret = harness
        .app
        .caret()
        .ok_or("the editor reported no IME caret")?;
    {
        let state = harness.app.state();
        if !state.editor.text_rect.contains(editor_caret.center()) {
            return Err(format!(
                "the editor IME caret {editor_caret:?} is outside the text area {:?}",
                state.editor.text_rect
            ));
        }
    }

    fn check(host: Option<Rect>, field: Rect, area: Rect, what: &str) -> Result<(), String> {
        let Some(host) = host else {
            return Err(format!("{what}: the host has no IME caret"));
        };
        if field.width() <= 0.0 || field.height() <= 0.0 {
            return Err(format!("{what}: the field recorded no caret rectangle"));
        }
        if host != field {
            return Err(format!(
                "{what}: the host IME caret {host:?} does not follow the field {field:?}"
            ));
        }
        if !area.contains(host.center()) {
            return Err(format!(
                "{what}: the IME caret {host:?} is outside its field {area:?}"
            ));
        }
        Ok(())
    }

    // Quick Open palette.
    harness.app.state_mut().open_quick_open();
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::quick_open::layout(
            state.viewport.logical_rect(),
            state.quick_open.results.len(),
        )
        .query;
        check(
            harness.app.caret(),
            state.quick_open.caret_rect,
            area,
            "Quick Open",
        )?;
    }
    harness.app.state_mut().close_quick_open();

    // Find bar (its two fields report separate carets).
    harness.app.state_mut().open_find(false);
    harness.frame()?;
    {
        let state = harness.app.state();
        let field = state.find.find_caret;
        check(
            harness.app.caret(),
            field,
            state.viewport.logical_rect(),
            "Find",
        )?;
        if field == editor_caret {
            return Err("the Find bar kept the editor's IME caret".into());
        }
    }
    harness.app.state_mut().close_find();

    // Project search.
    harness.app.state_mut().open_project_search();
    harness.frame()?;
    {
        let state = harness.app.state();
        check(
            harness.app.caret(),
            state.search.caret_rect,
            state.viewport.logical_rect(),
            "Project Search",
        )?;
    }
    harness.app.state_mut().close_project_search();

    // Go to Line.
    harness.app.state_mut().open_goto_line();
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::goto_line::layout(state.viewport.logical_rect()).field;
        check(
            harness.app.caret(),
            state.goto_line.caret_rect,
            area,
            "Go to Line",
        )?;
    }
    harness.app.state_mut().close_goto_line();

    // Quick-create prompt.
    harness.app.state_mut().open_new_file_in(dir.to_path_buf());
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::new_file::layout(state.viewport.logical_rect()).field;
        check(
            harness.app.caret(),
            state.new_file.caret_rect,
            area,
            "New File",
        )?;
    }
    harness.app.state_mut().close_new_file();

    // Symbol rename overlay.
    {
        let mut state = harness.app.state_mut();
        state.rename_symbol.open = true;
        state.rename_symbol.old_name = "alpha".into();
        state.rename_symbol.name = igui_ui::TextEdit::new("beta");
        state.rename_symbol.error = None;
    }
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::rename_symbol::layout(state.viewport.logical_rect()).field;
        check(
            harness.app.caret(),
            state.rename_symbol.caret_rect,
            area,
            "Rename Symbol",
        )?;
    }
    harness.app.state_mut().close_rename_symbol();

    // Settings panel: the search field owns text input until a number is typed.
    harness.app.state_mut().open_settings();
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::settings::layout(&state).search;
        check(
            harness.app.caret(),
            state.settings_ui.search_caret,
            area,
            "Settings search",
        )?;
    }
    harness.type_text("font");
    harness.frame()?;
    harness.key(Key::Enter); // Font Size starts a typed edit
    harness.frame()?;
    {
        let state = harness.app.state();
        let area = crate::ui::settings::layout(&state)
            .rows
            .iter()
            .find(|slot| slot.spec.row == Row::FontSize)
            .map(|slot| slot.value)
            .ok_or("no Font Size row")?;
        check(
            harness.app.caret(),
            state.settings_ui.edit_caret,
            area,
            "Settings number edit",
        )?;
    }
    harness.key(Key::Escape); // cancel the typed number
    harness.key(Key::Escape); // clear the search query
    harness.key(Key::Escape); // close the panel
    harness.frame()?;

    // Closing every overlay returns the caret to the editor.
    if harness.app.caret() != Some(editor_caret) {
        return Err(format!(
            "the IME caret did not return to the editor: {:?} != {editor_caret:?}",
            harness.app.caret()
        ));
    }
    Ok(())
}

fn check_minimap(dir: &Path) -> Result<(), String> {
    use crate::model::minimap as map_model;

    let map_dir = dir.join("minimap");
    fs::create_dir_all(&map_dir).map_err(|error| format!("minimap fixture: {error}"))?;
    let mut text = String::new();
    for index in 0..400 {
        let indent = " ".repeat((index % 4) * 4);
        text.push_str(&format!(
            "{indent}const value_{index} = \"{}\";\n",
            "x".repeat(index % 30)
        ));
    }
    let path = map_dir.join("minimap.js");
    fs::write(&path, &text).map_err(|error| format!("minimap fixture: {error}"))?;
    let config = map_dir.join("session.conf");
    let mut harness = Harness::new(Some(path), config.clone(), ScriptedDialogs::new(vec![]));
    let frame = harness.frame()?;

    let (map, text_rect, slider, line_h, line_count, content_h, view_h) = {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document for the minimap check")?;
        (
            state.editor.minimap.rect,
            state.editor.text_rect,
            state.editor.minimap.slider,
            state.editor_metrics().line_h,
            doc.line_count(),
            state.editor.content_h,
            state.editor.view_h,
        )
    };
    if map.is_empty() {
        return Err("the minimap was not painted for a normal window".into());
    }
    if (map.left() - text_rect.right()).abs() > 0.5 {
        return Err(format!(
            "the minimap does not start at the text area edge ({:.1} vs {:.1})",
            map.left(),
            text_rect.right()
        ));
    }
    {
        let state = harness.app.state();
        if (map.right() - state.editor.vbar.left()).abs() > 0.5 {
            return Err("the minimap does not end at the vertical scrollbar".into());
        }
    }
    let mut inked = 0usize;
    let mut ink_left = f32::MAX;
    let mut ink_top = f32::MAX;
    let mut ink_bottom = f32::MIN;
    let mut ink_height = 0.0_f32;
    let mut ink_rows: Vec<u32> = Vec::new();
    for command in frame.commands() {
        let DrawCommand::FillRect { rect, .. } = command else {
            continue;
        };
        // Skip the map background (full strip) and the 1 px left rule; count
        // the actual ink runs.
        if rect.width() <= 1.0
            || rect.width() > map.width() - 0.5
            || !map.contains_rect(*rect)
            || (!slider.is_empty() && *rect == slider)
        {
            continue;
        }
        inked += 1;
        ink_left = ink_left.min(rect.left());
        ink_top = ink_top.min(rect.top());
        ink_bottom = ink_bottom.max(rect.bottom());
        ink_height = ink_height.max(rect.height());
        ink_rows.push((rect.top() * 4.0) as u32);
    }
    if inked < 20 {
        return Err(format!("the minimap painted only {inked} content rects"));
    }
    if inked > 4200 {
        return Err(format!(
            "the minimap painted {inked} rects, over the draw-command cap"
        ));
    }
    // The fixture starts at column 0 and fills the document, so the ink must
    // reach the strip's left edge and both ends of its content box.
    if ink_left > map.left() + 2.0 {
        return Err(format!(
            "minimap ink starts at {:.1}, expected the strip edge {:.1}",
            ink_left,
            map.left()
        ));
    }
    if ink_top > map.top() + 3.0 || ink_bottom < map.bottom() - 3.0 {
        return Err(format!(
            "minimap ink does not span the strip (top {ink_top:.1}, bottom {ink_bottom:.1}, map {:.1}..={:.1})",
            map.top(),
            map.bottom()
        ));
    }
    // Every row leaves a 1 px gap before the next one.
    let map_content_h = map_model::content_height(line_count, map.height());
    let map_row_h =
        map_content_h / crate::model::minimap::bucket_count(line_count, map.height()) as f32;
    if ink_height > map_row_h - 0.5 {
        return Err(format!(
            "minimap rows are not separated (ink {ink_height:.1} px, row {map_row_h:.1} px)"
        ));
    }
    // Several runs on one row: a line must not collapse into one solid bar.
    ink_rows.sort_unstable();
    if !ink_rows.windows(2).any(|window| window[0] == window[1]) {
        return Err("the minimap paints one solid block per row".into());
    }
    if slider.is_empty() || !map.contains_rect(slider) {
        return Err("the minimap viewport slider is missing or outside the strip".into());
    }
    {
        // The JS fixture has keywords and strings on every line, so the raster
        // that paint reads must contain styled cells and internal gaps.
        let mut state = harness.app.state_mut();
        let rows = crate::model::minimap::bucket_count(line_count, map.height());
        let cells = (map.width() - 1.0).ceil().max(1.0) as usize;
        let raster = state.editor.minimap.cache.raster(rows, cells);
        if !raster
            .iter()
            .any(|&code| code != 0 && crate::model::minimap::kind_of(code).is_styled())
        {
            return Err("the minimap raster carries no syntax color".into());
        }
        let first = raster.iter().position(|&code| code != 0);
        let last = raster.iter().rposition(|&code| code != 0);
        if !first
            .zip(last)
            .is_some_and(|(first, last)| raster[first..=last].contains(&0))
        {
            return Err("the minimap raster lost its horizontal gaps".into());
        }
    }

    // Clicking the strip scrolls the clicked line to the top and keeps the
    // caret where it was.
    let caret_before = harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.caret());
    let click_at = Vec2::new(map.center().x, map.top() + map.height() * 0.5);
    let expected_line = map_model::line_at(
        click_at.y - map.top(),
        line_count,
        map_model::content_height(line_count, map.height()),
    );
    harness.click(click_at);
    harness.frame()?;
    let max_scroll = (content_h - view_h).max(0.0);
    let expected_scroll = (expected_line as f32 * line_h).min(max_scroll);
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document after the minimap click")?;
        if (doc.scroll_y - expected_scroll).abs() > 1.0 {
            return Err(format!(
                "minimap click scrolled to {:.1}, expected {:.1} (line {expected_line})",
                doc.scroll_y, expected_scroll
            ));
        }
        if doc.caret() != caret_before {
            return Err("clicking the minimap moved the caret".into());
        }
        if state.editor.minimap.first_line != expected_line {
            return Err(format!(
                "minimap first_line is {}, expected {expected_line}",
                state.editor.minimap.first_line
            ));
        }
    }

    // Dragging from the strip scrolls to the end and clamps.
    let from = harness.app.state().editor.minimap.slider.center();
    let to = Vec2::new(map.center().x, map.bottom() - 1.0);
    harness.drag(from, to);
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no document after the minimap drag")?;
        let max_scroll = (state.editor.content_h - state.editor.view_h).max(0.0);
        if (doc.scroll_y - max_scroll).abs() > 1.0 {
            return Err(format!(
                "minimap drag ended at {:.1}, expected the clamp {:.1}",
                doc.scroll_y, max_scroll
            ));
        }
        if doc.caret() != caret_before {
            return Err("dragging the minimap moved the caret".into());
        }
    }

    // The toggle removes the strip, widens the text area, persists, and the
    // shortcut is the same command.
    let shortcut = Modifiers {
        meta: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        alt: true,
        ..Modifiers::NONE
    };
    harness.shortcut(Key::Character('m'), shortcut);
    harness.frame()?;
    let widened = {
        let state = harness.app.state();
        if !state.editor.minimap.rect.is_empty() {
            return Err("the minimap toggle did not remove the strip".into());
        }
        if state.session.minimap {
            return Err("the minimap toggle was not persisted in the session".into());
        }
        state.editor.text_rect.width()
    };
    if widened <= text_rect.width() + map.width() * 0.5 {
        return Err("hiding the minimap did not widen the text area".into());
    }
    let restored = EditorApp::with_config(
        None,
        None,
        config.clone(),
        Box::new(ScriptedDialogs::new(vec![])),
    );
    if restored.state().session.minimap {
        return Err("the hidden minimap was not restored from the session".into());
    }

    // Auto-hide: a narrow window drops the strip without changing the toggle.
    let mut close = Harness::new(
        Some(map_dir.join("minimap.js")),
        map_dir.join("auto-hide.conf"),
        ScriptedDialogs::new(vec![]),
    );
    close.frame()?;
    close.viewport = ViewportSize::new(Size::new(640.0, 800.0));
    close.frame()?;
    {
        let state = close.app.state();
        if !state.editor.minimap.rect.is_empty() {
            return Err("the minimap stayed visible in a narrow window".into());
        }
        if !state.session.minimap {
            return Err("auto-hiding the minimap changed the persisted toggle".into());
        }
    }
    close.viewport = ViewportSize::new(Size::new(WIDTH, HEIGHT));
    close.frame()?;
    if close.app.state().editor.minimap.rect.is_empty() {
        return Err("the minimap did not return after widening the window".into());
    }

    // Soft wrap: the map scrolls by visual rows for the clicked line.
    let wrap_path = map_dir.join("minimap-wrap.txt");
    let mut wrapped = String::new();
    for index in 0..120 {
        wrapped.push_str(&format!("{index} {}\n", "word ".repeat(40)));
    }
    fs::write(&wrap_path, &wrapped).map_err(|error| format!("wrap fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(wrap_path),
        map_dir.join("wrap-session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    {
        let mut state = harness.app.state_mut();
        state.set_soft_wrap(true);
    }
    harness.frame()?;
    let (map, line_h, line_count, wrapped_rows) = {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no wrapped document for the minimap")?;
        (
            state.editor.minimap.rect,
            state.editor_metrics().line_h,
            doc.line_count(),
            state.editor.wrap.len(),
        )
    };
    if wrapped_rows <= line_count {
        return Err("soft wrap did not create visual rows for the minimap check".into());
    }
    if map.is_empty() {
        return Err("the minimap disappeared under soft wrap".into());
    }
    let click_at = Vec2::new(map.center().x, map.top() + map.height() * 0.4);
    let expected_line = map_model::line_at(
        click_at.y - map.top(),
        line_count,
        map_model::content_height(line_count, map.height()),
    );
    harness.click(click_at);
    harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no wrapped document after the minimap click")?;
        let max_scroll = (state.editor.content_h - state.editor.view_h).max(0.0);
        let expected =
            (state.editor.wrap.first_row_of_line(expected_line) as f32 * line_h).min(max_scroll);
        if (doc.scroll_y - expected).abs() > 1.0 {
            return Err(format!(
                "wrapped minimap click scrolled to {:.1}, expected {:.1}",
                doc.scroll_y, expected
            ));
        }
    }

    // No document: no strip.
    let mut empty = Harness::new(
        None,
        dir.join("minimap-empty.conf"),
        ScriptedDialogs::new(vec![]),
    );
    empty.frame()?;
    if !empty.app.state().editor.minimap.rect.is_empty() {
        return Err("the minimap was painted without a document".into());
    }
    Ok(())
}

/// Native GDScript outline: rows appear without a language server, the dock
/// tab is painted, and a click jumps to the declaration.
fn check_godot_outline(dir: &Path) -> Result<(), String> {
    let path = dir.join("outline.gd");
    fs::write(
        &path,
        "class_name Brick\nextends Node\n\nsignal died\n\n\
         func _ready():\n\tvar local = 1\n\nfunc hit(amount):\n\tpass\n",
    )
    .map_err(|error| format!("GDScript outline fixture: {error}"))?;
    let mut harness = Harness::new(
        Some(path.clone()),
        dir.join("outline-gd-session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let frame = harness.frame()?;
    for name in ["OUTLINE", "Brick", "died", "_ready", "hit"] {
        expect(&texts(&frame), name, "native GDScript outline row")?;
    }
    if harness.app.state().outline.rows.len() < 4 {
        return Err("the GDScript outline painted no local rows".into());
    }
    let target = {
        let mut state = harness.app.state_mut();
        // Use the document's canonical path: the fixture directory may resolve
        // through a symlink (macOS `/var` -> `/private/var`).
        let doc_path = state
            .ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(Path::to_path_buf)
            .ok_or("no active GDScript document")?;
        let symbols = state.outline_symbols_for(&doc_path);
        let index = symbols
            .iter()
            .position(|symbol| symbol.name == "hit")
            .ok_or("the GDScript outline has no hit row")?;
        state
            .outline
            .rows
            .iter()
            .find(|(row, _)| *row == index)
            .map(|(_, rect)| rect.center())
            .ok_or("the hit outline row was not painted")?
    };
    harness.click(target);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .map_or(0, |doc| doc.line_col().0)
        != 8
    {
        return Err("clicking a native GDScript outline row did not jump".into());
    }
    Ok(())
}

#[cfg(unix)]
fn check_lsp(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let project = dir.join("lsp-project");
    fs::create_dir_all(project.join("src")).map_err(|error| format!("lsp fixture: {error}"))?;
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"lsp-fixture\"\nversion = \"0.1.0\"\n",
    )
    .map_err(|error| format!("lsp fixture: {error}"))?;
    let main = project.join("src").join("main.rs");
    fs::write(
        &main,
        "fn main() {\n    let diagnostic = 1;\n    println!(\"hello\");\n}\n",
    )
    .map_err(|error| format!("lsp fixture: {error}"))?;
    let other = project.join("src").join("other.rs");
    fs::write(&other, "fn main() {}\n").map_err(|error| format!("lsp fixture: {error}"))?;
    let closed = project.join("src").join("closed.rs");
    fs::write(&closed, "fn main() {}\n").map_err(|error| format!("lsp fixture: {error}"))?;
    let server = project.join("fake-lsp.py");
    fs::write(
        &server,
        r#"#!/usr/bin/env python3
import os, sys, json

WATCH_LOG = os.environ.get("IGUI_LSP_WATCH_LOG")

def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii", "replace").strip()
        if not line:
            break
        key, value = line.split(":", 1)
        headers[key.strip().lower()] = value.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])))

def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(("Content-Length: %d\r\n\r\n" % len(body)).encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"capabilities": {
            "semanticTokensProvider": {
                "full": True,
                "legend": {"tokenTypes": ["variable", "function"], "tokenModifiers": []},
            },
            "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
            "referencesProvider": True,
            "documentHighlightProvider": True,
            "documentLinkProvider": {},
            "renameProvider": {"prepareProvider": True},
        }}})
        # Ask for every file change; the editor must report externally created
        # files through workspace/didChangeWatchedFiles.
        send({"jsonrpc": "2.0", "id": "register-watch", "method": "client/registerCapability",
              "params": {"registrations": [{
                  "id": "watch-all",
                  "method": "workspace/didChangeWatchedFiles",
                  "registerOptions": {"watchers": [{"globPattern": "**/*"}]},
              }]}})
    elif method == "workspace/didChangeWatchedFiles":
        if WATCH_LOG:
            with open(WATCH_LOG, "a") as log:
                log.write(json.dumps(params) + "\n")
    elif method == "textDocument/didSave":
        if WATCH_LOG:
            with open(WATCH_LOG, "a") as log:
                log.write(json.dumps({"didSave": params}) + "\n")
    elif method == "textDocument/didOpen":
        uri = params["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "diagnostics": [{
                "range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 18}},
                "severity": 1,
                "message": "fake diagnostic",
            }, {
                "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 10}},
                "severity": 2,
                "message": "fake warning",
            }],
        }})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"contents": {"kind": "markdown", "value": "lsp hover card\n\nUse `struct` inline.\n\n```rust\nfn main() {}\nlet very_long_identifier_name_for_wrapping = another_long_identifier_name_that_never_fits + yet_another_long_identifier_name_for_wrapping;\n```"}}})
    elif method == "textDocument/prepareRename":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "range": {"start": {"line": 0, "character": 3},
                      "end": {"line": 0, "character": 7}},
            "placeholder": "main",
        }})
    elif method == "textDocument/rename":
        uri = params["textDocument"]["uri"]
        base = uri.rsplit("/", 1)[0]
        new_name = params.get("newName", "")
        edit = {"range": {"start": {"line": 0, "character": 3},
                          "end": {"line": 0, "character": 7}},
                "newText": new_name}
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "changes": {uri: [edit], base + "/closed.rs": [edit]},
        }})
    elif method == "textDocument/references":
        uri = params["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "id": request_id, "result": [
            {"uri": uri, "range": {"start": {"line": 0, "character": 3},
                                    "end": {"line": 0, "character": 7}}},
            {"uri": uri, "range": {"start": {"line": 1, "character": 8},
                                    "end": {"line": 1, "character": 18}}},
        ]})
    elif method == "textDocument/documentHighlight":
        send({"jsonrpc": "2.0", "id": request_id, "result": [
            {"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}}},
            {"range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 18}}},
        ]})
    elif method == "textDocument/signatureHelp":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "signatures": [{
                "label": "func clamp(value: Variant, min: Variant, max: Variant) -> Variant",
                "documentation": {"kind": "markdown", "value": "Clamp the value.\n\nMore docs."},
                "parameters": [{"label": "value: Variant"}, {"label": "min: Variant"},
                               {"label": "max: Variant"}],
            }],
            "activeSignature": 0,
            "activeParameter": 1,
        }})
    elif method == "textDocument/completion":
        items = [{"label": "completion_item", "detail": "fake"},
                 {"label": "\"res://fake.gd\"", "insertText": "\"res://fake.gd\"",
                  "detail": "resource path", "kind": 17}]
        items += [{"label": "filler_%d" % i, "insertText": "filler_%d" % i} for i in range(300)]
        items += [{"label": "print(…)", "insertText": "print", "detail": "fake global"}]
        send({"jsonrpc": "2.0", "id": request_id, "result": {"items": items}})
    elif method == "completionItem/resolve":
        label = params.get("label", "")
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "label": label,
            "detail": "resolved",
            "documentation": {"kind": "markdown", "value":
                "func @GlobalScope.print(...) -> void\n\nResolved completion docs."},
        }})
    elif method == "textDocument/nativeSymbol":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "name": "Node", "native_class": "Node",
            "detail": "class Node extends Object",
            "documentation": "Use [method _ready] and [member Node.name].",
            "kind": 5,
            "children": [{
                "name": "_ready", "native_class": "Node",
                "detail": "func Node._ready() -> void",
                "documentation": "Ready.", "kind": 6,
            }],
        }})
    elif method == "textDocument/definition":
        uri = params["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "id": request_id, "result": [{
            "uri": uri,
            "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9}},
        }]})
    elif method == "textDocument/semanticTokens/full":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"data": [
            0, 3, 4, 1, 0,
            1, 8, 10, 0, 0,
        ]}})
    elif method == "textDocument/documentLink":
        uri = params["textDocument"]["uri"]
        base = uri.rsplit("/", 1)[0]
        send({"jsonrpc": "2.0", "id": request_id, "result": [{
            "range": {"start": {"line": 2, "character": 14},
                      "end": {"line": 2, "character": 19}},
            "target": base + "/other.rs",
        }]})
    elif method == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": request_id, "result": [{
            "name": "main",
            "kind": 12,
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 1}},
            "selectionRange": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}},
            "children": [{
                "name": "diagnostic",
                "kind": 13,
                "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 18}},
                "selectionRange": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 18}},
            }, {
                "name": "changed",
                "kind": 24,
                "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 10}},
                "selectionRange": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 10}},
            }],
        }]})
"#,
    )
    .map_err(|error| format!("lsp fixture: {error}"))?;
    let mut permissions = fs::metadata(&server)
        .map_err(|error| format!("lsp fixture: {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&server, permissions).map_err(|error| format!("lsp fixture: {error}"))?;

    let previous = std::env::var_os("IGUI_EDITOR_RUST_ANALYZER");
    std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", &server);
    let result = check_lsp_flow(dir, &main)
        .and_then(|()| check_lsp_monorepo(dir))
        .and_then(|()| check_godot_attach(dir))
        .and_then(|()| check_tooling_prompt(dir));
    match previous {
        Some(value) => std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", value),
        None => std::env::remove_var("IGUI_EDITOR_RUST_ANALYZER"),
    }
    result
}

fn check_lsp_flow(dir: &Path, main: &Path) -> Result<(), String> {
    let watch_log = dir.join("lsp-watch.log");
    let _ = fs::remove_file(&watch_log);
    std::env::set_var("IGUI_LSP_WATCH_LOG", &watch_log);
    let _watch_guard = EnvGuard("IGUI_LSP_WATCH_LOG");
    let config = dir.join("lsp-session.conf");
    let mut harness = Harness::new(
        Some(main.to_path_buf()),
        config,
        ScriptedDialogs::new(vec![]),
    );
    let path = canonical_target(main.to_path_buf());

    for _ in 0..150 {
        harness.frame()?;
        if !harness.app.state().lsp.diagnostics_for(&path).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let state = harness.app.state();
        let diagnostics = state.lsp.diagnostics_for(&path);
        if diagnostics.len() != 2
            || diagnostics[0].message != "fake diagnostic"
            || diagnostics[1].message != "fake warning"
        {
            return Err(format!(
                "LSP diagnostics missing: {:?} status {:?}",
                diagnostics.len(),
                state.lsp.status()
            ));
        }
        if state.lsp.summary(Some(&path)).as_deref() != Some("LSP 1E 1W") {
            return Err("LSP footer summary is wrong".into());
        }
        if state.lsp.semantic_for(&path).len() != 2 {
            return Err("LSP semantic tokens were not received".into());
        }
        if state.lsp.symbols_for(&path).len() != 3 {
            return Err("LSP document symbols were not received".into());
        }
    }

    // Document links cover path strings and open their target on Cmd/Ctrl-click.
    let other = canonical_target(main.with_file_name("other.rs"));
    for _ in 0..150 {
        harness.frame()?;
        let state = harness.app.state();
        let Some(doc) = state.ws.active_doc() else {
            return Err("the active document disappeared before document links".into());
        };
        let byte = crate::model::lsp::byte_for_position(doc, 2, 14);
        if state
            .lsp
            .link_at(&path, doc, byte)
            .as_deref()
            .is_some_and(|target| target == other.as_path())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let mut state = harness.app.state_mut();
        let doc = state
            .ws
            .active_doc()
            .ok_or("no active document for document link")?;
        let byte = crate::model::lsp::byte_for_position(doc, 2, 14);
        if !state.open_lsp_document_link_at(byte) {
            return Err("the document link did not open its target".into());
        }
    }
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .and_then(|doc| doc.path())
        .map(|path| canonical_target(path.to_path_buf()))
        != Some(other.clone())
    {
        return Err("the document link opened the wrong file".into());
    }
    // The link target is a real tab with its own diagnostics; close it before
    // the PROBLEMS assertions below, then return to the main fixture.
    let opened = harness.app.state().ws.active;
    harness.app.state_mut().request_close_tab(opened);
    harness.frame()?;
    harness.app.state_mut().open_path(main.to_path_buf());
    harness.frame()?;

    let frame = harness.frame()?;
    let semantic_variable = crate::theme::Theme::new(Mode::Dark)
        .p
        .syntax
        .variable
        .to_rgba8();
    if !draw_text_has_color(&frame, "diagnostic", semantic_variable) {
        return Err("LSP semantic variable color was not painted".into());
    }
    let error = crate::theme::Theme::new(Mode::Dark).p.error.to_rgba8();
    if !frame.commands().iter().any(|command| {
        matches!(
            command,
            DrawCommand::FillRect { rect, paint }
                if paint.color.to_rgba8() == error && rect.size.height <= 2.0
        )
    }) {
        return Err("LSP diagnostic underline was not painted".into());
    }

    // The footer chip opens the bottom dock's PROBLEMS tab; warnings and errors
    // are separate rows and a click jumps without hiding the panel.
    let lsp_hit = harness.app.state().lsp_hit;
    if lsp_hit == Rect::ZERO {
        return Err("LSP footer chip has no hit target".into());
    }
    harness.click(lsp_hit.center());
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.bottom_tab != crate::state::Panel::Problems || !state.terminal.open {
            return Err("the footer chip did not open the PROBLEMS dock tab".into());
        }
        if state.problems.rows.len() != 2 {
            return Err(format!(
                "the PROBLEMS panel cached {} rows, expected 2",
                state.problems.rows.len()
            ));
        }
    }
    let text = texts(&frame);
    if !text.iter().any(|text| text == "PROBLEMS")
        || !text.iter().any(|text| text == "fake diagnostic")
        || !text.iter().any(|text| text == "fake warning")
    {
        return Err(format!(
            "the PROBLEMS panel did not paint the diagnostics: {text:?}"
        ));
    }
    // The tab title carries the counts, so the dock says what is wrong before
    // the list is read.
    if !text.iter().any(|text| text == "1E 1W") {
        return Err(format!(
            "the PROBLEMS tab did not show its error/warning counts: {text:?}"
        ));
    }
    let row = harness
        .app
        .state()
        .problems
        .hits
        .first()
        .map(|(_, row)| row.center())
        .ok_or("the PROBLEMS panel has no row hit target")?;
    harness.click(row);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 != 1)
    {
        return Err("clicking a diagnostic did not jump to its line".into());
    }
    if !crate::state::Panel::Problems.is_open(&harness.app.state()) {
        return Err("clicking a diagnostic must keep the PROBLEMS panel open".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }
    // The dock switch must rebuild the shell back to the terminal body.
    harness
        .app
        .state_mut()
        .dock_show(crate::state::Panel::Terminal);
    harness.frame()?;
    if !harness.app.state().terminal.open
        || harness.app.state().bottom_tab != crate::state::Panel::Terminal
    {
        return Err("switching back to the TERMINAL dock tab failed".into());
    }

    // -- external file watch --------------------------------------------
    // The server registered `**/*`; a file created by another tool (Godot writes
    // `<script>.gd.uid`) must reach the sidebar and the language server.
    let watch_log = dir.join("lsp-watch.log");
    let watched = harness.app.state().ws.root.join("godot_note.gd.uid");
    let _ = fs::remove_file(&watched);
    {
        // Baseline snapshot, then the file appears behind our back.
        harness.app.state_mut().poll_filesystem();
        fs::write(&watched, "uid://watch\n").map_err(|error| format!("watch fixture: {error}"))?;
        harness.app.state_mut().poll_filesystem();
    }
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state
            .ws
            .tree
            .rows()
            .iter()
            .any(|row| row.name == "godot_note.gd.uid")
        {
            return Err(format!(
                "an externally created file did not reach the sidebar: {:?}",
                state
                    .ws
                    .tree
                    .rows()
                    .iter()
                    .map(|row| row.name.clone())
                    .collect::<Vec<_>>()
            ));
        }
    }
    for _ in 0..100 {
        harness.frame()?;
        if watch_log_has(&watch_log, "godot_note.gd.uid", 1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !watch_log_has(&watch_log, "godot_note.gd.uid", 1) {
        return Err(format!(
            "the language server was not told about the external file: {:?}",
            fs::read_to_string(&watch_log).unwrap_or_default()
        ));
    }
    fs::remove_file(&watched).map_err(|error| format!("watch fixture: {error}"))?;
    harness.app.state_mut().poll_filesystem();
    harness.frame()?;
    {
        let state = harness.app.state();
        if state
            .ws
            .tree
            .rows()
            .iter()
            .any(|row| row.name == "godot_note.gd.uid")
        {
            return Err("an externally deleted file stayed in the sidebar".into());
        }
    }
    for _ in 0..100 {
        harness.frame()?;
        if watch_log_has(&watch_log, "godot_note.gd.uid", 3) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !watch_log_has(&watch_log, "godot_note.gd.uid", 3) {
        return Err(format!(
            "the language server was not told about the external delete: {:?}",
            fs::read_to_string(&watch_log).unwrap_or_default()
        ));
    }
    // Saving an open document sends `textDocument/didSave`.
    let primary = if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    };
    harness.shortcut(Key::Character('s'), primary);
    for _ in 0..100 {
        harness.frame()?;
        if fs::read_to_string(&watch_log)
            .unwrap_or_default()
            .contains("didSave")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !fs::read_to_string(&watch_log)
        .unwrap_or_default()
        .contains("didSave")
    {
        return Err("saving an open document did not send textDocument/didSave".into());
    }

    let frame = harness.frame()?;
    if harness.app.state().outline.rows.len() < 3
        || !texts(&frame).iter().any(|text| text == "OUTLINE")
        || !texts(&frame).iter().any(|text| text == "diagnostic")
        || !texts(&frame).iter().any(|text| text == "S")
    {
        return Err("outline panel did not paint document symbols and the signal glyph".into());
    }
    let outline_row = harness.app.state().outline.rows[1].1.center();
    harness.click(outline_row);
    harness.frame()?;
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 != 1)
    {
        return Err("clicking an outline symbol did not jump to its line".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }
    let outline_width = harness.app.state().outline.rect.width();
    let outline_handle = harness.app.state().outline.handle_rect.center();
    harness.drag(
        outline_handle,
        Vec2::new(outline_handle.x - 30.0, outline_handle.y),
    );
    harness.frame()?;
    if harness.app.state().outline.rect.width() <= outline_width {
        return Err("dragging the outline handle did not widen the panel".into());
    }

    // Hovering an underlined diagnostic shows its message immediately; a
    // normal identifier still gets the delayed server hover card.
    let frame = harness.frame()?;
    let diagnostic_pos = draw_position(&frame, "diagnostic")
        .map(|pos| Vec2::new(pos.x + 2.0, pos.y - 4.0))
        .ok_or("diagnostic text was not painted for hover")?;
    harness.event(InputEvent::PointerMove {
        position: diagnostic_pos,
    });
    let frame = harness.frame()?;
    if !texts(&frame).iter().any(|text| text == "fake diagnostic") {
        return Err("diagnostic hover did not paint the error message".into());
    }

    let frame = harness.frame()?;
    let hover_pos = draw_position(&frame, "main")
        .map(|pos| Vec2::new(pos.x + 2.0, pos.y - 4.0))
        .ok_or("hover fixture text was not painted")?;
    harness.event(InputEvent::PointerMove {
        position: hover_pos,
    });
    for _ in 0..100 {
        harness.frame()?;
        if texts(&harness.frame()?)
            .iter()
            .any(|text| text == "lsp hover card")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !texts(&harness.frame()?)
        .iter()
        .any(|text| text == "lsp hover card")
    {
        let state = harness.app.state();
        return Err(format!(
            "mouse dwell did not paint the LSP hover card (target={:?} status={:?} hover={:?} text_rect={:?} pos={:?})",
            state.hover_target,
            state.lsp.status(),
            state.lsp.hover.is_some(),
            state.editor.text_rect,
            hover_pos
        ));
    }
    // A long fenced-code line must wrap inside the card, not be clipped at
    // the right edge (which hid its tail and read as missing characters).
    let frame = harness.frame()?;
    let card = harness.app.state().lsp.hover_rect;
    let measurer = igui_ui::text_measurer(harness.app.tree());
    let mut code_runs = 0;
    for command in frame.commands() {
        let DrawCommand::DrawText {
            text,
            position,
            font_size,
            family,
            ..
        } = command
        else {
            continue;
        };
        if text.is_empty()
            || position.x < card.left()
            || position.x > card.right()
            || position.y < card.top()
            || position.y > card.bottom()
        {
            continue;
        }
        let width = measurer.measure_run_weighted_in(
            text,
            *font_size,
            igui_core::FontWeight::NORMAL,
            family.as_deref(),
        );
        if position.x + width > card.right() - 8.0 {
            return Err(format!(
                "the hover card clipped a {:.0}px text run at x={:.1}: {text:?}",
                width, position.x
            ));
        }
        if text.contains("very_long_identifier") {
            code_runs += 1;
        }
    }
    if code_runs == 0 {
        return Err("the long hover code line was not painted as wrapped runs".into());
    }

    // A prompt above the editor owns the pointer: moving onto the hover card
    // keeps the card, and opening Quick Open dismisses it instead of leaving it
    // painted on top of the prompt.
    let card = card.center();
    harness.event(InputEvent::PointerMove { position: card });
    if harness.app.state().hover_target.is_some() {
        return Err("the hover card left the editor hover target alive".into());
    }
    let quick_open_modifiers = if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    };
    harness.shortcut(Key::Character('p'), quick_open_modifiers);
    harness.frame()?;
    {
        let state = harness.app.state();
        if !state.quick_open.open {
            return Err("Quick Open did not open above the hover card".into());
        }
        if state.lsp.hover.is_some() {
            return Err("the LSP hover card survived a popup opened above it".into());
        }
    }
    harness.key(Key::Escape);
    harness.frame()?;
    harness.event(InputEvent::PointerMove {
        position: hover_pos,
    });
    harness.event(InputEvent::ModifiersChanged(Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    }));
    harness.event(InputEvent::PointerDown {
        position: hover_pos,
        button: PointerButton::Left,
    });
    harness.event(InputEvent::PointerUp {
        position: hover_pos,
        button: PointerButton::Left,
    });
    harness.event(InputEvent::ModifiersChanged(Modifiers::NONE));
    for _ in 0..100 {
        harness.frame()?;
        if harness
            .app
            .state()
            .ws
            .active_doc()
            .is_some_and(|doc| doc.line_col().0 == 2)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if harness
        .app
        .state()
        .ws
        .active_doc()
        .is_none_or(|doc| doc.line_col().0 != 2)
    {
        return Err("Ctrl+click did not request a definition".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }

    // -- document highlights ---------------------------------------------
    // The symbol under the caret gets occurrence tints; moving to whitespace
    // clears them immediately.
    for _ in 0..100 {
        harness.frame()?;
        if !harness.app.state().lsp.highlights_for(&path).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.lsp.highlights_for(&path).is_empty() {
            return Err(format!(
                "the document-highlight request never arrived (status {:?})",
                state.lsp.status()
            ));
        }
        let color = state.theme.p.selection.alpha_mul(0.55).to_rgba8();
        let line_h = state.editor_metrics().line_h;
        let painted = frame.commands().iter().any(|command| {
            matches!(command, DrawCommand::FillRect { rect, paint }
                if paint.color.to_rgba8() == color
                    && (rect.size.height - line_h).abs() < 0.5
                    && state.editor.text_rect.contains(rect.center()))
        });
        if !painted {
            return Err("occurrence highlights were not painted".into());
        }
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        let end = doc.text().len();
        doc.set_caret(end);
    }
    harness.frame()?;
    if !harness.app.state().lsp.highlights_for(&path).is_empty() {
        return Err("occurrence highlights survived a move off the symbol".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }
    harness.frame()?;

    // -- references list -------------------------------------------------
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(3); // on `main`
    }
    harness.frame()?;
    harness.shortcut(
        Key::F12,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().references.open {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.references.open || state.references.rows.len() != 2 {
            return Err(format!(
                "Shift+F12 did not open the references list (status {:?})",
                state.lsp.status()
            ));
        }
        let painted = texts(&frame);
        if !painted.iter().any(|text| text == "REFERENCES")
            || !painted.iter().any(|text| text == "2 RESULTS")
            || !painted.iter().any(|text| text == "main.rs:1")
        {
            return Err(format!("the references overlay did not paint: {painted:?}"));
        }
    }
    let row = crate::ui::references::layout(&harness.app.state()).rows[1].center();
    harness.click(row);
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.references.open {
            return Err("clicking a reference did not close the overlay".into());
        }
        if state
            .ws
            .active_doc()
            .is_none_or(|doc| doc.line_col().0 != 1)
        {
            return Err("clicking a reference did not jump to its line".into());
        }
    }
    // Reopen and close with Escape.
    harness.shortcut(
        Key::F12,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().references.open {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().references.open {
        return Err("Escape did not close the references overlay".into());
    }
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        doc.set_caret(0);
    }
    harness.frame()?;

    // -- symbol rename (open tab + closed file) ---------------------------
    let other = main.parent().unwrap().join("other.rs");
    let closed = main.parent().unwrap().join("closed.rs");
    let canonical_other = canonical_target(other.clone());
    {
        let mut state = harness.app.state_mut();
        state.open_path(other.clone());
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.set_caret(3); // on `main`
        }
    }
    harness.frame()?;
    harness.key(Key::F2);
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().rename_symbol.open {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.rename_symbol.open {
            return Err(format!(
                "F2 did not open the rename prompt (status {:?})",
                state.lsp.status()
            ));
        }
        if !texts(&frame).iter().any(|text| text == "RENAME SYMBOL") {
            return Err("the rename prompt was not painted".into());
        }
    }
    harness.type_text("hero");
    harness.key(Key::Enter);
    for _ in 0..100 {
        harness.frame()?;
        let open_changed = harness
            .app
            .state()
            .ws
            .tabs
            .iter()
            .find(|doc| doc.path() == Some(canonical_other.as_path()))
            .is_some_and(|doc| doc.text() == "fn hero() {}\n");
        let closed_changed = fs::read_to_string(&closed)
            .map(|text| text == "fn hero() {}\n")
            .unwrap_or(false);
        if open_changed && closed_changed {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let state = harness.app.state();
        let open_text = state
            .ws
            .tabs
            .iter()
            .find(|doc| doc.path() == Some(canonical_other.as_path()))
            .map(|doc| doc.text().to_string());
        let closed_text = fs::read_to_string(&closed).unwrap_or_default();
        if open_text.as_deref() != Some("fn hero() {}\n") || closed_text != "fn hero() {}\n" {
            return Err(format!(
                "rename did not update the open and closed files (open={open_text:?}, closed={closed_text:?}, status={:?}, message={:?})",
                state.lsp.status(),
                state.message()
            ));
        }
        if !state
            .message()
            .is_some_and(|(text, _)| text.contains("renamed main → hero"))
        {
            return Err("rename did not report its result".into());
        }
    }
    // The open buffer is one undo step; the closed file was written to disk.
    {
        let mut state = harness.app.state_mut();
        let doc = state
            .ws
            .tabs
            .iter_mut()
            .find(|doc| doc.path() == Some(canonical_other.as_path()))
            .expect("renamed tab");
        assert!(doc.undo());
        if doc.text() != "fn main() {}\n" {
            return Err("undo did not restore the renamed buffer".into());
        }
    }
    // Return to the original fixture for the remaining checks. The extra tab
    // must not stay active, or the final definition check would target it.
    {
        let mut state = harness.app.state_mut();
        if let Some(index) = state
            .ws
            .tabs
            .iter()
            .position(|doc| doc.path() == Some(canonical_other.as_path()))
        {
            state.request_close_tab(index);
        }
        state.open_path(main.to_path_buf());
        if let Some(doc) = state.ws.active_doc_mut() {
            doc.set_caret(0);
        }
    }
    harness.frame()?;

    // Hover: F1, wait for the response, and assert the overlay is painted.
    harness.key(Key::F1);
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.hover.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    if !texts(&frame).iter().any(|text| text == "lsp hover card") {
        return Err("LSP hover overlay was not painted".into());
    }
    let theme = harness.app.state().theme;
    if !draw_text_has_color(&frame, "fn", theme.p.syntax.keyword.to_rgba8()) {
        return Err("hover code block keyword `fn` was not syntax-highlighted".into());
    }
    if !draw_text_has_color(&frame, "main", theme.p.syntax.function.to_rgba8()) {
        return Err("hover code block function `main` was not syntax-highlighted".into());
    }
    if !draw_text_has_color(&frame, "struct", theme.p.syntax.keyword.to_rgba8()) {
        return Err("hover inline code `struct` was not syntax-highlighted".into());
    }

    // Completion: platform accelerator, navigate, insert.
    let completion_modifier = if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    };
    harness.shortcut(Key::Character('i'), completion_modifier);
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.completion.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    if !texts(&frame).iter().any(|text| text == "completion_item") {
        return Err("LSP completion overlay was not painted".into());
    }
    harness.key(Key::Enter);
    harness.frame()?;
    if !harness
        .app
        .state()
        .ws
        .active_doc()
        .is_some_and(|doc| doc.text().starts_with("completion_item"))
    {
        return Err("LSP completion was not inserted".into());
    }

    // A global completion beyond the old 200-item cap is filtered before the
    // cap. Godot returns thousands of items and `print` is not near the front.
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        let end = doc.text().len();
        doc.set_caret(end);
        doc.insert_text("\npri");
    }
    harness.shortcut(Key::Character('i'), completion_modifier);
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.completion.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let state = harness.app.state();
        let completion = state.lsp.completion.as_ref();
        if completion.is_none_or(|completion| {
            completion.items.first().map(|item| item.insert.as_str()) != Some("print")
        }) {
            return Err(format!(
                "completion did not surface global `print` (items: {:?})",
                completion.map(|completion| {
                    completion
                        .items
                        .iter()
                        .take(8)
                        .map(|item| item.label.clone())
                        .collect::<Vec<_>>()
                })
            ));
        }
    }
    harness.app.state_mut().lsp.dismiss_transient();
    harness.frame()?;

    // Native hover declarations expose the documentation action.
    harness.app.state_mut().lsp.hover = Some(crate::model::lsp::HoverUi {
        text: "<Native> class Node extends Object\n\nNode docs".into(),
        native: Some(crate::model::lsp::NativeTarget {
            class: "Node".into(),
            symbol: "Node".into(),
            kind: crate::model::lsp::NativeDocKind::Class,
        }),
    });
    let frame = harness.frame()?;
    if !texts(&frame).iter().any(|text| text == "OPEN GODOT DOCS") {
        return Err("native hover did not paint the docs action".into());
    }
    harness.app.state_mut().lsp.dismiss_transient();

    // Typing an identifier schedules completion with a short debounce; the
    // selected item gets a resolved hover-style preview.
    if let Some(doc) = harness.app.state_mut().ws.active_doc_mut() {
        let end = doc.text().len();
        doc.set_caret(end);
        doc.insert_text("\n");
        let end = doc.text().len();
        doc.set_caret(end);
    }
    harness.type_text("pri");
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.completion.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..100 {
        harness.frame()?;
        let state = harness.app.state();
        if state
            .lsp
            .completion
            .as_ref()
            .is_some_and(|c| c.preview.is_some() || c.preview_pending)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let state = harness.app.state();
        let completion = state.lsp.completion.as_ref();
        if completion.is_none_or(|c| c.items.first().map(|i| i.insert.as_str()) != Some("print")) {
            let doc = harness
                .app
                .state()
                .ws
                .active_doc()
                .map(|d| (d.text().to_string(), d.caret()));
            return Err(format!(
                "automatic completion did not select global `print` (doc={doc:?}, items={:?})",
                completion.map(|c| c
                    .items
                    .iter()
                    .take(12)
                    .map(|i| i.label.clone())
                    .collect::<Vec<_>>())
            ));
        }
        if !completion.is_some_and(|c| {
            c.preview
                .as_deref()
                .is_some_and(|preview| preview.contains("Resolved completion docs"))
        }) {
            return Err("completion preview was not resolved".into());
        }
    }
    harness.app.state_mut().lsp.dismiss_transient();
    harness.frame()?;

    // -- signature help -------------------------------------------------
    // Typing `(` in an argument list asks the server for the signature; the
    // active parameter is painted with the accent color.
    {
        let mut state = harness.app.state_mut();
        let doc = state.ws.active_doc_mut().expect("active document");
        let end = doc.text().len();
        doc.set_caret(end);
    }
    harness.type_text("\ncall(");
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.signature_help.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.lsp.signature_help.is_none() {
            return Err(format!(
                "typing `(` did not request signature help (status {:?})",
                state.lsp.status()
            ));
        }
        let theme = state.theme;
        if !texts(&frame).iter().any(|text| text.contains("clamp")) {
            return Err("signature help did not paint the signature".into());
        }
        if !draw_text_has_color(&frame, "min: Variant", theme.p.accent.to_rgba8()) {
            return Err("signature help did not highlight the active parameter".into());
        }
        if !texts(&frame)
            .iter()
            .any(|text| text.contains("Clamp the value."))
        {
            return Err("signature help did not paint its documentation".into());
        }
    }
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().lsp.signature_help.is_some() {
        return Err("Escape did not dismiss signature help".into());
    }

    // -- resource-path completion ---------------------------------------
    // The server returns a quoted `"res://…"` insertion; the editor replaces
    // the whole string literal instead of inserting it after the partial path.
    {
        let mut state = harness.app.state_mut();
        let doc = state.ws.active_doc_mut().expect("active document");
        let end = doc.text().len();
        doc.set_caret(end);
    }
    harness.type_text("\npreload(");
    harness.type_text("\"");
    harness.type_text("res://f");
    for _ in 0..100 {
        harness.frame()?;
        if harness.app.state().lsp.completion.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let mut state = harness.app.state_mut();
        let completion = state
            .lsp
            .completion
            .as_mut()
            .ok_or("typing a resource path did not open completion")?;
        let index = completion
            .items
            .iter()
            .position(|item| item.insert.starts_with('"'))
            .ok_or("the completion list has no quoted resource path")?;
        completion.selected = index;
    }
    harness.key(Key::Enter);
    harness.frame()?;
    {
        let state = harness.app.state();
        let text = state
            .ws
            .active_doc()
            .map(|doc| doc.text().to_string())
            .unwrap_or_default();
        if !text.ends_with("preload(\"res://fake.gd\"") {
            return Err(format!(
                "resource-path completion did not replace the string: {text:?}"
            ));
        }
    }
    {
        let mut state = harness.app.state_mut();
        state.lsp.dismiss_transient();
        state.clear_signature_help();
    }
    harness.frame()?;

    // Embedded Godot docs are a normal read-only editor tab.
    {
        let target = crate::model::lsp::NativeTarget {
            class: "Node".into(),
            symbol: "Node".into(),
            kind: crate::model::lsp::NativeDocKind::Class,
        };
        harness.app.state_mut().open_native_docs(&target);
    }
    for _ in 0..100 {
        harness.frame()?;
        if harness
            .app
            .state()
            .ws
            .active_doc()
            .is_some_and(|doc| doc.is_docs_view())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        let doc = state.ws.active_doc();
        if !doc.is_some_and(|doc| doc.is_docs_view()) {
            return Err("nativeSymbol did not open a documentation tab".into());
        }
        if !texts(&frame).iter().any(|text| text == "GODOT DOCS") {
            return Err("embedded Godot docs tab was not painted".into());
        }
        let linked = doc.is_some_and(|doc| {
            state.docs.links.get(&doc.id()).is_some_and(|links| {
                links.iter().any(|(_, _, link)| {
                    matches!(link, crate::model::bbcode::DocLink::Native { .. })
                })
            })
        });
        if !linked {
            return Err("native docs did not produce clickable BBCode links".into());
        }
    }
    let index = harness.app.state().ws.active;
    harness.app.state_mut().request_close_tab(index);
    harness.frame()?;

    // Definition: F12 returns a location in the same file.
    harness.key(Key::F12);
    for _ in 0..100 {
        harness.frame()?;
        let state = harness.app.state();
        if state
            .ws
            .active_doc()
            .is_some_and(|doc| doc.line_col().0 == 2)
        {
            drop(state);
            let mut state = harness.app.state_mut();
            state.shutdown_children();
            if !matches!(state.lsp.status(), crate::model::lsp::LspStatus::Off)
                || !state.lsp.diagnostics_for(&path).is_empty()
            {
                return Err("shutdown did not stop the LSP session".into());
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("LSP definition did not move the caret".into())
}

/// A stand-in for a *running* Godot editor's language server: listens on a
/// free loopback port, reports that port through a file, and answers `initialize`
/// + `didOpen` with one diagnostic.
#[cfg(unix)]
const FAKE_GODOT_EDITOR_LSP: &str = r##"#!/usr/bin/env python3
import json, os, socket, sys

LOG = os.environ.get("IGUI_GODOT_LSP_LOG")
ready = sys.argv[1]
server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", 0))
server.listen(1)
with open(ready, "w") as handle:
    handle.write(str(server.getsockname()[1]))
conn, _ = server.accept()
stream = conn.makefile("rwb")

def read():
    length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        key, value = line.split(b":", 1)
        if key.strip().lower() == b"content-length":
            length = int(value)
    return json.loads(stream.read(length))

def send(message):
    body = json.dumps(message).encode()
    stream.write(("Content-Length: %d\r\n\r\n" % len(body)).encode())
    stream.write(body)
    stream.flush()

def note(entry):
    if LOG:
        with open(LOG, "a") as handle:
            handle.write(json.dumps(entry) + "\n")

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "capabilities": {"textDocumentSync": 1},
            "serverInfo": {"name": "godot-editor-fixture"},
        }})
    elif method == "textDocument/didOpen":
        document = params["textDocument"]
        uri = document["uri"]
        note({"method": "textDocument/didOpen", "uri": uri})
        stale = uri.rsplit("/", 1)[0] + "/stale.gd"
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                "severity": 1,
                "message": "attached diagnostic",
            }],
        }})
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": stale,
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                "severity": 2,
                "message": "deleted-file warning",
            }],
        }})
    elif method == "textDocument/didChange":
        note({"method": "textDocument/didChange",
              "uri": params["textDocument"]["uri"],
              "text": params["contentChanges"][0]["text"]})
    elif method == "workspace/didChangeWatchedFiles":
        note({"method": "workspace/didChangeWatchedFiles", "changes": params.get("changes")})
"##;

#[cfg(unix)]
const FAKE_LOG_LSP: &str = r#"#!/usr/bin/env python3
import json, os, sys

LOG = os.environ.get("IGUI_FAKE_LSP_LOG")

def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii", "replace").strip()
        if not line:
            break
        key, value = line.split(":", 1)
        headers[key.strip().lower()] = value.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])))

def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(("Content-Length: %d\r\n\r\n" % len(body)).encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def note(entry):
    if LOG:
        with open(LOG, "a") as handle:
            handle.write(json.dumps(entry) + "\n")

note({"event": "started", "pid": os.getpid()})

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize":
        note({"event": "initialize", "root": params.get("rootUri")})
        send({"jsonrpc": "2.0", "id": request_id, "result": {"capabilities": {}}})
    elif method == "textDocument/didOpen":
        document = params["textDocument"]
        note({"event": "didOpen", "uri": document["uri"], "languageId": document["languageId"]})
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": document["uri"],
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                "severity": 2,
                "message": "fake go warning",
            }],
        }})
    elif method == "textDocument/semanticTokens/full":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"data": []}})
    elif method == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": request_id, "result": []})
    elif request_id is not None:
        send({"jsonrpc": "2.0", "id": request_id, "result": None})
"#;

/// Whether the watch log contains a change of `kind` (1/2/3) for a file whose
/// name ends with `name`.
fn watch_log_has(path: &Path, name: &str, kind: u64) -> bool {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|entry| {
            entry["changes"].as_array().is_some_and(|changes| {
                changes.iter().any(|change| {
                    change["type"].as_u64() == Some(kind)
                        && change["uri"]
                            .as_str()
                            .is_some_and(|uri| uri.ends_with(name))
                })
            })
        })
}

fn lsp_log_events(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(unix)]
fn wait_for_lsp(
    harness: &mut Harness,
    mut ready: impl FnMut(&crate::state::AppState) -> bool,
) -> Result<(), String> {
    for _ in 0..250 {
        harness.frame()?;
        if ready(&harness.app.state()) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("LSP condition timed out".into())
}

/// A monorepo must run one server for the whole workspace (`go.work`), keep
/// both modules' diagnostics alive, and restart only when re-enabled.
/// A running Godot editor already serves the project: the client must attach to
/// it and never spawn a second editor (the fixture points the command at a path
/// that does not exist).
#[cfg(unix)]
fn check_godot_attach(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let project = dir.join("godot-project");
    fs::create_dir_all(&project).map_err(|error| format!("godot fixture: {error}"))?;
    fs::write(project.join("project.godot"), "config_version=5\n")
        .map_err(|error| format!("godot fixture: {error}"))?;
    let script = project.join("main.gd");
    fs::write(&script, "extends Node\n\nfunc _ready():\n\tpass\n")
        .map_err(|error| format!("godot fixture: {error}"))?;
    let stale = project.join("stale.gd");
    fs::write(&stale, "extends Node\n").map_err(|error| format!("godot fixture: {error}"))?;
    let log = project.join("godot-lsp.log");
    let _ = fs::remove_file(&log);
    std::env::set_var("IGUI_GODOT_LSP_LOG", &log);
    let _log_guard = EnvGuard("IGUI_GODOT_LSP_LOG");

    let server = project.join("godot-editor-lsp.py");
    fs::write(&server, FAKE_GODOT_EDITOR_LSP).map_err(|error| format!("godot fixture: {error}"))?;
    let mut permissions = fs::metadata(&server)
        .map_err(|error| format!("godot fixture: {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&server, permissions).map_err(|error| format!("godot fixture: {error}"))?;

    let ready = project.join("godot-lsp.port");
    let _ = fs::remove_file(&ready);
    let mut editor = std::process::Command::new(&server)
        .arg(&ready)
        .spawn()
        .map_err(|error| format!("godot fixture: {error}"))?;
    let port = (|| {
        for _ in 0..300 {
            if let Ok(text) = fs::read_to_string(&ready) {
                if let Ok(port) = text.trim().parse::<u16>() {
                    return Some(port);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    })()
    .ok_or("the scripted Godot editor never listened")?;
    // Attach-only: the spec has no command to run, so only the address matters.
    std::env::set_var("IGUI_EDITOR_GODOT_LSP", format!("127.0.0.1:{port}"));
    let _lsp_guard = EnvGuard("IGUI_EDITOR_GODOT_LSP");

    let mut harness = Harness::new(
        Some(script.clone()),
        project.join("godot-attach-session.conf"),
        ScriptedDialogs::new(vec![]),
    );
    let path = canonical_target(script.clone());
    let result = wait_for_lsp(&mut harness, |state| {
        !state.lsp.diagnostics_for(&path).is_empty()
    })
    .and_then(|()| {
        let state = harness.app.state();
        if state.tooling.prompt.is_some() {
            return Err("attaching must not need a Godot executable".into());
        }
        if *state.lsp.status() != crate::model::lsp::LspStatus::Ready {
            return Err(format!(
                "attached session is not ready: {:?}",
                state.lsp.status()
            ));
        }
        let messages: Vec<&str> = state
            .lsp
            .diagnostics_for(&path)
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        if messages != ["attached diagnostic"] {
            return Err(format!(
                "the running editor's diagnostics did not arrive: {messages:?}"
            ));
        }
        Ok(())
    })
    .and_then(|()| {
        // A deleted file never publishes another diagnostics update: its rows
        // must disappear from the PROBLEMS panel immediately.
        let stale = canonical_target(stale.clone());
        if harness.app.state().lsp.diagnostics_for(&stale).is_empty() {
            return Err("the fixture did not publish the deleted-file diagnostic".into());
        }
        harness.app.state_mut().poll_filesystem();
        fs::remove_file(&stale).map_err(|error| format!("godot fixture: {error}"))?;
        harness.app.state_mut().poll_filesystem();
        harness.frame()?;
        {
            let state = harness.app.state();
            if !state.lsp.diagnostics_for(&stale).is_empty() {
                return Err("deleting a file kept its cached diagnostics".into());
            }
            if state.problems.rows.iter().any(|row| row.path == stale) {
                return Err("the PROBLEMS panel kept a deleted file's row".into());
            }
        }

        // Godot ignores `workspace/didChangeWatchedFiles`; a structural change
        // must instead re-send the open document once the scan has run.
        wait_for_lsp(&mut harness, |_state| {
            lsp_log_events(&log)
                .iter()
                .any(|event| event["method"] == "textDocument/didChange")
        })?;
        let events = lsp_log_events(&log);
        if events
            .iter()
            .any(|event| event["method"] == "workspace/didChangeWatchedFiles")
        {
            return Err("Godot was sent a watched-file notification it ignores".into());
        }
        if !events
            .iter()
            .any(|event| event["method"] == "textDocument/didOpen")
        {
            return Err("the attached fixture never saw the document open".into());
        }
        let text =
            fs::read_to_string(&script).map_err(|error| format!("godot fixture: {error}"))?;
        let resent = events
            .iter()
            .find(|event| event["method"] == "textDocument/didChange")
            .ok_or("no document re-parse was sent")?;
        if resent["uri"].as_str() != Some(&crate::model::lsp::path_to_uri(&path)) {
            return Err(format!(
                "the re-parse was sent for the wrong document: {}",
                resent["uri"]
            ));
        }
        if resent["text"].as_str() != Some(text.as_str()) {
            return Err("the re-parse did not carry the full document text".into());
        }

        println!("selfcheck: Godot rescan instead of watched files ok");
        Ok(())
    });
    let _ = editor.kill();
    let _ = editor.wait();
    result
}

#[cfg(unix)]
fn check_lsp_monorepo(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let project = dir.join("lsp-monorepo");
    for package in ["a", "b"] {
        fs::create_dir_all(project.join(package))
            .map_err(|error| format!("monorepo fixture: {error}"))?;
        fs::write(
            project.join(package).join("Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n"),
        )
        .map_err(|error| format!("monorepo fixture: {error}"))?;
        fs::write(
            project.join(package).join("main.rs"),
            format!("fn {package}() {{}}\nfn main() {{ {package}(); }}\n"),
        )
        .map_err(|error| format!("monorepo fixture: {error}"))?;
    }
    // A workspace-level marker keeps both packages on one server.
    fs::write(project.join("Cargo.lock"), "")
        .map_err(|error| format!("monorepo fixture: {error}"))?;

    let server = dir.join("fake-lsp-log.py");
    fs::write(&server, FAKE_LOG_LSP).map_err(|error| format!("lsp fixture: {error}"))?;
    let mut permissions = fs::metadata(&server)
        .map_err(|error| format!("lsp fixture: {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&server, permissions).map_err(|error| format!("lsp fixture: {error}"))?;

    let log = dir.join("lsp-monorepo.log");
    let _ = fs::remove_file(&log);
    let previous_server = std::env::var_os("IGUI_EDITOR_RUST_ANALYZER");
    let previous_log = std::env::var_os("IGUI_FAKE_LSP_LOG");
    std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", &server);
    std::env::set_var("IGUI_FAKE_LSP_LOG", &log);
    let result = lsp_monorepo_flow(dir, &project, &log);
    match previous_server {
        Some(value) => std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", value),
        None => std::env::remove_var("IGUI_EDITOR_RUST_ANALYZER"),
    }
    match previous_log {
        Some(value) => std::env::set_var("IGUI_FAKE_LSP_LOG", value),
        None => std::env::remove_var("IGUI_FAKE_LSP_LOG"),
    }
    result
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn lsp_monorepo_flow(dir: &Path, project: &Path, log: &Path) -> Result<(), String> {
    let a = canonical_target(project.join("a/main.rs"));
    let b = canonical_target(project.join("b/main.rs"));
    let config = dir.join("lsp-monorepo.conf");
    let mut harness = Harness::new(Some(a.clone()), config, ScriptedDialogs::new(vec![]));
    wait_for_lsp(&mut harness, |state| {
        !state.lsp.diagnostics_for(&a).is_empty()
    })?;
    harness.app.state_mut().open_path(b.clone());
    wait_for_lsp(&mut harness, |state| {
        !state.lsp.diagnostics_for(&b).is_empty()
    })?;
    {
        let state = harness.app.state();
        if state.lsp.diagnostics_for(&a).is_empty() {
            return Err("monorepo: opening module B dropped module A diagnostics".into());
        }
        if state.lsp.summary(Some(&b)).as_deref() != Some("LSP 0E 1W") {
            return Err(format!(
                "monorepo: footer summary is {:?}",
                state.lsp.summary(Some(&b))
            ));
        }
    }

    let events = lsp_log_events(log);
    let initializes = events
        .iter()
        .filter(|event| event["event"] == "initialize")
        .count();
    if initializes != 1 {
        return Err(format!(
            "monorepo started {initializes} language servers, expected one"
        ));
    }
    let opens: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["event"] == "didOpen")
        .collect();
    if opens.len() < 2 {
        return Err(format!("monorepo opened {} documents", opens.len()));
    }
    if opens.iter().any(|event| event["languageId"] != "rust") {
        return Err("monorepo didOpen did not use the `rust` language id".into());
    }

    // Footer dot stops every server; the chip reports LSP OFF.
    harness.frame()?;
    let toggle = harness.app.state().lsp_toggle_hit;
    if toggle == Rect::ZERO {
        return Err("footer LSP on/off dot has no hit target".into());
    }
    harness.click(toggle.center());
    harness.frame()?;
    {
        let state = harness.app.state();
        if state.settings.lsp {
            return Err("footer dot did not disable language servers".into());
        }
        if state.lsp.enabled() {
            return Err("LSP controller is still enabled after the footer toggle".into());
        }
        if !state.lsp.diagnostics_for(&a).is_empty() {
            return Err("disabling LSP did not clear diagnostics".into());
        }
        if state.lsp.summary(Some(&a)).as_deref() != Some("LSP OFF") {
            return Err("footer chip does not report LSP OFF".into());
        }
    }

    // The settings row is the second entry point and re-enables the same state.
    harness
        .app
        .state_mut()
        .activate_setting(Row::LanguageIntelligence);
    wait_for_lsp(&mut harness, |state| {
        state.settings.lsp && !state.lsp.diagnostics_for(&a).is_empty()
    })?;
    let events = lsp_log_events(log);
    let initializes = events
        .iter()
        .filter(|event| event["event"] == "initialize")
        .count();
    if initializes != 2 {
        return Err(format!(
            "re-enabling LSP started {initializes} servers, expected a restart"
        ));
    }

    // AppKit quit does not unwind Rust destructors, so the app calls this
    // explicitly before replying to terminate. Verify the real child dies.
    let pid = lsp_log_events(log)
        .iter()
        .find_map(|event| event["pid"].as_u64())
        .ok_or_else(|| "fake language server did not log its pid".to_string())?
        as u32;
    harness.app.state_mut().shutdown_children();
    for _ in 0..50 {
        if !process_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if process_alive(pid) {
        return Err(format!(
            "shutdown_children left language server pid {pid} running"
        ));
    }
    {
        let state = harness.app.state();
        if !matches!(state.lsp.status(), crate::model::lsp::LspStatus::Off)
            || !state.lsp.diagnostics_for(&a).is_empty()
        {
            return Err("shutdown_children did not clear the LSP session".into());
        }
    }
    println!("selfcheck: monorepo language server ok");
    Ok(())
}

/// A missing server executable opens the copy-only tooling prompt; the editor
/// never installs anything itself.
#[cfg(unix)]
fn check_tooling_prompt(dir: &Path) -> Result<(), String> {
    let project = dir.join("missing-tool");
    fs::create_dir_all(&project).map_err(|error| format!("tooling fixture: {error}"))?;
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"tooling-fixture\"\nversion = \"0.1.0\"\n",
    )
    .map_err(|error| format!("tooling fixture: {error}"))?;
    let main = project.join("main.rs");
    fs::write(&main, "fn main() {}\n").map_err(|error| format!("tooling fixture: {error}"))?;
    let missing = project.join("missing-rust-analyzer-binary");
    let previous = std::env::var_os("IGUI_EDITOR_RUST_ANALYZER");
    std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", &missing);
    let result = tooling_flow(dir, &canonical_target(main));
    match previous {
        Some(value) => std::env::set_var("IGUI_EDITOR_RUST_ANALYZER", value),
        None => std::env::remove_var("IGUI_EDITOR_RUST_ANALYZER"),
    }
    result
}

#[cfg(unix)]
fn tooling_flow(dir: &Path, main: &Path) -> Result<(), String> {
    let config = dir.join("tooling.conf");
    let mut harness = Harness::new(
        Some(main.to_path_buf()),
        config,
        ScriptedDialogs::new(vec![]),
    );
    wait_for_lsp(&mut harness, |state| state.tooling.prompt.is_some())?;
    {
        let state = harness.app.state();
        let prompt = state
            .tooling
            .prompt
            .as_ref()
            .ok_or("tooling prompt is missing")?;
        if prompt.tool != "rust-analyzer"
            || !prompt
                .command
                .contains("rustup component add rust-analyzer")
        {
            return Err(format!(
                "tooling prompt has the wrong install hint: {} / {}",
                prompt.tool, prompt.command
            ));
        }
        if prompt.command.contains('\n') {
            return Err("install command must stay on one line".into());
        }
        if state.lsp.summary(Some(main)).as_deref() != Some("LSP ERR") {
            return Err("missing tool did not surface in the footer chip".into());
        }
    }
    let frame = harness.frame()?;
    let painted = texts(&frame);
    if !painted.iter().any(|text| text == "RUST-ANALYZER NOT FOUND") {
        return Err("tooling prompt title was not painted".into());
    }
    if !painted.iter().any(|text| text == "COPY COMMAND") {
        return Err("tooling prompt has no COPY COMMAND button".into());
    }

    let layout = crate::ui::tooling::layout(&harness.app.state());
    let copy = layout.copy.ok_or("tooling COPY button has no rect")?;
    harness.click(copy.center());
    if !harness
        .app
        .state()
        .tooling
        .prompt
        .as_ref()
        .is_some_and(|prompt| prompt.copied)
    {
        return Err("COPY COMMAND did not confirm the copy".into());
    }

    // RETRY clears the prompt and re-arms the start attempt.
    let layout = crate::ui::tooling::layout(&harness.app.state());
    harness.click(layout.retry.center());
    if harness.app.state().tooling.prompt.is_some() {
        return Err("RETRY did not close the tooling prompt".into());
    }
    wait_for_lsp(&mut harness, |state| state.tooling.prompt.is_some())?;

    // Esc dismisses the prompt without touching the editor.
    harness.key(Key::Escape);
    harness.frame()?;
    if harness.app.state().tooling.prompt.is_some() {
        return Err("Esc did not close the tooling prompt".into());
    }
    println!("selfcheck: tooling install prompt ok");
    Ok(())
}

#[cfg(not(unix))]
fn check_lsp(_dir: &Path) -> Result<(), String> {
    Ok(())
}

fn fixture() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("igui-editor-selfcheck-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).map_err(|error| format!("fixture: {error}"))?;
    fs::write(dir.join("main.rs"), MAIN_RS).map_err(|error| format!("fixture: {error}"))?;
    let mut notes = String::from(NOTES_MD);
    for index in 0..60 {
        notes.push_str(&format!("- filler item {index:02}\n"));
    }
    fs::write(dir.join("notes.md"), notes).map_err(|error| format!("fixture: {error}"))?;
    fs::write(dir.join("src").join("lib.rs"), "pub fn lib() {}\n")
        .map_err(|error| format!("fixture: {error}"))?;
    let mut large = String::new();
    for index in 0..6_000 {
        large.push_str(&format!(
            "- large markdown line {index} with enough padding text to reach the truncation limit quickly\n"
        ));
    }
    fs::write(dir.join("large.md"), large).map_err(|error| format!("fixture: {error}"))?;
    let huge =
        fs::File::create(dir.join("huge.txt")).map_err(|error| format!("fixture: {error}"))?;
    huge.set_len(33 * 1024 * 1024)
        .map_err(|error| format!("fixture: {error}"))?;
    Ok(dir)
}

fn canonical_target(path: PathBuf) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or(path),
        _ => path,
    }
}

fn record(app: &mut EditorApp, viewport: ViewportSize) -> Result<RecordedFrame, String> {
    let mut backend = RecordingBackend::new();
    backend
        .begin_frame(viewport)
        .map_err(|error| format!("begin_frame: {error}"))?;
    let mut ctx = PaintContext::new();
    app.paint(&mut ctx);
    backend
        .submit(&ctx.into_draw_list())
        .map_err(|error| format!("submit: {error}"))?;
    backend
        .end_frame()
        .map_err(|error| format!("end_frame: {error}"))?;
    backend
        .last_frame()
        .cloned()
        .ok_or_else(|| "nothing was recorded".to_string())
}

fn draw_text_has_color(frame: &RecordedFrame, needle: &str, color: [u8; 4]) -> bool {
    frame.commands().iter().any(|command| match command {
        DrawCommand::DrawText { text, paint, .. } => {
            text == needle && paint.color.to_rgba8() == color
        }
        _ => false,
    })
}

fn circle_color(frame: &RecordedFrame, center: Vec2) -> Option<[u8; 4]> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::FillCircle {
            center: painted,
            paint,
            ..
        } if (painted.x - center.x).abs() < 0.5 && (painted.y - center.y).abs() < 0.5 => {
            Some(paint.color.to_rgba8())
        }
        _ => None,
    })
}

fn grip_center_y(frame: &RecordedFrame, color: [u8; 4], center_x: f32) -> Option<f32> {
    find_fill_near_x(
        frame,
        color,
        crate::ui::handle::GRIP_WIDTH,
        crate::ui::handle::GRIP_HEIGHT,
        center_x,
        2.0,
    )
    .map(|rect| rect.center().y)
}

fn find_fill_near_x(
    frame: &RecordedFrame,
    color: [u8; 4],
    width: f32,
    height: f32,
    center_x: f32,
    tolerance: f32,
) -> Option<Rect> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::FillRect { rect, paint }
            if paint.color.to_rgba8() == color
                && (rect.size.width - width).abs() < 0.01
                && (rect.size.height - height).abs() < 0.01
                && (rect.center().x - center_x).abs() <= tolerance =>
        {
            Some(*rect)
        }
        _ => None,
    })
}

fn find_fill(frame: &RecordedFrame, color: [u8; 4], width: f32, height: f32) -> Option<Rect> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::FillRect { rect, paint }
            if paint.color.to_rgba8() == color
                && (rect.size.width - width).abs() < 0.01
                && (rect.size.height - height).abs() < 0.01 =>
        {
            Some(*rect)
        }
        _ => None,
    })
}

fn count_vertical_lines(frame: &RecordedFrame, x_from: f32, x_to: f32, height: f32) -> usize {
    frame
        .commands()
        .iter()
        .filter(|command| {
            matches!(
                command,
                DrawCommand::FillRect { rect, .. }
                    if (rect.size.width - 1.0).abs() < 0.01
                        && (rect.size.height - height).abs() < 0.01
                        && rect.left() >= x_from
                        && rect.left() <= x_to
            )
        })
        .count()
}

fn count_guides(frame: &RecordedFrame, line_h: f32) -> usize {
    frame
        .commands()
        .iter()
        .filter(|command| {
            matches!(
                command,
                DrawCommand::FillRect { rect, .. }
                    if (rect.size.width - 1.0).abs() < 0.01
                        && (rect.size.height - line_h).abs() < 0.01
            )
        })
        .count()
}

/// Status-bar runs must never overlap (the transient message used to sit on
/// top of the directory hint).
fn check_status_overlap(
    frame: &RecordedFrame,
    measurer: &dyn igui_ui::TextMeasurer,
) -> Result<(), String> {
    let Some(baseline) = frame.commands().iter().find_map(|command| match command {
        DrawCommand::DrawText { text, position, .. } if text == "UTF-8" => Some(position.y),
        _ => None,
    }) else {
        return Ok(());
    };
    let mut runs: Vec<(String, f32, f32)> = frame
        .commands()
        .iter()
        .filter_map(|command| match command {
            DrawCommand::DrawText {
                text,
                position,
                font_size,
                ..
            } if (position.y - baseline).abs() < 0.01 => {
                let width =
                    measurer.measure_run_weighted(text, *font_size, igui_core::FontWeight::NORMAL);
                Some((text.clone(), position.x, position.x + width))
            }
            _ => None,
        })
        .collect();
    runs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    for pair in runs.windows(2) {
        if pair[0].2 > pair[1].1 + 0.6 {
            return Err(format!(
                "status bar runs overlap: {:?} [{:.1}..{:.1}] and {:?} [{:.1}..{:.1}]",
                pair[0].0, pair[0].1, pair[0].2, pair[1].0, pair[1].1, pair[1].2
            ));
        }
    }
    Ok(())
}

fn sidebar_click_position(app: &EditorApp, name: &str) -> Result<Vec2, String> {
    let state = app.state();
    let rows = state.ws.tree.rows();
    let index = rows
        .iter()
        .position(|row| row.name == name)
        .ok_or_else(|| format!("{name} is not in the sidebar rows"))?;
    let m = state.theme.m;
    let rect = state.sidebar.rect;
    Ok(Vec2::new(
        rect.left() + 60.0,
        rect.top() + index as f32 * m.row_h + m.row_h * 0.5,
    ))
}

fn draw_position(frame: &RecordedFrame, needle: &str) -> Option<Vec2> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::DrawText { text, position, .. } if text == needle => Some(*position),
        _ => None,
    })
}

/// Baseline of the first painted run containing `needle` that lands inside
/// `area`; used to tell a preview run from the outline row with the same label.
fn text_y_in(frame: &RecordedFrame, needle: &str, area: Rect) -> Option<f32> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::DrawText { text, position, .. }
            if text.contains(needle) && area.contains(*position) =>
        {
            Some(position.y)
        }
        _ => None,
    })
}

fn texts(frame: &RecordedFrame) -> Vec<String> {
    frame
        .commands()
        .iter()
        .filter_map(|command| match command {
            DrawCommand::DrawText { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn expect(texts: &[String], needle: &str, what: &str) -> Result<(), String> {
    let joined = texts.join(" ");
    if texts.iter().any(|text| text.contains(needle)) || joined.contains(needle) {
        Ok(())
    } else {
        let sample = texts
            .iter()
            .take(24)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        Err(format!(
            "{what}: `{needle}` was not painted ({} runs: {sample})",
            texts.len()
        ))
    }
}

fn inspect_frame(app: &EditorApp, frame: &RecordedFrame) -> Result<(), String> {
    let mut stats = FrameStats::new(0);
    stats.counters = FrameCounters::new(0, app.control_count(), frame.command_count(), 1);
    let report = inspect(&frame.draw_list, &stats);
    if report.count_of(Severity::Error) > 0 {
        let details: Vec<String> = report
            .findings()
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| format!("{:?}: {}", finding.code, finding.message))
            .collect();
        return Err(format!("inspection found errors: {}", details.join("; ")));
    }
    Ok(())
}
