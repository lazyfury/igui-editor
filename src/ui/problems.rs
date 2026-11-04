//! Bottom-dock PROBLEMS panel.
//!
//! The editor already paints diagnostic underlines and gutter markers; this
//! panel is the workspace-wide message list. It shows every diagnostic from
//! every live language server, grouped by file, and jumps to a row on click.
//! The dock header (tab, badge and close button) is painted by `ui::dock`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use igui_core::{NodeId, Rect, Size, Vec2};
use igui_scene::SceneTree;
use igui_ui::{Container, PaintEnv, PointerPhase};

use crate::kit::{self, RectExt};
use crate::model::lsp::{byte_for_position, ProblemEntry};
use crate::state::{AppState, Focus};

/// One file-group header row.
pub const GROUP_H: f32 = 24.0;
/// One diagnostic row.
pub const ROW_H: f32 = 24.0;
const PAD: f32 = 14.0;

/// Bottom-dock body: shared with the Terminal tab by the dock.
pub fn build(tree: &mut SceneTree, parent: NodeId, state: Rc<RefCell<AppState>>) -> NodeId {
    let id = kit::node(tree, parent, "problems");
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
            if phase != PointerPhase::Down {
                return;
            }
            let mut state = state.borrow_mut();
            if click(&mut state, pos) {
                igui_ui::request_paint(tree);
            }
        }
    });
    kit::on_scroll(tree, id, {
        let state = state.clone();
        move |tree, _id, delta| {
            let mut state = state.borrow_mut();
            let max = state.problems.scroll_max;
            crate::ui::scroll::apply(&mut state.problems.scroll, delta.y, max);
            state.dirty = true;
            igui_ui::request_paint(tree);
        }
    });
    kit::set_cursor(tree, id, igui_core::Cursor::Default);
    id
}

/// Row text for a file group: relative to the project when possible.
fn group_label(state: &AppState, path: &Path) -> String {
    if let Some(root) = state.ws.folder() {
        if let Ok(relative) = path.strip_prefix(root) {
            return relative.display().to_string();
        }
    }
    path.display().to_string()
}

fn group_count(rows: &[ProblemEntry], start: usize) -> usize {
    let path = &rows[start].path;
    rows[start..]
        .iter()
        .take_while(|entry| &entry.path == path)
        .count()
}

fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    state.problems.rect = rect;
    state.problems.hits.clear();
    kit::fill(env.ctx, rect, p.bg);

    if state.problems.rows.is_empty() {
        state.problems.scroll = 0.0;
        state.problems.scroll_max = 0.0;
        state.problems.hover = None;
        let message = if !state.settings.lsp {
            "language servers are off"
        } else {
            "no problems detected"
        };
        kit::text(
            env.ctx,
            message,
            rect.left() + PAD + 2.0,
            rect.top() + 26.0,
            m.font_ui,
            p.faint,
        );
        return;
    }

    // Total content height: one group header per file, one row per diagnostic.
    let mut content = 0.0;
    let mut previous: Option<&Path> = None;
    for entry in &state.problems.rows {
        if previous != Some(entry.path.as_path()) {
            content += GROUP_H;
            previous = Some(entry.path.as_path());
        }
        content += ROW_H;
    }
    state.problems.scroll_max = (content - rect.height()).max(0.0);
    crate::ui::scroll::clamp(&mut state.problems.scroll, state.problems.scroll_max);

    env.ctx.save();
    env.ctx.clip_rect(rect);
    let mut y = rect.top() - state.problems.scroll;
    let mut previous: Option<&Path> = None;
    for (index, entry) in state.problems.rows.iter().enumerate() {
        if previous != Some(entry.path.as_path()) {
            let header =
                Rect::from_min_size(Vec2::new(rect.left(), y), Size::new(rect.width(), GROUP_H));
            if header.bottom() > rect.top() && header.top() < rect.bottom() {
                let label = group_label(state, &entry.path);
                let count = group_count(&state.problems.rows, index);
                let label = kit::truncate(
                    measurer,
                    &label,
                    m.font_small,
                    (rect.width() - PAD * 2.0 - 40.0).max(24.0),
                );
                kit::text(
                    env.ctx,
                    &label,
                    header.left() + PAD,
                    kit::baseline(measurer, header, m.font_small),
                    m.font_small,
                    p.dim,
                );
                kit::text_right(
                    env.ctx,
                    measurer,
                    &format!("({count})"),
                    header.right() - PAD,
                    kit::baseline(measurer, header, m.font_small),
                    m.font_small,
                    p.faint,
                );
            }
            y += GROUP_H;
            previous = Some(entry.path.as_path());
        }
        let row = Rect::from_min_size(Vec2::new(rect.left(), y), Size::new(rect.width(), ROW_H));
        y += ROW_H;
        if row.bottom() <= rect.top() || row.top() >= rect.bottom() {
            continue;
        }
        let diagnostic = &entry.diagnostic;
        let color = state.lsp.diagnostic_color(diagnostic.severity, &p);
        if state.problems.hover == Some(index) {
            kit::fill(env.ctx, row, p.hover);
            kit::vline(env.ctx, row.left(), row.top(), row.bottom(), color);
        }
        let glyph = match diagnostic.severity {
            1 => "E",
            2 => "W",
            3 => "I",
            _ => "H",
        };
        let baseline = kit::baseline(measurer, row, m.font_small);
        kit::text(
            env.ctx,
            glyph,
            row.left() + PAD + 8.0,
            baseline,
            m.font_small,
            color,
        );
        let position = format!(
            "{}:{}",
            diagnostic.start_line + 1,
            diagnostic.start_char + 1
        );
        kit::text(
            env.ctx,
            &position,
            row.left() + PAD + 26.0,
            baseline,
            m.font_small,
            p.faint,
        );
        let message_x =
            row.left() + PAD + 26.0 + kit::width(measurer, &position, m.font_small) + 12.0;
        let message = kit::truncate(
            measurer,
            &diagnostic.message,
            m.font_small,
            (row.right() - message_x - PAD).max(24.0),
        );
        kit::text(env.ctx, &message, message_x, baseline, m.font_small, p.text);
        state.problems.hits.push((index, row));
    }
    env.ctx.restore();

    if state.problems.scroll_max > 0.0 {
        let track = Rect::from_min_size(
            Vec2::new(rect.right() - 4.0, rect.top()),
            Size::new(3.0, rect.height()),
        );
        let thumb_h = (rect.height() * rect.height() / content)
            .min(rect.height())
            .max(1.0);
        let thumb_y = track.top()
            + (state.problems.scroll / state.problems.scroll_max) * (track.height() - thumb_h);
        kit::fill(
            env.ctx,
            Rect::from_min_size(Vec2::new(track.left(), thumb_y), Size::new(3.0, thumb_h)),
            p.faint,
        );
    }
}

/// Diagnostic row under `pos` (index into `state.problems.rows`).
pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    state
        .problems
        .hits
        .iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(index, _)| *index)
}

/// Handles a press in the panel. Returns whether the panel consumed it.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.problems.rect.contains(pos) {
        return false;
    }
    if let Some(index) = hover_at(state, pos) {
        jump(state, index);
    }
    true
}

/// Opens the diagnostic's file (when needed), places the caret and reveals it.
pub fn jump(state: &mut AppState, index: usize) {
    let Some(entry) = state.problems.rows.get(index).cloned() else {
        return;
    };
    let ProblemEntry { path, diagnostic } = entry;
    let same_doc = state
        .ws
        .active_doc()
        .and_then(|doc| doc.path())
        .is_some_and(|active| active == path);
    if !same_doc {
        if !path.is_file() {
            state.error(format!("missing file: {}", path.display()));
            return;
        }
        state.open_path(path.clone());
    }
    if let Some(doc) = state.ws.active_doc_mut() {
        let byte = byte_for_position(
            doc,
            diagnostic.start_line as u32,
            diagnostic.start_char as u32,
        );
        doc.set_caret(byte);
    }
    state.focus = Focus::Editor;
    state.editor.reveal = true;
    state.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::lsp::Diagnostic;
    use std::path::PathBuf;

    fn entry(path: &str, line: usize, severity: u32) -> ProblemEntry {
        ProblemEntry {
            path: PathBuf::from(path),
            diagnostic: Diagnostic {
                start_line: line,
                start_char: 0,
                end_line: line,
                end_char: 1,
                severity,
                message: "boom".into(),
            },
        }
    }

    #[test]
    fn group_counts_stop_at_the_next_file() {
        let rows = vec![
            entry("/w/a.rs", 0, 1),
            entry("/w/a.rs", 4, 2),
            entry("/w/b.rs", 1, 1),
        ];
        assert_eq!(group_count(&rows, 0), 2);
        assert_eq!(group_count(&rows, 2), 1);
    }
}
