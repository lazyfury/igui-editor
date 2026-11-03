//! File dialogs behind a trait, so the app stays testable off-screen.
//!
//! The dialog calls are **asynchronous** on purpose: on macOS `rfd`'s blocking
//! API runs a nested modal loop inside the winit callback, which risks
//! re-entering the application handler. The async API attaches a *sheet* to the
//! parent window instead, and the app polls the returned future once per frame
//! (`Waker::noop` + `needs_frame`) — no nested loop, no re-entrancy.
//!
//! `host.rs` installs the native implementation (macOS sheets, Linux XDG
//! portal, Windows common dialogs). The headless self-check installs
//! [`ScriptedDialogs`], whose answers are queued in advance, so a test never
//! opens an OS window.

#[cfg(any(test, feature = "selfcheck"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "selfcheck"))]
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(any(test, feature = "selfcheck"))]
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

/// One in-flight dialog. `Ready(None)` is a cancelled dialog.
pub type DialogFuture = Pin<Box<dyn Future<Output = Option<PathBuf>>>>;

/// Polls a dialog future without a runtime. `None` means "still open".
pub fn poll_dialog(future: &mut DialogFuture) -> Option<Option<PathBuf>> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    }
}

/// The dialog surface the app uses. Implemented by the native host and by the
/// scripted test double.
pub trait Dialogs {
    fn open_file(&mut self, start: Option<&Path>) -> DialogFuture;
    fn open_folder(&mut self, start: Option<&Path>) -> DialogFuture;
    fn save_file(&mut self, start: Option<&Path>, name: &str) -> DialogFuture;
}

/// Which dialog a scripted call consumed.
#[cfg(any(test, feature = "selfcheck"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Call {
    OpenFile,
    OpenFolder,
    SaveFile,
}

#[cfg(any(test, feature = "selfcheck"))]
#[derive(Default)]
struct ScriptedState {
    results: VecDeque<Option<PathBuf>>,
    calls: Vec<Call>,
}

/// A test double: each dialog call pops the next queued answer.
///
/// Cloneable and shared, so the test keeps a handle to push answers and inspect
/// the calls while `EditorApp` owns the `dyn Dialogs` box.
#[cfg(any(test, feature = "selfcheck"))]
#[derive(Clone, Default)]
pub struct ScriptedDialogs {
    state: Rc<RefCell<ScriptedState>>,
}

#[cfg(any(test, feature = "selfcheck"))]
impl ScriptedDialogs {
    pub fn new(results: Vec<Option<PathBuf>>) -> Self {
        let dialogs = Self::default();
        dialogs.state.borrow_mut().results = results.into();
        dialogs
    }

    /// Queues the answer to the next dialog call.
    pub fn push(&self, result: Option<PathBuf>) {
        self.state.borrow_mut().results.push_back(result);
    }

    /// Calls made so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state.borrow().calls.clone()
    }

    fn next(&self, call: Call) -> DialogFuture {
        let mut state = self.state.borrow_mut();
        state.calls.push(call);
        let result = state.results.pop_front().flatten();
        Box::pin(std::future::ready(result))
    }
}

#[cfg(any(test, feature = "selfcheck"))]
impl Dialogs for ScriptedDialogs {
    fn open_file(&mut self, _start: Option<&Path>) -> DialogFuture {
        self.next(Call::OpenFile)
    }

    fn open_folder(&mut self, _start: Option<&Path>) -> DialogFuture {
        self.next(Call::OpenFolder)
    }

    fn save_file(&mut self, _start: Option<&Path>, _name: &str) -> DialogFuture {
        self.next(Call::SaveFile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_dialogs_answer_in_order_and_record_calls() {
        let handle = ScriptedDialogs::new(vec![
            Some(PathBuf::from("/tmp/a.rs")),
            None,
            Some(PathBuf::from("/tmp/out.md")),
        ]);
        let mut dialogs = handle.clone();
        let mut open = dialogs.open_file(None);
        assert_eq!(
            poll_dialog(&mut open),
            Some(Some(PathBuf::from("/tmp/a.rs")))
        );
        let mut folder = dialogs.open_folder(None);
        assert_eq!(poll_dialog(&mut folder), Some(None));
        let mut save = dialogs.save_file(None, "out.md");
        assert_eq!(
            poll_dialog(&mut save),
            Some(Some(PathBuf::from("/tmp/out.md")))
        );
        assert_eq!(
            handle.calls(),
            vec![Call::OpenFile, Call::OpenFolder, Call::SaveFile]
        );
    }
}
