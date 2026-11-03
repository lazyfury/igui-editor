//! Pure app model: no backend, no scene, no window.
//!
//! Everything here is unit-tested with `cargo test`; the UI layer is a view
//! over it.

pub mod brackets;
pub mod doc;
pub mod find;
pub mod fs_tree;
pub mod highlight;
pub mod markdown;
pub mod session;
pub mod workspace;

pub use workspace::Workspace;
