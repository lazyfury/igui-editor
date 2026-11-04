# AGENTS.md

Instructions for coding agents working in `igui-editor`. The closest
`AGENTS.md` wins; explicit user instructions override this file.

This file is a **router and rule set**, not a full design document. Load only
the document named in the routing table below. Details live in [`docs/`](docs/),
whose index is [`docs/README.md`](docs/README.md).

## Project in one paragraph

A compact text/Markdown editor built on the sibling `igui` engine (`winit` +
`wgpu`). It is **not** a member of the `igui` workspace. The app owns all UI,
state, documents, preview, terminal, Git and LSP glue; the engine only supplies
input, scene/layout, text measurement, painting and the window/GPU host.

## Product priority (MUST)

This is a lightweight, focused editor — not an IDE. Every scope decision follows
this order; a higher priority wins when they conflict:

1. **Text editing + Markdown** — the highest goal. Editing, caret/selection,
   undo, find/replace, soft wrap, Markdown preview, and their performance and
   polish.
2. **Godot (GDScript)** — second goal. The editor-attached language server and
   GDScript editing behaviour, only as far as it improves editing those files.
3. **Everything else** — the minimum: simple LSP attach and syntax
   highlighting. Do not build feature depth beyond that.

Follow-on rules:

- Prefer simplifying or deleting over adding. A new surface must tie clearly to
  priority 1 or 2; otherwise decline, or implement only the LSP/highlight
  minimum.
- Do not grow general-IDE features: no plugin system, multi-root workspaces,
  debugger, task runner, AI assistant, component library, telemetry or network
  calls. No new dependency without a reason (rule 10 below).
- Lightweight is measured, not asserted: startup time, binary size and the
  Markdown budgets in [`docs/benchmarks.md`](docs/benchmarks.md) must not
  regress.
- When in doubt, cut scope: implement the smallest change that serves editing.

## Non-negotiable rules (MUST)

1. **Do not modify `../igui`.** If an engine capability is missing, build it in
   this app first; propose an upstream change separately.
2. **Run the gate before every commit:**
   ```bash
   scripts/check.sh
   ```
   For release/packaging changes also run `scripts/check.sh --release`.
3. **Add or update tests** for every behavior change. Pure logic belongs in
   `src/model/` unit tests; pipeline/UI behavior belongs in `--selfcheck`.
   Do not use screenshots or OS capture as verification.
4. **Keep the layering:**
   - `src/model/` — pure Rust, no `igui_*`, no scene, no window.
   - `src/state/` — the single shared `AppState`; one domain per child module.
   - `src/ui/` — paint + hit-test geometry only; no file IO or background I/O.
   - `src/app.rs` — shell construction, input routing, `igui_app` glue.
   - `src/host.rs` — platform plugins/dialogs/fonts.
   - `src/selfcheck.rs` — headless assertions against the real pipeline.
5. **One source of truth for commands.** Add commands to `Action` and bind them
   in `src/keymap.rs`. Menu hints are resolved from the active `Keymap`; never
   hard-code a second shortcut label table.
6. **Geometry is shared.** A popup/overlay must compute one layout function used
   by both paint and hit-testing. Record hit rectangles during paint; do not
   create a parallel layout implementation.
7. **Performance-sensitive changes need a benchmark.** Run
   `scripts/bench.sh` before/after changes touching Markdown parsing, preview
   layout/paint, editor paint, or startup. Keep the budgets in
   [`docs/benchmarks.md`](docs/benchmarks.md).
8. **Do not commit generated artifacts** (`target/`, `dist/`, build icons).
   Packaging output stays git-ignored.
9. **Keep documentation truthful.** Behavior/architecture changes update the
   relevant file under `docs/`; update the routing table here if a module or
   document moves.
10. **No new dependency without a reason.** Prefer the existing engine
    primitives and small app-local modules. Do not add a component library,
    async runtime, or plugin ABI.

## Workflow

1. **Route the task** with the table below; read the listed files first.
2. **Plan** the smallest change that preserves the layering and adds coverage.
   Check it against [Product priority](#product-priority-must); drop or shrink
   anything that does not serve text/Markdown first, Godot second.
3. **Implement**; keep edits local and avoid broad rewrites.
4. **Verify** with `scripts/check.sh`; for performance work also
   `scripts/bench.sh` and record numbers.
5. **Document** user-visible behavior and design decisions.
6. **Commit** with an imperative subject. One logical change per commit.
   Never commit a failing tree.

If a task is ambiguous, inspect the closest existing feature and follow its
pattern before inventing a new one.

## File routing

Read the minimum set; do not preload the whole tree.

| Task | Read first | Then / notes |
|---|---|---|
| Text buffer, caret, selection, undo | `src/model/doc.rs` | Tests at the bottom of the file. |
| Tabs, files, save/open, folder | `src/model/workspace.rs`, `src/state/files.rs` | `src/state/tabs.rs` for lifecycle. |
| Editor painting / gutter / IME | `src/ui/editor.rs`, `src/kit.rs` | `src/fonts.rs` for family policy. |
| Icons / header panel toggles / dock grouping | `src/ui/icon.rs`, `assets/icons/README.md`, `src/ui/menu.rs`, `src/state/dock.rs`, `src/ui/dock.rs` | Vendored Lucide SVGs; render through engine `igui_svg`, never fetch at build time. One dock header layout serves paint and hit-test. |
| Bottom dock tabs (Terminal / Problems) | `src/state/dock.rs`, `src/ui/dock.rs`, `src/ui/terminal.rs`, `src/ui/problems.rs` | Closing a dock hides it; shells keep running. |
| External file changes / file watches | `src/state/watch.rs`, `src/model/watch.rs`, `src/model/fs_tree.rs`, `src/model/lsp.rs` | Poll only: no OS watcher dependency. Refresh the tree with `FsTree::refresh` (keeps expansions), never `reload`. Report changes via `FileEvent`. |
| App shell / input precedence | `src/app.rs`, `src/input.rs` | `src/state/mod.rs` for `Action`; `src/wake.rs` for frame scheduling/wakeups. |
| Commands / shortcuts / profiles | `src/keymap.rs`, `src/state/actions.rs` | [`docs/keymaps.md`](docs/keymaps.md), [`docs/keys.md`](docs/keys.md). |
| Shared state / a domain method | `src/state/mod.rs` + matching `src/state/*.rs` | Keep the struct in `mod.rs`; put impls in the domain file. Time-based wakeups live in `src/state/schedule.rs`. |
| Markdown parsing / preview | `src/model/markdown.rs`, `src/ui/preview.rs` | [`docs/benchmarks.md`](docs/benchmarks.md); run bench. |
| Syntax highlighting | `src/syntax/`, `src/model/highlight.rs` | Language packs: `src/syntax/pack.rs`. |
| Find / replace / project search | `src/model/find.rs`, `src/ui/find_bar.rs`, `src/model/search.rs`, `src/ui/project_search.rs` | `src/model/replace.rs` for files. |
| LSP (Rust/Godot) | `src/model/lsp.rs`, `src/ui/lsp.rs` | Godot attaches to the running editor (no headless spawn); [`docs/services.md`](docs/services.md). |
| Indentation / Format Document | `src/model/doc.rs`, `src/model/godot.rs` | Per-file tabs/spaces detection, GDScript `:` newline, `editor.format`; [`docs/design.md`](docs/design.md). |
| Terminal | `src/model/pty.rs`, `src/state/terminal.rs`, `src/ui/terminal.rs` | Login shell (`-l`), UTF-8 locale and augmented PATH are enforced for child processes. |
| Git panel | `src/model/git.rs`, `src/ui/git.rs` | Service boundary in `docs/services.md`. |
| Missing tool / install hint prompt | `src/state/tooling.rs`, `src/ui/tooling.rs`, `src/services/mod.rs` | Copy-only command, never auto-install; shared by every native service. |
| Settings / theme / metrics | `src/settings.rs`, `src/theme.rs` | [`docs/design.md`](docs/design.md). |
| Dialogs / macOS integration | `src/dialogs.rs`, `src/host.rs`, `src/macos/`, `src/native.rs` | Async sheets only; native menu/window chrome route through `NativeRequest`. |
| Session / recovery / window state | `src/model/session.rs`, `src/model/recovery.rs`, `src/state/reliability.rs` | Atomic writes. |
| Headless verification | `src/selfcheck.rs` | [`docs/verification.md`](docs/verification.md). |
| Packaging / release profile | `packaging/`, `Cargo.toml` | [`docs/packaging.md`](docs/packaging.md). |
| Docs-only change | `docs/README.md` | Keep README short; update links. |

## Commands

```bash
# Format, lint, unit tests, headless pipeline
scripts/check.sh

# Also build the shipped release binary (no selfcheck feature)
scripts/check.sh --release

# Markdown parse/layout/paint/typing performance
scripts/bench.sh

# Focused debug run
cargo run -- --dark README.md
cargo test model::doc::tests -- --nocapture
cargo clippy --all-targets -- -D warnings
```

The self-check is development-only. Release bundles are built with
`--no-default-features`; the recorder/profiler must not be linked into the app.

## Coding conventions

- Rust 2021, `rustfmt` defaults. Keep public items documented when the reason is
  not obvious.
- Follow the existing immediate-mode pattern: state fields are plain values,
  painters read/write them, and input callbacks mutate them.
- Prefer `pub(crate)` over `pub` for internals. `model/` APIs stay testable.
- No `unwrap()`/`expect()` on user input, file paths, process output, or paint
  data. Tests may unwrap fixture setup.
- Errors surfaced to the user go through `state.status` / `state.error`; do not
  print from UI code.
- Keep new code allocation-conscious in paint loops; avoid per-frame `clone`
  of large vectors, `format!` in hot paths, and text measurement of off-screen
  content.
- UI text uses `kit::baseline` / `kit::line_baseline` so vertical centering stays
  consistent. Sizes come from `theme::Metrics`, not ad-hoc constants.
- Markdown preview must cull off-screen primitives and preserve the large-file
  debounce; see `src/ui/preview.rs`.

## Verification expectations

- Pure logic: `cargo test` with deterministic fixtures; no sleeps except PTY
  round-trips.
- UI/pipeline: add a `selfcheck.rs` assertion that would fail if the behavior
  regresses. Prefer visible-text/geometry assertions over internal state alone.
- Performance: report before/after numbers from `scripts/bench.sh` in the commit
  or review log when touching preview/Markdown/editor paint.
- Manual smoke test: launch a packaged or debug window when changing host,
  packaging, fonts, or terminal behavior; note it in the final response.

## Security and safety

- Git is invoked as `git` with an argument vector, never through a shell.
- The PTY supports arbitrary shell input by design; do not add implicit command
  execution outside it.
- Native dialogs are asynchronous; never run a blocking dialog inside a frame
  callback.
- Paths from LSP/Git/disk are untrusted; use `Path`/`PathBuf`, not string
  concatenation.
- Do not add telemetry or network calls. LSP endpoints are user/project
  configured.

## Documentation map (progressive disclosure)

1. Root `README.md` — 5-minute overview, quick start, verification.
2. `docs/README.md` — index and reading rules.
3. Task-specific doc from the routing table.
4. `docs/design.md` — only when changing architecture or a cross-cutting rule.
5. `docs/review.md` — historical review/optimization log; do not treat it as
   current API documentation.

When a fact is duplicated, keep the canonical copy in `docs/` and link to it.
The README must not grow back into a feature manual.
