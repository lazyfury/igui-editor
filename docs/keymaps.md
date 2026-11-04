# Keymap design and editor audit

Status: command routing goes through a pure `keymap` resolver; VS Code and
JetBrains profiles are wired, and Vim has a Normal/Visual/Insert subset. In-app
menus resolve their shortcut hints from the active profile; on macOS the
application menu is native (system menu bar) and the in-app header is hidden in
favour of simulated window controls. This document records the design and the
remaining scope.

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
| Tabs | `primary+1…8` select by index, `primary+9` last, `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle |
| Menus | macOS: native `NSMenu`; other platforms: mouse-driven in-app menus with keymap-derived hints |
| Search | Find/replace, project search + replace, next/previous |
| Views | Edit/split/preview, soft wrap, scroll sync |
| Language | Syntax highlighting, language packs, LSP diagnostics/hover/completion/definition, semantic tokens |
| Terminal | Native PTY, key encoding, focus/caret, resize |
| Git | Status, diff/commit read-only views, stage/commit/history |

### Gaps and rough edges

- Global/editor commands have one binding table and no fallback table. Overlay
  keys (find, quick open, completion, modal) are still hand-written modules that
  run before the keymap; they are not representable in a profile yet.
- `primary` is Meta on macOS and Ctrl elsewhere. On Linux/Windows a literal
  `ctrl+…` binding normalizes to the same host bit; on macOS Ctrl stays distinct.
  Ctrl+Left/Right still map to line start/end rather than word movement on
  Linux/Windows (a future platform-specific profile refinement).
- Overlay key handling is hand-written (find, quick open, project search,
  completion, modal, confirm dialogs). The precedence is correct, but not
  reusable by a keymap profile.
- No modal editor state. A real Vim profile needs Normal/Visual/Insert,
  operators and counts; mapping `dd` to "delete line" alone would be misleading.
- No `PageUp`/`PageDown` in `igui_core::Key` (the model has `Document::page`),
  and no multi-cursor or column selection.
- Word-delete, duplicate/move-line and comment-toggle commands are implemented.
- The active profile persists in the session, and the Settings panel exposes the
  Keymap row. Per-chord overrides live in `<config>/keymaps.json`.
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
file.save, file.saveAs, file.open, file.openProject (alias file.openFolder),
file.closeProject (alias file.closeFolder), file.closeTab
edit.undo, edit.redo, edit.cut, edit.copy, edit.paste, edit.selectAll
edit.cutLine, edit.copyLine, edit.deleteLine, edit.duplicateLine
edit.moveLineUp, edit.moveLineDown, edit.toggleComment, editor.format
motion.left, motion.right, motion.wordLeft, motion.wordRight
motion.lineStart, motion.lineEnd, motion.docStart, motion.docEnd
editor.find, editor.findNext, editor.findPrevious
view.split, view.preview, view.softWrap, view.sidebar, view.explorer, view.git,
view.outline, view.terminal, view.problems
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

## Implementation status

1. **Inventory + command ids** — done in `keymap.rs` and the expanded `Action`
   set; global and editor keys route through it, with the old input fallback
   removed.
2. **Resolver + VS Code profile** — done; tests cover primary/Cmd/Ctrl,
   profile bindings and user overrides.
3. **Settings/session** — done: Keymap row, session `keymap=`, and
   `<config>/keymaps.json` overrides.
4. **JetBrains profile** — done for duplicate/delete/move line and word delete.
5. **Vim modal layer** — first slice done: Normal/Insert and basic Visual,
   motions/operators listed above; Ex commands, text objects and registers are
   explicitly not done.
6. Optional: Emacs profile, command palette, custom per-language overrides.

### User override example

```json
{
  "bindings": {
    "editor": {
      "primary+shift+k": "edit.deleteLine",
      "alt+up": "edit.moveLineUp"
    },
    "global": {
      "primary+shift+p": "quickOpen"
    }
  }
}
```

Null unbinds a chord. Known command ids are listed in `action_from_id` in
`src/keymap.rs`; unknown ids are ignored. The file is read once at startup;
changing the profile re-applies it synchronously.

## Test plan

- Pure tests: `resolve(scope, sequence)`, longest match, prefix timeout, platform
  primary, user override precedence, conflict detection.
- Headless selfcheck per profile: drive `KeyDown`/`TextInput` through the real
  app and assert model state; no OS screenshots.
- Regression: existing tests and the LSP/Git/terminal/keymap selfchecks must
  stay green after each routing change.

## Non-goals for the first keymap slice

- Multi-cursor / column selection.
- Full Vim ex commands, macros, registers or plugins.
- OS-level global hotkeys.
- Dynamic native keymap plugins; data-only JSON overrides are sufficient first.
