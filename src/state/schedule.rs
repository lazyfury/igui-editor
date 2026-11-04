//! Frame scheduling: when the app must wake even though nothing happened.
//!
//! The winit host sleeps in `ControlFlow::Wait`. Anything that used to rely on
//! "there is always another frame" (status-message expiry, hover dwell,
//! autosave, the external-change poll, a coalesced preview rebuild) must now
//! name its deadline here. [`super::AppState::next_wake`] returns the earliest
//! one; `EditorApp` hands it to [`crate::wake::FrameClock`], which asks for one
//! frame when it elapses.
//!
//! Keeping the durations in one place means the tick functions and the
//! scheduler cannot drift apart.

use std::time::{Duration, Instant};

use super::AppState;

/// How long a status-bar message stays visible.
pub(crate) const STATUS_TTL: Duration = Duration::from_secs(3);
/// Cursor dwell before a hover request is sent.
pub(crate) const HOVER_DELAY: Duration = Duration::from_millis(350);
/// How long a dismissed hover tooltip stays up before it is removed.
pub(crate) const HOVER_DISMISS: Duration = Duration::from_millis(1_500);
/// Idle time before autosave/recovery snapshots are written.
pub(crate) const AUTOSAVE_DELAY: Duration = Duration::from_secs(2);
/// Poll interval for "an open file changed on disk".
pub(crate) const EXTERNAL_POLL: Duration = Duration::from_secs(1);

impl AppState {
    /// The earliest instant the app needs a frame without input.
    ///
    /// Only real deadlines belong here. A "keep drawing until done" state
    /// (highlight catch-up, a busy worker) stays in `EditorApp::needs_frame`,
    /// which asks for the next frame immediately.
    pub fn next_wake(&self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut add = |at: Instant| next = Some(next.map_or(at, |cur| cur.min(at)));

        if let Some((_, _, at)) = self.message.as_ref() {
            add(*at + STATUS_TTL);
        }
        if let Some(at) = self.hover_leave_at {
            add(at + HOVER_DISMISS);
        }
        if let Some(target) = self.hover_target.as_ref() {
            if !target.requested {
                add(target.since + HOVER_DELAY);
            }
        }
        if let Some(at) = self.auto_completion_at {
            add(at);
        }
        if let Some(at) = self.preview.rebuild_deadline() {
            add(at);
        }
        // Only open documents with a path can change on disk.
        if self.ws.tabs.iter().any(|doc| doc.path().is_some()) {
            add(self.last_external_check + EXTERNAL_POLL);
        }
        // Autosave only has work while a buffer is dirty.
        if self.ws.tabs.iter().any(|doc| doc.is_dirty()) {
            add(self.last_activity + AUTOSAVE_DELAY);
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal state with no I/O-backed services. The deadline logic only
    /// reads the fields it is given, so an empty workspace is enough.
    fn state() -> AppState {
        let config = std::env::temp_dir().join(format!(
            "igui-editor-schedule-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        AppState::new(
            crate::theme::Theme::dark(),
            crate::model::Workspace::detached(),
            crate::model::session::Session::default(),
            config,
        )
    }

    #[test]
    fn no_work_means_no_wakeup() {
        let mut state = state();
        state.message = None;
        state.hover_leave_at = None;
        state.hover_target = None;
        state.auto_completion_at = None;
        state.preview.cancel_rebuild();
        assert_eq!(state.next_wake(), None);
    }

    #[test]
    fn the_earliest_deadline_wins() {
        let mut state = state();
        state.hover_leave_at = None;
        state.hover_target = None;
        state.preview.cancel_rebuild();
        let now = Instant::now();
        state.last_external_check = now;
        // The message expires in ~20 ms; autocompletion is 100 ms out.
        state.message = Some((
            "m".into(),
            false,
            now - STATUS_TTL + Duration::from_millis(20),
        ));
        state.auto_completion_at = Some(now + Duration::from_millis(100));
        let next = state.next_wake().expect("a deadline");
        assert!(next <= now + Duration::from_millis(25));
    }
}
