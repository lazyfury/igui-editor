# Improvement plan

Status: done (Session 1). This file is the execution plan for the review findings in
[`review.md`](review.md) and the current architecture. Each phase is implemented
only when its acceptance checks pass; the progress log at the bottom is updated
after every phase.

## Constraints

- Keep `model/` pure Rust and unit-tested.
- Keep `state/` as one `AppState`; add domain methods in the matching module.
- Keep one source of truth for commands in `src/keymap.rs` and `Action`.
- Every behavior change needs a unit test or a `--selfcheck` assertion.
- Run `scripts/check.sh` before closing a phase.
- Do not modify `../igui`.

## Phase 1: keyboard routing

- [x] Remove the hard-coded shortcut fallback in `src/input.rs`.
- [x] Fix literal `ctrl` matching on Linux/Windows while keeping `primary`.
- [x] Move `Cmd/Ctrl+Arrow` movement into keymap actions.
- [x] Add command ids for the movement actions.
- [x] Tests: unbind is respected, `Ctrl+Space`/`Ctrl+\`` resolve, movement
      actions select or move the caret.

Acceptance:

- `keymaps.json` `{"bindings":{"global":{"primary+s":null}}}` really disables
  Save.
- The same key table drives dispatch, menu hints and tests.
- `Ctrl+Space` and `Ctrl+\`` work on non-macOS hosts without a fallback.

## Phase 2: document IO and dirty state

- [x] Add a shared atomic writer (`temp + fsync + rename`, permissions
      preserved) and use it for `Document::save`.
- [x] Preserve the file's line ending (LF/CRLF) across open/save/reload and
      recovery snapshots.
- [x] Track a save point in the undo stack so undo/redo can clear `dirty`
      correctly.
- [x] Tests: CRLF round-trip, atomic-writer temp cleanup, undo to the save
      point is clean, redo is dirty, save-point invalidation after a divergent
      edit.

Acceptance:

- `scripts/check.sh` passes.
- Saving a CRLF fixture keeps CRLF.
- Typing then undoing to the saved content no longer asks to save.

## Phase 3: find and replace at scale

- [x] Replace `find_all`'s full-text boundary vector with a streaming scan.
- [x] Cap matches and expose truncation to the find bar.
- [x] Add `Document::replace_ranges` implementing one batch edit / one undo.
- [x] Route Replace All through the batch API.
- [x] Tests: cap behavior, Unicode, overlapping matches, one-step undo.
- [x] Verify the change does not touch the Markdown preview path (no bench run
      required; the editor/find tests and self-check cover it).

Acceptance:

- A 2 MB fixture with many matches does not allocate a per-character boundary
  vector.
- Replace All is one undo step.
- `cargo test` and `--selfcheck` pass.

## Phase 4: focus-correct clipboard

- [x] Route Paste to the PTY when the terminal owns focus.
- [x] Prevent Copy/Cut/SelectAll from silently operating on the editor while
      the terminal is focused.
- [x] Test the terminal-focus path without requiring a real PTY where possible.

Acceptance:

- `Cmd+V` with terminal focus never changes the editor document.
- Editor and find-field clipboard behavior is unchanged.

## Phase 5: UI cost and external URL safety

- [x] Cache the flattened sidebar rows; expose `&[Row]` instead of
      `Vec<Row>` so hover/paint/scroll stop cloning the tree.
- [x] Validate external URLs before opening them; only allow approved schemes.
- [x] Replace the Windows `cmd /C start` path with a non-shell opener.
- [x] Tests: row cache invalidation after toggle/expand, URL validation.

Acceptance:

- Pointer movement over the editor does not clone sidebar rows.
- A malicious `[url=file:///...]` or `javascript:` link is refused.
- `scripts/check.sh` passes.

## Phase 6: documentation and verification

- [x] Update session version, terminal state, roadmap and keymap documents.
- [x] Add this plan to `docs/README.md` and the root routing table if needed.
- [x] Record final `scripts/check.sh` output in the progress log.
- [x] Leave deferred items (workspace trust, CI, close guard, terminal
      selection/scrollback) in the next-step section.

Acceptance:

- Docs match the implemented session version and feature state.
- No stale “next candidate” item that is already implemented.

## Follow-up task log

### Task 1: incremental line index (done)

- [x] Track only the affected line starts after an edit, undo or redo instead of
      rebuilding the whole index from the text.
- [x] Keep insertion-at-line-start, newline deletion, merge and split cases
      covered by tests.
- [x] Compare against a full rebuild in a unit test helper.
- [x] Benchmark before/after (same fixture): typing `1.41 ms/frame` →
      `1.04 ms/frame` (single run, noisy; Markdown parse/layout/paint were
      unchanged).
- [x] `scripts/check.sh` passes with 234 unit tests + 2 doc tests.

Acceptance:

- `line_starts` matches a full rebuild after typing, newline insertion,
  backspace, undo and redo.
- The editor/self-check pipeline remains green.

### Task 2: word-boundary soft wrap (done)

- [x] Break at the last whitespace after real content; keep the separator on
      the previous row so the next row starts with a word.
- [x] Preserve leading indentation and fall back to character boundaries for
      unbreakable long tokens.
- [x] Tests: word wrapping, long-token fallback, indentation, visual row
      mapping.
- [x] `scripts/check.sh` passes with 237 unit tests + 2 doc tests.

Acceptance:

- A wrapped line does not start with a separator unless the logical line itself
  starts with whitespace.
- Long unbreakable tokens still split instead of overflowing.
- Caret/click visual row mapping remains consistent.

### Task 3: trusted workspace binaries (done)

- [x] Stop resolving language servers from `node_modules/.bin` by default.
- [x] Allow them only with `IGUI_EDITOR_TRUST_WORKSPACE=1`; PATH and global
      toolchains still work.
- [x] Test the trusted and untrusted resolution paths.
- [x] Document the guard in features/services.

Acceptance:

- A repository-local shim is not selected unless explicitly trusted.
- The normal PATH/toolchain resolution still works.

## Deferred to the next feature slice

These are intentionally not part of this plan's implementation phases:

- Native macOS close guard + Dock reopen.
- Terminal multiple sessions, selection/copy, scrollback.
- Workspace Trust UI (the opt-in local-binary guard is implemented).
- CI workflow with a pinned sibling `igui` checkout.
- Incremental line index and incremental Markdown preview layout.
- Grapheme-aware caret movement.

## Progress log

| Date | Phase | Status | Notes |
|---|---|---|---|
| Session 1 | Baseline | done | `cargo fmt`, clippy, 217 unit tests + 2 doc tests, `--selfcheck` pass. |
| Session 1 | 1 keyboard | done | Fallback removed; literal Ctrl normalized; movement actions added; 232 unit tests. |
| Session 1 | 2 document IO | done | Atomic save, CRLF preservation, save-point dirty tracking, recovery EOL. |
| Session 1 | 3 find/replace | done | Streaming scan, 10k cap, one-step batch Replace All. |
| Session 1 | 4 terminal focus | done | Terminal paste/copy/clipboard routes are focus-aware. |
| Session 1 | 5 UI/URL | done | Sidebar row cache, URL scheme allowlist, Windows non-shell opener. |
| Session 1 | 6 docs | done | Docs updated; final `scripts/check.sh` passed with 232 unit tests + 2 doc tests. |
| Session 2 | Follow-up 1 | done | Incremental line index; 234 unit tests; typing bench 1.41 → 1.04 ms/frame. |
| Session 2 | Follow-up 2 | done | Word-boundary soft wrap; 237 unit tests; selfcheck green. |
| Session 2 | Follow-up 3 | done | Repo-local LSP shims require explicit trust; 238 unit tests. |

## Final acceptance checklist

- [x] `scripts/check.sh` passes.
- [x] New unit tests cover every behavior change.
- [x] Existing `--selfcheck` remains green; new UI-adjacent paths are covered by
      unit tests where a full window is not required.
- [x] Documentation is updated.
- [x] No generated artifacts are committed.
