//! Native macOS integration. All public functions are no-ops off macOS.

#[cfg(target_os = "macos")]
mod imp;

#[cfg(not(target_os = "macos"))]
pub use fallback::*;
#[cfg(target_os = "macos")]
pub use imp::{
    begin_window_drag, clear_system_recent, continue_window_drag, end_window_drag, install,
    install_dock, minimize_window, note_recent, request_quit, set_recent, toggle_zoom,
    window_drag_button_down, window_is_zoomed,
};

#[cfg(not(target_os = "macos"))]
mod fallback {
    use std::path::{Path, PathBuf};

    pub fn install(_window: igui_winit::SharedWindow) {}
    pub fn install_dock() {}
    pub fn set_recent(_recent: &[PathBuf]) {}
    pub fn note_recent(_path: &Path) {}
    pub fn clear_system_recent() {}
    pub fn minimize_window() {}
    pub fn toggle_zoom() {}
    pub fn begin_window_drag() {}
    pub fn window_drag_button_down() -> bool {
        true
    }
    pub fn continue_window_drag() {}
    pub fn end_window_drag() {}
    pub fn request_quit() {}
    pub fn show_window() {}
    pub fn window_is_zoomed() -> bool {
        false
    }
}
