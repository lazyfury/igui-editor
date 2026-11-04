//! Find / replace bar: a compact two-row strip above the editor.
//!
//! Fields hold the engine's pure [`TextEdit`] state but are painted here in the
//! app's square, dense style. Keys/IME are dispatched by `input.rs` (the bar
//! never sees the tree), and clicks are resolved from the hit rectangles
//! recorded during paint — the same pattern as the menu and the file tree.

use std::cell::RefCell;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase, TextEdit};

use crate::kit::{self, RectExt};
use crate::state::{AppState, FindHit, Focus};
use crate::theme::Theme;

pub const BAR_H: f32 = 74.0;
const ROW_H: f32 = 28.0;
const FIELD_W: f32 = 280.0;
const PAD: f32 = 10.0;

pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "find");
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
        move |tree, _id, phase, _rect, pos| {
            if phase == PointerPhase::Down {
                let mut state = state.borrow_mut();
                click(&mut state, pos);
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
    let measurer = env.measurer;
    kit::fill(env.ctx, rect, p.panel);
    kit::hline(
        env.ctx,
        rect.left(),
        rect.right(),
        rect.bottom() - 1.0,
        p.border,
    );

    let row1 = Rect::from_min_size(
        Vec2::new(rect.left() + PAD, rect.top() + 6.0),
        Size::new(rect.width() - PAD * 2.0, ROW_H),
    );
    let row2 = Rect::from_min_size(
        Vec2::new(rect.left() + PAD, rect.top() + 6.0 + ROW_H + 6.0),
        Size::new(rect.width() - PAD * 2.0, ROW_H),
    );

    // Row 1: query, toggles, count, prev/next, close.
    // Shrink the fields on narrow windows so the buttons never leave the bar.
    let controls_w = 30.0 * 4.0 + 140.0;
    let field_w = FIELD_W.min((row1.width() - controls_w).max(80.0));
    let find_rect = Rect::from_min_size(row1.origin, Size::new(field_w, ROW_H));
    let case_rect = Rect::from_min_size(
        Vec2::new(find_rect.right() + 6.0, row1.top()),
        Size::new(30.0, ROW_H),
    );
    let word_rect = Rect::from_min_size(
        Vec2::new(case_rect.right() + 4.0, row1.top()),
        Size::new(30.0, ROW_H),
    );
    let close_rect = Rect::from_min_size(
        Vec2::new(row1.right() - 30.0, row1.top()),
        Size::new(30.0, ROW_H),
    );
    let next_rect = Rect::from_min_size(
        Vec2::new(close_rect.left() - 34.0, row1.top()),
        Size::new(30.0, ROW_H),
    );
    let prev_rect = Rect::from_min_size(
        Vec2::new(next_rect.left() - 30.0, row1.top()),
        Size::new(30.0, ROW_H),
    );

    let mut hits: Vec<(FindHit, Rect)> = vec![
        (FindHit::FindField, find_rect),
        (FindHit::Case, case_rect),
        (FindHit::Word, word_rect),
        (FindHit::Prev, prev_rect),
        (FindHit::Next, next_rect),
        (FindHit::Close, close_rect),
    ];

    // Row 2: replacement text and buttons.
    let replace_rect = Rect::from_min_size(row2.origin, Size::new(field_w, ROW_H));
    let replace_one = Rect::from_min_size(
        Vec2::new(replace_rect.right() + 6.0, row2.top()),
        Size::new(84.0, ROW_H),
    );
    let replace_all = Rect::from_min_size(
        Vec2::new(replace_one.right() + 4.0, row2.top()),
        Size::new(104.0, ROW_H),
    );
    hits.push((FindHit::ReplaceField, replace_rect));
    hits.push((FindHit::ReplaceOne, replace_one));
    hits.push((FindHit::ReplaceAll, replace_all));

    let focus = state.focus;
    paint_field(
        env,
        &theme,
        find_rect,
        &state.find.query,
        focus == Focus::Find,
        state.find.preedit.as_deref(),
        "Find",
    );
    paint_field(
        env,
        &theme,
        replace_rect,
        &state.find.replace,
        focus == Focus::Replace,
        None,
        "Replace",
    );

    paint_toggle(env, &theme, case_rect, "Aa", state.find.case_sensitive);
    paint_toggle(env, &theme, word_rect, "W", state.find.whole_word);

    // Count.
    let count = if state.find.matches.is_empty() {
        "no matches".to_string()
    } else {
        let current = state.find.current.map_or(0, |index| index + 1);
        if state.find.truncated {
            format!("{current} / {}+", state.find.matches.len())
        } else {
            format!("{current} / {}", state.find.matches.len())
        }
    };
    let count_x = word_rect.right() + 10.0;
    kit::text(
        env.ctx,
        &count,
        count_x,
        kit::baseline(measurer, row1, m.font_small),
        m.font_small,
        if state.find.matches.is_empty() {
            p.faint
        } else {
            p.dim
        },
    );

    paint_arrow(env, &theme, prev_rect, true);
    paint_arrow(env, &theme, next_rect, false);
    paint_close(env, &theme, close_rect);

    paint_button(
        env,
        &theme,
        replace_one,
        "Replace",
        !state.find.matches.is_empty(),
        false,
    );
    paint_button(
        env,
        &theme,
        replace_all,
        "Replace All",
        !state.find.matches.is_empty(),
        true,
    );

    state.find.hits = hits;
}

fn paint_field(
    env: &mut PaintEnv<'_>,
    theme: &Theme,
    rect: Rect,
    edit: &TextEdit,
    focused: bool,
    preedit: Option<&str>,
    placeholder: &str,
) {
    let p = theme.p;
    let m = theme.m;
    let measurer = env.measurer;
    kit::fill(env.ctx, rect, p.bg);
    env.ctx
        .stroke_rect(rect, 1.0, if focused { p.accent } else { p.border });
    let inner = Rect::from_min_size(
        Vec2::new(rect.left() + 6.0, rect.top()),
        Size::new((rect.width() - 12.0).max(8.0), rect.height()),
    );
    env.ctx.save();
    env.ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    let size = m.font_ui;

    if edit.text().is_empty() && preedit.is_none() {
        kit::text(
            env.ctx,
            placeholder,
            inner.left() + 2.0,
            baseline,
            size,
            p.faint,
        );
    }

    if let Some((start, end)) = edit.selection() {
        let x0 = inner.left() + 2.0 + kit::width(measurer, &edit.text()[..start], size);
        let x1 = inner.left() + 2.0 + kit::width(measurer, &edit.text()[..end], size);
        env.ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(x0, rect.top() + 3.0),
                Size::new((x1 - x0).max(1.0), rect.height() - 6.0),
            ),
            p.selection,
        );
    }
    kit::text(
        env.ctx,
        edit.text(),
        inner.left() + 2.0,
        baseline,
        size,
        p.text,
    );
    let caret_x = inner.left() + 2.0 + kit::width(measurer, &edit.text()[..edit.caret()], size);
    if let Some(preedit) = preedit {
        kit::text(env.ctx, preedit, caret_x, baseline, size, p.accent);
        env.ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(caret_x, baseline + 2.0),
                Size::new(kit::width(measurer, preedit, size).max(2.0), 1.5),
            ),
            p.accent,
        );
    }
    if focused {
        env.ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(caret_x, rect.top() + 3.0),
                Size::new(2.0, rect.height() - 6.0),
            ),
            p.accent,
        );
    }
    env.ctx.restore();
}

fn paint_toggle(env: &mut PaintEnv<'_>, theme: &Theme, rect: Rect, label: &str, on: bool) {
    let p = theme.p;
    let m = theme.m;
    let measurer = env.measurer;
    if on {
        kit::fill(env.ctx, rect, p.selection);
        env.ctx.stroke_rect(rect, 1.0, p.accent);
    } else {
        env.ctx.stroke_rect(rect, 1.0, p.border);
    }
    let width = kit::width(measurer, label, m.font_ui);
    kit::text(
        env.ctx,
        label,
        rect.center().x - width * 0.5,
        kit::baseline(measurer, rect, m.font_ui),
        m.font_ui,
        if on { p.accent } else { p.dim },
    );
}

fn paint_button(
    env: &mut PaintEnv<'_>,
    theme: &Theme,
    rect: Rect,
    label: &str,
    enabled: bool,
    primary: bool,
) {
    let p = theme.p;
    let m = theme.m;
    let measurer = env.measurer;
    if primary {
        kit::fill(env.ctx, rect, if enabled { p.accent } else { p.hover });
    }
    env.ctx.stroke_rect(
        rect,
        1.0,
        if primary && enabled {
            p.accent
        } else {
            p.border
        },
    );
    let width = kit::width(measurer, label, m.font_small);
    let color = if primary && enabled {
        p.bg
    } else if enabled {
        p.dim
    } else {
        p.faint
    };
    kit::text(
        env.ctx,
        label,
        rect.center().x - width * 0.5,
        kit::baseline(measurer, rect, m.font_small),
        m.font_small,
        color,
    );
}

fn paint_arrow(env: &mut PaintEnv<'_>, theme: &Theme, rect: Rect, up: bool) {
    let p = theme.p;
    let c = rect.center();
    let color = p.dim;
    if up {
        env.ctx.draw_line(
            Vec2::new(c.x - 4.0, c.y + 2.0),
            Vec2::new(c.x, c.y - 2.0),
            1.5,
            color,
        );
        env.ctx.draw_line(
            Vec2::new(c.x, c.y - 2.0),
            Vec2::new(c.x + 4.0, c.y + 2.0),
            1.5,
            color,
        );
    } else {
        env.ctx.draw_line(
            Vec2::new(c.x - 4.0, c.y - 2.0),
            Vec2::new(c.x, c.y + 2.0),
            1.5,
            color,
        );
        env.ctx.draw_line(
            Vec2::new(c.x, c.y + 2.0),
            Vec2::new(c.x + 4.0, c.y - 2.0),
            1.5,
            color,
        );
    }
}

fn paint_close(env: &mut PaintEnv<'_>, theme: &Theme, rect: Rect) {
    let color = theme.p.dim;
    let c = rect.center();
    env.ctx.draw_line(
        Vec2::new(c.x - 4.0, c.y - 4.0),
        Vec2::new(c.x + 4.0, c.y + 4.0),
        1.5,
        color,
    );
    env.ctx.draw_line(
        Vec2::new(c.x - 4.0, c.y + 4.0),
        Vec2::new(c.x + 4.0, c.y - 4.0),
        1.5,
        color,
    );
}

fn click(state: &mut AppState, pos: Vec2) {
    let Some((hit, _)) = state
        .find
        .hits
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .copied()
    else {
        return;
    };
    match hit {
        FindHit::FindField => state.focus = Focus::Find,
        FindHit::ReplaceField => state.focus = Focus::Replace,
        FindHit::Case => {
            state.find.case_sensitive = !state.find.case_sensitive;
            state.find.dirty = true;
            state.focus = Focus::Find;
        }
        FindHit::Word => {
            state.find.whole_word = !state.find.whole_word;
            state.find.dirty = true;
            state.focus = Focus::Find;
        }
        FindHit::Prev => state.find_step(-1),
        FindHit::Next => state.find_step(1),
        FindHit::ReplaceOne => state.replace_one(),
        FindHit::ReplaceAll => state.replace_all(),
        FindHit::Close => state.close_find(),
    }
    state.dirty = true;
}
