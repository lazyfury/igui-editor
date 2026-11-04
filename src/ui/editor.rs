//! The editor view: gutter, text, selection, caret and scrollbars.
//!
//! One control paints the whole editing surface and owns the pointer/key/text/
//! IME callbacks, plus the caret provider the host uses to place the IME
//! candidate window. Text is drawn line by line with the backend's real
//! measurer (the host installs a CJK-capable default face), so the caret, the
//! selection and the glyphs always agree.
//!
//! Layout rules kept simple on purpose:
//!
//! - every line is one draw run; tabs render as [`Metrics::tab`] spaces,
//! - only visible lines are measured and painted,
//! - scroll offsets live on the document (switching tabs keeps them),
//! - horizontal scroll clamps to the widest *visible* line, which is enough
//!   for M1 and avoids measuring a 20k-line file every frame.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{Color, Cursor, EventResult, FontWeight, ImeEvent, Key, NodeId, Rect, Size, Vec2};
use igui_render::{PaintContext, TextAlign};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::model::wrap::{self, WrapKey, WrapLayout};
use crate::state::{Action, AppState, Drag, Focus};

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "editor");
    kit::set_container(tree, id, Container::Leaf);
    kit::clip(tree, id, true);
    kit::focusable(tree, id);
    kit::set_cursor(tree, id, Cursor::Text);
    kit::draw(tree, id, {
        let state = state.clone();
        move |env, rect, _| {
            let mut state = state.borrow_mut();
            paint(&mut state, env, rect);
        }
    });

    kit::on_pointer(tree, id, {
        let state = state.clone();
        move |tree, _id, phase, _rect, pos| {
            let shift = igui_ui::modifiers(tree).shift;
            let measurer = igui_ui::text_measurer(tree);
            let mut state = state.borrow_mut();
            match phase {
                PointerPhase::Down => {
                    state.clear_hover_target();
                    if state.ws.active_doc().is_none() {
                        let action = state
                            .editor
                            .welcome_actions
                            .iter()
                            .find(|(_, rect)| rect.contains(pos))
                            .map(|(action, _)| action.clone());
                        if let Some(action) = action {
                            state.run(action);
                        }
                        state.editor.drag = Drag::None;
                    } else {
                        pointer_down(&mut state, pos, shift, measurer.as_ref());
                    }
                }
                PointerPhase::Move => {
                    if state.editor.drag == Drag::None {
                        if let Some((line, column)) = hit(&state, pos, measurer.as_ref()) {
                            if let Some(doc) = state.ws.active_doc() {
                                let doc_id = doc.id();
                                let byte = doc.byte_at(line, column);
                                state.set_hover_target(doc_id, byte, pos);
                            }
                        } else {
                            state.clear_hover_target();
                        }
                    }
                    pointer_move(&mut state, pos, measurer.as_ref());
                }
                PointerPhase::Up => state.editor.drag = Drag::None,
                PointerPhase::DoubleClick => {
                    state.clear_hover_target();
                    let Some((line, column)) = hit(&state, pos, measurer.as_ref()) else {
                        return;
                    };
                    let triple = state.editor.last_double.is_some_and(|(at, previous)| {
                        at.elapsed() < std::time::Duration::from_millis(450)
                            && (previous - pos).length() <= 6.0
                    });
                    if let Some(doc) = state.ws.active_doc_mut() {
                        let byte = doc.byte_at(line, column);
                        if triple {
                            doc.select_line_at(byte);
                            state.editor.last_double = None;
                        } else {
                            doc.select_word_at(byte);
                            state.editor.last_double = Some((std::time::Instant::now(), pos));
                        }
                    }
                    // The host sends the second Down first. Stop that press's
                    // selection drag here so a tiny pointer move cannot shrink
                    // or move the word selection after it appears.
                    state.editor.drag = Drag::None;
                    state.editor.reveal = false;
                    state.dirty = true;
                }
            }
            igui_ui::request_paint(tree);
        }
    });

    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let shift = igui_ui::modifiers(tree).shift;
            let mut state = state.borrow_mut();
            state.clear_hover_target();
            if let Some(doc) = state.ws.active_doc_mut() {
                if shift {
                    doc.scroll_x += delta.y;
                } else {
                    doc.scroll_x += delta.x;
                    doc.scroll_y += delta.y;
                }
            }
            state.editor.reveal = false;
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });

    kit::on_key(tree, id, {
        let state = state.clone();
        move |tree, _id, key, pressed, modifiers| {
            if !pressed {
                return EventResult::Ignored;
            }
            let mut state = state.borrow_mut();
            if modifiers.meta || modifiers.ctrl {
                // App-wide commands are dispatched in `input.rs`.
                return EventResult::Ignored;
            }
            let shift = modifiers.shift;
            let measurer = igui_ui::text_measurer(tree);
            if matches!(key, Key::ArrowUp | Key::ArrowDown)
                && state.session.soft_wrap
                && !state.editor.wrap.is_empty()
            {
                move_wrapped_vertical(
                    &mut state,
                    matches!(key, Key::ArrowUp),
                    shift,
                    measurer.as_ref(),
                );
                state.editor.reveal = true;
                state.find.dirty = true;
                state.dirty = true;
                igui_ui::request_paint(tree);
                return EventResult::Handled;
            }
            let Some(doc) = state.ws.active_doc_mut() else {
                return EventResult::Ignored;
            };
            let read_only = doc.is_read_only();
            let handled = match key {
                Key::Backspace | Key::Delete | Key::Enter | Key::Tab | Key::Space if read_only => {
                    false
                }
                Key::Backspace => {
                    doc.backspace();
                    true
                }
                Key::Delete => {
                    doc.delete_forward();
                    true
                }
                Key::Enter => {
                    doc.insert_newline();
                    true
                }
                Key::Space => {
                    doc.insert_text(" ");
                    true
                }
                Key::Tab => {
                    if shift {
                        doc.outdent();
                    } else {
                        doc.indent();
                    }
                    true
                }
                Key::Home => {
                    doc.move_home(shift);
                    true
                }
                Key::End => {
                    doc.move_end(shift);
                    true
                }
                Key::ArrowLeft => {
                    doc.move_left(shift, modifiers.alt);
                    true
                }
                Key::ArrowRight => {
                    doc.move_right(shift, modifiers.alt);
                    true
                }
                Key::ArrowUp => {
                    doc.move_up(shift);
                    true
                }
                Key::ArrowDown => {
                    doc.move_down(shift);
                    true
                }
                _ => false,
            };
            if handled {
                state.editor.reveal = true;
                state.find.dirty = true;
                state.dirty = true;
                igui_ui::request_paint(tree);
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }
    });

    kit::on_text(tree, id, {
        let state = state.clone();
        move |tree, _id, text| {
            let modifiers = igui_ui::modifiers(tree);
            if modifiers.meta || modifiers.ctrl {
                return; // shortcuts, not text
            }
            let mut state = state.borrow_mut();
            if let Some(doc) = state.ws.active_doc_mut() {
                if !doc.is_read_only() {
                    doc.insert_text(text);
                }
            }
            state.editor.preedit = None;
            state.editor.reveal = true;
            state.find.dirty = true;
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });

    kit::on_ime(tree, id, {
        let state = state.clone();
        move |tree, _id, event| {
            let mut state = state.borrow_mut();
            match event {
                ImeEvent::Preedit { text, cursor } => {
                    state.editor.preedit = Some((text.clone(), *cursor));
                }
                ImeEvent::Commit(text) => {
                    state.editor.preedit = None;
                    if let Some(doc) = state.ws.active_doc_mut() {
                        if !doc.is_read_only() {
                            doc.insert_text(text);
                        }
                    }
                    state.editor.reveal = true;
                    state.find.dirty = true;
                }
                ImeEvent::Disabled => state.editor.preedit = None,
                ImeEvent::Enabled => {}
            }
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });

    kit::set_caret_provider(tree, id, {
        let state = state.clone();
        move || {
            let state = state.borrow();
            let rect = state.editor.caret_rect;
            (rect.size.width > 0.0 && rect.size.height > 0.0).then_some(rect)
        }
    });

    id
}

// -- geometry ----------------------------------------------------------------

fn gutter_width(
    line_count: usize,
    measurer: &dyn TextMeasurer,
    font: f32,
    pad: f32,
    family: Option<&str>,
) -> f32 {
    let digits = line_count.to_string().len().max(2) as f32;
    pad * 2.0 + digits * crate::fonts::advance(measurer, '0', font, family) + 4.0
}

fn x_in_line(
    measurer: &dyn TextMeasurer,
    line: &str,
    byte: usize,
    font: f32,
    tab: usize,
    family: Option<&str>,
) -> f32 {
    let prefix = &line[..byte.min(line.len())];
    wrap::width_of(measurer, prefix, font, tab, family)
}

/// Draws `text` with fixed-pitch narrow runs and UI-family wide runs.
///
/// Wide glyphs are measured and advanced in their own face, so the caret, the
/// selection and syntax overlaps stay aligned on mixed Chinese/Latin lines.
fn draw_text_mixed(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    text: &str,
    pos: Vec2,
    font: f32,
    family: Option<&str>,
    color: Color,
) {
    let mut x = pos.x;
    for (run, run_family) in crate::fonts::family_runs(text, family) {
        ctx.draw_text_in(
            run,
            Vec2::new(x, pos.y),
            font,
            FontWeight::NORMAL,
            TextAlign::Left,
            run_family,
            color,
        );
        x += measurer.measure_run_weighted_in(run, font, FontWeight::NORMAL, run_family);
    }
}

#[derive(Clone, Copy)]
struct VisRow {
    row: usize,
    line: usize,
    start_byte: usize,
    end_byte: usize,
    first: bool,
}

/// `(line, column)` under a viewport point, or `None` outside the text area.
/// Byte offset under a viewport position in the editor text area.
pub fn byte_at(state: &AppState, pos: Vec2, measurer: &dyn TextMeasurer) -> Option<usize> {
    let (line, column) = hit(state, pos, measurer)?;
    state.ws.active_doc().map(|doc| doc.byte_at(line, column))
}

fn hit(state: &AppState, pos: Vec2, measurer: &dyn TextMeasurer) -> Option<(usize, usize)> {
    let doc = state.ws.active_doc()?;
    let family = state.editor.font.clone();
    let family = family.as_deref();
    let editor = &state.editor;
    let m = state.theme.m;
    let metrics = state.editor_metrics();
    if pos.y < editor.text_rect.top() || pos.x < editor.text_rect.left() {
        return None;
    }
    let row = (((pos.y - editor.text_rect.top()) + doc.scroll_y) / metrics.line_h).floor() as usize;
    let tab = metrics.tab_columns();
    if state.session.soft_wrap && state.editor.wrap.matches(doc.id(), doc.rev()) {
        let wrap = &state.editor.wrap;
        let row = wrap.row(row)?;
        let line_text = doc.line(row.line);
        let row_start_x = x_in_line(
            measurer,
            line_text,
            row.start_byte,
            metrics.font,
            tab,
            family,
        );
        let x = pos.x - editor.text_rect.left() - m.text_pad - row_start_x;
        let segment = &line_text[row.start_byte..row.end_byte];
        let column = wrap::column_at_x(measurer, segment, x, metrics.font, tab, family);
        return Some((row.line, row.start_col + column));
    }
    let line = row.min(doc.line_count().saturating_sub(1));
    let x = pos.x - editor.text_rect.left() - m.text_pad + doc.scroll_x;
    let column = wrap::column_at_x(measurer, doc.line(line), x, metrics.font, tab, family);
    Some((line, column))
}

fn pointer_down(state: &mut AppState, pos: Vec2, shift: bool, measurer: &dyn TextMeasurer) {
    let (vbar, vthumb, hbar, hthumb) = {
        let editor = &state.editor;
        (editor.vbar, editor.vthumb, editor.hbar, editor.hthumb)
    };
    if vbar.contains(pos) {
        state.editor.drag = Drag::VScroll {
            grab: pos.y - vthumb.top(),
        };
        drag_to(state, pos, measurer);
        return;
    }
    if hbar.contains(pos) {
        state.editor.drag = Drag::HScroll {
            grab: pos.x - hthumb.left(),
        };
        drag_to(state, pos, measurer);
        return;
    }
    let Some((line, column)) = hit(state, pos, measurer) else {
        return;
    };
    state.focus = Focus::Editor;
    state.editor.drag = Drag::Select;
    if let Some(doc) = state.ws.active_doc_mut() {
        doc.click(line, column, shift);
    }
    state.editor.reveal = true;
    state.dirty = true;
}

fn pointer_move(state: &mut AppState, pos: Vec2, measurer: &dyn TextMeasurer) {
    match state.editor.drag {
        Drag::None => {}
        Drag::Select => {
            let Some((line, column)) = hit(state, pos, measurer) else {
                return;
            };
            if let Some(doc) = state.ws.active_doc_mut() {
                let byte = doc.byte_at(line, column);
                doc.extend_to(byte);
            }
            state.editor.reveal = true;
            state.dirty = true;
        }
        Drag::VScroll { .. } | Drag::HScroll { .. } => drag_to(state, pos, measurer),
    }
}

fn drag_to(state: &mut AppState, pos: Vec2, _measurer: &dyn TextMeasurer) {
    let editor = state.editor.clone();
    match editor.drag {
        Drag::VScroll { grab } => {
            let track = editor.vbar.height() - editor.vthumb.height();
            if track <= 0.0 {
                return;
            }
            let thumb_top = (pos.y - grab).clamp(
                editor.vbar.top(),
                editor.vbar.bottom() - editor.vthumb.height(),
            );
            let ratio = (thumb_top - editor.vbar.top()) / track;
            let max = (editor.content_h - editor.view_h).max(0.0);
            if let Some(doc) = state.ws.active_doc_mut() {
                doc.scroll_y = ratio * max;
            }
            state.editor.reveal = false;
            state.dirty = true;
        }
        Drag::HScroll { grab } => {
            let track = editor.hbar.width() - editor.hthumb.width();
            if track <= 0.0 {
                return;
            }
            let thumb_left = (pos.x - grab).clamp(
                editor.hbar.left(),
                editor.hbar.right() - editor.hthumb.width(),
            );
            let ratio = (thumb_left - editor.hbar.left()) / track;
            let max = (editor.content_w - editor.view_w).max(0.0);
            if let Some(doc) = state.ws.active_doc_mut() {
                doc.scroll_x = ratio * max;
            }
            state.editor.reveal = false;
            state.dirty = true;
        }
        Drag::None | Drag::Select => {}
    }
}

fn diff_line_color(line: &str, p: &crate::theme::Palette) -> igui_core::Color {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
        p.dim
    } else if line.starts_with('+') {
        p.accent
    } else if line.starts_with('-') {
        p.error
    } else if line.starts_with("@@") {
        p.faint
    } else {
        p.text
    }
}

fn move_wrapped_vertical(
    state: &mut AppState,
    up: bool,
    select: bool,
    measurer: &dyn TextMeasurer,
) {
    let metrics = state.editor_metrics();
    let font = metrics.font;
    let tab = metrics.tab_columns();
    let family = state.editor.font.clone();
    let family = family.as_deref();
    let Some(doc) = state.ws.active_doc_mut() else {
        return;
    };
    if !state.editor.wrap.matches(doc.id(), doc.rev()) {
        if up {
            doc.move_up(select);
        } else {
            doc.move_down(select);
        }
        return;
    }
    let (line, column) = doc.line_col();
    let Some(current) = state.editor.wrap.row_of(line, column) else {
        return;
    };
    let row_count = state.editor.wrap.len();
    let target = if up {
        current.checked_sub(1)
    } else if current + 1 < row_count {
        Some(current + 1)
    } else {
        None
    };
    let Some(target) = target else {
        if up {
            doc.move_doc_start(select);
        } else {
            doc.move_doc_end(select);
        }
        return;
    };
    let Some(target_row) = state.editor.wrap.row(target).copied() else {
        return;
    };
    let current_text = doc.line(line);
    let current_x = x_in_line(
        measurer,
        current_text,
        doc.caret() - doc.line_start(line),
        font,
        tab,
        family,
    );
    let row_start_x = state.editor.wrap.row(current).map_or(0.0, |row| {
        x_in_line(measurer, current_text, row.start_byte, font, tab, family)
    });
    let target_text = &doc.line(target_row.line)[target_row.start_byte..target_row.end_byte];
    let column_in_row = wrap::column_at_x(
        measurer,
        target_text,
        current_x - row_start_x,
        font,
        tab,
        family,
    );
    doc.click(
        target_row.line,
        target_row.start_col + column_in_row,
        select,
    );
}

// -- paint -------------------------------------------------------------------

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    kit::fill(env.ctx, rect, p.bg);

    if state.ws.active_doc().is_none() {
        paint_welcome(state, env, rect);
        state.editor.text_rect = rect;
        return;
    }

    // The editor asks for the same fixed-pitch face as the terminal. Wide
    // glyphs inside the text are still painted with `UI_FAMILY` (see
    // `draw_text_mixed`), so this is a per-run choice, not the default face.
    if !state.editor.font_checked {
        state.editor.font = crate::fonts::mono_family(measurer, state.editor_metrics().font);
        state.editor.font_checked = state.editor.font.is_some();
    }
    let family = state.editor.font.clone();
    let family = family.as_deref();

    let metrics = state.editor_metrics();
    let line_h = metrics.line_h;
    let font = metrics.font;
    let tab = metrics.tab_columns();
    let sb = m.scrollbar;
    let show_gutter = state.settings.line_numbers;

    let (gutter_w, line_count, scroll_x, scroll_y, caret, selection, preedit, doc_id, rev, len) = {
        let doc = state.ws.active_doc().expect("checked above");
        (
            if show_gutter {
                gutter_width(doc.line_count(), measurer, font, m.gutter_pad, family)
            } else {
                0.0
            },
            doc.line_count(),
            doc.scroll_x,
            doc.scroll_y,
            doc.caret(),
            doc.selection(),
            state.editor.preedit.clone(),
            doc.id(),
            doc.rev(),
            doc.text().len(),
        )
    };
    let text_rect = Rect::from_min_size(
        Vec2::new(rect.left() + gutter_w, rect.top()),
        Size::new(
            (rect.width() - gutter_w - sb).max(16.0),
            (rect.height() - sb).max(16.0),
        ),
    );
    let pad = m.text_pad;

    // Soft wrap is rebuilt only when the document/width/typography changes.
    let wrap_enabled = state.session.soft_wrap
        && len <= wrap::MAX_WRAP_BYTES
        && line_count <= wrap::MAX_WRAP_LINES;
    let wrap_width = (text_rect.width() - pad * 2.0).max(16.0);
    if wrap_enabled {
        let key = WrapKey::new(doc_id, rev, wrap_width, font, tab, family);
        if state.editor.wrap.key() != Some(key) {
            let layout = {
                let doc = state.ws.active_doc().expect("checked above");
                WrapLayout::build(doc, wrap_width, font, tab, family, measurer)
            };
            state.editor.wrap = layout;
        }
    } else if state.editor.wrap.key().is_some() {
        state.editor.wrap = WrapLayout::default();
    }
    let wrap_on = wrap_enabled;
    let row_count = if wrap_on {
        state.editor.wrap.len()
    } else {
        line_count
    };
    let content_h = row_count as f32 * line_h;
    let max_scroll_y = (content_h - text_rect.height()).max(0.0);

    // Visible window (and the widest visible line for horizontal clamping).
    let mut scroll_y = scroll_y.clamp(0.0, max_scroll_y);
    let mut scroll_x = if wrap_on { 0.0 } else { scroll_x };
    let first_row = (scroll_y / line_h).floor() as usize;
    let visible_count = (text_rect.height() / line_h).ceil() as usize + 1;
    let last_row = (first_row + visible_count).min(row_count.saturating_sub(1));
    let mut widest = 0.0_f32;
    if !wrap_on {
        let doc = state.ws.active_doc().expect("checked above");
        for line in first_row..=last_row {
            widest = widest.max(wrap::width_of(measurer, doc.line(line), font, tab, family));
        }
    }
    let content_w = if wrap_on {
        text_rect.width()
    } else {
        widest + pad * 2.0
    };
    let max_scroll_x = (content_w - text_rect.width()).max(0.0);

    // Reveal the caret on caret-changing input, not while the user scrolls.
    if state.editor.reveal {
        let doc = state.ws.active_doc().expect("checked above");
        let (caret_line, caret_col) = doc.line_col();
        let caret_row = if wrap_on {
            state
                .editor
                .wrap
                .row_of(caret_line, caret_col)
                .unwrap_or(caret_line)
        } else {
            caret_line
        };
        let caret_x = x_in_line(
            measurer,
            doc.line(caret_line),
            caret - doc.line_start(caret_line),
            font,
            tab,
            family,
        ) - if wrap_on {
            let row = state.editor.wrap.row(caret_row);
            row.map_or(0.0, |row| {
                x_in_line(
                    measurer,
                    doc.line(caret_line),
                    row.start_byte,
                    font,
                    tab,
                    family,
                )
            })
        } else {
            0.0
        };
        let caret_y = caret_row as f32 * line_h;
        if caret_y < scroll_y {
            scroll_y = caret_y;
        } else if caret_y + line_h > scroll_y + text_rect.height() {
            scroll_y = caret_y + line_h - text_rect.height() + 2.0;
        }
        if !wrap_on {
            if caret_x < scroll_x {
                scroll_x = caret_x;
            } else if caret_x + 12.0 > scroll_x + text_rect.width() - pad {
                scroll_x = caret_x + 12.0 - text_rect.width() + pad;
            }
        }
        state.editor.reveal = false;
    }
    scroll_y = scroll_y.clamp(0.0, max_scroll_y);
    scroll_x = scroll_x.clamp(0.0, max_scroll_x);
    if let Some(doc) = state.ws.active_doc_mut() {
        if doc.scroll_y != scroll_y || doc.scroll_x != scroll_x {
            doc.scroll_y = scroll_y;
            doc.scroll_x = scroll_x;
        }
    }

    // Record geometry for hit-testing and IME placement.
    let editor = &mut state.editor;
    editor.text_rect = text_rect;
    editor.gutter_w = gutter_w;
    editor.view_w = text_rect.width();
    editor.view_h = text_rect.height();
    editor.content_h = content_h;
    editor.content_w = content_w;

    let gutter_rect = Rect::from_min_size(rect.origin, Size::new(gutter_w, text_rect.height()));
    if show_gutter {
        kit::fill(env.ctx, gutter_rect, p.gutter);
        kit::vline(
            env.ctx,
            gutter_rect.right() - 1.0,
            gutter_rect.top(),
            gutter_rect.bottom(),
            p.border,
        );
    }

    let first_row = (scroll_y / line_h).floor() as usize;
    let last_row = (first_row + visible_count).min(row_count.saturating_sub(1));
    let (caret_line, caret_col) = {
        let doc = state.ws.active_doc().expect("checked above");
        doc.line_col()
    };
    let caret_row = if wrap_on {
        state
            .editor
            .wrap
            .row_of(caret_line, caret_col)
            .unwrap_or(caret_line)
    } else {
        caret_line
    };
    let vis: Vec<VisRow> = {
        let doc = state.ws.active_doc().expect("checked above");
        if wrap_on {
            (first_row..=last_row)
                .filter_map(|index| {
                    state.editor.wrap.row(index).map(|row| VisRow {
                        row: index,
                        line: row.line,
                        start_byte: row.start_byte,
                        end_byte: row.end_byte,
                        first: row.first,
                    })
                })
                .collect()
        } else {
            (first_row..=last_row)
                .map(|line| VisRow {
                    row: line,
                    line,
                    start_byte: 0,
                    end_byte: doc.line(line).len(),
                    first: true,
                })
                .collect()
        }
    };
    let row_y = |row: usize| {
        text_rect.top() + (row as f32 - first_row as f32) * line_h
            - (scroll_y - first_row as f32 * line_h)
    };

    // Current-line highlight, full width (gutter + text), all wrapped rows.
    for vis in &vis {
        if vis.line != caret_line {
            continue;
        }
        let y = row_y(vis.row);
        env.ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(rect.left(), y),
                Size::new(rect.width() - sb, line_h),
            ),
            if state.focus == Focus::Editor {
                p.current_line
            } else {
                p.current_line.alpha_mul(0.5)
            },
        );
    }

    // Indent guides: one 1 px rule per indentation level of each visible line.
    if state.settings.indent_guides {
        let space = crate::fonts::advance(measurer, ' ', font, family);
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        let doc = state.ws.active_doc().expect("checked above");
        for vis in &vis {
            if !vis.first {
                continue;
            }
            let text = doc.line(vis.line);
            let mut columns = 0usize;
            for ch in text.chars() {
                match ch {
                    ' ' => columns += 1,
                    '\t' => columns += tab,
                    _ => break,
                }
            }
            if columns == 0 {
                continue;
            }
            let y = row_y(vis.row);
            let mut column = tab;
            while column <= columns {
                let x = text_rect.left() + pad + column as f32 * space - scroll_x;
                env.ctx.fill_rect(
                    Rect::from_min_size(Vec2::new(x, y), Size::new(1.0, line_h)),
                    p.border.alpha_mul(0.55),
                );
                column += tab;
            }
        }
        env.ctx.restore();
    }

    // Selection rectangles, per visual row.
    if let Some((sel_start, sel_end)) = selection {
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        let doc = state.ws.active_doc().expect("checked above");
        for vis in &vis {
            let line_text = doc.line(vis.line);
            let line_start = doc.line_start(vis.line);
            let line_end = doc.line_end(vis.line);
            let row_start_byte = line_start + vis.start_byte;
            let row_end_byte = line_start + vis.end_byte;
            let from = sel_start.max(row_start_byte);
            let to = sel_end.min(row_end_byte);
            let covers_newline =
                vis.end_byte >= line_text.len() && sel_end > line_end && sel_start <= line_end;
            if from > to || (from == to && !covers_newline) {
                continue;
            }
            let row_start_x = x_in_line(measurer, line_text, vis.start_byte, font, tab, family);
            let row_end_x = x_in_line(measurer, line_text, vis.end_byte, font, tab, family);
            let x0 = if sel_start <= row_start_byte {
                0.0
            } else {
                x_in_line(measurer, line_text, from - line_start, font, tab, family) - row_start_x
            };
            let x1 = if sel_end >= row_end_byte {
                if covers_newline {
                    text_rect.width()
                } else {
                    row_end_x - row_start_x
                }
            } else {
                x_in_line(measurer, line_text, to - line_start, font, tab, family) - row_start_x
            };
            let y = row_y(vis.row);
            env.ctx.fill_rect(
                Rect::from_min_size(
                    Vec2::new(text_rect.left() + pad + x0 - scroll_x, y),
                    Size::new((x1 - x0).max(1.0), line_h),
                ),
                p.selection,
            );
        }
        env.ctx.restore();
    }

    // Find matches: every hit tinted, the current one stronger.
    if state.find.open && !state.find.matches.is_empty() {
        let current = state.find.current;
        let matches = &state.find.matches;
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        let doc = state.ws.active_doc().expect("checked above");
        for vis in &vis {
            let line_text = doc.line(vis.line);
            let line_start = doc.line_start(vis.line);
            let line_end = doc.line_end(vis.line);
            let row_start_byte = line_start + vis.start_byte;
            let row_end_byte = line_start + vis.end_byte;
            let row_start_x = x_in_line(measurer, line_text, vis.start_byte, font, tab, family);
            let row_end_x = x_in_line(measurer, line_text, vis.end_byte, font, tab, family);
            let y = row_y(vis.row);
            for (index, &(match_start, match_end)) in matches.iter().enumerate() {
                if match_start >= row_end_byte || match_end <= row_start_byte {
                    continue;
                }
                let from = match_start.max(row_start_byte);
                let to = match_end.min(row_end_byte);
                if from >= to {
                    continue;
                }
                let x0 = x_in_line(measurer, line_text, from - line_start, font, tab, family)
                    - row_start_x;
                let x1 = if match_end >= line_end {
                    row_end_x - row_start_x
                } else {
                    x_in_line(measurer, line_text, to - line_start, font, tab, family) - row_start_x
                };
                let color = if Some(index) == current {
                    p.accent.alpha_mul(0.32)
                } else {
                    p.accent.alpha_mul(0.14)
                };
                env.ctx.fill_rect(
                    Rect::from_min_size(
                        Vec2::new(text_rect.left() + pad + x0 - scroll_x, y),
                        Size::new((x1 - x0).max(1.0), line_h),
                    ),
                    color,
                );
            }
        }
        env.ctx.restore();
    }

    // Text rows.
    env.ctx.save();
    env.ctx.clip_rect(text_rect);
    let ascent = measurer.ascent_in(font, family);
    let (git_view, preview_start) = {
        let doc = state.ws.active_doc().expect("checked above");
        (doc.is_git_view(), doc.preview_start())
    };
    let mut scratch = String::new();
    for vis in &vis {
        let line_text = {
            let doc = state.ws.active_doc().expect("checked above");
            doc.line(vis.line)
        };
        let segment = &line_text[vis.start_byte..vis.end_byte];
        scratch.clear();
        if segment.contains('\t') {
            scratch.push_str(&wrap::expand_tabs(segment, tab));
        } else {
            scratch.push_str(segment);
        }
        if scratch.is_empty() {
            continue;
        }
        let row_start_x = x_in_line(measurer, line_text, vis.start_byte, font, tab, family);
        let y = row_y(vis.row);
        let baseline = kit::line_baseline_with_ascent(y, line_h, font, ascent);
        let color = if git_view && preview_start.is_none_or(|start| vis.line < start) {
            diff_line_color(segment, &p)
        } else {
            p.text
        };
        draw_text_mixed(
            env.ctx,
            measurer,
            scratch.as_str(),
            Vec2::new(text_rect.left() + pad + row_start_x - scroll_x, baseline),
            font,
            family,
            color,
        );
    }

    // Syntax tokens are painted over the base text (same glyphs, token color),
    // which keeps tab expansion and horizontal metrics identical to the caret.
    if state.settings.syntax && !git_view {
        let doc = state.ws.active_doc().expect("checked above");
        for vis in &vis {
            let Some(tokens) = state.highlight.tokens(vis.line) else {
                continue;
            };
            if tokens.is_empty() {
                continue;
            }
            let line_text = doc.line(vis.line);
            let row_start_x = x_in_line(measurer, line_text, vis.start_byte, font, tab, family);
            let y = row_y(vis.row);
            let baseline = kit::line_baseline_with_ascent(y, line_h, font, ascent);
            for token in tokens {
                let from = token.start.max(vis.start_byte);
                let to = token.end.min(vis.end_byte);
                if from >= to || token.end <= token.start || token.end > line_text.len() {
                    continue;
                }
                let text = &line_text[from..to];
                let x = x_in_line(measurer, line_text, from, font, tab, family) - row_start_x;
                let drawn = wrap::expand_tabs(text, tab);
                draw_text_mixed(
                    env.ctx,
                    measurer,
                    drawn.as_str(),
                    Vec2::new(text_rect.left() + pad + x - scroll_x, baseline),
                    font,
                    family,
                    theme.token_color(token.kind),
                );
            }
        }
    }
    env.ctx.restore();

    // LSP semantic tokens are painted over the syntax pass, so variables,
    // parameters, fields, functions and types get server-aware colors without
    // losing syntax colors for tokens the server did not classify.
    if !git_view && state.settings.syntax {
        let doc = state.ws.active_doc().expect("checked above");
        let semantic: Vec<crate::model::lsp::SemanticToken> = doc
            .path()
            .map(|path| state.lsp.semantic_for(path).to_vec())
            .unwrap_or_default();
        if !semantic.is_empty() {
            env.ctx.save();
            env.ctx.clip_rect(text_rect);
            for vis in &vis {
                let line_text = doc.line(vis.line);
                let line_start = doc.line_start(vis.line);
                for token in &semantic {
                    if token.line != vis.line {
                        continue;
                    }
                    let Some(color) = state.lsp.semantic_color(&token.kind, &p) else {
                        continue;
                    };
                    let start = crate::model::lsp::byte_for_position(
                        doc,
                        token.line as u32,
                        token.start as u32,
                    )
                    .saturating_sub(line_start)
                    .min(line_text.len());
                    let end = crate::model::lsp::byte_for_position(
                        doc,
                        token.line as u32,
                        token.end as u32,
                    )
                    .saturating_sub(line_start)
                    .min(line_text.len())
                    .max(start);
                    let row_start_byte = line_start + vis.start_byte;
                    let row_end_byte = line_start + vis.end_byte;
                    let from = (line_start + start).max(row_start_byte);
                    let to = (line_start + end).min(row_end_byte);
                    if from >= to {
                        continue;
                    }
                    let start_column = line_text[..from - line_start].chars().count();
                    let row = if wrap_on {
                        state
                            .editor
                            .wrap
                            .row_of(vis.line, start_column)
                            .unwrap_or(vis.row)
                    } else {
                        vis.line
                    };
                    if vis.row != row {
                        continue;
                    }
                    let row_start_x = if wrap_on {
                        x_in_line(measurer, line_text, vis.start_byte, font, tab, family)
                    } else {
                        0.0
                    };
                    let x = x_in_line(measurer, line_text, from - line_start, font, tab, family)
                        - row_start_x;
                    let y = row_y(vis.row);
                    let baseline = kit::line_baseline_with_ascent(y, line_h, font, ascent);
                    draw_text_mixed(
                        env.ctx,
                        measurer,
                        &line_text[from - line_start..to - line_start],
                        Vec2::new(text_rect.left() + pad + x - scroll_x, baseline),
                        font,
                        family,
                        color,
                    );
                }
            }
            env.ctx.restore();
        }
    }

    // Language-server diagnostics: a thin severity-colored underline on the
    // affected range, plus a gutter marker on the first line.
    {
        let doc = state.ws.active_doc().expect("checked above");
        let diagnostics: Vec<crate::model::lsp::Diagnostic> = doc
            .path()
            .map(|path| state.lsp.diagnostics_for(path).to_vec())
            .unwrap_or_default();
        if !diagnostics.is_empty() {
            env.ctx.save();
            env.ctx.clip_rect(text_rect);
            for vis in &vis {
                let y = row_y(vis.row);
                for diagnostic in &diagnostics {
                    if diagnostic.start_line != vis.line {
                        continue;
                    }
                    let line_text = doc.line(vis.line);
                    let line_start = doc.line_start(vis.line);
                    let start = crate::model::lsp::byte_for_position(
                        doc,
                        diagnostic.start_line as u32,
                        diagnostic.start_char as u32,
                    )
                    .saturating_sub(line_start);
                    let end = if diagnostic.end_line == diagnostic.start_line {
                        crate::model::lsp::byte_for_position(
                            doc,
                            diagnostic.end_line as u32,
                            diagnostic.end_char as u32,
                        )
                        .saturating_sub(line_start)
                    } else {
                        line_text.len()
                    };
                    let start = start.min(line_text.len());
                    let end = end.min(line_text.len()).max(start);
                    // Only the visual row that owns the diagnostic start paints
                    // it, and the underline never spills into the next row.
                    let start_column = line_text[..start].chars().count();
                    let row = if wrap_on {
                        state
                            .editor
                            .wrap
                            .row_of(vis.line, start_column)
                            .unwrap_or(vis.row)
                    } else {
                        vis.line
                    };
                    if vis.row != row {
                        continue;
                    }
                    let row_start_x = if wrap_on {
                        x_in_line(measurer, line_text, vis.start_byte, font, tab, family)
                    } else {
                        0.0
                    };
                    let end = end.min(vis.end_byte).max(start);
                    let x0 = x_in_line(measurer, line_text, start, font, tab, family) - row_start_x;
                    let x1 = x_in_line(measurer, line_text, end, font, tab, family) - row_start_x;
                    env.ctx.fill_rect(
                        Rect::from_min_size(
                            Vec2::new(text_rect.left() + pad + x0 - scroll_x, y + line_h - 2.0),
                            Size::new((x1 - x0).max(3.0), 2.0),
                        ),
                        state.lsp.diagnostic_color(diagnostic.severity, &p),
                    );
                }
            }
            env.ctx.restore();
        }
    }

    // IME preedit at the caret (composition text is not committed yet).
    if let Some((text, _cursor)) = preedit {
        let doc = state.ws.active_doc().expect("checked above");
        let line = doc.line_of(caret);
        let row = state
            .editor
            .wrap
            .row_of(line, caret_col)
            .unwrap_or(if wrap_on { caret_row } else { line });
        let row_start_x = if wrap_on {
            state.editor.wrap.row(row).map_or(0.0, |row| {
                x_in_line(
                    measurer,
                    doc.line(row.line),
                    row.start_byte,
                    font,
                    tab,
                    family,
                )
            })
        } else {
            0.0
        };
        let x = x_in_line(
            measurer,
            doc.line(line),
            caret - doc.line_start(line),
            font,
            tab,
            family,
        ) - row_start_x;
        let y = row_y(row);
        let baseline = kit::line_baseline_with_ascent(y, line_h, font, ascent);
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        draw_text_mixed(
            env.ctx,
            measurer,
            text.as_str(),
            Vec2::new(text_rect.left() + pad + x - scroll_x, baseline),
            font,
            family,
            p.accent,
        );
        let preedit_w =
            crate::fonts::measure_run_weighted(measurer, &text, font, FontWeight::NORMAL, family);
        env.ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(text_rect.left() + pad + x - scroll_x, baseline + 2.0),
                Size::new(preedit_w.max(4.0), 1.5),
            ),
            p.accent,
        );
        env.ctx.restore();
    }

    // Caret.
    {
        let doc = state.ws.active_doc().expect("checked above");
        let line = doc.line_of(caret);
        let row = if wrap_on {
            state
                .editor
                .wrap
                .row_of(line, caret_col)
                .unwrap_or(caret_row)
        } else {
            line
        };
        let row_start_x = if wrap_on {
            state.editor.wrap.row(row).map_or(0.0, |row| {
                x_in_line(
                    measurer,
                    doc.line(row.line),
                    row.start_byte,
                    font,
                    tab,
                    family,
                )
            })
        } else {
            0.0
        };
        let x = x_in_line(
            measurer,
            doc.line(line),
            caret - doc.line_start(line),
            font,
            tab,
            family,
        ) - row_start_x;
        let y = row_y(row);
        let focused = state.focus == Focus::Editor;
        let caret_rect = Rect::from_min_size(
            Vec2::new(text_rect.left() + pad + x - scroll_x, y),
            Size::new(if focused { 2.0 } else { 1.0 }, line_h),
        );
        state.editor.caret_rect = caret_rect;
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        env.ctx
            .fill_rect(caret_rect, if focused { p.accent } else { p.faint });
        env.ctx.restore();
    }

    // Matched bracket underlines.
    if let Some((open, close)) = state.bracket {
        let doc = state.ws.active_doc().expect("checked above");
        let text = doc.text();
        env.ctx.save();
        env.ctx.clip_rect(text_rect);
        for byte in [open, close] {
            if byte >= text.len() {
                continue;
            }
            let line = doc.line_of(byte);
            let column = text[doc.line_start(line)..byte].chars().count();
            let row = if wrap_on {
                state.editor.wrap.row_of(line, column).unwrap_or(line)
            } else {
                line
            };
            if row < first_row || row > last_row {
                continue;
            }
            let row_start_x = if wrap_on {
                state.editor.wrap.row(row).map_or(0.0, |row| {
                    x_in_line(
                        measurer,
                        doc.line(row.line),
                        row.start_byte,
                        font,
                        tab,
                        family,
                    )
                })
            } else {
                0.0
            };
            let offset = byte - doc.line_start(line);
            let x = x_in_line(measurer, doc.line(line), offset, font, tab, family) - row_start_x;
            let ch = text[byte..].chars().next().unwrap_or(' ');
            let width = crate::fonts::advance(measurer, ch, font, family).max(2.0);
            let y = row_y(row);
            env.ctx.fill_rect(
                Rect::from_min_size(
                    Vec2::new(text_rect.left() + pad + x - scroll_x, y + line_h - 2.0),
                    Size::new(width, 1.5),
                ),
                p.accent,
            );
        }
        env.ctx.restore();
    }

    // Gutter numbers: one label per logical line (on its first visual row).
    if show_gutter {
        let number_font = (font - 1.0).max(9.0);
        let right = gutter_rect.right() - m.gutter_pad;
        for vis in &vis {
            if !vis.first {
                continue;
            }
            let y = row_y(vis.row);
            let number_ascent = measurer.ascent_in(number_font, family);
            let baseline = kit::line_baseline_with_ascent(y, line_h, number_font, number_ascent);
            let label = (vis.line + 1).to_string();
            let label_w =
                measurer.measure_run_weighted_in(&label, number_font, FontWeight::NORMAL, family);
            env.ctx.draw_text_in(
                &label,
                Vec2::new(right - label_w, baseline),
                number_font,
                FontWeight::NORMAL,
                TextAlign::Left,
                family,
                if vis.line == caret_line {
                    p.accent
                } else {
                    p.faint
                },
            );
            let severity = state
                .ws
                .active_doc()
                .and_then(|doc| doc.path())
                .and_then(|path| {
                    state
                        .lsp
                        .diagnostics_for(path)
                        .iter()
                        .find(|diagnostic| diagnostic.start_line == vis.line)
                        .map(|diagnostic| diagnostic.severity)
                });
            if let Some(severity) = severity {
                // A full-height severity rail is easier to spot than the old
                // four-pixel dot when the editor is dense.
                env.ctx.fill_rect(
                    Rect::from_min_size(
                        Vec2::new(gutter_rect.left() + 1.0, y + 2.0),
                        Size::new(3.0, (line_h - 4.0).max(3.0)),
                    ),
                    state.lsp.diagnostic_color(severity, &p),
                );
            }
        }
    }

    // Scrollbars: square thumbs, no track chrome.
    let vbar = Rect::from_min_size(
        Vec2::new(rect.right() - sb, rect.top()),
        Size::new(sb, text_rect.height()),
    );
    let mut vthumb = Rect::ZERO;
    if content_h > text_rect.height() {
        let thumb_h = (text_rect.height() * text_rect.height() / content_h)
            .min(text_rect.height())
            .max(1.0);
        let travel = text_rect.height() - thumb_h;
        let y = vbar.top() + (scroll_y / max_scroll_y.max(f32::EPSILON)) * travel;
        vthumb = Rect::from_min_size(
            Vec2::new(vbar.left() + 2.0, y),
            Size::new(sb - 4.0, thumb_h),
        );
        env.ctx.fill_rect(vthumb, p.border);
    }
    let hbar = Rect::from_min_size(
        Vec2::new(text_rect.left(), rect.bottom() - sb),
        Size::new(text_rect.width(), sb),
    );
    let mut hthumb = Rect::ZERO;
    if content_w > text_rect.width() {
        let thumb_w = (text_rect.width() * text_rect.width() / content_w)
            .min(text_rect.width())
            .max(1.0);
        let travel = text_rect.width() - thumb_w;
        let x = hbar.left() + (scroll_x / max_scroll_x.max(f32::EPSILON)) * travel;
        hthumb = Rect::from_min_size(Vec2::new(x, hbar.top() + 2.0), Size::new(thumb_w, sb - 4.0));
        env.ctx.fill_rect(hthumb, p.border);
    }
    let editor = &mut state.editor;
    editor.vbar = vbar;
    editor.vthumb = vthumb;
    editor.hbar = hbar;
    editor.hthumb = hthumb;
}

fn paint_welcome(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    state.editor.welcome_actions.clear();
    state.editor.caret_rect = Rect::ZERO;

    let mut actions = vec![Action::Open, Action::OpenFolder, Action::New];
    if state.ws.folder_open {
        actions.push(Action::QuickOpen);
        actions.push(Action::ProjectSearch);
    }
    let recent_projects: Vec<Action> = state
        .session
        .recent_projects
        .iter()
        .filter(|path| path.is_dir())
        .take(4)
        .cloned()
        .map(Action::OpenRecentProject)
        .collect();
    let row_h = 28.0;
    let row_w = 380.0_f32.min((rect.width() - 48.0).max(220.0));
    let recent_h = if recent_projects.is_empty() {
        0.0
    } else {
        30.0 + recent_projects.len() as f32 * (row_h + 6.0)
    };
    let total_h = 70.0 + actions.len() as f32 * (row_h + 6.0) + recent_h + 36.0;
    let left = rect.center().x - row_w * 0.5;
    let top = (rect.center().y - total_h * 0.5).max(rect.top() + 24.0);

    let title = Rect::from_min_size(Vec2::new(left, top), Size::new(row_w, 28.0));
    kit::text_center(
        env.ctx,
        measurer,
        "igui-editor",
        title,
        top + 20.0,
        18.0,
        p.text,
    );
    let subtitle = if state.ws.folder_open {
        "no document open — pick a file in the sidebar"
    } else {
        "no project open"
    };
    kit::text_center(
        env.ctx,
        measurer,
        subtitle,
        Rect::from_min_size(Vec2::new(left, top + 30.0), Size::new(row_w, 22.0)),
        top + 48.0,
        m.font_ui,
        p.dim,
    );

    let mut y = top + 66.0;
    for action in actions {
        let row = Rect::from_min_size(Vec2::new(left, y), Size::new(row_w, row_h));
        env.ctx.stroke_rect(row, 1.0, p.border);
        kit::text(
            env.ctx,
            &action.label(),
            row.left() + 12.0,
            kit::baseline(measurer, row, m.font_ui),
            m.font_ui,
            p.text,
        );
        let hint = state.keymap.shortcut_label(&action).unwrap_or_default();
        kit::text_right(
            env.ctx,
            measurer,
            &hint,
            row.right() - 12.0,
            kit::baseline(measurer, row, m.font_small),
            m.font_small,
            p.faint,
        );
        state.editor.welcome_actions.push((action, row));
        y += row_h + 6.0;
    }

    if !recent_projects.is_empty() {
        y += 6.0;
        kit::text(
            env.ctx,
            "RECENT PROJECTS",
            left,
            y + 13.0,
            m.font_small,
            p.faint,
        );
        y += 22.0;
        for action in recent_projects {
            let row = Rect::from_min_size(Vec2::new(left, y), Size::new(row_w, row_h));
            env.ctx.stroke_rect(row, 1.0, p.border);
            kit::text(
                env.ctx,
                &action.label(),
                row.left() + 12.0,
                kit::baseline(measurer, row, m.font_ui),
                m.font_ui,
                p.text,
            );
            state.editor.welcome_actions.push((action, row));
            y += row_h + 6.0;
        }
    }

    let hint = "tip: drop files or a project onto the window";
    let hint_width = kit::width(measurer, hint, m.font_small);
    kit::text(
        env.ctx,
        hint,
        rect.center().x - hint_width * 0.5,
        y + 16.0,
        m.font_small,
        p.faint,
    );
}
