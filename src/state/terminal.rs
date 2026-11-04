//! PTY terminal lifecycle, resize, input and polling.

use super::*;
use crate::kit::RectExt;

impl AppState {
    /// Grid to spawn the next PTY/parser with. The previous session's grid is
    /// a good first guess (the panel usually reopens at the same size); the
    /// painter corrects it against the parser as soon as it knows the body.
    pub(crate) fn terminal_grid(&self) -> (u16, u16) {
        (self.terminal.rows.max(2), self.terminal.cols.max(8))
    }

    pub fn toggle_terminal(&mut self) {
        if self.terminal.open {
            self.terminal.open = false;
            self.terminal.focused = false;
            self.terminal.session = None;
            self.terminal.parser = None;
            self.session.terminal_open = false;
            self.session.terminal_height = self.terminal.height;
            self.config_dirty = true;
            self.focus = Focus::Editor;
            self.needs_rebuild = true;
            self.dirty = true;
            return;
        }
        let cwd = self
            .git
            .status
            .root
            .clone()
            .or_else(|| self.git_cwd())
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let (spawn_rows, spawn_cols) = self.terminal_grid();
        match PtySession::spawn(&cwd, spawn_rows, spawn_cols, self.wake.clone()) {
            Ok(session) => {
                let rows = std::num::NonZeroU16::new(spawn_rows).unwrap();
                let cols = std::num::NonZeroU16::new(spawn_cols).unwrap();
                self.terminal.parser = Some(vt100::Parser::new(rows, cols, 4000));
                self.terminal.session = Some(session);
                self.terminal.cwd = cwd;
                self.terminal.open = true;
                self.terminal.focused = true;
                self.terminal.scrollback = 0;
                self.session.terminal_open = true;
                self.session.terminal_height = self.terminal.height;
                self.config_dirty = true;
                self.focus = Focus::Terminal;
                self.needs_rebuild = true;
                self.dirty = true;
            }
            Err(error) => self.error(format!("terminal: {error}")),
        }
    }

    /// Restores a persisted terminal panel after startup (new shell).
    pub fn restore_terminal(&mut self) {
        if self.terminal.open && self.terminal.session.is_none() {
            let cwd = self
                .git
                .cwd
                .clone()
                .or_else(|| self.git_cwd())
                .or_else(|| self.ws.folder().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."));
            let (spawn_rows, spawn_cols) = self.terminal_grid();
            if let Ok(session) = PtySession::spawn(&cwd, spawn_rows, spawn_cols, self.wake.clone())
            {
                self.terminal.parser = Some(vt100::Parser::new(
                    std::num::NonZeroU16::new(spawn_rows).unwrap(),
                    std::num::NonZeroU16::new(spawn_cols).unwrap(),
                    4000,
                ));
                self.terminal.session = Some(session);
                self.terminal.cwd = cwd;
                self.terminal.scrollback = 0;
            } else {
                self.terminal.open = false;
            }
        }
    }

    pub fn finish_terminal_resize(&mut self) {
        if self.terminal.drag.take().is_some() {
            self.session.terminal_height = self.terminal.height;
            self.session.terminal_open = self.terminal.open;
            self.config_dirty = true;
            self.dirty = true;
        }
    }

    pub fn tick_terminal(&mut self) {
        let chunks = self
            .terminal
            .session
            .as_mut()
            .map(PtySession::try_read)
            .unwrap_or_default();
        for chunk in chunks {
            if let Some(parser) = self.terminal.parser.as_mut() {
                parser.process(&chunk);
            }
            self.dirty = true;
        }
        if self
            .terminal
            .session
            .as_ref()
            .is_some_and(|session| session.exit.is_some())
            && self.terminal.open
        {
            self.status("terminal process exited");
            self.terminal.session = None;
        }
    }

    pub fn terminal_write(&mut self, bytes: &[u8]) {
        // Typing is an explicit "go back to the live prompt" action.
        self.terminal.scrollback = 0;
        self.terminal.selection = None;
        self.terminal.selecting = false;
        if let Some(session) = self.terminal.session.as_mut() {
            session.write(bytes);
        }
    }

    /// Cell under a viewport point, clamped to the terminal grid.
    pub fn terminal_cell_at(&self, pos: Vec2) -> Option<(u16, u16)> {
        let terminal = &self.terminal;
        let rect = terminal.rect;
        if rect.width() <= 0.0
            || rect.height() <= 0.0
            || terminal.cell_w <= 0.0
            || terminal.line_h <= 0.0
            || !rect.contains(pos)
        {
            return None;
        }
        let col = ((pos.x - rect.left()) / terminal.cell_w)
            .floor()
            .clamp(0.0, (terminal.cols.saturating_sub(1)) as f32) as u16;
        let row = ((pos.y - rect.top()) / terminal.line_h)
            .floor()
            .clamp(0.0, (terminal.rows.saturating_sub(1)) as f32) as u16;
        Some((row, col))
    }

    pub fn terminal_begin_selection(&mut self, cell: (u16, u16)) {
        self.terminal.selection = Some(TerminalSelection {
            anchor: cell,
            caret: cell,
        });
        self.terminal.selecting = true;
        self.dirty = true;
    }

    pub fn terminal_update_selection(&mut self, cell: (u16, u16)) {
        if !self.terminal.selecting {
            return;
        }
        if let Some(selection) = self.terminal.selection.as_mut() {
            selection.caret = cell;
            self.dirty = true;
        }
    }

    pub fn terminal_finish_selection(&mut self) {
        if !self.terminal.selecting {
            return;
        }
        self.terminal.selecting = false;
        if self
            .terminal
            .selection
            .is_some_and(|selection| selection.anchor == selection.caret)
        {
            self.terminal.selection = None;
        }
        self.dirty = true;
    }

    pub fn terminal_select_all(&mut self) {
        let last_row = self.terminal.rows.saturating_sub(1);
        let last_col = self.terminal.cols.saturating_sub(1);
        self.terminal.selection = Some(TerminalSelection {
            anchor: (0, 0),
            caret: (last_row, last_col),
        });
        self.terminal.selecting = false;
        self.dirty = true;
    }

    pub fn terminal_selection_text(&self) -> Option<String> {
        let selection = self.terminal.selection?;
        let parser = self.terminal.parser.as_ref()?;
        let (start, end) = selection.ordered();
        Some(selection_contents(
            parser.screen(),
            start,
            end,
            self.terminal.cols,
        ))
    }

    /// Scrolls the terminal viewport by `delta` rows (positive = older).
    pub fn terminal_scroll(&mut self, delta: isize) {
        let max = self
            .terminal
            .parser
            .as_ref()
            .map_or(0, |parser| parser.screen().scrollback_len());
        self.terminal.scrollback = clamp_scrollback(self.terminal.scrollback, delta, max);
        self.terminal.selection = None;
        self.terminal.selecting = false;
        self.dirty = true;
    }
}

fn selection_contents(
    screen: &vt100::Screen,
    start: (u16, u16),
    end: (u16, u16),
    cols: u16,
) -> String {
    // `contents_between` treats the end column as exclusive.
    let end_col = (end.1 + 1).min(cols);
    screen.contents_between(start.0, start.1, end.0, end_col)
}

fn clamp_scrollback(current: usize, delta: isize, max: usize) -> usize {
    let next = current as isize + delta;
    next.clamp(0, max as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::{clamp_scrollback, selection_contents, TerminalSelection};

    #[test]
    fn scrollback_clamps_to_the_history_bounds() {
        assert_eq!(clamp_scrollback(0, 5, 100), 5);
        assert_eq!(clamp_scrollback(5, -10, 100), 0);
        assert_eq!(clamp_scrollback(99, 10, 100), 100);
        assert_eq!(clamp_scrollback(10, 5, 0), 0);
    }

    #[test]
    fn selection_is_inclusive_and_ordered() {
        let selection = TerminalSelection {
            anchor: (2, 4),
            caret: (1, 1),
        };
        assert_eq!(selection.ordered(), ((1, 1), (2, 4)));
        assert!(selection.contains(1, 1));
        assert!(selection.contains(1, 9));
        assert!(selection.contains(2, 0));
        assert!(selection.contains(2, 4));
        assert!(!selection.contains(2, 5));
        assert!(!selection.contains(0, 9));
    }

    #[test]
    fn selection_copy_reads_vt100_contents() {
        use std::num::NonZeroU16;
        let mut parser =
            vt100::Parser::new(NonZeroU16::new(2).unwrap(), NonZeroU16::new(20).unwrap(), 0);
        parser.process(b"hello world");
        let screen = parser.screen();
        assert_eq!(selection_contents(screen, (0, 0), (0, 4), 20), "hello");
        assert_eq!(selection_contents(screen, (0, 6), (0, 10), 20), "world");
    }
}
