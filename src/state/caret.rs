//! Where the platform IME candidate window points.
//!
//! The editor, the terminal and every app overlay paint their own text fields,
//! so the host cannot discover a field's caret from the scene tree. Painters
//! record the caret rectangle in the field's state; this module picks the one
//! that currently owns text input.
//!
//! The order mirrors `app::event` routing: the first open overlay wins, then the
//! focused tree/element. Only one overlay can be open at a time, so the list is
//! about precedence, not coexistence.

use super::*;

impl AppState {
    /// Caret rectangle of the field that owns text input, in viewport space.
    ///
    /// `None` parks the platform candidate window at the origin instead of
    /// leaving it at a stale position.
    pub fn ime_caret(&self) -> Option<Rect> {
        let visible =
            |rect: Rect| (rect.size.width > 0.0 && rect.size.height > 0.0).then_some(rect);

        if self.rename_symbol.open {
            return visible(self.rename_symbol.caret_rect);
        }
        if self.new_file.open {
            return visible(self.new_file.caret_rect);
        }
        if self.goto_line.open {
            return visible(self.goto_line.caret_rect);
        }
        if self.quick_open.open {
            return visible(self.quick_open.caret_rect);
        }
        if self.search.open {
            return visible(self.search.caret_rect);
        }
        if self.find.open && matches!(self.focus, Focus::Find | Focus::Replace) {
            return visible(match self.focus {
                Focus::Replace => self.find.replace_caret,
                _ => self.find.find_caret,
            });
        }
        if self.settings_open {
            return visible(crate::ui::settings::ime_caret(self));
        }
        if self.terminal.open && self.bottom_tab == Panel::Terminal && self.focus == Focus::Terminal
        {
            return visible(
                self.terminal
                    .active()
                    .map_or(Rect::ZERO, |session| session.caret_rect),
            );
        }
        let editor_visible = self.focus == Focus::Editor
            && !self.git.diff_visible
            && self.view_of_active() != ViewMode::Preview
            && !self.ws.active_doc().is_some_and(|doc| doc.is_docs_view());
        if editor_visible {
            return visible(self.editor.caret_rect);
        }
        None
    }
}
