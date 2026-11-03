//! Native PTY session used by the built-in terminal.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    pub exit: Option<u32>,
}

impl PtySession {
    pub fn spawn(cwd: &Path, rows: u16, cols: u16) -> Result<Self, String> {
        let size = PtySize {
            rows: rows.max(2),
            cols: cols.max(8),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| format!("openpty: {error}"))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|error| format!("spawn shell: {error}"))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("pty reader: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("pty writer: {error}"))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("igui-pty-reader".into())
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .ok();
        Ok(Self {
            master: pair.master,
            writer,
            child,
            rx,
            exit: None,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(8),
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn try_read(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        while let Ok(chunk) = self.rx.try_recv() {
            chunks.push(chunk);
        }
        if self.exit.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit = Some(status.exit_code());
            }
        }
        chunks
    }

    pub fn cwd_hint(&self) -> Option<PathBuf> {
        None
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_round_trip() {
        let dir = std::env::temp_dir();
        let mut session = PtySession::spawn(&dir, 24, 80).unwrap();
        session.write(b"printf terminal-ok\\n");
        let mut seen = String::new();
        for _ in 0..100 {
            for chunk in session.try_read() {
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
            if seen.contains("terminal-ok") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(seen.contains("terminal-ok"), "{seen:?}");
    }
}
