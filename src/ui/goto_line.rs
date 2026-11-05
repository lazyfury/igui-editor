//! Go to Line prompt (`⌃G` / `⌘L`): a centered one-field line-number input.
//!
//! Same overlay pattern as the quick-create prompt. The state module owns
//! validation and the jump; this module only paints the field and forwards
//! editing keys. Only ASCII digits reach the field.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 460.0;
const HEIGHT: f32 = 184.0;
const PAD: f32 = 20.0;
const FIELD_H: f32 = 38.0;
const FOOTER_H: f32 = 30.0;

pub struct Layout {
    pub panel: Rect,
    pub title: Rect,
    pub field: Rect,
    pub error: Rect,
    pub footer: Rect,
}

pub fn layout(viewport: Rect) -> Layout {
    let width = WIDTH.min(viewport.width() - 48.0).max(260.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, HEIGHT));
    let title = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, panel.top() + PAD),
        Size::new(panel.width() - PAD * 2.0, 24.0),
    );
    let field = Rect::from_min_size(
        Vec2::new(title.left(), title.bottom() + 10.0),
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
        field,
        error,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.goto_line.open {
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
        "Go to Line",
        layout.title.left(),
        kit::baseline(measurer, layout.title, m.font_ui),
        m.font_ui,
        p.text,
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
    let text = &state.goto_line.field;
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
    let caret_x = inner.left() + kit::width(measurer, &text.text()[..text.caret()], m.font_ui);
    if let Some(preedit) = &state.goto_line.preedit {
        kit::text(ctx, preedit, caret_x, baseline, m.font_ui, p.accent);
    }
    ctx.fill_rect(
        Rect::from_min_size(
            Vec2::new(caret_x, layout.field.top() + 4.0),
            Size::new(2.0, layout.field.height() - 8.0),
        ),
        p.accent,
    );
    ctx.restore();

    if let Some(error) = &state.goto_line.error {
        kit::text(
            ctx,
            error,
            layout.error.left(),
            kit::baseline(measurer, layout.error, m.font_small),
            m.font_small,
            p.error,
        );
    } else {
        kit::text(
            ctx,
            "line number · 1-based",
            layout.error.left(),
            kit::baseline(measurer, layout.error, m.font_small),
            m.font_small,
            p.faint,
        );
    }
    kit::text(
        ctx,
        "Enter jump · Esc close",
        layout.footer.left(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

/// Returns true when the press belongs to the prompt; an outside press closes
/// it and is consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.goto_line.open {
        return false;
    }
    let layout = layout(state.viewport.logical_rect());
    if !layout.panel.contains(pos) {
        state.close_goto_line();
    }
    true
}

/// Handles keys while the prompt is open. Returns false only for app-level
/// accelerators (so ⌘S etc. still work).
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.goto_line.open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    let shift = modifiers.shift;
    if accel {
        match key {
            Key::Character(ch) => match ch.to_ascii_lowercase() {
                'a' => {
                    state.goto_line.field.select_all();
                    state.goto_line.error = None;
                }
                'c' => {
                    if let Some(text) = state.goto_line.field.selected_text() {
                        let text = text.to_string();
                        state.set_clipboard(&text);
                    }
                }
                'x' => {
                    if let Some(text) = state.goto_line.field.selected_text() {
                        let text = text.to_string();
                        state.set_clipboard(&text);
                    }
                    if state.goto_line.field.delete_selection() {
                        state.goto_line.error = None;
                    }
                }
                'v' => {
                    if let Some(text) = state.get_clipboard() {
                        state.type_goto_line(&text);
                    }
                }
                _ => return false,
            },
            _ => return false,
        }
        state.dirty = true;
        return true;
    }
    match key {
        Key::Escape => state.close_goto_line(),
        Key::Enter => state.submit_goto_line(),
        Key::Backspace => {
            state.goto_line.field.backspace();
            state.goto_line.error = None;
        }
        Key::Delete => {
            state.goto_line.field.delete();
            state.goto_line.error = None;
        }
        Key::ArrowLeft => state.goto_line.field.move_left(shift, modifiers.alt),
        Key::ArrowRight => state.goto_line.field.move_right(shift, modifiers.alt),
        Key::Home => state.goto_line.field.move_home(shift),
        Key::End => state.goto_line.field.move_end(shift),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn on_text(state: &mut AppState, text: &str) {
    if !state.goto_line.open {
        return;
    }
    state.type_goto_line(text);
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    if !state.goto_line.open {
        return;
    }
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.goto_line.preedit = Some(text.clone());
        }
        igui_core::ImeEvent::Commit(text) => {
            state.type_goto_line(text);
            state.goto_line.preedit = None;
        }
        igui_core::ImeEvent::Disabled => state.goto_line.preedit = None,
        igui_core::ImeEvent::Enabled => {}
    }
    state.dirty = true;
}
