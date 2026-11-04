//! Cross-thread "draw a frame" wakeups and the deadline scheduler.
//!
//! The winit host sleeps in `ControlFlow::Wait` and only produces frames when
//! something asks for one. Two things outside the main thread need to ask:
//!
//! - the PTY reader thread, when the shell prints (otherwise terminal output
//!   would sit unread until the next click);
//! - [`FrameClock`], when a deadline elapses (status-message expiry, hover
//!   dwell, autosave, the external-change poll, …).
//!
//! [`WakeFn`] is deliberately opaque: `AppState` and the model layer pass it to
//! the PTY without knowing about winit. Only [`WindowWake`] touches the window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

pub use crate::model::pty::WakeFn;

/// A lazily-bound [`WakeFn`] for the app window.
///
/// Constructed before the window exists (the PTY session is restored while the
/// app is being built), filled on first resume. The closure is a no-op until
/// then, which keeps headless runs and tests working as before.
#[derive(Clone, Default)]
pub struct WindowWake {
    window: Arc<Mutex<Option<Arc<winit::window::Window>>>>,
}

impl WindowWake {
    /// The handle to hand to state/`PtySession` and the frame clock.
    pub fn function(&self) -> WakeFn {
        let slot = self.window.clone();
        Arc::new(move || {
            let window = slot.lock().ok().and_then(|guard| guard.clone());
            if let Some(window) = window {
                // `Window` is `Send + Sync`; winit schedules the call on the
                // main thread, so this is safe from a reader thread.
                window.request_redraw();
            }
        })
    }

    /// Binds the window. Idempotent: later resumes reuse the first window.
    pub fn set(&self, window: Arc<winit::window::Window>) {
        if let Ok(mut guard) = self.window.lock() {
            if guard.is_none() {
                *guard = Some(window);
            }
        }
    }
}

/// Wakes the host at the next deadline even without input.
///
/// `AppLogic::needs_frame` cannot express "wait 350 ms": returning `true` asks
/// for the *next* frame immediately, so time-based work would turn back into a
/// busy 60 fps loop. This thread holds the deadline and calls [`WakeFn`] when it
/// elapses; the app replaces the deadline on every frame.
pub struct FrameClock {
    shared: Arc<Clock>,
}

struct Clock {
    deadline: Mutex<Option<Instant>>,
    changed: Condvar,
    wake: WakeFn,
    stopped: AtomicBool,
}

impl FrameClock {
    pub fn start(wake: WakeFn) -> Self {
        let shared = Arc::new(Clock {
            deadline: Mutex::new(None),
            changed: Condvar::new(),
            wake,
            stopped: AtomicBool::new(false),
        });
        let thread = shared.clone();
        let _ = std::thread::Builder::new()
            .name("igui-frame-clock".into())
            .spawn(move || run(thread));
        Self { shared }
    }

    /// Replaces the next wakeup. `None` sleeps until an input event.
    pub fn set_deadline(&self, at: Option<Instant>) {
        let Ok(mut deadline) = self.shared.deadline.lock() else {
            return;
        };
        if *deadline == at {
            return;
        }
        *deadline = at;
        self.shared.changed.notify_one();
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
    }
}

fn run(clock: Arc<Clock>) {
    loop {
        let Ok(mut deadline) = clock.deadline.lock() else {
            return;
        };
        if clock.stopped.load(Ordering::SeqCst) {
            return;
        }
        let Some(at) = *deadline else {
            let guard = clock.changed.wait(deadline);
            drop(guard);
            continue;
        };
        let now = Instant::now();
        if at <= now {
            *deadline = None;
            drop(deadline);
            (clock.wake)();
            continue;
        }
        let Ok((guard, _)) = clock.changed.wait_timeout(deadline, at - now) else {
            return;
        };
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn clock_wakes_once_at_the_deadline() {
        let count = Arc::new(AtomicUsize::new(0));
        let wake = {
            let count = count.clone();
            Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }) as WakeFn
        };
        let clock = FrameClock::start(wake);
        clock.set_deadline(Some(Instant::now() + Duration::from_millis(10)));
        let deadline = Instant::now() + Duration::from_secs(2);
        while count.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "one frame per deadline");
        // A cleared deadline must not keep firing.
        clock.set_deadline(None);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn elapsed_deadlines_resolve_immediately() {
        let count = Arc::new(AtomicUsize::new(0));
        let wake = {
            let count = count.clone();
            Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }) as WakeFn
        };
        let clock = FrameClock::start(wake);
        clock.set_deadline(Some(Instant::now() - Duration::from_millis(1)));
        let deadline = Instant::now() + Duration::from_secs(2);
        while count.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(count.load(Ordering::SeqCst) >= 1);
    }
}
