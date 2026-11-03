//! Compact LSP problems popup.
//!
//! The editor already paints diagnostic underlines and gutter markers; this
//! popup is the actual message UI. It opens from the footer `LSP 1E 1W` chip,
//! lists every diagnostic for the active document, and jumps to a row on click.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::model::lsp::{byte_for_position, Diagnostic, LspStatus};
use crate::state::{AppState, Focus};

const WIDTH: f32 = 680.0;
const MAX_ROWS: usize = 14;
const HEADER_H: f32 = 30.0;
const ROW_H: f32 = 25.0;
const FOOTER_H: f32 = 16.0;
const PAD: f32 = 10.0;
const GAP: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Outside,
    Consumed,
    Jump(usize),
}

pub fn diagnostics(state: &AppState) -> Vec<Diagnostic> {
    let Some(path) = state.ws.active_doc().and_then(|doc| doc.path()) else {
        return Vec::new();
    };
    state.lsp.diagnostics_for(path).to_vec()
}

pub fn toggle(state: &mut AppState) {
    if state.problems.open {
        close(state);
    } else {
        open(state);
    }
}

pub fn open(state: &mut AppState) {
    if diagnostics(state).is_empty() {
        if let LspStatus::Error(error) = state.lsp.status() {
            state.error(format!("LSP: {error}"));
        } else {
            state.status("no LSP diagnostics");
        }
        return;
    }
    state.problems.open = true;
    state.problems.hover = None;
    state.problems.scroll = 0.0;
    state.lsp.dismiss_transient();
    state.dirty = true;
}

pub fn close(state: &mut AppState) {
    if state.problems.open {
        state.problems.open = false;
        state.problems.hover = None;
        state.dirty = true;
    }
}

/// Panel rectangle and visible `(diagnostic index, row)` hit rects. Uses the
/// same geometry for paint, clicks and hover so there is no second layout.
pub fn layout(state: &AppState) -> (Rect, Vec<(usize, Rect)>) {
    let diagnostics = diagnostics(state);
    if diagnostics.is_empty() {
        return (Rect::ZERO, Vec::new());
    }
    let viewport = state.viewport.logical_rect();
    let width = (viewport.width() - 32.0).clamp(320.0, WIDTH);
    let rows = diagnostics.len().min(MAX_ROWS);
    let height = HEADER_H + rows as f32 * ROW_H + FOOTER_H;
    let bottom = viewport.bottom() - state.theme.m.status_h - GAP;
    let x = (viewport.center().x - width * 0.5).clamp(
        viewport.left() + 8.0,
        (viewport.right() - width - 8.0).max(8.0),
    );
    let y = (bottom - height).max(viewport.top() + 8.0);
    let rect = Rect::from_min_size(Vec2::new(x, y), Size::new(width, height));

    let list_h = (height - HEADER_H - FOOTER_H).max(0.0);
    let scroll = state.problems.scroll;
    let first = (scroll / ROW_H).floor().max(0.0) as usize;
    let mut hits = Vec::new();
    for index in first..diagnostics.len() {
        let row_y = rect.top() + HEADER_H + (index - first) as f32 * ROW_H - scroll % ROW_H;
        if row_y >= rect.top() + HEADER_H + list_h {
            break;
        }
        if row_y + ROW_H < rect.top() + HEADER_H {
            continue;
        }
        hits.push((
            index,
            Rect::from_min_size(
                Vec2::new(rect.left(), row_y),
                Size::new(rect.width(), ROW_H),
            ),
        ));
    }
    (rect, hits)
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    let diagnostics = diagnostics(state);
    if diagnostics.is_empty() {
        state.problems.open = false;
        state.problems.rect = Rect::ZERO;
        state.problems.hits.clear();
        state.problems.scroll_max = 0.0;
        return;
    }
    if !state.problems.open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let (rect, hits) = layout(state);
    let list_h = (rect.height() - HEADER_H - FOOTER_H).max(0.0);
    state.problems.rect = rect;
    state.problems.hits = hits.clone();
    state.problems.scroll_max = (diagnostics.len() as f32 * ROW_H - list_h).max(0.0);
    crate::ui::scroll::clamp(&mut state.problems.scroll, state.problems.scroll_max);

    kit::fill(ctx, rect, p.raised);
    ctx.stroke_rect(rect, 1.0, p.border);
    let header = Rect::from_min_size(rect.origin, Size::new(rect.width(), HEADER_H));
    kit::hline(
        ctx,
        header.left(),
        header.right(),
        header.bottom(),
        p.border,
    );

    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == 1)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == 2)
        .count();
    let count = if errors == 0 && warnings == 0 {
        format!("{} problem(s)", diagnostics.len())
    } else {
        format!("{errors} error(s)  {warnings} warning(s)")
    };
    kit::text(
        ctx,
        "PROBLEMS",
        rect.left() + PAD,
        header.bottom() - 9.0,
        m.font_small,
        p.dim,
    );
    kit::text_right(
        ctx,
        measurer,
        &count,
        rect.right() - PAD,
        header.bottom() - 9.0,
        m.font_small,
        p.faint,
    );

    for (index, row) in &hits {
        let diagnostic = &diagnostics[*index];
        let color = state.lsp.diagnostic_color(diagnostic.severity, &p);
        if state.problems.hover == Some(*index) {
            kit::fill(ctx, *row, p.hover);
            kit::vline(ctx, row.left(), row.top(), row.bottom(), color);
        }
        let glyph = match diagnostic.severity {
            1 => "E",
            2 => "W",
            3 => "I",
            _ => "H",
        };
        kit::text(
            ctx,
            glyph,
            row.left() + PAD,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            color,
        );
        let position = format!(
            "{}:{}",
            diagnostic.start_line + 1,
            diagnostic.start_char + 1
        );
        kit::text(
            ctx,
            &position,
            row.left() + PAD + 18.0,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            p.faint,
        );
        let message_x =
            row.left() + PAD + 18.0 + kit::width(measurer, &position, m.font_small) + 12.0;
        let message = kit::truncate(
            measurer,
            &diagnostic.message,
            m.font_small,
            (row.right() - message_x - PAD).max(24.0),
        );
        kit::text(
            ctx,
            &message,
            message_x,
            kit::baseline(measurer, *row, m.font_small),
            m.font_small,
            p.text,
        );
    }

    let footer = Rect::from_min_size(
        Vec2::new(rect.left() + PAD, rect.bottom() - FOOTER_H),
        Size::new(rect.width() - PAD * 2.0, FOOTER_H),
    );
    kit::text(
        ctx,
        "click a row to jump · Esc close",
        footer.left(),
        footer.bottom() - 4.0,
        m.font_small,
        p.faint,
    );
}

pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    if !state.problems.open {
        return None;
    }
    state
        .problems
        .hits
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(index, _)| *index)
}

pub fn click(state: &mut AppState, pos: Vec2) -> Click {
    if !state.problems.open {
        return Click::Outside;
    }
    if !state.problems.rect.contains(pos) {
        close(state);
        return Click::Outside;
    }
    if let Some(index) = hover_at(state, pos) {
        jump(state, index);
        return Click::Jump(index);
    }
    Click::Consumed
}

pub fn wheel(state: &mut AppState, delta: f32) {
    if !state.problems.open {
        return;
    }
    state.problems.scroll =
        (state.problems.scroll + delta * ROW_H).clamp(0.0, state.problems.scroll_max);
    state.dirty = true;
}

pub fn jump(state: &mut AppState, index: usize) {
    let diagnostics = diagnostics(state);
    let Some(diagnostic) = diagnostics.get(index) else {
        return;
    };
    let (line, character) = (diagnostic.start_line as u32, diagnostic.start_char as u32);
    let Some(doc) = state.ws.active_doc_mut() else {
        return;
    };
    let byte = byte_for_position(doc, line, character);
    doc.set_caret(byte);
    state.focus = Focus::Editor;
    state.editor.reveal = true;
    state.problems.open = false;
    state.problems.hover = None;
    state.dirty = true;
}
