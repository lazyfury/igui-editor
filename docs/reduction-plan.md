# Reduction plan — Session 5

Status: **in progress** (phase 1 landed). Follows the Session 4 review and the product priority
in [`AGENTS.md`](../AGENTS.md): text/Markdown first, Godot second, everything
else at the LSP/highlight minimum. The goal is a smaller, more reliable editor,
not a broader one.

Baseline before this plan: `scripts/check.sh` green (396 unit + 2 doc tests,
`--selfcheck`), release binary 4.8 MB.

## Rules

- One logical change per commit; `scripts/check.sh` green before each commit.
- Every behavior change gets a unit test or a `--selfcheck` assertion.
- A removed feature also loses its setting, session key, docs and self-check
  coverage in the same commit.
- Keep the layering: `model/` pure, `state/` one value, `ui/` paint only.
- Do not add CI (not part of this plan).

## Phase 1 — correctness and reliability

### 1.1 Stop the autosave/recovery write loop

`state/reliability.rs` flushed recovery snapshots for every dirty buffer on
every idle tick, even when the buffer had not changed. With autosave off (or a
failing write) a dirty file was rewritten and fsynced every two seconds
indefinitely.

- Track the last snapshotted revision per document and skip unchanged buffers.
- Only run a flush when a document changed since the previous flush.
- Back off a failing autosave instead of retrying it every tick.
- Tests: an unchanged dirty buffer writes once; a changed one writes again; a
  failed autosave is not retried every two seconds.

### 1.2 Bound the undo stack

`UndoStack` kept every delta forever; a select-all delete on a 32 MB buffer
stored a 32 MB `removed` string per entry.

- Cap the retained history by total bytes (and keep the redo tail semantics).
- Tests: the cap drops the oldest entries and undo still stops at the oldest
  retained edit; clearing a huge selection repeatedly stays under the cap.

## Phase 2 — scope reduction (priority-driven)

Each item removes a surface that does not serve text/Markdown or Godot editing.

- **2.1 Image asset preview** — read-only image tab, PNG decode, setting,
  session key and the `igui_assets` dependency. Out of scope: the editor is not
  an asset viewer.
- **2.2 Git stage/commit/history** — keep repository status markers and the
  read-only diff; drop `stage`/`unstage`/`commit`/`log`/commit detail. Git is
  outside the priority and the editor already refused destructive/network
  operations.
- **2.3 Material file-type icons** — 43 embedded SVGs and a fill-span
  rasterizer. Folder disclosure and row rhythm stay; files are labelled by name.
- **2.4 Extra language servers** — keep the generic LSP client, Rust and Godot;
  drop the Go/TypeScript/C-C++/Python specs. No new languages after this.

Not doing: a command palette (Quick Open already covers the same surface);
terminal split panes; terminal profiles; further Git write operations.

## Phase 3 — consistency and budgets

- [x] 3.1 One Markdown extension table shared by view mode, syntax and file
  icons (removed the `.mdx` mismatch).
- 3.2 One `SESSION_VERSION` constant and an explicit migration chain.
- 3.3 `scripts/check.sh --release` asserts the shipped binary size, so "keep it
  lightweight" is measured rather than asserted.
- 3.4 Documentation truth: features, roadmap, design, code map, README,
  `AGENTS.md` routing and the Session 4 plan status.

Deferred (behavior-neutral, tracked here so they are not lost):

- Splitting `selfcheck.rs` / `model/lsp.rs` / `app.rs` / `state/mod.rs`.
- A shared `ui::list` row/scroll/hit helper.
- A table-driven settings/session record (only the version constant and the
  migration seam land in this session).
- Adaptive filesystem-poll backoff (needs a window-focus signal from the host).

## Progress log

| Item | Status | Notes |
|---|---|---|
| Plan | done | This document; indexed from `docs/README.md` and `AGENTS.md`. |
| 1.1 Reliability | done | Snapshots are skipped when the revision is unchanged; a flush is scheduled only by an edit or a 30 s failure backoff; restored buffers schedule one snapshot. 3 new tests. |
| 1.2 Undo cap | done | 64 MB / 5000-entry byte budget, oldest entries dropped, save point shifted. 2 new tests. |
| 2.1–2.4 Scope cut | pending | |
| 3.1 Markdown extensions | done | `MARKDOWN_EXTENSIONS` is the single table; the syntax provider and file icons read it. 2 new tests. |
| 3.2–3.4 | pending | |
