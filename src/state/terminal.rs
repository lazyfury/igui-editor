//! PTY terminal lifecycle, resize, input and polling.

use super::*;
use crate::kit::RectExt;

impl AppState {
    /// Grid to spawn the next PTY/parser with. The active session's grid is a
    /// good first guess; the painter corrects it as soon as it knows the body.
    pub(crate) fn terminal_grid(&self) -> (u16, u16) {
        self.terminal.active().map_or((24, 80), |session| {
            (session.rows.max(2), session.cols.max(8))
        })
    }

    fn terminal_cwd(&self) -> PathBuf {
        self.terminal
            .active()
            .map(|session| session.cwd.clone())
            .or_else(|| self.git.status.root.clone())
            .or_else(|| self.git_cwd())
            .or_else(|| self.ws.folder().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn spawn_terminal_session(&mut self, cwd: PathBuf) -> Result<usize, String> {
        let (rows, cols) = self.terminal_grid();
        let session = PtySession::spawn(&cwd, rows, cols, self.wake.clone())
            .map_err(|error| format!("terminal: {error}"))?;
        let parser = vt100::Parser::new(
            std::num::NonZeroU16::new(rows).unwrap(),
            std::num::NonZeroU16::new(cols).unwrap(),
            4000,
        );
        self.terminal.sessions.push(TerminalSession {
            session: Some(session),
            parser: Some(parser),
            cwd,
            rows,
            cols,
            ..TerminalSession::default()
        });
        Ok(self.terminal.sessions.len() - 1)
    }

    pub fn toggle_terminal(&mut self) {
        if self.terminal.open {
            self.terminal.open = false;
            self.terminal.focused = false;
            self.terminal.sessions.clear();
            self.terminal.active = 0;
            self.session.terminal_open = false;
            self.session.terminal_height = self.terminal.height;
            self.config_dirty = true;
            self.focus = Focus::Editor;
            self.needs_rebuild = true;
            self.dirty = true;
            return;
        }
        match self.spawn_terminal_session(self.terminal_cwd()) {
            Ok(index) => {
                self.terminal.active = index;
                self.terminal.open = true;
                self.terminal.focused = true;
                self.session.terminal_open = true;
                self.session.terminal_height = self.terminal.height;
                self.config_dirty = true;
                self.focus = Focus::Terminal;
                self.needs_rebuild = true;
                self.dirty = true;
            }
            Err(error) => self.error(error),
        }
    }

    /// Opens another shell in the same panel.
    pub fn terminal_new_session(&mut self) {
        let cwd = self.terminal_cwd();
        match self.spawn_terminal_session(cwd) {
            Ok(index) => {
                self.terminal.active = index;
                self.terminal.focused = true;
                self.focus = Focus::Terminal;
                self.dirty = true;
            }
            Err(error) => self.error(error),
        }
    }

    pub fn terminal_select_session(&mut self, index: usize) {
        if index < self.terminal.sessions.len() {
            self.terminal.active = index;
            self.terminal.focused = true;
            self.focus = Focus::Terminal;
            self.dirty = true;
        }
    }

    pub fn terminal_close_session(&mut self, index: usize) {
        if index >= self.terminal.sessions.len() {
            return;
        }
        self.terminal.sessions.remove(index);
        if self.terminal.sessions.is_empty() {
            self.terminal.open = false;
            self.terminal.focused = false;
            self.terminal.active = 0;
            self.session.terminal_open = false;
            self.focus = Focus::Editor;
            self.needs_rebuild = true;
        } else if index < self.terminal.active {
            self.terminal.active -= 1;
        } else if self.terminal.active >= self.terminal.sessions.len() {
            self.terminal.active = self.terminal.sessions.len() - 1;
        }
        self.dirty = true;
    }

    /// Restores a persisted terminal panel after startup (new shell).
    pub fn restore_terminal(&mut self) {
        if self.terminal.open && self.terminal.sessions.is_empty() {
            let cwd = self
                .git
                .cwd
                .clone()
                .or_else(|| self.git_cwd())
                .or_else(|| self.ws.folder().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."));
            match self.spawn_terminal_session(cwd) {
                Ok(index) => self.terminal.active = index,
                Err(_) => self.terminal.open = false,
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
        let mut exited = false;
        for terminal in &mut self.terminal.sessions {
            let chunks = terminal
                .session
                .as_mut()
                .map(PtySession::try_read)
                .unwrap_or_default();
            for chunk in chunks {
                if let Some(parser) = terminal.parser.as_mut() {
                    parser.process(&chunk);
                }
                self.dirty = true;
            }
            if terminal
                .session
                .as_ref()
                .is_some_and(|session| session.exit.is_some())
            {
                exited = true;
            }
        }
        if exited && self.terminal.open {
            let before = self.terminal.sessions.len();
            let old_active = self.terminal.active;
            let mut kept = Vec::with_capacity(before);
            let mut new_active = None;
            for (index, terminal) in self.terminal.sessions.drain(..).enumerate() {
                if terminal
                    .session
                    .as_ref()
                    .is_some_and(|session| session.exit.is_some())
                {
                    continue;
                }
                if index == old_active {
                    new_active = Some(kept.len());
                }
                kept.push(terminal);
            }
            let removed = before - kept.len();
            self.terminal.sessions = kept;
            if self.terminal.sessions.is_empty() {
                self.terminal.open = false;
                self.terminal.focused = false;
                self.terminal.active = 0;
                self.session.terminal_open = false;
                self.focus = Focus::Editor;
                self.needs_rebuild = true;
            } else {
                self.terminal.active =
                    new_active.unwrap_or_else(|| old_active.min(self.terminal.sessions.len() - 1));
            }
            self.status(format!(
                "{removed} terminal process{} exited",
                if removed == 1 { "" } else { "es" }
            ));
            self.dirty = true;
        }
    }

    pub fn terminal_write(&mut self, bytes: &[u8]) {
        if let Some(terminal) = self.terminal.active_mut() {
            // Typing is an explicit "go back to the live prompt" action.
            terminal.scrollback = 0;
            terminal.selection = None;
            terminal.selecting = false;
            if let Some(session) = terminal.session.as_mut() {
                session.write(bytes);
            }
        }
    }

    /// Cell under a viewport point, clamped to the active terminal grid.
    pub fn terminal_cell_at(&self, pos: Vec2) -> Option<(u16, u16)> {
        let terminal = &self.terminal;
        let session = terminal.active()?;
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
            .clamp(0.0, (session.cols.saturating_sub(1)) as f32) as u16;
        let row = ((pos.y - rect.top()) / terminal.line_h)
            .floor()
            .clamp(0.0, (session.rows.saturating_sub(1)) as f32) as u16;
        Some((row, col))
    }

    pub fn terminal_begin_selection(&mut self, cell: (u16, u16)) {
        if let Some(terminal) = self.terminal.active_mut() {
            terminal.selection = Some(TerminalSelection {
                anchor: cell,
                caret: cell,
            });
            terminal.selecting = true;
            self.dirty = true;
        }
    }

    pub fn terminal_update_selection(&mut self, cell: (u16, u16)) {
        if let Some(terminal) = self.terminal.active_mut() {
            if !terminal.selecting {
                return;
            }
            if let Some(selection) = terminal.selection.as_mut() {
                selection.caret = cell;
                self.dirty = true;
            }
        }
    }

    pub fn terminal_finish_selection(&mut self) {
        if let Some(terminal) = self.terminal.active_mut() {
            if !terminal.selecting {
                return;
            }
            terminal.selecting = false;
            if terminal
                .selection
                .is_some_and(|selection| selection.anchor == selection.caret)
            {
                terminal.selection = None;
            }
            self.dirty = true;
        }
    }

    pub fn terminal_select_all(&mut self) {
        if let Some(terminal) = self.terminal.active_mut() {
            let last_row = terminal.rows.saturating_sub(1);
            let last_col = terminal.cols.saturating_sub(1);
            terminal.selection = Some(TerminalSelection {
                anchor: (0, 0),
                caret: (last_row, last_col),
            });
            terminal.selecting = false;
            self.dirty = true;
        }
    }

    pub fn terminal_selection_text(&self) -> Option<String> {
        let terminal = self.terminal.active()?;
        let selection = terminal.selection?;
        let parser = terminal.parser.as_ref()?;
        let (start, end) = selection.ordered();
        Some(selection_contents(
            parser.screen(),
            start,
            end,
            terminal.cols,
        ))
    }

    /// Scrolls the active terminal viewport by `delta` rows (positive = older).
    pub fn terminal_scroll(&mut self, delta: isize) {
        if let Some(terminal) = self.terminal.active_mut() {
            let max = terminal
                .parser
                .as_ref()
                .map_or(0, |parser| parser.screen().scrollback_len());
            terminal.scrollback = clamp_scrollback(terminal.scrollback, delta, max);
            terminal.selection = None;
            terminal.selecting = false;
            self.dirty = true;
        }
    }

    /// Stops every terminal shell before an AppKit/OS quit.
    pub fn shutdown_terminal_sessions(&mut self) {
        self.terminal.sessions.clear();
        self.terminal.active = 0;
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
