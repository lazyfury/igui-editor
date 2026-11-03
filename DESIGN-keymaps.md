# Keymap design and editor audit

Status: audit complete; implementation not started. This document exists so the
next milestone can move shortcuts out of `input.rs` / `ui/editor.rs` without
changing behavior by accident.

## Current editor audit

### Implemented

| Area | Behavior |
|---|---|
| Text input | Characters via host `TextInput`; Space/Tab are named keys handled by the editor; IME preedit/commit |
| Editing | Insert, newline with indentation, backspace/delete, block indent/outdent, undo/redo with coalescing |
| Movement | Arrows, word left/right, smart Home, End, document start/end, wrapped-row Up/Down |
| Selection | Shift movement, mouse drag, double-click word, triple-click line, Select All, Alt-click extend |
| Clipboard | Cut/copy/paste; empty-selection cut/copy operates on the current line |
| Files | Open/save/save-as, tabs, close guards, external-change reload, autosave/recovery |
| Search | Find/replace, project search + replace, next/previous |
| Views | Edit/split/preview, soft wrap, scroll sync |
| Language | Syntax highlighting, language packs, LSP diagnostics/hover/completion/definition, semantic tokens |
| Terminal | Native PTY, key encoding, focus/caret, resize |
| Git | Status, diff/commit read-only views, stage/commit/history |

### Gaps and rough edges

- Shortcuts are spread across `input.rs`, `ui/editor.rs`, `app.rs`, and overlay
  modules. There is no single binding table, no chord support and no user
  overrides.
- `modifiers.meta || modifiers.ctrl` is used as "primary". That is Mac-oriented:
  VS Code on Windows/Linux uses Ctrl+Left/Right for word movement, while the
  current global path maps Ctrl/Cmd+Left/Right to line start/end.
- Overlay key handling is hand-written (find, quick open, project search,
  completion, modal, confirm dialogs). The precedence is correct, but not
  reusable by a keymap profile.
- No modal editor state. A real Vim profile needs Normal/Visual/Insert,
  operators and counts; mapping `dd` to "delete line" alone would be misleading.
- No `PageUp`/`PageDown` in `igui_core::Key` (the model has `Document::page`), no
  word-delete command, no duplicate/move-line command, no comment toggle, no
  multi-cursor or column selection.
- Bindings are not persisted and there is no "Keymap" settings row.
- Printable characters must never be intercepted in Insert mode; Space/Tab are
  named keys while other text arrives as `TextInput`. A keymap layer must keep
  this distinction.

None of these block a keymap layer; the missing editing commands can be added
one at a time once the command registry exists.

## Goals

1. One source of truth for shortcuts; existing behavior preserved during the
   move.
2. Selectable built-in profiles: **VS Code**, **JetBrains**, **Vim** (Vim is a
   modal profile, not a relabeled VS Code table).
3. Platform-aware primary modifier: Meta on macOS, Ctrl elsewhere; display text
   follows the platform.
4. Chords/sequences (`g g`, `Ctrl+K Ctrl+S`) with timeout and prefix display.
5. Contextual dispatch: overlays and modal editor modes take precedence; an
   unhandled printable key still becomes text.
6. User overrides in a small config file, without rebuilding.
7. Headless tests for resolution and dispatch; no screenshots.

## Proposed architecture

```text
host key event
   -> normalize (Key, Modifiers)
   -> Keymap::resolve(scope, sequence)
   -> Match::{Command(id), Prefix, None}
   -> command executor (AppState)
   -> fallback: TextInput / editor-local handling
```

### Commands

Keep the existing `Action` as the first command set, then split it into stable
command ids:

```text
file.save, file.saveAs, file.open, file.openFolder, file.closeTab
edit.undo, edit.redo, edit.cut, edit.copy, edit.paste, edit.selectAll
edit.cutLine, edit.copyLine, edit.deleteLine, edit.duplicateLine
edit.moveLineUp, edit.moveLineDown, edit.toggleComment
motion.left, motion.right, motion.wordLeft, motion.wordRight
motion.lineStart, motion.lineEnd, motion.docStart, motion.docEnd
editor.find, editor.findNext, editor.findPrevious
view.split, view.preview, view.softWrap, view.sidebar, view.terminal
lsp.hover, lsp.completion, lsp.definition
```

A command carries metadata used by menus, settings and conflict reporting:
`id`, `label`, `available(context)`, `checked(state)`, optional platform
binding. Menus and keymaps resolve to the same command executor.

### Key representation

```text
KeyChord { key: Key, modifiers: Modifiers }   // normalized
Sequence = Vec<KeyChord>                       // 1..N
Modifiers = Ctrl | Alt | Shift | Meta          // physical
Primary = Meta on macOS, Ctrl elsewhere        // profile-level alias
```

- Store profiles with `primary` rather than hard-coding Cmd/Ctrl.
- Keep `Shift` in the chord for selection/uppercase decisions; text delivery is
  still the host's `TextInput`.
- Do not derive text from `Key::Character`: the host owns keyboard layout and
  composition.
- Match the longest binding; a partial match is a pending prefix, cancelled by
  timeout, Escape, focus loss or an unmatched key.

### Scopes and precedence

```text
Modal / dialog              (highest: Enter, Escape, typed fields)
Overlay: completion / quick open / project search / find / tab menu
Editor: Visual / Normal / Insert
Terminal
Global app commands
```

- Overlay fields consume editing keys first. Global commands may still be
  allowed explicitly (save, close, next match).
- In Vim Insert, printable keys fall through to text; in Vim Normal/Visual,
  printable keys are bindings (so `i`, `a`, `dd` never become text).
- Terminal keeps raw byte encoding; only a tiny escape hatch (focus move,
  terminal close) is global.
- `Escape` has a scope-specific chain: close overlay → leave Visual/Normal →
  clear selection → pass.

### Profiles

#### VS Code (platform-aware)

- mac: `Cmd` primary; `Option` word movement; `Cmd+Shift+K` delete line;
  `Cmd+X` empty-selection line cut; `Cmd+/` comment.
- Windows/Linux: `Ctrl` primary; `Ctrl+Backspace` word delete;
  `Ctrl+Shift+K` delete line; `Shift+Alt+Down` duplicate line.
- Existing app keys (`Cmd+P`, `Shift+Cmd+F`, etc.) remain app commands.
- Keep `Cmd+I` as the completion binding on macOS because the OS reserves
  Ctrl+Space for input sources; allow `Ctrl+Space` where it reaches the app.

#### JetBrains

- mac: `Cmd` primary; `Cmd+D` duplicate; `Cmd+Backspace` delete line.
- Windows/Linux: `Ctrl+D` duplicate; `Ctrl+Y` delete line.
- `Ctrl+X`/`Cmd+X` with empty selection cuts the line (already implemented and
  shared with VS Code).
- Smart Home/End and word movement follow the platform convention.

#### Vim (phase 2, modal)

Implement as a real modal layer, not as a handful of aliases:

- `Mode::{Insert, Normal, Visual, VisualLine}`.
- Normal: `h j k l`, `w b e`, `0 ^ $`, `gg G`, `i a I A o O`, `x`, `dd`, `yy`,
  `p`, `u`, `Ctrl+r`, `/`, `n N`, `v`, `V`, `:` (small command subset).
- Pending operators and counts (`d2w`, `3dd`) are a small state machine; show
  the pending prefix in the footer.
- Insert mode delegates to the existing text path; `Esc` returns to Normal.
- Visual mode uses the existing anchor/caret selection model; operators apply to
  the selection.
- `Ctrl+X` is not a line cut in Vim; line deletion is `dd`.

Start with Insert/Normal + motions/`dd`/`i`/`Esc`; Visual and text objects are a
later slice. Do not ship a profile named Vim that cannot enter Normal mode.

### User overrides

`<config>/keymaps.json` (or a `keymap.*` block in the session) with a small
schema:

```json
{
  "base": "vscode",
  "bindings": {
    "global": { "primary+shift+p": "quickOpen" },
    "editor.insert": { "alt+up": "edit.moveLineUp" },
    "editor.normal": { "g g": "motion.docStart" }
  }
}
```

- Unknown commands/chords are reported once, then ignored; the app still starts.
- Built-in bindings win only when the user did not override the same chord in
  the same scope.
- A future UI can show conflicts and let the user unbind (`null`).

### Persistence and UI

- Add `keymap=vscode|idea|vim` to the session (next session version).
- Add one Settings row: **Keymap** (cycles profiles, applies immediately).
- Footer shows the active profile briefly when changed; optional status chip
  later.
- Do not persist transient chord prefixes or Vim mode across restarts.

## Migration plan

1. **Inventory + command ids** — map every current hard-coded key to an id;
   extract command execution from `input.rs` without behavior change.
2. **Resolver + VS Code profile** — pure `keymap` module, unit tests for
   primary/Cmd/Ctrl, chords, scope precedence; route global and editor keys
   through it.
3. **Settings/session** — Keymap row, persistence, user override loader.
4. **JetBrains profile** — add duplicate/delete/word-delete commands, platform
   mappings, selfcheck.
5. **Vim modal layer** — Normal/Insert first, then Visual/operators; separate
   mode state, no plain-letter bindings in Insert.
6. Optional: Emacs profile, command palette, custom per-language overrides.

## Test plan

- Pure tests: `resolve(scope, sequence)`, longest match, prefix timeout, platform
  primary, user override precedence, conflict detection.
- Headless selfcheck per profile: drive `KeyDown`/`TextInput` through the real
  app and assert model state; no OS screenshots.
- Regression: existing 124 + 2 tests and the LSP/Git/terminal selfchecks must
  stay green after each routing change.

## Non-goals for the first keymap slice

- Multi-cursor / column selection.
- Full Vim ex commands, macros, registers or plugins.
- OS-level global hotkeys.
- Dynamic native keymap plugins; data-only JSON overrides are sufficient first.
