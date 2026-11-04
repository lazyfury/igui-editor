//! PTY terminal lifecycle, resize, input and polling.

use super::*;

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
        if let Some(session) = self.terminal.session.as_mut() {
            session.write(bytes);
        }
    }

    /// Scrolls the terminal viewport by `delta` rows (positive = older).
    pub fn terminal_scroll(&mut self, delta: isize) {
        let max = self
            .terminal
            .parser
            .as_ref()
            .map_or(0, |parser| parser.screen().scrollback_len());
        self.terminal.scrollback = clamp_scrollback(self.terminal.scrollback, delta, max);
        self.dirty = true;
    }
}

fn clamp_scrollback(current: usize, delta: isize, max: usize) -> usize {
    let next = current as isize + delta;
    next.clamp(0, max as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::clamp_scrollback;

    #[test]
    fn scrollback_clamps_to_the_history_bounds() {
        assert_eq!(clamp_scrollback(0, 5, 100), 5);
        assert_eq!(clamp_scrollback(5, -10, 100), 0);
        assert_eq!(clamp_scrollback(99, 10, 100), 100);
        assert_eq!(clamp_scrollback(10, 5, 0), 0);
    }
}
