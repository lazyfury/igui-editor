# Verification

```bash
scripts/check.sh                            # fmt + clippy + tests + self-check
scripts/check.sh --release                  # also builds the shipped binary
scripts/check.sh --fast                     # same, via the `release-fast` profile
```

The local gate is `scripts/check.sh`; run it before every commit. `--fast`
builds with [`release-fast`](#build-iteration), which keeps the shipped opt
levels but links in ~1/3 of the time; packaging always uses `--release`.
`--release` also asserts the shipped binary stays under the 6 MB budget in
[`benchmarks.md`](benchmarks.md#bundle-budget).

CI automation is **not currently checked in** — restoring the macOS gate is
deferred and tracked in [improvement-plan.md §4.5](improvement-plan.md).
When restored it must check out this repository plus the `igui` engine at tag
`v0.6.0` as siblings, install Rust 1.85 with rustfmt/clippy and run the same
gate on `macos-latest`.

Or individually:

```bash
cargo test                                  # model: caret, undo, find, markdown, session, FS tree
cargo run -- --selfcheck                    # headless DrawList assertions
cargo run --release --features selfcheck -- --selfcheck  # same, optimized
scripts/bench.sh                            # Markdown pipeline performance
```

`--selfcheck` drives the real app (update -> layout -> paint -> record) into
`igui_backend_recording`, with **scripted dialogs** and a **temporary session
file**, then asserts:

1. `igui_profile::inspect` finds no `Error`-level structural problems,
2. the opened file's text, line numbers, status facts and (non-macOS) menu
   labels are in   the recorded draw list, and the editor emits a clip,
3. typing paints and mutates the model, `⌘S` round-trips through disk,
   `⌘Z` / `⇧⌘Z` undo/redo, the window title tracks the dirty flag,
4. `File > Save` works through the in-app drop-down on non-macOS (macOS runs
   the same command directly because the menu is native),
5. `⌘F` → typing finds and selects a match (`1 / 1` in the bar), Replace All
   rewrites the document, `Esc` returns typing to the editor,
6. `⇧⌘S` and `⌘O` go through the dialog layer and land on disk / in a tab,
   Markdown opens in Split with the rendered heading, list and code label,
7. the Markdown table renders as cells (header baseline, column order,
   right-aligned column, body below the header), the preview wheel scrolls in
   the right direction, and `⇧⌘M` cycles the pane structure (preview-only has
   no editor caret and no raw pipe text),
8. `⌘W` on a dirty tab shows the modal; `Esc` keeps the tab, `Discard` closes
   it, `Save` writes and closes it,
9. a sidebar click opens a file **and returns keyboard focus**; drag selection
   and wheel scrolling work,
10. the footer `DARK`/`LIGHT` chip and `⇧⌘L` both repaint with the other
    palette, `⇧⌘O` retargets the sidebar root, and a fresh app restores the
    theme, the open tabs and current tab, the recent projects/files lists and every editor
    setting from the session file,
11. an idle app with a terminal panel open requests **no** further frames
    (`needs_frame` is false; async workers are given a bounded settle window),
    and a second unchanged frame reuses the cached tree `DrawList`.

M3 checks: syntax token colors (keyword/function/string/comment), incremental
re-highlight after an edit, indent guides on/off, caret bracket matching, the
settings panel (font, tab width, gutter, guides, syntax, sidebar width), sidebar
drag-resize, the large-preview truncation banner, the >32 MB open refusal, tab
overflow scrolling, and settings restored from the session.

M5 checks: a temporary JSON language pack highlights a `.lua` fixture; the
footer language type (bottom-right) opens a popup above the status bar, and
lua → rust → Auto changes token colors and the footer label; overrides survive a
session restore; File → Close Project clears the tabs, terminal and persisted
project; switching projects resets terminal/LSP/outline and closes the old tabs;
replace-in-files (cancel is a no-op, apply rewrites a non-open file, an
open tab is skipped); and scroll sync moves the preview from the editor and the
editor back from the preview (including the disable toggle).

M7 checks: native request queue opens a file and triggers New File; quick-create
creates a real file and refuses to overwrite; `application:openURLs:` is verified
against cold and running `open -a` launches; Dock menu/quit methods are installed
on the existing winit delegate.

LSP checks: a scripted stdio language server is launched through the real
client; diagnostics appear under the changed line and in the footer, the footer
chip opens the bottom dock's PROBLEMS tab (workspace rows, click jumps and keeps
the panel open), `F1` paints
a hover card, `⌘I`/`Ctrl+I` opens a completion list and inserts the selected
item, and `F12` jumps to the returned definition. A second scripted server
checks a `Cargo.lock` monorepo: one `initialize`, two `didOpen`s with the `rust`
language id, both modules' diagnostics alive at once, the footer dot stopping
every child (`LSP OFF`, diagnostics cleared) and the settings row restarting it.
A file created/deleted behind the editor's back (a Godot `.gd.uid`) checks the
external watch: it appears/disappears in the Explorer with folders still
expanded, and the scripted server receives `workspace/didChangeWatchedFiles`
type 1/3; saving an open document sends `textDocument/didSave`. A scripted
running Godot editor checks the rescan path instead: a deleted file's
diagnostics leave PROBLEMS at once, the ignored `didChangeWatchedFiles`
notification is never sent, and the open document is re-sent as a full-text
`didChange` once the configured rescan delay has passed. A GDScript buffer checks that `Enter` after `func _ready():`
adds a tab and that Format Document converts it to the configured four spaces.

A missing clangd executable checks the copy-only tooling prompt: install hint,
painted title/button, COPY confirmation, RETRY re-arming the start, and `Esc`
dismissal.

Typing checks (`check_typing`): an opener pairs with the caret inside and its
closer steps over, Backspace between the halves deletes both, Enter between an
empty pair opens an indented block, a selection is wrapped and stays selected,
a Markdown bullet continues on Enter and an empty item ends the list, committed
IME text is inserted verbatim (never paired), Select Next Occurrence selects the
word then the next occurrence (both painted), typing edits every site and Undo
restores the text, `Esc` collapses the extra carets, the Auto Pairs setting
turns pairing off again, and `⌘S` writes a tidied file (trailing whitespace
removed, final newline added).

M6 checks: tab drag-reorder and the right-click Close menu, Explorer context
menu actions for files/folders/blank space (new file, new folder, rename,
delete), Enter-to-rename in the focused Explorer, soft-wrap layout
(long-line split, wrapped click / Up / Down, `⌥Z`), clean/dirty external reload
with the Keep/Reload modal, and the session soft-wrap setting persisted.

M4 checks: welcome/no-project state and empty sidebar, drop overlay, dropping a
file vs a project, CLI folder argument, Quick Open (filter → open), project
search (debounced worker → result → jump selects the match), split-divider drag
+ double-click reset + persisted ratio, autosave write, recovery snapshot
creation/removal, the startup restore/discard prompt, window geometry
persistence, and project/split/window restored from the session.

Unit tests additionally cover the frame wake path: PTY output invokes the wake
callback, `FrameClock` fires once per deadline, and `AppState::next_wake` picks
the earliest of the status/hover/autocomplete/preview/poll deadlines.

The quality pass adds tests for: keymap unbind not being resurrected by a
fallback table, literal Ctrl chords on both platform paths, movement actions,
native child cleanup on LSP shutdown,
terminal session tabs/close, shell tabs surviving a dock tab switch / dock close
and scrollback clamping, dock tab switching and session-tab hit geometry,
CRLF save round-trips, undo/redo save-point cleanliness, divergent save-point
invalidation, batched replace (one undo step), bounded find matches, terminal
focus keeping clipboard/history out of the editor document, sidebar row-cache
invalidation and external-URL scheme rejection.

## Build iteration

Profile choice (see `Cargo.toml`):

| Profile | What it is for | App-only edit |
|---|---|---|
| `dev` | editing, `cargo run`, self-check; incremental, line tables only | ~1.5 s |
| `release-fast` | release-only problems, manual testing of the optimized build; thin LTO, 16 codegen units | ~11 s |
| `release` | shipped bundles and `scripts/bench.sh`; fat LTO, 1 codegen unit | ~34 s |

```bash
cargo build --profile release-fast --no-default-features   # fast optimized build
cargo run --profile release-fast -- --dark README.md
```

`[profile.dev] debug = "line-tables-only"` and `[profile.test] debug = 0` keep
`target/` small; test panics still print their source location, backtraces under
`RUST_BACKTRACE=1` only lose line numbers. Delete `target/debug/incremental`
when it grows (it is pure cache) — nothing else in `target/` is stale.

Optional compile cache: `sccache` only helps when dependency artifacts are
lost (`cargo clean`, toolchain switch, a fresh checkout) and does not cache the
app's incremental units, so it is **not** enabled in `scripts/check.sh`:

```bash
export RUSTC_WRAPPER=sccache           # or: RUSTC_WRAPPER=sccache scripts/check.sh
sccache --show-stats                   # hits, misses, cache size
```

Measured here: `cargo clean` → rebuild 24 s → 17 s with a warm cache; the hot
loop pays ~1 s extra, which is why the wrapper stays opt-in.

No OS capture of any kind is used. Native AppKit menus and the borderless
window chrome are a manual smoke-test item; the headless check covers the
custom header hit rectangles, hover feedback and double-click routing but cannot
drive macOS's menu bar, window drag or native zoom. Performance methodology and
budgets are in [benchmarks.md](benchmarks.md).
