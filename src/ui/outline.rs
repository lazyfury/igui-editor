//! Right-side document outline from LSP `textDocument/documentSymbol`.
//!
//! Symbols are stored as flat rows with a `depth`; clicking a row moves the
//! editor caret to the symbol's selection start and reveals it.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::{AppState, Focus};

pub const WIDTH: f32 = 224.0;
pub const HEADER_H: f32 = 28.0;
const ROW_H: f32 = 20.0;
const PAD: f32 = 10.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "outline");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let max = state.outline.scroll_max;
            crate::ui::scroll::apply(&mut state.outline.scroll, delta.y, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            if phase != PointerPhase::Down {
                return;
            }
            let mut state = state.borrow_mut();
            if click(&mut state, pos) {
                igui_ui::request_paint(tree);
            }
        }
    });
    id
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    state.outline.rect = rect;
    kit::fill(env.ctx, rect, p.panel);
    kit::vline(env.ctx, rect.left(), rect.top(), rect.bottom(), p.border);

    let header = Rect::from_min_size(rect.origin, Size::new(rect.width(), HEADER_H));
    kit::hline(
        env.ctx,
        header.left(),
        header.right(),
        header.bottom(),
        p.border,
    );
    kit::text(
        env.ctx,
        "OUTLINE",
        rect.left() + PAD,
        header.bottom() - 9.0,
        m.font_small,
        p.dim,
    );

    let path = state
        .ws
        .active_doc()
        .and_then(|doc| doc.path())
        .map(|path| path.to_path_buf());
    let symbols = path
        .as_deref()
        .map(|path| state.lsp.symbols_for(path).to_vec())
        .unwrap_or_default();
    let count = symbols.len();
    if count > 0 {
        kit::text_right(
            env.ctx,
            env.measurer,
            &count.to_string(),
            rect.right() - PAD,
            header.bottom() - 9.0,
            m.font_small,
            p.faint,
        );
    }

    let list = Rect::from_min_size(
        Vec2::new(rect.left(), header.bottom()),
        Size::new(rect.width(), (rect.height() - HEADER_H).max(0.0)),
    );
    let content_h = count as f32 * ROW_H;
    state.outline.scroll_max = (content_h - list.height()).max(0.0);
    crate::ui::scroll::clamp(&mut state.outline.scroll, state.outline.scroll_max);
    state.outline.rows.clear();

    if count == 0 {
        let text = match (state.ws.active_doc(), state.lsp.status()) {
            (None, _) => "No document",
            (Some(_), crate::model::lsp::LspStatus::Starting) => "Indexing…",
            (Some(_), _) => "No symbols",
        };
        kit::text(
            env.ctx,
            text,
            rect.left() + PAD,
            list.top() + 22.0,
            m.font_small,
            p.faint,
        );
        return;
    }

    let first = (state.outline.scroll / ROW_H).floor().max(0.0) as usize;
    let visible = (list.height() / ROW_H).ceil().max(1.0) as usize + 1;
    env.ctx.save();
    env.ctx.clip_rect(list);
    for (index, symbol) in symbols.iter().enumerate().skip(first).take(visible) {
        let y = list.top() + index as f32 * ROW_H - state.outline.scroll;
        let row = Rect::from_min_size(Vec2::new(list.left(), y), Size::new(list.width(), ROW_H));
        state.outline.rows.push((index, row));
        if state.outline.hover == Some(index) {
            kit::fill(env.ctx, row, p.hover);
        }
        let color = symbol_color(symbol.kind, &p);
        let indent = PAD + symbol.depth as f32 * 11.0;
        kit::text(
            env.ctx,
            symbol_glyph(symbol.kind),
            list.left() + indent,
            kit::baseline(env.measurer, row, m.font_small),
            m.font_small,
            color,
        );
        let name_x = list.left() + indent + 16.0;
        let width = (row.right() - 8.0 - name_x).max(20.0);
        let name = kit::truncate(env.measurer, &symbol.name, m.font_small, width);
        kit::text(
            env.ctx,
            &name,
            name_x,
            kit::baseline(env.measurer, row, m.font_small),
            m.font_small,
            if state.outline.hover == Some(index) {
                p.text
            } else {
                p.dim
            },
        );
    }
    env.ctx.restore();
}

fn symbol_glyph(kind: u32) -> &'static str {
    match kind {
        2..=4 => "N",
        5 | 10 | 11 | 23 => "T",
        6 | 12 | 9 => "ƒ",
        7 | 8 => "P",
        13 => "v",
        14 => "c",
        22 => "e",
        26 => "t",
        _ => "·",
    }
}

fn symbol_color(kind: u32, p: &crate::theme::Palette) -> igui_core::Color {
    match kind {
        5 | 10 | 11 | 23 | 26 => p.syntax.type_name,
        6 | 12 | 9 => p.syntax.function,
        7 | 8 => p.syntax.property,
        13 => p.syntax.variable,
        14 | 22 => p.syntax.constant,
        2..=4 => p.syntax.keyword,
        _ => p.faint,
    }
}

/// Handles a click in the outline panel. Returns whether it consumed the input.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.show_outline || !state.outline.rect.contains(pos) {
        return false;
    }
    let Some((index, _)) = state
        .outline
        .rows
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .copied()
    else {
        return true;
    };
    let Some(path) = state
        .ws
        .active_doc()
        .and_then(|doc| doc.path().map(|path| path.to_path_buf()))
    else {
        return true;
    };
    let symbol = state.lsp.symbols_for(&path).get(index).cloned();
    if let Some(symbol) = symbol {
        let byte = state
            .ws
            .active_doc()
            .map(|doc| crate::model::lsp::byte_for_position(doc, symbol.line, symbol.character));
        if let (Some(doc), Some(byte)) = (state.ws.active_doc_mut(), byte) {
            doc.set_caret(byte);
        }
        state.editor.reveal = true;
        state.focus = Focus::Editor;
        state.dirty = true;
    }
    true
}

/// Updates the hovered outline row for pointer feedback.
pub fn hover_at(state: &mut AppState, pos: Vec2) {
    let hover = if state.show_outline && state.outline.rect.contains(pos) {
        state
            .outline
            .rows
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(index, _)| *index)
    } else {
        None
    };
    if state.outline.hover != hover {
        state.outline.hover = hover;
        state.dirty = true;
    }
}
