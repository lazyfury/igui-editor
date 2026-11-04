# Architecture & interaction review

Status: review of `b1b65a5`; improvement cycle 1 applied.
Baseline: `cargo test` 164 passed, `cargo run -- --selfcheck` ok,
`cargo clippy --all-targets` clean (only the upstream `block v0.1.6`
future-incompat notice).

## What already works well

- The layering is explicit and enforced: `model/` is pure Rust,
  `ui/` only paints, `state/` is the shared value, and `selfcheck.rs`
  verifies the real pipeline without screenshots.
- Overlay geometry is computed once and shared by paint and hit-testing
  (menu, tab/tree context menus, modal, quick open). This avoids the usual
  immediate-mode drift between drawing and clicking.
- Pure logic has real unit tests (document, find, markdown, git porcelain,
  session, wrapping, keymap resolution).
- Native concerns are isolated behind small traits (`Dialogs`) and platform
  modules (`host`, `macos`, `services`).
- The app is deliberately dependency-light: no component library, no
  dynamic plugin ABI, and no hidden state outside `AppState`.

## Findings

| # | Priority | Finding | Evidence | Direction |
|---|---|---|---|---|
| 1 | P1 | `state.rs` is a 4.8k-line god object mixing terminal, git, LSP, find, tabs, settings, files, and action dispatch. | `src/state.rs` | Split the `impl AppState` blocks into domain submodules (`state/terminal.rs`, `state/git.rs`, ...) while keeping one `AppState` value. |
| 2 | P1 | Shortcut labels are duplicated. `Action::shortcut` hand-codes labels while `keymap.rs` owns the real bindings, so the menu can advertise a key that the profile does not use. | `state.rs::Action::shortcut`, `keymap.rs` | Resolve the label from the active `Keymap`; keep a fallback only for actions without a binding. |
| 3 | P1 | Keyboard-only navigation is incomplete: an open menu cannot be traversed with arrows/Enter, and there is no tab switching (`⌘1…9`, `Ctrl+Tab`). | `app.rs` key routing, `keymap.rs` tables | Add menu navigation and tab commands through the existing command/keymap layer. |
| 4 | P2 | Tab activation is duplicated between `ui/tabs.rs`, the session restore, and other call sites. | `ui/tabs.rs::click`, `app.rs::workspace_for` | One `AppState::activate_tab` used by all of them. |
| 5 | P2 | `selfcheck.rs` is 4.5k lines with one `run()`; failures are hard to locate and the file is slow to navigate. | `src/selfcheck.rs` | Split checks into `selfcheck/{main,workspace,lsp,keymap}.rs` (next cycle). |
| 6 | P2 | Overlay modules repeat the same "layout + hit + key" skeleton. It is correct but not reusable. | `ui/menu.rs`, `ui/tab_menu.rs`, `ui/tree_menu.rs`, `ui/modal.rs` | Extract a small `Overlay`/`PopupList` helper only after a third real caller needs it (avoid premature abstraction). |
| 7 | P3 | `unwrap`/`expect` count is 184, mostly in tests; UI paths should stay panic-free. | `grep` | Add a lint-friendly pass later; do not mass-rewrite blindly. |
| 8 | P3 | Verification is manual (two commands) and there is no CI/check script. | repo root | Add `scripts/check.sh` as the single local gate. |

## Improvement cycle 1 (this change)

Scope is deliberately small and independently verifiable.

1. **Modularize `AppState`** — keep the struct and constructors in
   `state/mod.rs`; move domain methods into child modules. No behaviour change.
2. **Keymap as the single source of shortcut labels** — menu hints and the
   README follow the active profile; actions without a binding show no hint.
3. **Keyboard interaction** — open menus with `F10`, traverse with
   `↑`/`↓`/`Home`/`End`, switch menus with `←`/`→`, run with `Enter`/`Space`;
   add `⌘1…8` (index), `⌘9` (last), `Ctrl+Tab`/`Ctrl+Shift+Tab` (or
   `Ctrl+Tab` on Linux/Windows) for tabs.
4. **Single tab activation path** — `ui/tabs.rs` and keyboard commands call
   `AppState::activate_tab`.
5. **Continuous verification** — `scripts/check.sh` runs fmt, clippy, tests,
   and the headless self-check.

## Acceptance checklist

- [x] `cargo test` passes (173 tests, including menu navigation, tab chords, keymap labels).
- [x] `cargo run -- --selfcheck` passes, including new menu/tab assertions.
- [x] `cargo clippy --all-targets` has no app warnings.
- [x] Headless acceptance covers `F10` + arrows + Enter, `⌘1`/`⌘2`/`⌘9`,
      `Ctrl+Tab` / `Ctrl+Shift+Tab`, and menu hints matching the profile.
- [x] Session restore, sidebar, find, preview and terminal still behave as
      before (self-check covers the main flow).
- [x] Startup smoke test: the binary opens `README.md` and stays up (no stderr).
- [ ] Manual visual pass on a macOS window (menu hint spacing, arrow highlight)
      before release; not available in the headless test environment.

## Improvement cycle 2: performance & bundle

Findings and fixes:

| Priority | Finding | Fix |
|---|---|---|
| P1 | Release used the default profile: no LTO, symbols retained, and the headless recorder/profiler were linked into every shipped binary. | Fat LTO, one codegen unit, `panic = "abort"`, stripped symbols; `selfcheck` became a feature and packaging builds `--no-default-features`. |
| P1 | Quick Open rebuilt workspace-relative and lowercase path strings for every file on every keystroke (20k files → ~60k allocations per keypress). | `FileIndex` caches `displays` and `lower`; query scores borrowed data and clones only the capped results. |
| P2 | Dependency code was optimized for speed even where bundle size dominates. | `opt-level = "s"` for third-party packages; app and `igui_*` stay at `opt-level = 3`. |

Acceptance:

- [x] `scripts/check.sh` passes (fmt, clippy, 175 tests, selfcheck).
- [x] `cargo build --release --no-default-features` is warning-free.
- [x] `target/release/igui-editor`: 9.1 MB → 4.0 MB; `.app` 10 MB → 5.0 MB;
      `.zip` 4.1 MB → 2.9 MB.
- [x] Quick Open 20k-file query benchmark: ~1.4× faster, no per-file path
      clones while scoring.
- [x] Packaged `.app` launches and `--version` / `--help` work; the release
      binary still opens the GUI (startup smoke test).

## Terminal locale fix (reported after packaging)

Root cause: a macOS `.app` launched from Finder has no `LANG`/`LC_*`. The PTY
child started in the `C` locale, so zsh rendered a Chinese project folder as
`\M-^V`-style meta escapes inside the prompt. The OSC title strings were still
consumed by `vt100`, but the prompt text was visibly garbled.

Fix:

- `PtySession::spawn` now calls `ensure_utf8_locale`: it removes an invalid
  `LC_ALL`, and sets `LC_CTYPE` / `LANG` to `en_US.UTF-8` (macOS) or `C.UTF-8`
  (elsewhere) only when the effective locale is missing or not UTF-8.
- Valid user locales (for example `zh_CN.UTF-8`) are preserved.
- Regression tests cover the locale policy and spawn a shell in a Chinese-named
  directory, asserting that the parsed grid contains neither `\M-` meta
  escapes nor OSC title fragments.

Verified with `LANG=C LC_ALL=C LC_CTYPE=C cargo test model::pty` and with a
rebuilt/shaded `.app` bundle.

## Improvement cycle 3: frame scheduling, paint reuse, font fallback

Measured on macOS arm64 (1193×656, terminal panel open, maximized session):

| Metric | Before | After |
|---|---|---|
| Idle CPU | 13–30% (vsync-locked 60 fps) | **0.0–0.1%** |
| Incremental frames | none | **none** (0 fps) |
| RSS | 480 MB | **237 MB** |
| Physical footprint | 565 MB | **315 MB** |
| Preview paint (bench) | 0.04 ms/iter | **0.01 ms/iter** |
| Typing (bench) | 1.17 ms/frame | **1.06 ms/frame** |

Fixes:

- `needs_frame` no longer treats `terminal.open` (or a status message / hover
  dwell / preview debounce) as "keep drawing": those are deadlines now.
- `wake::FrameClock` (a Condvar thread) asks the window for one frame at the
  next deadline; `PtySession`'s reader thread wakes the same handle on output.
- `igui_ui::paint_cached` reuses the tree `DrawList`; `AppState::dirty` is
  consumed at paint entry so a paint-time change still schedules one more frame.
- `sync_scroll_positions`, `tick_reliability`, the native Dock/menu bridge and
  `sync_title` skip their per-frame work when nothing changed.
- Preview token measurement no longer allocates a `String` per character.
- The mono family is Menlo-first: it covers the command prompt's `➜`/`✗`
  glyphs, which otherwise made the font server load the ~180 MB emoji fallback
  face (the main RSS contributor while the terminal was open).
- A dead git worker is detected and its busy flags cleared instead of polling
  forever; the self-check asserts the loop settles and the draw list is reused.

`scripts/check.sh` (fmt, clippy, 191 unit tests, selfcheck) passes. The
self-check now covers idle-frame settling and `paint_cached` reuse.

## Next candidates

- Split `selfcheck.rs` by feature; report the failing check by name.
- Extract a shared popup-list helper from the menu and context menus.
- Add sidebar keyboard navigation (`⌘⇧E` focus, arrows, Enter) with a visible
  focus ring; `Focus` is currently editor/find/replace/terminal only.
- Native macOS main menu (Window/Help) so commands are discoverable outside the
  in-app menu bar.
- Command palette backed by the same `Action` list (Quick Open currently only
  lists files).
