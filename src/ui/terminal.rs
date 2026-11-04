//! Native terminal panel: vt100 screen painted as a cell grid.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Key, Modifiers, NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::state::{AppState, Focus};
use crate::ui::icon::{self, Icon};

pub const HEIGHT: f32 = 220.0;
const HEADER: f32 = 28.0;

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
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            // One wheel line per row; trackpads report fractional deltas.
            let rows = (-delta.y).round() as isize;
            if rows != 0 {
                state.terminal_scroll(rows);
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

/// One column's advance: the widest thing a single cell can paint.
///
/// With a fixed-pitch family this is the family's advance. The CJK/emoji
/// fallback may be wider, so it must fit in two columns or it would overlap
/// the next cell.
fn cell_width(measurer: &dyn TextMeasurer, font: f32, family: Option<&str>) -> f32 {
    // Wide glyphs are painted with the UI family (see `paint`), so measure the
    // wide probe with that family too. Asking the fixed-pitch face for a Han
    // glyph makes the font server probe every fallback face and load each font
    // file (tens of MB) just to answer the advance.
    measurer
        .advance_in('M', font, family)
        .max(crate::fonts::advance(measurer, '中', font, family) / 2.0)
        .max(1.0)
}

/// Left offset that centers a wide glyph in its two-column cell.
///
/// A wide glyph is drawn at the same em as everything else (font sizes must
/// stay comparable), but its advance is narrower than two fixed-pitch columns,
/// so the leftover space is split evenly instead of piling up on the right.
/// The result stays inside the cell.
fn wide_glyph_offset(advance: f32, width: f32) -> f32 {
    ((width - advance) * 0.5).max(0.0)
}

/// Rect of cell `(row, col)` in `body`. `wide` cells span two columns.
fn cell_rect(body: Rect, cell_w: f32, line_h: f32, row: u16, col: u16, wide: bool) -> Rect {
    Rect::from_min_size(
        Vec2::new(
            body.left() + col as f32 * cell_w,
            body.top() + row as f32 * line_h,
        ),
        Size::new(if wide { cell_w * 2.0 } else { cell_w }, line_h),
    )
}

/// Caret rect for a screen cursor position, on the same grid as [`cell_rect`].
fn caret_rect(body: Rect, cell_w: f32, line_h: f32, row: u16, col: u16) -> Rect {
    Rect::from_min_size(
        Vec2::new(
            body.left() + col as f32 * cell_w,
            body.top() + row as f32 * line_h,
        ),
        Size::new(cell_w.max(2.0), line_h),
    )
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let focused = state.focus == Focus::Terminal;
    kit::fill(env.ctx, rect, p.bg);
    let icon_rect = Rect::from_min_size(
        Vec2::new(
            rect.left() + 10.0,
            rect.top() + (HEADER - icon::SMALL) * 0.5,
        ),
        Size::new(icon::SMALL, icon::SMALL),
    );
    icon::paint(
        env.ctx,
        Icon::Terminal,
        icon_rect,
        if focused { p.accent } else { p.faint },
    );
    kit::text(
        env.ctx,
        "TERMINAL",
        rect.left() + 10.0 + icon::SMALL + 5.0,
        rect.top() + 18.0,
        m.font_small,
        if focused { p.accent } else { p.faint },
    );
    let cwd = state.terminal.cwd.display().to_string();
    kit::text_right(
        env.ctx,
        env.measurer,
        &kit::truncate_left(env.measurer, &cwd, m.font_small, rect.width() - 120.0),
        rect.right() - 10.0,
        rect.top() + 18.0,
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

    let editor = state.editor_metrics();
    let font = editor.font;
    let line_h = editor.line_h.max(font + 4.0);
    // The terminal is the one part of the UI that paints a cell grid, so it
    // asks for a fixed-pitch family (see `crate::fonts`) instead of the
    // editor's UI face. Probing needs the backend measurer, which only exists
    // while painting, hence the lazy + cached resolution.
    if !state.terminal.font_checked {
        state.terminal.font = crate::fonts::mono_family(env.measurer, font);
        // A failed probe is retried next frame: the first paint may run with
        // the approximate measurer before the host installs the real one.
        state.terminal.font_checked = state.terminal.font.is_some();
    }
    let family = state.terminal.font.clone();
    let family = family.as_deref();
    let cell_w = cell_width(env.measurer, font, family);
    let cols = (body.width() / cell_w).floor().max(8.0) as u16;
    let rows = (body.height() / line_h).floor().max(2.0) as u16;

    // The parser's screen is the source of truth for the grid: a freshly
    // spawned session starts at the *previous* session's size, which may not
    // match the current panel (reopening after a resize). Comparing against
    // the cached `rows/cols` instead skips this resize and leaves the shell
    // painting into a clipped screen.
    let mut resized = false;
    let Some(parser) = state.terminal.parser.as_mut() else {
        return;
    };
    let (screen_rows, screen_cols) = parser.screen().size();
    if rows != screen_rows.get() || cols != screen_cols.get() {
        parser.set_size(
            std::num::NonZeroU16::new(rows).unwrap(),
            std::num::NonZeroU16::new(cols).unwrap(),
        );
        resized = true;
    }
    if resized {
        if let Some(session) = state.terminal.session.as_mut() {
            session.resize(rows, cols);
        }
    }
    state.terminal.rows = rows;
    state.terminal.cols = cols;
    state.terminal.cell_w = cell_w;

    let Some(parser) = state.terminal.parser.as_mut() else {
        return;
    };
    let max_scrollback = parser.screen().scrollback_len();
    state.terminal.scrollback = state.terminal.scrollback.min(max_scrollback);
    parser
        .screen_mut()
        .set_scrollback(state.terminal.scrollback);
    let screen = parser.screen();
    let rows = rows.min(screen.size().0.get());
    let cols = cols.min(screen.size().1.get());
    env.ctx.save();
    env.ctx.clip_rect(body);
    let text_ascent = env.measurer.ascent_in(font, family);
    for row in 0..rows {
        let y = body.top() + row as f32 * line_h;
        let baseline = kit::line_baseline_with_ascent(y, line_h, font, text_ascent);
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            // A wide glyph owns both cells; the continuation cell is painted
            // as part of the lead cell so backgrounds tile exactly.
            if cell.is_wide_continuation() {
                continue;
            }
            let rect = cell_rect(body, cell_w, line_h, row, col, cell.is_wide());
            let fg = ansi_color(cell.fgcolor(), p.text);
            let bg = ansi_color(cell.bgcolor(), p.bg);
            if bg != p.bg {
                env.ctx.fill_rect(rect, bg);
            }
            let text = cell.contents();
            if !text.is_empty() && text != " " {
                // Wide glyphs (CJK, full-width punctuation) come from the UI
                // family: the fixed-pitch terminal face has no CJK coverage,
                // and the generic first-covering fallback is often a
                // Korean/Japanese face with the wrong regional glyph shapes.
                // Same em as the Latin cells; centered in the two columns.
                let (draw_family, draw_x) = if cell.is_wide() {
                    // Wide glyphs come from the UI family, same as the
                    // editor (the mono face usually has no Han coverage).
                    let wide_family = None;
                    let advance = env.measurer.measure_run_in(text, font, wide_family);
                    (
                        wide_family,
                        rect.left() + wide_glyph_offset(advance, rect.width()),
                    )
                } else {
                    (family, rect.left())
                };
                env.ctx.draw_text_in(
                    text,
                    Vec2::new(draw_x, baseline),
                    font,
                    igui_core::FontWeight::NORMAL,
                    igui_render::TextAlign::Left,
                    draw_family,
                    fg,
                );
            }
        }
    }
    let (crow, ccol) = screen.cursor_position();
    let crow = crow.min(rows.saturating_sub(1));
    let ccol = ccol.min(cols.saturating_sub(1));
    let caret = caret_rect(body, cell_w, line_h, crow, ccol);
    state.terminal.caret_rect = if state.terminal.scrollback == 0 {
        caret
    } else {
        Rect::ZERO
    };
    if state.terminal.scrollback == 0 {
        env.ctx.fill_rect(
            Rect::from_min_size(caret.origin, Size::new(2.0, line_h)),
            if focused { p.accent } else { p.faint },
        );
    }
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

    /// A measurer with one fixed-pitch family and a proportional default.
    struct Probe;

    impl TextMeasurer for Probe {
        fn advance(&self, ch: char, font_size: f32) -> f32 {
            if ch == '中' {
                font_size
            } else {
                font_size * 0.55
            }
        }

        fn line_height(&self, font_size: f32) -> f32 {
            font_size * 1.4
        }

        fn advance_in(&self, ch: char, font_size: f32, family: Option<&str>) -> f32 {
            match (family, ch) {
                (Some("Mono"), '中') => font_size,
                (Some("Mono"), _) => font_size * 0.6,
                _ => self.advance(ch, font_size),
            }
        }
    }

    #[test]
    fn a_fixed_pitch_family_sets_the_column_advance() {
        let cell_w = cell_width(&Probe, 14.0, Some("Mono"));
        assert!((cell_w - 8.4).abs() < 1e-4, "cell_w={cell_w}");
        // Without the family the default metrics still produce a usable grid.
        assert!(cell_width(&Probe, 14.0, None) > 1.0);
    }

    #[test]
    fn cells_tile_without_overlap_and_wide_cells_span_two_columns() {
        let body = Rect::from_min_size(Vec2::new(10.0, 20.0), Size::new(400.0, 200.0));
        let (cell_w, line_h) = (8.0, 20.0);
        let (first, second) = (
            cell_rect(body, cell_w, line_h, 2, 3, false),
            cell_rect(body, cell_w, line_h, 2, 4, false),
        );
        assert_eq!(first.left(), body.left() + 3.0 * cell_w);
        assert_eq!(first.top(), body.top() + 2.0 * line_h);
        assert_eq!(first.right(), second.left(), "cells must tile exactly");
        let wide = cell_rect(body, cell_w, line_h, 2, 3, true);
        assert_eq!(wide.width(), cell_w * 2.0);
        assert_eq!(
            wide.right(),
            cell_rect(body, cell_w, line_h, 2, 5, false).left()
        );
    }

    #[test]
    fn wide_glyphs_are_centered_in_two_columns() {
        assert!((wide_glyph_offset(14.0, 16.86) - 1.43).abs() < 1e-4);
        // A glyph wider than its cell (emoji fallback) stays at the left edge
        // instead of moving outside the cell.
        assert_eq!(wide_glyph_offset(20.0, 16.86), 0.0);
    }

    #[test]
    fn the_caret_sits_on_the_same_grid_as_its_cell() {
        let body = Rect::from_min_size(Vec2::new(10.0, 20.0), Size::new(400.0, 200.0));
        let caret = caret_rect(body, 8.0, 20.0, 4, 7);
        let cell = cell_rect(body, 8.0, 20.0, 4, 7, false);
        assert_eq!(caret.origin, cell.origin);
        assert_eq!(caret.size.height, cell.size.height);
        assert!(caret.size.width >= 2.0);
    }

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
