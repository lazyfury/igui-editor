//! Quick Open palette (`⌘P`): fuzzy file picker over the workspace index.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::model::workspace_index::MAX_RESULTS;
use crate::state::AppState;

const WIDTH: f32 = 620.0;
const ROW_H: f32 = 24.0;
const QUERY_H: f32 = 34.0;
const PAD: f32 = 8.0;
const FOOTER_H: f32 = 24.0;
const MAX_ROWS: usize = 10;

/// Panel/query/row geometry (one function for paint and hit tests).
pub struct Layout {
    pub panel: Rect,
    pub query: Rect,
    pub rows: Vec<Rect>,
    pub footer: Rect,
}

pub fn layout(viewport: Rect, result_count: usize) -> Layout {
    let visible = result_count.clamp(1, MAX_ROWS);
    let height = PAD + QUERY_H + 6.0 + visible as f32 * ROW_H + FOOTER_H + PAD;
    let width = WIDTH.min(viewport.width() - 48.0).max(260.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));
    let query = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, panel.top() + PAD),
        Size::new(panel.width() - PAD * 2.0, QUERY_H),
    );
    let mut rows = Vec::with_capacity(visible);
    let mut y = query.bottom() + 6.0;
    for _ in 0..visible {
        rows.push(Rect::from_min_size(
            Vec2::new(panel.left() + PAD, y),
            Size::new(panel.width() - PAD * 2.0, ROW_H),
        ));
        y += ROW_H;
    }
    let footer = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, y),
        Size::new(panel.width() - PAD * 2.0, FOOTER_H),
    );
    Layout {
        panel,
        query,
        rows,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.quick_open.open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let layout = layout(viewport, state.quick_open.results.len());

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

    paint_query(state, measurer, ctx, layout.query);

    let visible = layout.rows.len();
    let count = state.quick_open.results.len();
    let selected = state.quick_open.selected.min(count.saturating_sub(1));
    let mut first = state.quick_open.scroll;
    if selected < first {
        first = selected;
    } else if selected >= first + visible {
        first = selected + 1 - visible;
    }
    first = first.min(count.saturating_sub(visible));
    state.quick_open.scroll = first;

    for (index, row) in layout.rows.iter().enumerate() {
        let result = first + index;
        let Some(path) = state.quick_open.results.get(result).cloned() else {
            break;
        };
        let is_selected = result == selected;
        if is_selected {
            kit::fill(ctx, *row, p.hover);
            kit::vline(ctx, row.left(), row.top(), row.bottom(), p.accent);
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = kit::truncate(measurer, &name, m.font_ui, row.width() * 0.45);
        let name_width = kit::width(measurer, &name, m.font_ui);
        kit::text(
            ctx,
            &name,
            row.left() + 10.0,
            kit::baseline(measurer, *row, m.font_ui),
            m.font_ui,
            if is_selected { p.text } else { p.dim },
        );
        let display = state.quick_open_display(&path);
        let display = kit::truncate_left(
            measurer,
            &display,
            m.font_small,
            row.width() - name_width - 30.0,
        );
        kit::text_right(
            ctx,
            measurer,
            &display,
            row.right() - 10.0,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            p.faint,
        );
    }

    if state.quick_open.results.is_empty() {
        kit::text(
            ctx,
            "no matching files",
            layout.rows[0].left() + 10.0,
            kit::baseline(measurer, layout.rows[0], m.font_ui),
            m.font_ui,
            p.faint,
        );
    }

    kit::text(
        ctx,
        "↑↓ move · Enter open · Esc close",
        layout.footer.left(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
    kit::text_right(
        ctx,
        measurer,
        &format!("{} files", count.min(MAX_RESULTS)),
        layout.footer.right(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

fn paint_query(state: &AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    kit::fill(ctx, rect, p.bg);
    ctx.stroke_rect(rect, 1.0, p.accent);
    let inner = Rect::from_min_size(
        Vec2::new(rect.left() + 8.0, rect.top()),
        Size::new((rect.width() - 16.0).max(8.0), rect.height()),
    );
    ctx.save();
    ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    let query = &state.quick_open.query;
    if query.text().is_empty() && state.quick_open.preedit.is_none() {
        kit::text(
            ctx,
            "Type a file name…",
            inner.left(),
            baseline,
            m.font_ui,
            p.faint,
        );
    }
    if let Some((start, end)) = query.selection() {
        let x0 = inner.left() + kit::width(measurer, &query.text()[..start], m.font_ui);
        let x1 = inner.left() + kit::width(measurer, &query.text()[..end], m.font_ui);
        ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(x0, rect.top() + 4.0),
                Size::new((x1 - x0).max(1.0), rect.height() - 8.0),
            ),
            p.selection,
        );
    }
    kit::text(ctx, query.text(), inner.left(), baseline, m.font_ui, p.text);
    let caret_x = inner.left() + kit::width(measurer, &query.text()[..query.caret()], m.font_ui);
    if let Some(preedit) = &state.quick_open.preedit {
        kit::text(ctx, preedit, caret_x, baseline, m.font_ui, p.accent);
    }
    ctx.fill_rect(
        Rect::from_min_size(
            Vec2::new(caret_x, rect.top() + 4.0),
            Size::new(2.0, rect.height() - 8.0),
        ),
        p.accent,
    );
    ctx.restore();
}

/// Handles a pointer press. Returns `true` when consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.quick_open.open {
        return false;
    }
    let layout = layout(
        state.viewport.logical_rect(),
        state.quick_open.results.len(),
    );
    if !layout.panel.contains(pos) {
        state.close_quick_open();
        return true;
    }
    if layout.query.contains(pos) {
        return true;
    }
    for (index, row) in layout.rows.iter().enumerate() {
        if row.contains(pos) {
            let result = state.quick_open.scroll + index;
            if result < state.quick_open.results.len() {
                state.quick_open.selected = result;
                state.quick_open_accept();
            }
            return true;
        }
    }
    true
}

/// Handles a key while the palette is open. `false` lets app shortcuts through.
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.quick_open.open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    if accel {
        if matches!(key, Key::Character('p') | Key::Character(',')) {
            state.close_quick_open();
            return true;
        }
        return false;
    }
    let shift = modifiers.shift;
    match key {
        Key::Escape => state.close_quick_open(),
        Key::Enter => state.quick_open_accept(),
        Key::ArrowUp => state.quick_open_move(-1),
        Key::ArrowDown => state.quick_open_move(1),
        Key::Backspace => {
            state.quick_open.query.backspace();
            state.quick_open.selected = 0;
            state.refresh_quick_open();
        }
        Key::Delete => {
            state.quick_open.query.delete();
            state.quick_open.selected = 0;
            state.refresh_quick_open();
        }
        Key::ArrowLeft => state.quick_open.query.move_left(shift, modifiers.alt),
        Key::ArrowRight => state.quick_open.query.move_right(shift, modifiers.alt),
        Key::Home => state.quick_open.query.move_home(shift),
        Key::End => state.quick_open.query.move_end(shift),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn on_text(state: &mut AppState, text: &str) {
    state.quick_open.query.insert(text);
    state.quick_open.selected = 0;
    state.quick_open.preedit = None;
    state.refresh_quick_open();
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.quick_open.preedit = Some(text.clone());
            state.dirty = true;
        }
        igui_core::ImeEvent::Commit(text) => on_text(state, text),
        igui_core::ImeEvent::Disabled => {
            state.quick_open.preedit = None;
            state.dirty = true;
        }
        igui_core::ImeEvent::Enabled => {}
    }
}
