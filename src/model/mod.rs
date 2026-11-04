//! Pure app model: no backend, no scene, no window.
//!
//! Everything here is unit-tested with `cargo test`; the UI layer is a view
//! over it.

pub mod bbcode;
pub mod brackets;
pub mod doc;
pub mod docs;
pub mod find;
pub mod fs_tree;
pub(crate) mod fsutil;
pub mod fuzzy;
pub mod git;
pub(crate) mod grapheme;
pub mod highlight;
pub mod lsp;
pub mod markdown;
pub mod processes;
pub mod pty;
pub mod recovery;
pub mod replace;
pub mod search;
pub mod session;
pub mod workspace;
pub mod workspace_index;
pub mod wrap;

pub use workspace::Workspace;
