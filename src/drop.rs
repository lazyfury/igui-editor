//! File-drop target shared by the winit observer and the app.
//!
//! `winit` reports `HoveredFile` / `HoveredFileCancelled` / `DroppedFile` as
//! window events; the host's observer pushes them here and `EditorApp::update`
//! drains the state (so drop handling never runs inside the event observer).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Hover flag plus the paths of the current drop.
#[derive(Default)]
pub struct DropTarget {
    pub hovered: bool,
    pub paths: Vec<PathBuf>,
}

/// Shared handle installed as an `igui_app` service.
pub type SharedDropTarget = Rc<RefCell<DropTarget>>;

/// Wraps a fresh target as a service value.
pub fn shared() -> SharedDropTarget {
    Rc::new(RefCell::new(DropTarget::default()))
}
