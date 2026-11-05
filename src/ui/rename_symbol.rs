//! Symbol rename prompt (`F2`): a centered single-field overlay.
//!
//! The state layer validates the name and asks the language server; this module
//! only paints the field and forwards editing keys.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 196.0;
const PAD: f32 = 20.0;
const FIELD_H: f32 = 38.0;
const FOOTER_H: f32 = 30.0;

pub struct Layout {
    pub panel: Rect,
    pub title: Rect,
    pub context: Rect,
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
    let context = Rect::from_min_size(
        Vec2::new(title.left(), title.bottom() + 4.0),
        Size::new(title.width(), 22.0),
    );
    let field = Rect::from_min_size(
        Vec2::new(title.left(), context.bottom() + 10.0),
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
        context,
        field,
        error,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.rename_symbol.open {
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
        "RENAME SYMBOL",
        layout.title.left(),
        kit::baseline(measurer, layout.title, m.font_ui),
        m.font_ui,
        p.text,
    );
    let context = state.rename_symbol.context.as_ref();
    let hint = context.map_or_else(
        || state.rename_symbol.old_name.clone(),
        |context| {
            format!(
                "{}  ·  {}",
                context.old_name,
                crate::model::workspace_index::display_path(&state.ws.root, &context.path)
            )
        },
    );
    let hint = kit::truncate_left(measurer, &hint, m.font_small, layout.context.width());
    kit::text(
        ctx,
        &hint,
        layout.context.left(),
        kit::baseline(measurer, layout.context, m.font_small),
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
    let text = &state.rename_symbol.name;
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
    if let Some(preedit) = &state.rename_symbol.preedit {
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

    if let Some(error) = &state.rename_symbol.error {
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
            "every use in the project · invalid names are rejected",
            layout.error.left(),
            kit::baseline(measurer, layout.error, m.font_small),
            m.font_small,
            p.faint,
        );
    }
    kit::text(
        ctx,
        "Enter rename · Esc cancel",
        layout.footer.left(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
    let count = state.rename_symbol.name.text().chars().count();
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
    if !state.rename_symbol.open {
        return false;
    }
    let layout = layout(state.viewport.logical_rect());
    if !layout.panel.contains(pos) {
        state.close_rename_symbol();
    }
    true
}

/// Handles keys while the prompt is open. Returns false only for app-level
/// accelerators (so the prompt behaves like the other inline fields).
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.rename_symbol.open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    let shift = modifiers.shift;
    if accel {
        match key {
            Key::Character(ch) => match ch.to_ascii_lowercase() {
                'a' => state.rename_symbol.name.select_all(),
                'c' => {
                    if let Some(text) = state.rename_symbol.name.selected_text() {
                        state.set_clipboard(text);
                    }
                }
                'x' => {
                    if let Some(text) = state.rename_symbol.name.selected_text() {
                        state.set_clipboard(text);
                        state.rename_symbol.name.backspace();
                    }
                }
                'v' => {
                    if let Some(text) = state.get_clipboard() {
                        state.rename_symbol.name.insert(&text);
                    }
                }
                _ => return false,
            },
            _ => return false,
        }
        state.rename_symbol.error = None;
        state.dirty = true;
        return true;
    }
    match key {
        Key::Escape => state.close_rename_symbol(),
        Key::Enter => state.submit_rename_symbol(),
        Key::Backspace => {
            state.rename_symbol.name.backspace();
            state.rename_symbol.error = None;
        }
        Key::Delete => {
            state.rename_symbol.name.delete();
            state.rename_symbol.error = None;
        }
        Key::ArrowLeft => state.rename_symbol.name.move_left(shift, modifiers.alt),
        Key::ArrowRight => state.rename_symbol.name.move_right(shift, modifiers.alt),
        Key::Home => state.rename_symbol.name.move_home(shift),
        Key::End => state.rename_symbol.name.move_end(shift),
        Key::Space => state.rename_symbol.name.insert(" "),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn on_text(state: &mut AppState, text: &str) {
    if !state.rename_symbol.open {
        return;
    }
    state.rename_symbol.name.insert(text);
    state.rename_symbol.error = None;
    state.rename_symbol.preedit = None;
    state.dirty = true;
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    if !state.rename_symbol.open {
        return;
    }
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.rename_symbol.preedit = Some(text.clone());
        }
        igui_core::ImeEvent::Commit(text) => {
            state.rename_symbol.name.insert(text);
            state.rename_symbol.error = None;
            state.rename_symbol.preedit = None;
        }
        igui_core::ImeEvent::Disabled => state.rename_symbol.preedit = None,
        igui_core::ImeEvent::Enabled => {}
    }
    state.dirty = true;
}
