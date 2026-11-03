//! Project search panel (`⇧⌘F`): query, toggles, streamed results.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const WIDTH: f32 = 760.0;
const ROW_H: f32 = 26.0;
const QUERY_H: f32 = 34.0;
const PAD: f32 = 8.0;
const FOOTER_H: f32 = 24.0;
const MAX_ROWS: usize = 14;
const TOGGLE: f32 = 26.0;
const LINE_W: f32 = 46.0;

pub struct Layout {
    pub panel: Rect,
    pub query: Rect,
    pub case_rect: Rect,
    pub word_rect: Rect,
    pub rows: Vec<Rect>,
    pub footer: Rect,
}

pub fn layout(viewport: Rect, hit_count: usize) -> Layout {
    let visible = hit_count.clamp(1, MAX_ROWS);
    let height = PAD + QUERY_H + 6.0 + visible as f32 * ROW_H + FOOTER_H + PAD;
    let width = WIDTH.min(viewport.width() - 48.0).max(300.0);
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));
    let case_rect = Rect::from_min_size(
        Vec2::new(panel.right() - PAD - TOGGLE, panel.top() + PAD),
        Size::new(TOGGLE, QUERY_H),
    );
    let word_rect = Rect::from_min_size(
        Vec2::new(case_rect.left() - TOGGLE - 4.0, case_rect.top()),
        Size::new(TOGGLE, QUERY_H),
    );
    let query = Rect::from_min_size(
        Vec2::new(panel.left() + PAD, panel.top() + PAD),
        Size::new(
            (word_rect.left() - 6.0 - (panel.left() + PAD)).max(80.0),
            QUERY_H,
        ),
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
        case_rect,
        word_rect,
        rows,
        footer,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.search.open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let layout = layout(viewport, state.search.hits.len());

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

    paint_query(state, measurer, ctx, &layout);
    paint_toggle(
        ctx,
        measurer,
        &theme,
        layout.case_rect,
        "Aa",
        state.search.case_sensitive,
    );
    paint_toggle(
        ctx,
        measurer,
        &theme,
        layout.word_rect,
        "W",
        state.search.whole_word,
    );

    let visible = layout.rows.len();
    let count = state.search.hits.len();
    let selected = state.search.selected.min(count.saturating_sub(1));
    let mut first = state.search.scroll;
    if selected < first {
        first = selected;
    } else if selected >= first + visible {
        first = selected + 1 - visible;
    }
    first = first.min(count.saturating_sub(visible));
    state.search.scroll = first;

    for (index, row) in layout.rows.iter().enumerate() {
        let result = first + index;
        let Some(hit) = state.search.hits.get(result).cloned() else {
            break;
        };
        let is_selected = result == selected;
        if is_selected {
            kit::fill(ctx, *row, p.hover);
            kit::vline(ctx, row.left(), row.top(), row.bottom(), p.accent);
        }
        let baseline = kit::baseline(measurer, *row, m.font_ui);
        kit::text_right(
            ctx,
            measurer,
            &format!("{}", hit.line + 1),
            row.left() + LINE_W - 8.0,
            baseline,
            m.font_small,
            p.faint,
        );

        let path_width = (row.width() * 0.34).min(260.0);
        let display = state.quick_open_display(&hit.path);
        let display = kit::truncate_left(measurer, &display, m.font_small, path_width);
        kit::text_right(
            ctx,
            measurer,
            &display,
            row.right() - 10.0,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            p.faint,
        );

        let available = (row.width() - LINE_W - path_width - 24.0).max(40.0);
        let preview = kit::truncate(measurer, &hit.preview, m.font_ui, available);
        let x = row.left() + LINE_W;
        if preview == hit.preview && hit.end_in_line <= preview.len() {
            draw_preview(
                ctx,
                measurer,
                &preview,
                x,
                baseline,
                m.font_ui,
                &p,
                hit.start_in_line,
                hit.end_in_line,
                is_selected,
            );
        } else {
            kit::text(
                ctx,
                &preview,
                x,
                baseline,
                m.font_ui,
                if is_selected { p.text } else { p.dim },
            );
        }
    }

    if count == 0 {
        let message = if state.search.query.text().trim().is_empty() {
            "type to search the folder"
        } else if state.search_busy() {
            "searching…"
        } else {
            "no matches"
        };
        kit::text(
            ctx,
            message,
            layout.rows[0].left() + LINE_W,
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
    let status = if state.search_busy() {
        format!("scanning… {} files", state.search.files_scanned)
    } else {
        format!("{} results", count)
    };
    kit::text_right(
        ctx,
        measurer,
        &status,
        layout.footer.right(),
        kit::baseline(measurer, layout.footer, m.font_small),
        m.font_small,
        p.faint,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_preview(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    preview: &str,
    x: f32,
    baseline: f32,
    size: f32,
    p: &crate::theme::Palette,
    match_start: usize,
    match_end: usize,
    selected: bool,
) {
    let base = if selected { p.text } else { p.dim };
    let before = &preview[..match_start.min(preview.len())];
    let matched = &preview[match_start.min(preview.len())..match_end.min(preview.len())];
    let after = &preview[match_end.min(preview.len())..];
    let mut cursor = x;
    if !before.is_empty() {
        kit::text(ctx, before, cursor, baseline, size, base);
        cursor += kit::width(measurer, before, size);
    }
    if !matched.is_empty() {
        kit::text(ctx, matched, cursor, baseline, size, p.accent);
        cursor += kit::width(measurer, matched, size);
    }
    if !after.is_empty() {
        kit::text(ctx, after, cursor, baseline, size, base);
    }
}

fn paint_query(
    state: &AppState,
    measurer: &dyn TextMeasurer,
    ctx: &mut PaintContext,
    layout: &Layout,
) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let rect = layout.query;
    kit::fill(ctx, rect, p.bg);
    ctx.stroke_rect(rect, 1.0, p.accent);
    let inner = Rect::from_min_size(
        Vec2::new(rect.left() + 8.0, rect.top()),
        Size::new((rect.width() - 16.0).max(8.0), rect.height()),
    );
    ctx.save();
    ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    let query = &state.search.query;
    if query.text().is_empty() && state.search.preedit.is_none() {
        kit::text(
            ctx,
            "Search in folder…",
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
    if let Some(preedit) = &state.search.preedit {
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

fn paint_toggle(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    theme: &crate::theme::Theme,
    rect: Rect,
    label: &str,
    on: bool,
) {
    let p = theme.p;
    let m = theme.m;
    if on {
        kit::fill(ctx, rect, p.selection);
        ctx.stroke_rect(rect, 1.0, p.accent);
    } else {
        ctx.stroke_rect(rect, 1.0, p.border);
    }
    let width = kit::width(measurer, label, m.font_ui);
    kit::text(
        ctx,
        label,
        rect.center().x - width * 0.5,
        rect.center().y + 4.0,
        m.font_ui,
        if on { p.accent } else { p.dim },
    );
}

/// Pointer press. Returns `true` when consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.search.open {
        return false;
    }
    let layout = layout(state.viewport.logical_rect(), state.search.hits.len());
    if !layout.panel.contains(pos) {
        state.close_project_search();
        return true;
    }
    if layout.case_rect.contains(pos) {
        state.search.case_sensitive = !state.search.case_sensitive;
        state.search_query_changed();
        return true;
    }
    if layout.word_rect.contains(pos) {
        state.search.whole_word = !state.search.whole_word;
        state.search_query_changed();
        return true;
    }
    if layout.query.contains(pos) {
        return true;
    }
    for (index, row) in layout.rows.iter().enumerate() {
        if row.contains(pos) {
            let result = state.search.scroll + index;
            if result < state.search.hits.len() {
                state.search.selected = result;
                state.search_accept();
            }
            return true;
        }
    }
    true
}

/// Key press. `false` lets app shortcuts through.
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.search.open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    if accel {
        if modifiers.shift && matches!(key, Key::Character('f')) {
            state.close_project_search();
            return true;
        }
        return false;
    }
    let shift = modifiers.shift;
    match key {
        Key::Escape => state.close_project_search(),
        Key::Enter => state.search_accept(),
        Key::ArrowUp => state.search_move(-1),
        Key::ArrowDown => state.search_move(1),
        Key::Backspace => {
            state.search.query.backspace();
            state.search_query_changed();
        }
        Key::Delete => {
            state.search.query.delete();
            state.search_query_changed();
        }
        Key::ArrowLeft => state.search.query.move_left(shift, modifiers.alt),
        Key::ArrowRight => state.search.query.move_right(shift, modifiers.alt),
        Key::Home => state.search.query.move_home(shift),
        Key::End => state.search.query.move_end(shift),
        Key::Character('a') if accel => state.search.query.select_all(),
        _ => {}
    }
    state.dirty = true;
    true
}

pub fn on_text(state: &mut AppState, text: &str) {
    state.search.query.insert(text);
    state.search.preedit = None;
    state.search_query_changed();
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.search.preedit = Some(text.clone());
            state.dirty = true;
        }
        igui_core::ImeEvent::Commit(text) => on_text(state, text),
        igui_core::ImeEvent::Disabled => {
            state.search.preedit = None;
            state.dirty = true;
        }
        igui_core::ImeEvent::Enabled => {}
    }
}
