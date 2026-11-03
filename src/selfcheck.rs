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
//! scrolling, theme switching, Open Folder and session restore.
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
use crate::kit::RectExt;
use crate::model::recovery::RecoveryStore;
use crate::model::session::WindowGeometry;
use crate::state::{Action, Focus, Menu, ModalButton, ViewMode};
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
    let result = check(&dir).and_then(|()| check_workspace_and_recovery(&dir));
    let _ = fs::remove_dir_all(&dir);
    if result.is_ok() {
        println!("selfcheck ok: main flow + workspace/recovery checks");
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
    expect(&painted, "fn main() {", "the opened file's first line")?;
    expect(&painted, "println!", "the opened file's second line")?;
    expect(&painted, "File", "menu bar labels")?;
    expect(&painted, "UTF-8", "status bar facts")?;
    expect(&painted, "main.rs", "active tab / status file")?;
    expect(&painted, "EXPLORER", "sidebar header")?;
    expect(&painted, "notes.md", "sidebar rows")?;
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

    // -- File menu -> Save ----------------------------------------------
    harness.type_text(" ");
    harness.frame()?;
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
        if state.session.recent.first() != Some(&canonical_out) {
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
    {
        let state = harness.app.state();
        let editor_right = state.editor.text_rect.right();
        let preview_left = state.preview.rect.left();
        if preview_left < editor_right {
            return Err("split preview must sit right of the editor".into());
        }
        if state.preview.rect.width() < 300.0 {
            return Err("split preview is implausibly narrow".into());
        }
    }
    // Table: header cells share a baseline, columns order left → right, the
    // body row sits below the header, and the right column is right-aligned.
    let key = draw_position(&frame, "Key").ok_or("table header cell `Key` is missing")?;
    let meaning =
        draw_position(&frame, "Meaning").ok_or("table header cell `Meaning` is missing")?;
    let alpha = draw_position(&frame, "alpha").ok_or("table cell `alpha` is missing")?;
    let beta = draw_position(&frame, "beta").ok_or("table cell `beta` is missing")?;
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
            state.sidebar.rect.top()
                + crate::ui::sidebar::HEADER_H
                + index as f32 * m.row_h
                + m.row_h * 0.5,
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
        (
            Vec2::new(rect.left() + m.text_pad + 2.0, rect.top() + m.line_h),
            Vec2::new(
                rect.left() + m.text_pad + 120.0,
                rect.top() + m.line_h * 2.0,
            ),
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
    harness.app.state_mut().set_language(Some("rust".into()));
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
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
    harness.app.state_mut().set_language(None);
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
    harness.key(Key::ArrowDown); // Tab Width
    harness.key(Key::ArrowRight);
    if harness.app.state().settings.tab_width != 8 {
        return Err("the settings panel did not cycle the tab width".into());
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
    harness.key(Key::ArrowDown); // Sidebar Width
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
    // Persist an 8-space tab width for the session-restore assertion.
    for _ in 0..5 {
        harness.key(Key::ArrowUp); // -> Tab Width row
    }
    harness.key(Key::ArrowRight);
    if harness.app.state().settings.tab_width != 8 {
        return Err("tab width navigation did not land on the Tab Width row".into());
    }
    harness.key(Key::Escape);
    if harness.app.state().settings_open {
        return Err("Esc did not close the settings panel".into());
    }

    // Discoverability: View > Settings… opens the same panel.
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
    harness.frame()?;
    if !harness.app.state().settings_open {
        return Err("View > Settings… did not open the settings panel".into());
    }
    harness.key(Key::Escape);
    harness.frame()?;

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
            rect.height(),
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
            state.sidebar.rect.height(),
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
    let frame = harness.frame()?;
    expect(
        &texts(&frame),
        "preview truncated at 512 KB",
        "large preview banner",
    )?;
    if harness.app.state().ws.active_doc().map(|doc| doc.name()) != Some("large.md") {
        return Err("the large Markdown fixture did not open".into());
    }
    harness.shortcut(
        Key::Character('w'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    harness.frame()?;
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

    // -- Open Folder through a dialog -----------------------------------
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
        return Err("⇧⌘O did not ask the dialog layer for a folder".into());
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
        return Err("Open Folder did not retarget the sidebar root".into());
    }
    expect(&texts(&frame), "lib.rs", "sidebar now lists the new root")?;

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
            .recent
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
                "session restore lost the workspace folder ({:?})",
                state.ws.root
            ));
        }
    }

    let _ = restored_dialogs;
    println!(
        "selfcheck: main flow ok ({} commands in the final frame, {} controls)",
        frame.command_count(),
        harness.app.control_count()
    );
    Ok(())
}

/// Welcome/no-folder state, drag & drop, CLI folder and the recovery prompt.
fn check_workspace_and_recovery(dir: &Path) -> Result<(), String> {
    // -- no folder, no document: welcome + empty sidebar -----------------
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
    expect(&painted, "no folder open", "welcome subtitle")?;
    expect(&painted, "Open Folder…", "welcome actions")?;
    expect(&painted, "NO FOLDER OPEN", "sidebar empty state")?;
    expect(
        &painted,
        "or drop files / folders here",
        "sidebar drop hint",
    )?;
    {
        let state = harness.app.state();
        if state.ws.folder_open || !state.ws.tabs.is_empty() {
            return Err("a folder-less startup must show the welcome state".into());
        }
    }

    // Drop hover overlay.
    harness.app.state_mut().drop_hover = true;
    let frame = harness.frame()?;
    expect(&texts(&frame), "Drop to open", "drop overlay")?;
    harness.app.state_mut().drop_hover = false;
    harness.frame()?;

    // Dropping a file opens a tab without opening a folder.
    harness.app.drop_paths(vec![dir.join("notes.md")]);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if state.ws.folder_open {
            return Err("dropping a file must not open a folder".into());
        }
        if state.ws.active_doc().map(|doc| doc.name()) != Some("notes.md") {
            return Err("dropping a file did not open it".into());
        }
    }
    expect(&texts(&frame), "# Notes", "dropped file content")?;

    // Dropping a folder opens it as the workspace.
    harness.app.drop_paths(vec![dir.join("src")]);
    let frame = harness.frame()?;
    {
        let state = harness.app.state();
        if !state.ws.folder_open {
            return Err("dropping a folder did not open it".into());
        }
        if state.ws.root.file_name().and_then(|name| name.to_str()) != Some("src") {
            return Err(format!("dropped folder root is wrong: {:?}", state.ws.root));
        }
    }
    expect(&texts(&frame), "lib.rs", "dropped folder sidebar")?;
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
            return Err("a CLI folder must open the folder without a tab".into());
        }
    }
    expect(&texts(&frame), "igui-editor", "CLI folder welcome")?;
    expect(&texts(&frame), "lib.rs", "CLI folder sidebar")?;
    drop(harness);

    // -- recovery prompt: restore ---------------------------------------
    let restore_dir = dir.join("recovery-restore");
    fs::create_dir_all(&restore_dir).map_err(|error| format!("fixture: {error}"))?;
    let store = RecoveryStore::new(restore_dir.join("recovery"));
    store
        .save(
            7,
            "recovered.txt",
            Some(Path::new("/tmp/original.txt")),
            "recovered body\n",
            3,
            1,
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
        .save(9, "discard.txt", None, "discard me", 0, 0)
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
        rect.top() + crate::ui::sidebar::HEADER_H + index as f32 * m.row_h + m.row_h * 0.5,
    ))
}

fn draw_position(frame: &RecordedFrame, needle: &str) -> Option<Vec2> {
    frame.commands().iter().find_map(|command| match command {
        DrawCommand::DrawText { text, position, .. } if text == needle => Some(*position),
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
