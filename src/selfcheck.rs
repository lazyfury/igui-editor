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

use igui_backend_recording::{RecordedFrame, RecordingBackend};
use igui_core::{InputEvent, Key, Modifiers, PointerButton, Size, Vec2, ViewportSize};
use igui_profile::{inspect, FrameCounters, FrameStats, Severity};
use igui_render::{DrawCommand, PaintContext, RenderBackend};

use crate::app::EditorApp;
use crate::dialogs::{Call, ScriptedDialogs};
use crate::kit::RectExt;
use crate::state::{Action, Focus, Menu, ModalButton, ViewMode};
use crate::theme::Mode;

const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 800.0;

const MAIN_RS: &str = "fn main() {\n    println!(\"hello\");\n}\n";
const NOTES_MD: &str = "# Notes\n\n## Notes\n\n- square corners\n- compact rows\n\n\
                        | Key | Meaning |\n|:----|--------:|\n| a   | alpha   |\n| b   | **beta** |\n\n\
                        ```rust\nfn preview() {}\n```\n";

pub fn run() -> Result<(), String> {
    let dir = fixture()?;
    let result = check(&dir);
    let _ = fs::remove_dir_all(&dir);
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

    // -- startup ---------------------------------------------------------
    let frame = harness.frame()?;
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
    harness.frame()?;
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
    }

    let _ = restored_dialogs;
    println!(
        "selfcheck ok: {} commands in the final frame, {} controls",
        frame.command_count(),
        harness.app.control_count()
    );
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
