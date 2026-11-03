//! Pure app model: no backend, no scene, no window.
//!
//! Everything here is unit-tested with `cargo test`; the UI layer is a view
//! over it.

pub mod brackets;
pub mod doc;
pub mod find;
pub mod fs_tree;
pub mod fuzzy;
pub mod highlight;
pub mod markdown;
pub mod recovery;
pub mod search;
pub mod session;
pub mod workspace;
pub mod workspace_index;

pub use workspace::Workspace;
