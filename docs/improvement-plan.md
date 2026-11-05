# Improvement plan — Session 4

Status: **ready to start**. Baseline verified on the current tree:

- `scripts/check.sh` green: fmt, clippy, **356 unit tests + 2 doc tests**,
  `--selfcheck` (main flow, typing, LSP attach/rescan, tooling, keymap).
- No uncommitted changes; the findings below were confirmed against the code.

This plan turns the [Session 4 review](#findings-register) into small,
independently verifiable slices. It follows the product priority in
[`AGENTS.md`](../AGENTS.md): **text editing + Markdown first, Godot second**,
everything else at the LSP/highlight minimum. History lives in
[`improvement-log.md`](improvement-log.md); the review narrative lives in
[`review.md`](review.md).

## Guardrails

- Do not modify `../igui`. Build engine gaps locally first.
- Keep `model/` pure Rust, `state/` one `AppState` (one domain per child module),
  `ui/` paint + hit-test only, `app.rs` shell + routing.
- One source of truth for commands; no second shortcut/label table.
- Every behavior change gets a unit test or a `--selfcheck` assertion.
- Run `scripts/check.sh` before every commit. Run `scripts/bench.sh` before/after
  changes touching Markdown parse/layout/paint or editor paint.
- No new dependency without a written reason. No generated artifacts.
- Documentation is part of the change, not a follow-up.

## Findings register

| ID | Priority | Finding | Evidence |
|---|---|---|---|
| F1 | P1 | Restoring a session keeps at most **8 tabs** (`MAX_RECENT` reused for tabs) | `model/session.rs:20,265,334,372` |
| F2 | P1 | Quick Open **rebuilds the whole index synchronously on every `⌘P`** | `state/quick_open.rs:24`; `model/workspace_index.rs:50,66` |
| F3 | P1 | Autosave applies `tidy()` (rewrites trailing whitespace), pushes an undo step when it changes the buffer, and ignores write errors | `model/doc.rs:696,714`; `state/reliability.rs:49` |
| F4 | P2 | Explorer has no keyboard navigation; any unhandled key moves focus back to the editor | `app.rs:1081-1093` |
| F5 | P2 | Non-macOS menus are mouse-only; F10 navigation was removed but docs still claim it | commit `9776070`; `docs/review.md:44,58` |
| F6 | P2 | Per-document maps (`doc_views`, `external`) are not pruned when a tab closes | `state/tabs.rs:250`; `state/workspace.rs:63` |
| F7 | P2 | Command metadata is spread over 6 sites; ids are a separate inconsistent table | `state/mod.rs:171,240,314,383,401`; `keymap.rs:568` |
| F8 | P2 | Adding a setting touches 10+ places; session version is duplicated | `settings.rs`; `model/session.rs:103,272`; `state/settings.rs:440` |
| F9 | P2 | `AppState` exposes 107 `pub` fields (rule asks for `pub(crate)`); `new` is ~270 lines | `state/mod.rs:1212-1382` |
| F10 | P2 | CI is documented but `.github/workflows/check.yml` was deleted (`37873b1`) | `docs/verification.md:8` |
| F11 | P3 | Megafiles: `selfcheck.rs` 7.3k, `model/lsp.rs` 4.7k, `state/mod.rs` 2.3k, `app.rs` 2k | line counts |
| F12 | P3 | Markdown extension list duplicated in 3 places, with different sets | `state/mod.rs:151`; `model/doc.rs:758`; `syntax/markdown.rs:20`; `model/fileicon.rs:198` |
| F13 | P3 | Outline is LSP-only, so Markdown documents have no outline | `ui/outline.rs:1`; `model/markdown.rs:69,313` |
| F14 | P3 | Preview links are inert and images render as `▢ alt` only | `ui/preview.rs:9` |
| F15 | P3 | No Go to Line; menus are the only path to some commands off macOS | no command/overlay |

---

## Phase 0 — documentation truth (target: 0.5 day)

Cheap, and it unblocks trust in every later doc update.

- [x] **0.1** Correct stale facts:
  - `docs/design.md:189` (session v13 → current), `:569` (M8/M9 "first slice"),
    `:576` (M11 "in progress"), `:593` ("VCS integration: not planned").
  - `docs/services.md:127,145,173` (Godot headless spawn) and
    `docs/code-map.md:29`.
  - `docs/features.md:212` (headless start), `README.md:180` (M13).
  - `src/main.rs:43` help text ("refuses close" → modal) and
    `src/ui/preview.rs:9` ("tables not rendered").
  - `docs/keys.md`: add Select Next Occurrence (`⌘D` / `Alt+J`).
- [x] **0.2** CI decision: shipped as **docs-only**. The CI claim was removed
  from `docs/verification.md`; the deleted `.github/workflows/check.yml` is
  **not** restored in this task. Restoring the macOS gate and adding a Linux
  compile + `cargo test` job is deferred to **4.5**; no workflow file is
  checked in.
- [x] **0.3** Add `docs/improvement-log.md` to the index and link it from
  [docs/README.md](README.md).

Acceptance:

- `rg -n "headless|version 13|through M13|in progress|F10" docs README.md`
  surfaces no stale claim about current behavior.
- `scripts/check.sh` passes.

---

## Phase 1 — trust and latency (target: 2–3 days)

### [x] 1.1 Restore every tab (F1)

Files: `src/model/session.rs`, `src/state/files.rs`, tests.

- Decouple tabs from `MAX_RECENT`. Keep `MAX_RECENT` for the two recent lists;
  give tabs their own bound (recommended: no truncation, or a documented
  `MAX_TABS` that no UI can reach).
- Preserve tab order and `active_tab` after restore; keep the v14/v1 parsing
  rules untouched.
- Remove the three `take(MAX_RECENT)` sites for `tabs` (`parse`, `render`,
  `set_tabs`).

Tests:

- `Session::set_tabs` with 9 paths keeps 9 and round-trips `render`/`parse`.
- Active tab at index 8 survives.
- Recent lists still cap at 8.

Acceptance: `scripts/check.sh`; new session test fails before the fix.

### [x] 1.2 Quick Open without a UI stall (F2)

Files: `src/state/quick_open.rs`, `src/model/workspace_index.rs`,
`src/state/mod.rs`, `src/app.rs`, `src/ui/quick_open.rs`,
`src/state/search.rs` as the worker pattern, `src/state/watch.rs` for
invalidation.

- [x] **Step A — landed.** `open_quick_open` reuses `self.quick_open.index`
  when it belongs to the current root; only a missing or foreign index is
  rebuilt. The poll still sets `index = None` on structural changes. The walk
  uses `entry.file_type()` for the normal dir/file decision (no stat per entry)
  and keeps the `entry.metadata()` fallback for symlinks. State tests cover
  reuse, a missing index and a changed root; the walk tests still cover ignored
  and hidden entries.
- [x] **Step B — landed.** `IndexWorker` in `model/workspace_index.rs` builds
  the `FileIndex` on one `std::thread` and sends it over `mpsc`; dropping the
  worker drops the receiver and the thread exits when its send fails. The
  palette paints immediately with an `indexing project files…` row and a
  `quick_open_busy()` state that keeps `needs_frame` polling. `quick_open_tick`
  installs the result only when its root still matches, re-ranks the current
  query and also caches a result that lands after the palette closed. Project
  switches and structural changes drop the worker and the index; reopening with
  a completed index for the same root starts no worker. `refresh_quick_open`
  never builds.

Acceptance:

- `⌘P` on a 20k-file fixture paints its first frame in the same tick; no
  decode/scan happens on the UI thread.
- Reopening Quick Open without filesystem changes does not walk the tree.
- Existing Quick Open selfcheck path stays green.

### [x] 1.3 Autosave must not surprise the user (F3)

Files: `src/model/doc.rs`, `src/state/reliability.rs`, `src/state/files.rs`,
`src/selfcheck.rs`.

- Split the write paths:
  - explicit Save / Save As: `tidy()` then write (current behavior);
  - autosave: write the buffer **verbatim** (no undo entry, no whitespace
    rewrite).
- Optionally add a **Trim on Save** setting (default on) instead of hard-coding
  `tidy`; Markdown keeps its hard-break exemption either way.
- `flush_reliability` must surface failures: keep the buffer dirty, call
  `self.error(...)`, and do not report `autosaved N file(s)` for a failed write.
- Recovery snapshots and the 2 s idle deadline stay unchanged.

Tests:

- Autosave preserves trailing spaces / a missing final newline and does not add
  an undo step.
- Explicit save still tidies and remains one undo step.
- A failing autosave (read-only file or bad permission) leaves the tab dirty and
  shows an error.

Acceptance: `check.sh`; the selfcheck "tidy save" assertion is updated to cover
the explicit path only.

---

## Phase 2 — command metadata and state hygiene (target: 3–4 days)

### 2.1 One command table (F7, prerequisite for 3.6)

Status: **2.1a landed; 2.1b metadata/palette pending.**

2.1a (done): `src/state/commands.rs` holds the canonical id table consumed by
`Action::id()`/`Action::from_id()`; `keymap::apply_overrides` resolves through
it. Every id the old `keymap::action_from_id` accepted still maps to the same
`Action`, including the `view.terminal` → `ToggleTerminal` and `view.outline` →
`ToggleOutline` collisions. The header-panel actions gained distinct
`panel.terminal`/`panel.outline` ids. No binding, label, enabled predicate or
dispatch changed.

2.1b (pending): add label, menu category and enabled predicate columns; derive
menu labels/hints and the palette from the table. The palette itself is 3.6.

The original scope follows.

Files: `src/state/mod.rs` (extract), `src/state/commands.rs` (new),
`src/keymap.rs`, `src/ui/menu.rs`.

- Add a `CommandSpec` table keyed by `Action` (or an `Action::id()` plus a
  metadata table) carrying: stable id, label, menu category, and enabled
  predicate. Keep `run()`'s match as the dispatch implementation.
- Invert `keymap::action_from_id` from the same table; accept both the current
  ids (`file.save`, `edit.selectNextOccurrence`, …) and the short aliases.
- Derive menu labels/hints and the future palette from the table. Do not add a
  second label or id list.
- `enabled`/`checked` can stay as matches initially; the table is metadata, not
  a behavior rewrite.

Tests:

- Every `Action` that appears in a menu or a keymap profile has an id.
- Every id parses back to the same action; ids are unique.
- Existing keymap override tests (unbind, `Ctrl+Space`, `Ctrl+\``) stay green.

Acceptance: no behavior change; `check.sh` and selfcheck menu hints pass.

### [x] 2.2 Encapsulate `AppState` (F9)

Files: `src/state/mod.rs`, `src/model/*`, `src/services/mod.rs`, `src/state/tooling.rs`.

- Done: all 107 `pub` fields of `AppState` are `pub(crate)`. Names, types,
  ordering, `AppState::new` and every method (including
  `EditorApp::state`/`state_mut`) are unchanged, so behavior is identical.
  Child state modules already read the fields through `super::*`; `ui/` and
  `app.rs` are in the same crate.
- Narrowing removed the `pub`-field reachability chain that had been masking
  dead code, exposing 11 unused items. A follow-up cleanup deleted the
  genuinely dead data: the `PtySession::cwd_hint` stub,
  `GitCommitDetail::truncated`, `TreeMenuUi::name`, `ReplaceJob::files`/
  `occurrences` and `ToolingPrompt::source`. The service manifest/command
  metadata is the documented extension-host shape, so it stays and is now
  exercised by a unit test; only the three test-only helpers keep a
  `cfg_attr(not(test), expect(dead_code, …))` annotation.
- Follow-up (after 2.1): split the `Action`/menu tables and the UI state
  structs (`SidebarUi`, `EditorUi`, `QuickOpenState`, …) out of `state/mod.rs`
  and narrow their fields the same way. Do not group fields yet; that is a
  later, behavior-neutral refactor.

Tests: none needed — no test changes were required.

Acceptance: compiles with `pub(crate)` fields only and the gate is green (371
unit + 2 doc tests, selfcheck). The `state/mod.rs` size target (~1,500 lines)
waits for the split follow-up.

### [x] 2.3 Prune per-document state (F6)

Files: `src/state/tabs.rs`, `src/state/workspace.rs`.

- Add `forget_document(id, path)` and call it from `close_tab_now`. Prune:
  `doc_views`, `doc_revs`, `language_by_id`, `external`, `recovery`. Keep
  `language_by_path`: it is a persisted per-path user override.
- `reset_project_state` uses the same helper instead of duplicating the
  removals; `delete_path_now` also drops `external` stamps for the path (and
  everything below a deleted folder). Rename already moves the stamp old→new.
- Add a test that closing tabs returns every map to its pre-open length and
  that reopening gets fresh `DocUi` defaults.

Acceptance: no stale entries after close; `check.sh`.

---

## Phase 3 — text/Markdown UX (target: 5–8 days)

Ordered by product priority. Land 3.1/3.2/3.4 first; 3.6 depends on 2.1.

### [x] 3.1 Markdown heading outline (F13)

Files: `src/model/markdown.rs`, `src/state/outline.rs` (or `state/lsp.rs`),
`src/ui/outline.rs`.

- When the active document is Markdown and there is no LSP outline, derive rows
  from `BlockKind::Heading { level }` with their `start_line`.
- Clicking a row moves the caret to that heading and reveals it.
- Keep the LSP outline for code files; do not change the `OutlineUi` layout.

Tests/selfcheck: open a Markdown fixture, assert headings appear at the right
depth, click one, assert the caret line.

### [x] 3.2 Go to Line (F15)

Files: `src/keymap.rs`, `src/state/actions.rs`, `src/state/goto_line.rs` +
`src/ui/goto_line.rs` (quick-create prompt pattern).

- `⌃G` / `⌘L` opens a centered one-field prompt prefilled with the caret line
  and selected; `Enter` jumps to a validated 1-based line, `Esc` closes.
- `1..=line_count` is validated with an inline error that clears on edit; text,
  paste and IME commits filter to ASCII digits. Image preview tabs report a
  status instead of opening.
- Command id `edit.gotoLine`; the Edit menu lists it after Find; the shortcut
  lives in the shared `add_movement` bindings so every profile gets it.
- Selfcheck drives the host shortcut (`Ctrl+G` on macOS, `Ctrl+L` elsewhere),
  the prefill, a valid jump and Esc; state and keymap unit tests cover the rest.

### 3.3 Preview links (F14; images stay `▢ alt`)

Files: `src/model/markdown.rs`, `src/ui/preview.rs`, `src/state/lsp.rs`
(scheme allowlist).

- Record link hit rectangles in the preview layout (same geometry for paint and
  click; no second layout).
- External links: reuse the existing web/mail scheme allowlist; refuse
  `file:`/`javascript:`.
- Relative `.md` links open in a tab; local images stay `▢ alt` (the image
  preview was removed in Session 5).
- Cull off-screen link/image primitives; keep the large-file debounce.

Acceptance: `check.sh`; selfcheck click on a link opens the target; `bench.sh`
parse/layout/paint within the 20% rule.

### 3.4 Explorer keyboard navigation (F4)

Files: `src/app.rs` (key routing), `src/ui/sidebar.rs` (focus ring + reveal),
`src/state/keymap.rs` (focus shortcut), `src/state/sidebar.rs` if needed.

- With `Focus::Explorer`: `↑`/`↓` move the selected row, `←`/`→` collapse/expand
  a folder, `Home`/`End` jump, `Enter` opens (file) or toggles (folder), `Esc`
  returns to the editor.
- Add `⌘⇧E` to focus the Explorer (`view.explorerFocus`) and paint a visible
  focus ring; keep the existing row hit testing.
- Stop the current "any key returns focus to the editor" fallback except for
  text input and deliberate commands.

Selfcheck: keyboard-only open of a nested file from the Explorer.

### 3.5 Reopen closed tab and tab overflow affordance

Files: `src/state/tabs.rs`, `src/keymap.rs`, `src/ui/tabs.rs`.

- Keep a bounded closed-tab stack (path + position), restore it with `⇧⌘T`
  (`tabs.reopenClosed`); do not restore dirty content (saved paths only).
- At the 96 px minimum tab width, paint a subtle edge fade or an "all tabs"
  affordance; no scrollbar, matching the current spec. A dropdown is allowed if
  it reuses the existing popup list pattern.
- Tests: reopen after close, stack cap, restore keeps order.

### 3.6 Command palette (F5, F15; depends on 2.1)

Files: new `src/ui/palette.rs`, `src/state/palette.rs`, `src/keymap.rs`.

- `⇧⌘P` opens a centered fuzzy list over the command table: label, id, active
  keymap hint, disabled state.
- `↑↓`/`Enter`/`Esc`, mouse click, live filtering; reuse the Quick Open
  overlay geometry rather than a third skeleton.
- This is also the non-macOS keyboard path for menu-only commands
  (Close Project, Clear Recent, …). Keep F10 menu restoration optional.

Acceptance: keyboard-only user can run Save, Close Project and Toggle Theme via
the palette; selfcheck covers open/filter/run/escape.

---

## Phase 4 — scale-out (deferred until Phases 0–3 are green)

- **4.1 Settings/session single source (F8):** one persisted settings record or
  a key table, one `SESSION_VERSION` constant, a migration chain
  (`migrate_vN`), and golden fixtures for v1..current. Do not start before the
  behavior phases, since it touches every setting.
- **4.2 Split megafiles (F11):** `selfcheck` by feature with a named failing
  check; `model/lsp.rs` into transport/session/controller/parsers;
  `app.rs` into shell/event/frame. Add a soft 1,500-line budget to
  `AGENTS.md` when the split lands.
- **4.3 `ui::list` extraction (F11 adjacent):** one row/scroll/hit helper for
  `quick_open`, `project_search`, `problems`, `outline`, `references`,
  `settings`, `git`, `recovery`, `tooling`. Only after Phase 2 reduces the
  surface.
- **4.4 Markdown extension single source (F12):** `model::markdown::is_markdown_path`
  used by `ViewMode`, `tidy`, syntax and the file-icon table.
- **4.5 CI:** restore the macOS gate and add a Linux compile + test job (no
  workflow is checked in today); keep the pinned engine revision in one place
  and document how to update it.

## Explicitly deferred / not planned

- New broad surfaces (Git network operations, extension host, multi-root,
  debugger, AI, task runner, component library, telemetry).
- Rope/CRDT, syntax trees, OS filesystem watcher.
- Full multi-cursor beyond Select Next Occurrence; Vim completeness.
- `.tscn`/`.tres` language pack: revisit only if it clearly serves GDScript
  editing (priority 2) and stays a data-only pack.
- Incremental Markdown preview layout for very large documents (separate perf
  item; current debounce stays).

## Progress log

| Date | Item | Status | Notes |
|---|---|---|---|
| Session 4 | Baseline | done | `scripts/check.sh` green (356 unit + 2 doc tests, selfcheck). |
| Session 4 | 0.3 Index/archive | done | `improvement-log.md` added; plan rewritten for Session 4. |
| Session 4 | 0.1–0.2 Docs truth | done | Stale session/M13/Godot-headless/milestone claims corrected in `design.md`, `services.md`, `code-map.md`, `features.md`, `README.md`, `main.rs` help and the `preview.rs` module doc; Select Next Occurrence added to `keys.md`. 0.2 is docs-only: the CI claim is gone, workflow restoration deferred to 4.5. |
| Session 4 | 1.1 Tab restore | done | Tabs no longer capped by `MAX_RECENT` (parse/render/`set_tabs`); recents stay at 8. 2 new session tests, gate green: 358 unit + 2 doc tests, selfcheck. |
| Session 4 | 1.2 Quick Open | done | Step A: the palette reuses the index for the same root; the walk uses `file_type()` with a symlink `metadata()` fallback. Step B: `IndexWorker` builds the index on one std thread and `quick_open_tick` installs it only for the current root; the palette paints an `indexing project files…` placeholder, `needs_frame` polls while busy, structural changes and project switches drop the worker, and `refresh_quick_open` never builds. 2 new tests (worker round-trip, stale-root discard) plus the updated reuse/worker state tests; selfcheck settles the worker before ranking. Gate green: 368 unit + 2 doc tests, selfcheck. |
| Session 4 | 1.3 Autosave | done | Autosave writes verbatim through `Document::save_verbatim` (no tidy, no undo step); failures keep the tab dirty and show an error. 4 new tests (2 doc, 2 state). Gate green: 366 unit + 2 doc tests, selfcheck. |
| Session 4 | 2.1 Command table | partial | 2.1a id table landed; 2.1b metadata/palette pending. `src/state/commands.rs` is the single canonical id table; `Action::id`/`Action::from_id` read it and `keymap::apply_overrides` delegates. Old ids/aliases and the `view.terminal`/`view.outline`/`view.problems` mappings are preserved; new ids: `view.edit`/`view.split`/`view.preview`, `tabs.select1..9`, `file.clearRecent`, `language.auto`, and `panel.terminal`/`panel.outline` for the header-panel actions. 5 new tests (id uniqueness/round-trip, aliases, legacy regression list, header collisions, dynamic actions). Gate green: 376 unit + 2 doc tests, selfcheck. |
| Session 4 | 2.2 `pub(crate)` | done | All 107 `AppState` fields are `pub(crate)`; no field name/type/order, `new` or method changed, so behavior is unchanged. The narrowing removed the `pub`-field reachability chain that had masked dead code and exposed 11 unused items (model helpers, service-manifest metadata, three overlay fields); they keep their shape with a targeted `#[expect(dead_code, …)]` (three gated by `cfg_attr(not(test), …)` because only tests use them). UI state structs keep `pub` fields; that split follows 2.1. Gate green: 371 unit + 2 doc tests, selfcheck. |
| Session 4 | 2.2 cleanup | done | The dead data exposed by 2.2 was deleted: the `PtySession::cwd_hint` stub, `GitCommitDetail::truncated`, `TreeMenuUi::name`, `ReplaceJob::{files,occurrences}` and `ToolingPrompt::source`, with their parameters, locals and struct literals updated. The manifest/command metadata stays (documented extension-host shape), is re-exported from the crate root and is exercised by a new `builtin_metadata_is_well_formed` unit test. Only the three test-only helpers keep an annotation. Gate green: 377 unit + 2 doc tests, selfcheck. |
| Session 4 | 2.3 State pruning | done | `close_tab_now` calls one `forget_document` helper (`doc_views`, `doc_revs`, `language_by_id`, `recovery`, `external`); `reset_project_state` reuses it and `delete_path_now` drops path/folder stamps. `language_by_path` still outlives the tab. 3 new state tests. Gate green: 371 unit + 2 doc tests, selfcheck. |
| Session 4 | 3.1 Markdown outline | done | Without LSP symbols, Markdown files derive outline rows from a line scanner (`model::markdown::headings`) that handles ATX/setext headings, fenced/indented code and blockquote prefixes; rows are cached by `(doc id, rev)`, use `MARKDOWN_HEADING_KIND`/`H`, and click reuses `byte_for_position`. `model::markdown::is_markdown_path` is now the shared extension check for outline/view mode/tidy. 12 new tests (7 model, 3 state, 1 UI glyph, 1 truncate helper) plus a selfcheck open/paint/click flow. Gate green: 396 unit + 2 doc tests, selfcheck. Bench before → after (median of 3): parse 1.826 → 0.762 ms, layout 2.712 → 1.627 ms, paint 0.018 → 0.019 ms, typing 1.485 → 1.490 ms/frame (< 2 ms budget, no >20% regression). |
| Session 4 | 3.2 Go to Line | done | `⌃G` / `⌘L` opens a prefilled, digit-only prompt; `Enter` validates `1..=line_count`, moves to the line start, reveals it and reports `line N`; invalid input keeps an inline `line must be 1..=<count>` error that editing clears; Esc and outside clicks close without moving; image previews report a status instead. Command id `edit.gotoLine`, Edit-menu entry and shared movement binding. 7 new tests (5 state, 1 keymap, 1 command-id round-trip) plus a selfcheck flow. Gate green: 384 unit + 2 doc tests, selfcheck. |
| Session 4 | 3.3 Preview links | pending | |
| Session 4 | 3.4 Explorer keys | pending | |
| Session 4 | 3.5 Reopen/overflow | pending | |
| Session 4 | 3.6 Command palette | pending | After 2.1. |
| Session 4 | 4.x | pending | Deferred. 4.5 CI restoration is a separate decision; no workflow file is checked in. |

## First three commits (ready to start)

1. **`Restore every open tab across sessions`** — F1, one session test.
2. **`Reuse the Quick Open index and stat less`** — F2 Step A, one index test.
3. **`Write autosave verbatim and report failures`** — F3, four tests.

Phase 0 docs and Phase 1.2 Step B (background indexing) followed as their own
commits. Each commit is green on `scripts/check.sh` before the next one starts.
