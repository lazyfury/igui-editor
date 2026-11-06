# igui-editor

A compact, square-cornered **text / Markdown editor** built on the
[`igui`](https://github.com/lazyfury/igui) engine (Rust). Native window:
`winit` + `wgpu`, UI painted through `igui_ui` / `igui_render` only.

Tabs · undo/redo · find & replace · syntax highlighting · native LSP · outline ·
Markdown split preview (tables included) · Quick Open · project search +
replace · Git status/diff · built-in terminal · scroll sync ·
soft wrap · minimap · external-change detection · macOS Finder/Dock integration ·
drag & drop · settings · native dialogs · session/window restore · autosave and
crash recovery · large-file guards.

This repository is **independent from the igui engine**: it consumes the engine
through path dependencies and never modifies it. The two checkouts must be
siblings:

```text
workspace/
├── igui/
└── igui-editor/
```

```text
Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
```

## Documentation

Start here; load only the document you need.

| Document | Read when |
|---|---|
| [docs/features.md](docs/features.md) | You need user-facing feature behavior (LSP, Git, terminal, preview, settings, …). |
| [docs/keys.md](docs/keys.md) | You need shortcuts, profiles (VS Code / JetBrains / Vim) or keymap overrides. |
| [docs/design.md](docs/design.md) | You are changing architecture, layout, state, or a core design decision. |
| [docs/keymaps.md](docs/keymaps.md) | You are changing command dispatch or keymap profiles. |
| [docs/services.md](docs/services.md) | You are changing Git/PTY/LSP services or plugin boundaries. |
| [docs/verification.md](docs/verification.md) | You need the full self-check coverage and acceptance rules. |
| [docs/benchmarks.md](docs/benchmarks.md) | You are changing Markdown/preview performance. |
| [docs/code-map.md](docs/code-map.md) | You need the source layout and module ownership. |
| [docs/packaging.md](docs/packaging.md) | You are building/signing/notarizing the macOS app. |
| [docs/roadmap.md](docs/roadmap.md) | You need delivered milestones and known gaps. |
| [docs/review.md](docs/review.md) | You want the architecture/interaction review log. |
| [AGENTS.md](AGENTS.md) | You are a coding agent working in this repository. |

## Requirements

- Rust **1.85+** with Cargo.
- A sibling [`igui`](https://github.com/lazyfury/igui) checkout, because
  `Cargo.toml` uses `../igui/...` path dependencies.
- Language servers are optional and installed separately; the editor shows a
  copyable install command when one is missing: `rust-analyzer`
  (`rustup component add rust-analyzer`) and Godot 4 for GDScript (the language
  server is the one inside the running Godot editor; the app attaches to it and
  never starts a headless copy).
- macOS is the primary, manually tested platform. Linux/Windows host paths are
  compiled but not release-tested here.

## Build and run

```bash
cargo run --release                          # restore the last session, else welcome
cargo run --release -- src/app.rs            # open a file (its directory becomes the project)
cargo run --release -- src                    # open a project
cargo run --release -- --light README.md      # light palette
cargo run --release -- /path/to/godot_project # open a Godot project
```

`--dark` / `--light` force a palette; the default restores the last session.
`--selfcheck` runs the headless pipeline (development builds include it by
default):

```bash
cargo run -- --selfcheck
cargo run --release --features selfcheck -- --selfcheck
```

## Package the macOS app

```bash
packaging/package-macos.sh
open dist/igui-editor.app
```

The bundle is ad-hoc signed for local use. See
[docs/packaging.md](docs/packaging.md) for icons, bundle IDs, version/build
overrides and Developer ID/notarization.

## Verify

```bash
scripts/check.sh                            # fmt + clippy + tests + self-check
scripts/check.sh --release                  # also builds the shipped binary
scripts/check.sh --fast                     # same, via the thin-LTO `release-fast` profile
scripts/bench.sh                            # Markdown parse/layout/paint/typing benchmark
```

`cargo test` covers the pure model; `--selfcheck` drives the real
update → layout → paint → record pipeline with scripted dialogs and a temporary
session. No screenshots or OS capture are used. Full coverage is listed in
[docs/verification.md](docs/verification.md).

## Performance & bundle size

Release bundles are tuned for distribution:

- `lto = "fat"`, one codegen unit, `panic = "abort"`, stripped symbols;
- app and `igui_*` crates at `opt-level = 3`, third-party dependencies at
  `opt-level = "s"`;
- packaging builds with `--no-default-features`, so the headless recorder,
  profiler and `igui_headless` are not linked into the app.

For local iteration use `cargo build --profile release-fast` (thin LTO, 16
codegen units, the same opt levels): an app-only relink drops from ~34 s to
~11 s. `dev` keeps line tables only and `test` no debug info, which keeps
`target/` small. See [docs/verification.md](docs/verification.md#build-iteration).

Measured on macOS arm64 for 0.1.0:

| Artifact | Before | After |
|---|---|---|
| `target/release/igui-editor` | 9.1 MB | **4.0 MB** |
| `dist/igui-editor.app` | 10 MB | **4.8 MB** |
| `dist/*.zip` | 4.1 MB | **2.5 MB** |

Frames are event-driven: an idle window presents **zero** frames, and an open
terminal no longer keeps a 60 fps loop alive (PTY output wakes the window, timed
work runs on a deadline clock). The tree's `DrawList` is reused while the UI is
unchanged. Measured on macOS arm64 with a 1193×656 window and the terminal open:
**0.0–0.1% idle CPU, 237 MB RSS** (was 13–30% / 480 MB).

Quick Open caches workspace-relative/lowercase paths once per index; Markdown
preview culls off-screen draw primitives and coalesces full relayouts for large
documents. See [docs/benchmarks.md](docs/benchmarks.md) for numbers and how to
reproduce them.

## Features at a glance

- **Editing** — tabs, multi-line undo/redo, selection/mouse selection, block
  indent, line duplicate/move/delete/comment, smart home/end, soft wrap,
  minimap, IME.
  Caret movement, Backspace/Delete and double-click word selection keep
  grapheme clusters (combining marks, emoji ZWJ/skin-tone sequences) intact.
- **Find** — find/replace with case/whole-word, match highlighting; project
  search and replace-in-files with confirmation and conflict checks.
- **Markdown** — edit/split/preview-only per document; headings, inline styles,
  nested lists/quotes, code blocks, tables with alignment, scroll sync.
- **Language** — Rust/C/C++/JS/TS/Go/GDScript/Python/JSON/TOML/Markdown
  highlighting, data-only JSON language packs, and LSP for Rust/GDScript
  (diagnostics, hover, completion, definition, symbols).
- **Navigation** — file tree, Quick Open, outline, tab drag/reorder and context
  menu, project search.
- **Native** — Git panel (status/diff), PTY terminal, native macOS
  system menu bar + custom borderless window chrome, Finder/`open -a`/Dock
  Recent/open events, native open/save dialogs.
- **Reliability** — external-change detection, autosave, crash recovery,
  session + window restore, large-file guards.
- **Look** — own square "geek" palette (dark/light), no component library,
  CJK-safe chrome, fixed-pitch editor/terminal.

Detailed behavior and shortcuts: [docs/features.md](docs/features.md) and
[docs/keys.md](docs/keys.md).

## Code layout

```text
src/model/      pure document/workspace/Markdown/Git/PTY/LSP logic (unit-tested)
src/syntax/     highlighting API + providers + JSON language packs
src/state/      shared AppState; one domain implementation per submodule
src/ui/         shell regions and overlays (paint + hit-test geometry)
src/app.rs      shell construction, input routing, igui_app glue
src/host.rs     winit/wgpu host, native dialogs, CJK-capable face
src/selfcheck.rs headless pipeline assertions
```

Module ownership and the full file map: [docs/code-map.md](docs/code-map.md).

## Scope and known gaps

Delivered through M16: shell/editing/tabs, native dialogs, find/replace,
Markdown preview, syntax/packs, Quick Open/project search, split/scroll sync,
tab drag/context menu, soft wrap, minimap, external changes, read-only Git
status/diff, PTY terminal, LSP (Rust/Godot), macOS integration, settings and
recovery. The Session 5 reduction removed image preview, Git writes/history,
file-type icons and four extra language servers.

Known gaps and next candidates are tracked in [docs/roadmap.md](docs/roadmap.md).
