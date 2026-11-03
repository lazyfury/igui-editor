//! Native terminal panel: vt100 screen painted as a cell grid.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Key, Modifiers, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::state::{AppState, Focus};

pub const HEIGHT: f32 = 220.0;
const HEADER: f32 = 22.0;

pub fn build_handle(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "terminal-handle");
    kit::set_container(tree, id, Container::Leaf);
    kit::set_cursor(
        tree,
        id,
        crate::ui::handle::cursor_for(crate::ui::handle::Axis::Horizontal),
    );
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, interact| {
            let mut state = state.borrow_mut();
            state.terminal.handle_rect = rect;
            let grip_x = if state.terminal.rect.width() > 0.0 {
                Some(state.terminal.rect.center().x)
            } else {
                None
            };
            crate::ui::handle::paint(
                env,
                rect,
                &state.theme,
                interact.hovered,
                state.terminal.drag.is_some(),
                crate::ui::handle::Axis::Horizontal,
                crate::ui::handle::GripSide::Down,
                grip_x,
            );
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => {
                    state.terminal.drag = Some((pos.y, state.terminal.height));
                }
                PointerPhase::Move => {
                    if let Some((start_y, start_height)) = state.terminal.drag {
                        let height = (start_height + (start_y - pos.y)).clamp(120.0, 600.0);
                        if (height - state.terminal.height).abs() > 0.5 {
                            state.terminal.height = height;
                            if let Some(node) = state.terminal.node {
                                kit::flex(tree, node, 0.0, Some(height), 0.0);
                                kit::min_size(tree, node, 0.0, height);
                                igui_ui::mark_dirty(tree, node);
                            }
                            state.dirty = true;
                        }
                    }
                }
                PointerPhase::Up => state.finish_terminal_resize(),
                PointerPhase::DoubleClick => {
                    state.terminal.height = HEIGHT;
                    state.finish_terminal_resize();
                    state.needs_rebuild = true;
                }
            }
            igui_ui::request_paint(tree);
        }
    });
    id
}

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "terminal");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });
    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, _pos| {
            if phase == PointerPhase::Down {
                let mut state = state.borrow_mut();
                state.focus = Focus::Terminal;
                state.terminal.focused = true;
                state.dirty = true;
            }
            igui_ui::request_paint(tree);
        }
    });
    id
}

fn ansi_color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => {
            Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
        }
        vt100::Color::Idx(index) => match index {
            0 => Color::new(0.0, 0.0, 0.0, 1.0),
            1 => Color::new(0.8, 0.2, 0.2, 1.0),
            2 => Color::new(0.2, 0.75, 0.35, 1.0),
            3 => Color::new(0.85, 0.65, 0.2, 1.0),
            4 => Color::new(0.25, 0.45, 0.85, 1.0),
            5 => Color::new(0.7, 0.35, 0.8, 1.0),
            6 => Color::new(0.25, 0.7, 0.8, 1.0),
            7 => Color::new(0.8, 0.8, 0.8, 1.0),
            8 => Color::new(0.4, 0.4, 0.4, 1.0),
            15 => Color::new(0.95, 0.95, 0.95, 1.0),
            n => {
                let v = (n as f32 / 255.0 * 0.9 + 0.1).clamp(0.0, 1.0);
                Color::new(v, v, v, 1.0)
            }
        },
    }
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let focused = state.focus == Focus::Terminal;
    kit::fill(env.ctx, rect, p.bg);
    kit::text(
        env.ctx,
        "TERMINAL",
        rect.left() + 10.0,
        rect.top() + 15.0,
        m.font_small,
        if focused { p.accent } else { p.faint },
    );
    let cwd = state.terminal.cwd.display().to_string();
    kit::text_right(
        env.ctx,
        env.measurer,
        &kit::truncate_left(env.measurer, &cwd, m.font_small, rect.width() - 120.0),
        rect.right() - 10.0,
        rect.top() + 15.0,
        m.font_small,
        p.faint,
    );
    let body = Rect::from_min_size(
        Vec2::new(rect.left() + 8.0, rect.top() + HEADER),
        Size::new(
            (rect.width() - 16.0).max(16.0),
            (rect.height() - HEADER).max(16.0),
        ),
    );
    state.terminal.rect = body;
    let cell_w = env.measurer.advance(' ', m.font_ui).max(1.0);
    let line_h = (m.font_ui + 4.0).max(16.0);
    let cols = (body.width() / cell_w).floor().max(8.0) as u16;
    let rows = (body.height() / line_h).floor().max(2.0) as u16;
    if cols != state.terminal.cols || rows != state.terminal.rows {
        state.terminal.cols = cols;
        state.terminal.rows = rows;
        if let Some(parser) = state.terminal.parser.as_mut() {
            parser.set_size(
                std::num::NonZeroU16::new(rows).unwrap(),
                std::num::NonZeroU16::new(cols).unwrap(),
            );
        }
        if let Some(session) = state.terminal.session.as_mut() {
            session.resize(rows, cols);
        }
    }
    let Some(parser) = state.terminal.parser.as_ref() else {
        return;
    };
    let screen = parser.screen();
    env.ctx.save();
    env.ctx.clip_rect(body);
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let fg = ansi_color(cell.fgcolor(), p.text);
            let bg = ansi_color(cell.bgcolor(), p.bg);
            if bg != p.bg {
                env.ctx.fill_rect(
                    Rect::from_min_size(
                        Vec2::new(
                            body.left() + col as f32 * cell_w,
                            body.top() + row as f32 * line_h,
                        ),
                        Size::new(cell_w, line_h),
                    ),
                    bg,
                );
            }
            let text = cell.contents();
            if !text.is_empty() && text != " " {
                env.ctx.draw_text_weighted(
                    text,
                    Vec2::new(
                        body.left() + col as f32 * cell_w,
                        body.top() + row as f32 * line_h + line_h - 4.0,
                    ),
                    m.font_ui,
                    igui_core::FontWeight::NORMAL,
                    igui_render::TextAlign::Left,
                    fg,
                );
            }
        }
    }
    let (crow, ccol) = screen.cursor_position();
    let crow = crow.min(rows.saturating_sub(1));
    let ccol = ccol.min(cols.saturating_sub(1));
    let caret = Rect::from_min_size(
        Vec2::new(
            body.left() + ccol as f32 * cell_w,
            body.top() + crow as f32 * line_h,
        ),
        Size::new(cell_w.max(2.0), line_h),
    );
    state.terminal.caret_rect = caret;
    env.ctx.fill_rect(
        Rect::from_min_size(caret.origin, Size::new(2.0, line_h)),
        if focused { p.accent } else { p.faint },
    );
    env.ctx.restore();
}

/// Encode a key for the PTY. Plain text is handled by TextInput; this covers
/// named keys and control combinations.
pub fn encode_key(key: Key, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = modifiers.ctrl && !modifiers.meta;
    let alt = modifiers.alt;
    let mut bytes = Vec::new();
    if alt {
        bytes.push(0x1b);
    }
    let bytes = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace => vec![0x7f],
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Escape => vec![0x1b],
        Key::Space => b" ".to_vec(),
        Key::ArrowUp if application_cursor => b"\x1bOA".to_vec(),
        Key::ArrowDown if application_cursor => b"\x1bOB".to_vec(),
        Key::ArrowRight if application_cursor => b"\x1bOC".to_vec(),
        Key::ArrowLeft if application_cursor => b"\x1bOD".to_vec(),
        Key::ArrowUp => b"\x1b[A".to_vec(),
        Key::ArrowDown => b"\x1b[B".to_vec(),
        Key::ArrowRight => b"\x1b[C".to_vec(),
        Key::ArrowLeft => b"\x1b[D".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::Character(ch) if ctrl => {
            let lower = ch.to_ascii_lowercase();
            if lower.is_ascii_alphabetic() {
                vec![lower as u8 - b'a' + 1]
            } else if ch == '[' {
                vec![0x1b]
            } else if ch == '\\' {
                vec![0x1c]
            } else if ch == ']' {
                vec![0x1d]
            } else {
                return None;
            }
        }
        Key::Character(ch) if alt => ch.to_string().into_bytes(),
        _ => return None,
    };
    if alt && !bytes.starts_with(&[0x1b]) {
        let mut prefixed = vec![0x1b];
        prefixed.extend(bytes);
        Some(prefixed)
    } else {
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_cursor_sequences_follow_vim_mode() {
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::NONE, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(
                Key::Character('c'),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
                false
            ),
            Some(vec![3])
        );
    }
}
