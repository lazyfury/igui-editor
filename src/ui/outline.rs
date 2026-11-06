//! Right-side document outline from LSP `textDocument/documentSymbol`.
//!
//! Symbols are stored as flat rows with a `depth`; clicking a row moves the
//! editor caret to the symbol's selection start and reveals it. In preview-only
//! mode there is no editor pane, so the click scrolls the rendered Markdown
//! pane to the symbol's line instead.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::{AppState, Focus, ViewMode};

pub const DEFAULT_WIDTH: f32 = 224.0;
pub const MIN_WIDTH: f32 = 160.0;
pub const MAX_WIDTH: f32 = 420.0;

pub fn clamp_width(width: f32) -> f32 {
    width.round().clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The dock: shared tab strip on top, symbol tree body below.
pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "outline");
    kit::set_container(tree, id, kit::tight_column());

    let header = crate::ui::dock::build_tabs(tree, id, state.clone(), crate::state::DockId::Right);
    kit::flex(tree, header, 0.0, Some(crate::ui::dock::HEADER_H), 0.0);
    kit::min_size(tree, header, 0.0, crate::ui::dock::HEADER_H);

    let body = kit::node(tree, id, "outline-body");
    kit::set_container(tree, body, Container::Leaf);
    kit::clip(tree, body, true);
    kit::flex(tree, body, 1.0, None, 1.0);
    kit::draw(tree, body, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });
    kit::on_scroll(tree, body, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let max = state.outline.scroll_max;
            crate::ui::scroll::apply(&mut state.outline.scroll, delta.y, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::on_pointer(tree, body, {
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

/// Shared drag handle on the outline's left seam.
pub fn build_divider(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "outline-divider");
    kit::set_container(tree, id, Container::Leaf);
    kit::set_cursor(tree, id, crate::ui::handle::cursor());
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, interact| {
            let mut state = state.borrow_mut();
            crate::ui::handle::paint(
                env,
                rect,
                &state.theme,
                interact.hovered,
                state.outline_drag.is_some(),
                crate::ui::handle::Axis::Vertical,
                crate::ui::handle::GripSide::Left,
                state.content_center_y,
            );
            state.outline.handle_rect = rect;
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => {
                    state.outline_drag = Some((pos.x, state.session.outline_width));
                }
                PointerPhase::Move => {
                    if let Some((start_x, start_width)) = state.outline_drag {
                        let width = clamp_width(start_width + (start_x - pos.x));
                        if (width - state.session.outline_width).abs() > 0.5 {
                            state.session.outline_width = width;
                            if let Some(outline) = state.outline_node {
                                kit::flex(tree, outline, 0.0, Some(width), 0.0);
                                kit::min_size(tree, outline, width, 0.0);
                                igui_ui::mark_dirty(tree, outline);
                            }
                            state.config_dirty = true;
                            state.dirty = true;
                        }
                    }
                }
                PointerPhase::Up => {
                    if state.outline_drag.take().is_some() {
                        state.config_dirty = true;
                    }
                }
                PointerPhase::DoubleClick => {
                    state.outline_drag = None;
                    state.session.outline_width = DEFAULT_WIDTH;
                    if let Some(outline) = state.outline_node {
                        kit::flex(tree, outline, 0.0, Some(DEFAULT_WIDTH), 0.0);
                        kit::min_size(tree, outline, DEFAULT_WIDTH, 0.0);
                        igui_ui::mark_dirty(tree, outline);
                    }
                    state.config_dirty = true;
                    state.dirty = true;
                }
            }
            igui_ui::request_paint(tree);
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

    let path = state
        .ws
        .active_doc()
        .and_then(|doc| doc.path())
        .map(|path| path.to_path_buf());
    let symbols = path
        .as_deref()
        .map(|path| state.outline_symbols_for(path))
        .unwrap_or_default();
    let count = symbols.len();
    let list = rect;
    let content_h = count as f32 * m.row_h;
    state.outline.scroll_max = (content_h - list.height()).max(0.0);
    crate::ui::scroll::clamp(&mut state.outline.scroll, state.outline.scroll_max);
    state.outline.rows.clear();

    if count == 0 {
        let text = match (state.ws.active_doc(), state.lsp.status()) {
            (None, _) => "No document",
            (Some(_), crate::model::lsp::LspStatus::Starting) => "Indexing…",
            (Some(_), crate::model::lsp::LspStatus::Error(_error)) => "LSP error",
            (Some(_), _) => "No symbols",
        };
        kit::text(
            env.ctx,
            text,
            rect.left() + crate::ui::tree::PAD,
            list.top() + 24.0,
            m.font_ui,
            p.faint,
        );
        if let crate::model::lsp::LspStatus::Error(error) = state.lsp.status() {
            let error = kit::truncate(
                env.measurer,
                error,
                m.font_ui,
                (rect.width() - crate::ui::tree::PAD * 2.0).max(20.0),
            );
            kit::text(
                env.ctx,
                &error,
                rect.left() + crate::ui::tree::PAD,
                list.top() + 46.0,
                m.font_ui,
                p.error,
            );
        }
        return;
    }

    let row_h = m.row_h;
    // Match the Explorer's selected row to the symbol the caret is inside
    // (the nearest preceding document symbol).
    let caret_line = state
        .ws
        .active_doc()
        .map(|doc| doc.line_col().0)
        .unwrap_or(0);
    let current = symbols
        .iter()
        .rposition(|symbol| symbol.line as usize <= caret_line);
    let first = (state.outline.scroll / row_h).floor().max(0.0) as usize;
    let visible = (list.height() / row_h).ceil().max(1.0) as usize + 1;
    env.ctx.save();
    env.ctx.clip_rect(list);
    for (index, symbol) in symbols.iter().enumerate().skip(first).take(visible) {
        let y = list.top() + index as f32 * row_h - state.outline.scroll;
        let row = Rect::from_min_size(Vec2::new(list.left(), y), Size::new(list.width(), row_h));
        state.outline.rows.push((index, row));
        let selected = current == Some(index);
        let hovered = state.outline.hover == Some(index);
        let color = if selected || hovered { p.text } else { p.dim };
        crate::ui::tree::paint_row(
            env.ctx,
            &state.emoji,
            env.measurer,
            row,
            &m,
            &p,
            &crate::ui::tree::Row {
                depth: symbol.depth,
                marker: crate::ui::tree::Marker::Glyph {
                    text: symbol_glyph(symbol.kind),
                    color: symbol_color(symbol.kind, &p),
                },
                label: &symbol.name,
                color,
                selected,
                hovered,
                trailing: None,
            },
        );
    }
    env.ctx.restore();

    // Match the Explorer's slim scrollbar so both trees read the same.
    if state.outline.scroll_max > 0.0 {
        let track = Rect::from_min_size(
            Vec2::new(rect.right() - 4.0, list.top()),
            Size::new(3.0, list.height()),
        );
        let thumb_h = (list.height() * list.height() / content_h)
            .min(list.height())
            .max(1.0);
        let thumb_y = track.top()
            + (state.outline.scroll / state.outline.scroll_max) * (track.height() - thumb_h);
        kit::fill(
            env.ctx,
            Rect::from_min_size(Vec2::new(track.left(), thumb_y), Size::new(3.0, thumb_h)),
            p.faint,
        );
    }
}

fn symbol_glyph(kind: u32) -> &'static str {
    match kind {
        crate::state::MARKDOWN_HEADING_KIND => "H",
        2..=4 => "N",
        5 | 10 | 11 | 23 => "T",
        6 | 12 | 9 => "ƒ",
        7 | 8 => "P",
        13 => "v",
        14 => "c",
        // Godot reports GDScript signals as SymbolKind.Event (24).
        24 => "S",
        22 => "e",
        26 => "t",
        _ => "·",
    }
}

fn symbol_color(kind: u32, p: &crate::theme::Palette) -> igui_core::Color {
    match kind {
        crate::state::MARKDOWN_HEADING_KIND => p.syntax.keyword,
        5 | 10 | 11 | 23 | 26 => p.syntax.type_name,
        6 | 12 | 9 => p.syntax.function,
        7 | 8 | 24 => p.syntax.property,
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
    let symbol = state.outline_symbols_for(&path).get(index).cloned();
    if let Some(symbol) = symbol {
        let byte = state
            .ws
            .active_doc()
            .map(|doc| crate::model::lsp::byte_for_position(doc, symbol.line, symbol.character));
        if let (Some(doc), Some(byte)) = (state.ws.active_doc_mut(), byte) {
            doc.set_caret(byte);
        }
        // Preview-only: jump the rendered pane. The editor has no painter in
        // that mode, so `editor.reveal` alone would do nothing visible.
        if state.view_of_active() == ViewMode::Preview {
            state.preview.reveal_line = Some(symbol.line as usize);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdscript_signals_get_their_own_marker() {
        assert_eq!(symbol_glyph(24), "S");
        assert_ne!(symbol_glyph(24), symbol_glyph(13));
        let p = crate::theme::Theme::dark().p;
        assert_eq!(symbol_color(24, &p), p.syntax.property);
    }

    #[test]
    fn markdown_headings_get_their_own_marker() {
        assert_eq!(symbol_glyph(crate::state::MARKDOWN_HEADING_KIND), "H");
        let p = crate::theme::Theme::dark().p;
        assert_eq!(
            symbol_color(crate::state::MARKDOWN_HEADING_KIND, &p),
            p.syntax.keyword
        );
    }
}
