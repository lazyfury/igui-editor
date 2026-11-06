//! Quick-create-file prompt (`⇧⌘N`): a centered path/name field.
//!
//! Same overlay pattern as Quick Open. The app owns path validation and
//! `create_new`; this module only paints the field and forwards editing keys.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 210.0;
const PAD: f32 = 20.0;
const FIELD_H: f32 = 38.0;
const FOOTER_H: f32 = 30.0;

pub struct Layout {
    pub panel: Rect,
    pub title: Rect,
    pub directory: Rect,
    pub field: Rect,
    pub error: Rect,
    pub footer: Rect,
}

pub fn layout(viewport: Rect) -> Layout {
    let width = WIDTH.min(viewport.width() - 48.0).max(280.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, HEIGHT));
    let title = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, panel.top() + PAD),
        Size::new(panel.width() - PAD * 2.0, 24.0),
    );
    let directory = Rect::from_min_size(
        Vec2::new(title.left(), title.bottom() + 4.0),
        Size::new(title.width(), 22.0),
    );
    let field = Rect::from_min_size(
        Vec2::new(title.left(), directory.bottom() + 10.0),
        Size::new(title.width(), FIELD_H),
    );
    let error = Rect::from_min_size(
        Vec2::new(title.left(), field.bottom() + 8.0),
        Size::new(title.width(), 22.0),
    );
    let footer = Rect::from_min_size(
        Vec2::new(title.left(), panel.bottom() - PAD - FOOTER_H),
        Size::new(title.width(), FOOTER_H),
    );
    Layout {
        panel,
        title,
        directory,
        field,
        error,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.new_file.open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let layout = layout(viewport);

    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.35));
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );
    kit::text(
        ctx,
        state.new_file.kind.title(),
        layout.title.left(),
        kit::baseline(measurer, layout.title, m.font_ui),
        m.font_ui,
        p.text,
    );
    let dir = state.new_file.dir.display().to_string();
    let dir = kit::truncate_left(measurer, &dir, m.font_small, layout.directory.width());
    kit::text(
        ctx,
        &dir,
        layout.directory.left(),
        kit::baseline(measurer, layout.directory, m.font_small),
        m.font_small,
        p.faint,
    );

    kit::fill(ctx, layout.field, p.bg);
    ctx.stroke_rect(layout.field, 1.0, p.accent);
    let inner = Rect::from_min_size(
        Vec2::new(layout.field.left() + 8.0, layout.field.top()),
        Size::new(
            (layout.field.width() - 16.0).max(8.0),
            layout.field.height(),
        ),
    );
    ctx.save();
    ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, layout.field, m.font_ui);
    let text = &state.new_file.name;
    if let Some((start, end)) = text.selection() {
        let x0 = inner.left() + kit::width(measurer, &text.text()[..start], m.font_ui);
        let x1 = inner.left() + kit::width(measurer, &text.text()[..end], m.font_ui);
        ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(x0, layout.field.top() + 4.0),
                Size::new((x1 - x0).max(1.0), layout.field.height() - 8.0),
            ),
            p.selection,
        );
    }
    kit::text(ctx, text.text(), inner.left(), baseline, m.font_ui, p.text);
    let mut caret_x = inner.left() + kit::width(measurer, &text.text()[..text.caret()], m.font_ui);
    if let Some(preedit) = &state.new_file.preedit {
        kit::text(ctx, preedit, caret_x, baseline, m.font_ui, p.accent);
        caret_x += kit::width(measurer, preedit, m.font_ui);
    }
    let caret = kit::field_caret(inner, caret_x, layout.field);
    ctx.fill_rect(caret, p.accent);
    state.new_file.caret_rect = caret;
    ctx.restore();

    if let Some(error) = &state.new_file.error {
        kit::text(
            ctx,
            error,
            layout.error.left(),
            kit::baseline(measurer, layout.error, m.font_small),
            m.font_small,
            p.error,
        );
    } else {
        let hint = if state.new_file.kind == crate::state::NewFileKind::Rename {
            "single name · never overwrites"
        } else {
            "relative name · parents must exist · never overwrites"
        };
        kit::text(
            ctx,
            hint,
            layout.error.left(),
            kit::baseline(measurer, layout.error, m.font_small),
            m.font_small,
            p.faint,
        );
    }
    kit::text(
        ctx,
        if state.new_file.kind == crate::state::NewFileKind::Rename {
            "Enter rename · Esc cancel"
        } else {
            "Enter create · Esc cancel"
        },
        layout.footer.left(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
    let count = state.new_file.name.text().chars().count();
    kit::text_right(
        ctx,
        measurer,
        &format!("{count} chars"),
        layout.footer.right(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

/// Returns true when the press belongs to the prompt.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.new_file.open {
        return false;
    }
    let layout = layout(state.viewport.logical_rect());
    if !layout.panel.contains(pos) {
        state.close_new_file();
    }
    true
}

/// Handles keys while the prompt is open. Returns false only for app-level
/// accelerators (so ⌘S etc. still work).
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.new_file.open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    let shift = modifiers.shift;
    if accel {
        match key {
            Key::Character(ch) => match ch.to_ascii_lowercase() {
                'a' => state.new_file.name.select_all(),
                'c' => {
                    if let Some(text) = state.new_file.name.selected_text() {
                        state.set_clipboard(text);
                    }
                }
                'x' => {
                    if let Some(text) = state.new_file.name.selected_text() {
                        state.set_clipboard(text);
                        state.new_file.name.backspace();
                    }
                }
                'v' => {
                    if let Some(text) = state.get_clipboard() {
                        state.new_file.name.insert(&text);
                    }
                }
                _ => return false,
            },
            _ => return false,
        }
        state.new_file.error = None;
        state.dirty = true;
        return true;
    }
    match key {
        Key::Escape => state.close_new_file(),
        Key::Enter => state.submit_new_file(),
        Key::Backspace => {
            state.new_file.name.backspace();
            state.new_file.error = None;
        }
        Key::Delete => {
            state.new_file.name.delete();
            state.new_file.error = None;
        }
        Key::ArrowLeft => state.new_file.name.move_left(shift, modifiers.alt),
        Key::ArrowRight => state.new_file.name.move_right(shift, modifiers.alt),
        Key::Home => state.new_file.name.move_home(shift),
        Key::End => state.new_file.name.move_end(shift),
        Key::Space => state.new_file.name.insert(" "),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn on_text(state: &mut AppState, text: &str) {
    if !state.new_file.open {
        return;
    }
    state.new_file.name.insert(text);
    state.new_file.error = None;
    state.new_file.preedit = None;
    state.dirty = true;
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    if !state.new_file.open {
        return;
    }
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.new_file.preedit = Some(text.clone());
        }
        igui_core::ImeEvent::Commit(text) => {
            state.new_file.name.insert(text);
            state.new_file.error = None;
            state.new_file.preedit = None;
        }
        igui_core::ImeEvent::Disabled => state.new_file.preedit = None,
        igui_core::ImeEvent::Enabled => {}
    }
    state.dirty = true;
}
