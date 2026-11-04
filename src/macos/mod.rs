//! Native macOS integration. All public functions are no-ops off macOS.

#[cfg(target_os = "macos")]
mod imp;

#[cfg(not(target_os = "macos"))]
pub use fallback::*;
#[cfg(target_os = "macos")]
pub use imp::{
    begin_window_drag, clear_system_recent, install, install_dock, minimize_window, note_recent,
    reply_quit, request_quit, set_dirty_count, set_recent, toggle_zoom,
};

#[cfg(not(target_os = "macos"))]
mod fallback {
    use std::path::{Path, PathBuf};

    pub fn install(_window: igui_winit::SharedWindow) {}
    pub fn install_dock() {}
    pub fn set_recent(_recent: &[PathBuf]) {}
    pub fn note_recent(_path: &Path) {}
    pub fn set_dirty_count(_dirty: usize) {}
    pub fn reply_quit(_terminate: bool) {}
    pub fn clear_system_recent() {}
    pub fn minimize_window() {}
    pub fn toggle_zoom() {}
    pub fn begin_window_drag() {}
    pub fn request_quit() {}
}
