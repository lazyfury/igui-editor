//! Native PTY session used by the built-in terminal.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

/// "Please draw a frame" callback. Thread-safe by construction; the model does
/// not know what the other end does with it.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use super::processes;

/// Whether a locale name selects UTF-8 (`UTF-8`, `utf8`, `zh_CN.UTF-8`, …).
fn is_utf8_locale(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("utf-8") || value.contains("utf8")
}

/// Locale variables that need to be adjusted for the shell child.
#[derive(Debug, PartialEq, Eq)]
struct LocaleFix {
    remove_lc_all: bool,
    set_lc_ctype: bool,
    set_lang: bool,
}

/// macOS GUI apps launched from Finder often inherit no `LANG`/`LC_*` at all.
/// The child shell then runs in the `C` locale, and zsh prints multibyte
/// prompt text as `\M-^V`-style meta escapes (a Chinese folder name becomes
/// visible garbage). Pick a UTF-8 fallback only when the effective locale is
/// missing or not UTF-8; an already valid locale is left untouched.
fn locale_fix(lc_all: Option<&str>, lc_ctype: Option<&str>, lang: Option<&str>) -> LocaleFix {
    let effective = lc_all
        .filter(|value| !value.is_empty())
        .or_else(|| lc_ctype.filter(|value| !value.is_empty()))
        .or_else(|| lang.filter(|value| !value.is_empty()));
    if effective.is_some_and(is_utf8_locale) {
        return LocaleFix {
            remove_lc_all: false,
            set_lc_ctype: false,
            set_lang: false,
        };
    }
    LocaleFix {
        // `LC_ALL=C` would override any UTF-8 category we set below.
        remove_lc_all: lc_all.is_some_and(|value| !value.is_empty() && !is_utf8_locale(value)),
        set_lc_ctype: !lc_ctype.is_some_and(is_utf8_locale),
        set_lang: !lang.is_some_and(is_utf8_locale),
    }
}

/// Applies [`locale_fix`] to a shell command.
fn ensure_utf8_locale(cmd: &mut CommandBuilder) {
    let fix = locale_fix(
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LC_CTYPE").ok().as_deref(),
        std::env::var("LANG").ok().as_deref(),
    );
    let fallback = if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    };
    if fix.remove_lc_all {
        cmd.env_remove("LC_ALL");
    }
    if fix.set_lc_ctype {
        cmd.env("LC_CTYPE", fallback);
    }
    if fix.set_lang {
        cmd.env("LANG", fallback);
    }
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    pub exit: Option<u32>,
}

impl PtySession {
    /// `wake` is called from the reader thread whenever output arrives (and
    /// on EOF), so the app can stay asleep while the shell is idle.
    pub fn spawn(cwd: &Path, rows: u16, cols: u16, wake: Option<WakeFn>) -> Result<Self, String> {
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
        // A login shell reads `/etc/zprofile` and `~/.zprofile`, which is where
        // macOS path_helper, Homebrew and rustup put their PATH setup. Without
        // `-l` a packaged app starts with the Finder environment and `cargo`
        // is missing even though it works in Terminal.app.
        #[cfg(unix)]
        cmd.arg("-l");
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        ensure_utf8_locale(&mut cmd);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|error| format!("spawn shell: {error}"))?;
        if let Some(pid) = child.process_id() {
            processes::register(pid);
        }
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
                            if let Some(wake) = wake.as_ref() {
                                wake();
                            }
                        }
                    }
                }
                // EOF: the app still needs one frame to notice `try_wait`.
                if let Some(wake) = wake.as_ref() {
                    wake();
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
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Some(pid) = self.child.process_id() {
            processes::unregister(pid);
        }
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_wakes_the_host() {
        let dir = std::env::temp_dir().join(format!("igui-editor-pty-wake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let wake: WakeFn = Arc::new(move || {
            let _ = tx.send(());
        });
        let mut session = PtySession::spawn(&dir, 24, 80, Some(wake)).unwrap();
        // Any output (a prompt or the echoed line) must ask for a frame.
        session.write(b"echo awake\n");
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("pty output should wake the host");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utf8_locales_are_left_alone() {
        let fix = locale_fix(None, None, Some("zh_CN.UTF-8"));
        assert_eq!(
            fix,
            LocaleFix {
                remove_lc_all: false,
                set_lc_ctype: false,
                set_lang: false,
            }
        );
        let fix = locale_fix(Some("en_US.utf8"), Some("C"), Some("C"));
        assert!(!fix.remove_lc_all);
        assert!(!fix.set_lc_ctype);
        assert!(!fix.set_lang);
    }

    #[test]
    fn c_locale_is_replaced_with_a_utf8_category() {
        let fix = locale_fix(Some("C"), Some("C"), Some("C"));
        assert_eq!(
            fix,
            LocaleFix {
                remove_lc_all: true,
                set_lc_ctype: true,
                set_lang: true,
            }
        );
        // Missing locale (Finder launch) gets the same fallback.
        let fix = locale_fix(None, None, None);
        assert!(!fix.remove_lc_all);
        assert!(fix.set_lc_ctype);
        assert!(fix.set_lang);
        // Only a broken category is replaced, not a valid LANG.
        let fix = locale_fix(None, Some("C"), Some("zh_CN.UTF-8"));
        assert!(fix.set_lc_ctype);
        assert!(!fix.set_lang);
    }

    #[test]
    fn shell_prompt_in_a_chinese_folder_has_no_meta_escapes() {
        let dir = std::env::temp_dir().join(format!("igui-editor-中文终端-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = PtySession::spawn(&dir, 24, 80, None).unwrap();
        let mut parser = vt100::Parser::new(
            std::num::NonZeroU16::new(24).unwrap(),
            std::num::NonZeroU16::new(80).unwrap(),
            4000,
        );
        let mut text = String::new();
        for _ in 0..100 {
            for chunk in session.try_read() {
                parser.process(&chunk);
            }
            text = screen_text(parser.screen());
            if text.contains("中文终端") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !text.contains("\\M-"),
            "shell prompt used C-locale meta escapes: {text:?}"
        );
        assert!(
            !text.contains("]0;") && !text.contains("]1;") && !text.contains("]2;"),
            "an OSC title sequence leaked into the grid: {text:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn screen_text(screen: &vt100::Screen) -> String {
        let (rows, cols) = screen.size();
        let mut text = String::new();
        for row in 0..rows.get() {
            for col in 0..cols.get() {
                if let Some(cell) = screen.cell(row, col) {
                    text.push_str(cell.contents());
                }
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn pty_round_trip() {
        let dir = std::env::temp_dir();
        let mut session = PtySession::spawn(&dir, 24, 80, None).unwrap();
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
