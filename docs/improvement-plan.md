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
| F9 | P2 | `AppState` exposes 108 `pub` fields (rule asks for `pub(crate)`); `new` is ~270 lines | `state/mod.rs:1212-1382` |
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

### 1.2 Quick Open without a UI stall (F2)

Files: `src/state/quick_open.rs`, `src/model/workspace_index.rs`,
`src/state/search.rs` as the worker pattern, `src/state/watch.rs` for
invalidation.

- [x] **Step A — landed.** `open_quick_open` reuses `self.quick_open.index`
  when it belongs to the current root; only a missing or foreign index is
  rebuilt. The poll still sets `index = None` on structural changes. The walk
  uses `entry.file_type()` for the normal dir/file decision (no stat per entry)
  and keeps the `entry.metadata()` fallback for symlinks. State tests cover
  reuse, a missing index and a changed root; the walk tests still cover ignored
  and hidden entries.
- [ ] **Step B — pending (perceived responsiveness).**

- Build the index on a worker (mirror `model/search.rs`: `mpsc`, generation or
  `AtomicBool`, cancel on project switch). Paint the palette immediately with an
  "indexing…" row; never block the keydown frame.
- Keep `FileIndex` typed data as-is; move it into the worker and send it back.

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

### 2.2 Encapsulate `AppState` (F9)

Files: `src/state/mod.rs`, `src/ui/*`, `src/app.rs`, `src/input.rs`.

- Mechanical `pub` → `pub(crate)` for `AppState` fields. Child state modules can
  already read them through `super::*`; only UI/app siblings need `pub(crate)`.
- Split `Action`/menu tables and the UI state structs out of `state/mod.rs` once
  2.1 exists.
- Do not group fields yet; that is a later, behavior-neutral refactor.

Acceptance: compiles with `pub(crate)` only; no test changes required;
`state/mod.rs` shrinks below ~1,500 lines.

### 2.3 Prune per-document state (F6)

Files: `src/state/tabs.rs`, `src/state/workspace.rs`, `src/state/rename.rs`.

- Add `forget_document(id, path)` and call it from `close_tab_now`, rename and
  delete. Prune: `doc_views`, `doc_revs`, `language_by_id`, `external`,
  `recovery`.
- `reset_project_state` should use the same helper.
- Add a test that closing tabs returns every map to its pre-open length and
  that reopening gets fresh `DocUi` defaults.

Acceptance: no stale entries after close; `check.sh`.

---

## Phase 3 — text/Markdown UX (target: 5–8 days)

Ordered by product priority. Land 3.1/3.2/3.4 first; 3.6 depends on 2.1.

### 3.1 Markdown heading outline (F13)

Files: `src/model/markdown.rs`, `src/state/outline.rs` (or `state/lsp.rs`),
`src/ui/outline.rs`.

- When the active document is Markdown and there is no LSP outline, derive rows
  from `BlockKind::Heading { level }` with their `start_line`.
- Clicking a row moves the caret to that heading and reveals it.
- Keep the LSP outline for code files; do not change the `OutlineUi` layout.

Tests/selfcheck: open a Markdown fixture, assert headings appear at the right
depth, click one, assert the caret line.

### 3.2 Go to Line (F15)

Files: `src/keymap.rs`, `src/state/actions.rs`, new `src/state/goto_line.rs` +
`src/ui/goto_line.rs` (reuse the quick-create prompt pattern).

- `⌃G` / `⌘L` opens a centered one-field prompt; `Enter` jumps, `Esc` closes.
- Validate `1..line_count`, clear error on edit, never panic on empty input.
- Add the command id (`edit.gotoLine`) and a selfcheck assertion.

### 3.3 Preview links and local images (F14)

Files: `src/model/markdown.rs`, `src/ui/preview.rs`, `src/state/lsp.rs`
(scheme allowlist), `src/state/images.rs` if an image is reused.

- Record link hit rectangles in the preview layout (same geometry for paint and
  click; no second layout).
- External links: reuse the existing web/mail scheme allowlist; refuse
  `file:`/`javascript:`.
- Relative `.md` links open in a tab; local images render through the existing
  decoded-image path when cheap, or stay `▢ alt` with a documented limitation.
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
| Session 4 | 1.2 Quick Open | in progress | Step A done: the palette reuses the index for the same root; the walk uses `file_type()` with a symlink `metadata()` fallback. Step B (background worker) pending. Gate green: 362 unit + 2 doc tests, selfcheck. |
| Session 4 | 1.3 Autosave | done | Autosave writes verbatim through `Document::save_verbatim` (no tidy, no undo step); failures keep the tab dirty and show an error. 4 new tests (2 doc, 2 state). Gate green: 366 unit + 2 doc tests, selfcheck. |
| Session 4 | 2.1 Command table | pending | Keystone for 3.6. |
| Session 4 | 2.2 `pub(crate)` | pending | |
| Session 4 | 2.3 State pruning | pending | |
| Session 4 | 3.1 Markdown outline | pending | |
| Session 4 | 3.2 Go to Line | pending | |
| Session 4 | 3.3 Preview links | pending | |
| Session 4 | 3.4 Explorer keys | pending | |
| Session 4 | 3.5 Reopen/overflow | pending | |
| Session 4 | 3.6 Command palette | pending | After 2.1. |
| Session 4 | 4.x | pending | Deferred. 4.5 CI restoration is a separate decision; no workflow file is checked in. |

## First three commits (ready to start)

1. **`Restore every open tab across sessions`** — F1, one session test.
2. **`Reuse the Quick Open index and stat less`** — F2 Step A, one index test.
3. **`Write autosave verbatim and report failures`** — F3, four tests.

Then Phase 0 docs, then Phase 1.2 Step B (background indexing). Each commit is
green on `scripts/check.sh` before the next one starts.
