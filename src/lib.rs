//! `igui_editor` — a compact text/Markdown editor built on the `igui` engine.
//!
//! ```text
//! Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
//! ```
//!
//! The crate is both a library and the `igui-editor` binary: the library exposes
//! the extension points (`syntax`, `settings`, `theme`) and the application
//! implementation; `main.rs` only parses arguments and starts the host.
//!
//! - `model/`: pure document + workspace + Markdown logic (unit-tested, no UI).
//! - `syntax/`: the syntax-highlighting **extension API**, built-in providers
//!   (Rust, C, C++, JS, TS, Go, GDScript, JSON, TOML, Markdown) and declarative
//!   JSON language packs (`load_language_packs`, `<config>/languages/*.json`).
//! - `state/`: shared app state written by callbacks and read by painters
//!   (one domain implementation per child module).
//! - `ui/`: one module per shell region and overlay.
//! - `app.rs`: shell construction, input routing, `igui_app` glue.
//! - `host.rs`: winit + wgpu plugins, native dialogs, CJK-capable face.
//! - `selfcheck.rs`: headless pipeline assertions (`--selfcheck`).
//!
//! ```no_run
//! use std::sync::Arc;
//! use igui_editor::syntax::{Highlight, SyntaxProvider, TokenKind};
//!
//! struct MyLang;
//! impl SyntaxProvider for MyLang {
//!     fn name(&self) -> &'static str { "mylang" }
//!     fn extensions(&self) -> &'static [&'static str] { &["mylang"] }
//!     fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
//!         let _ = (line, state);
//!         (Vec::<Highlight>::new(), 0)
//!     }
//! }
//!
//! // The host owns the registry; a plugin host would call this once:
//! // app.register_syntax(Arc::new(MyLang));
//! let _ = Arc::new(MyLang) as Arc<dyn SyntaxProvider>;
//! let _ = TokenKind::Keyword;
//! ```

pub mod keymap;
pub mod settings;
pub mod syntax;
pub mod theme;

pub mod drop;
pub mod fonts;
pub mod host;
#[cfg(feature = "selfcheck")]
pub mod selfcheck;

mod app;
mod dialogs;
mod input;
mod kit;
mod macos;
mod model;
mod native;
mod services;
mod state;
mod ui;
mod vim;

pub use app::EditorApp;
