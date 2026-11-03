//! Commands from the native macOS shell (Finder open, Dock menu, quit) into
//! the app loop. Native callbacks only enqueue; `EditorApp::update` drains the
//! queue on the main thread, so they never touch UI state directly.

use std::cell::RefCell;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeRequest {
    /// Files/folders from Finder, `open -a`, or the Dock recent menu.
    Open(Vec<PathBuf>),
    NewFile,
    ClearRecent,
    Quit,
}

thread_local! {
    static QUEUE: RefCell<Vec<NativeRequest>> = const { RefCell::new(Vec::new()) };
}

pub fn push(request: NativeRequest) {
    QUEUE.with(|queue| queue.borrow_mut().push(request));
}

pub fn take() -> Vec<NativeRequest> {
    QUEUE.with(|queue| std::mem::take(&mut *queue.borrow_mut()))
}
