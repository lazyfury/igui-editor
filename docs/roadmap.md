# Scope and roadmap

Delivered (M1): shell, sidebar, tabs, editing, selection, undo/redo, clipboard,
save, dark/light, IME preedit, headless self-check.

Delivered (M2): native Open / Open Folder / Save As, find & replace with
case/whole-word and match highlighting, Markdown edit/split/preview with a
block renderer, dirty-close confirmation, recent files, session persistence
(theme, sidebar, open tabs), window title, centralized key dispatch.

Delivered (M3): syntax highlighting (10 languages + public provider API),
incremental highlight cache with per-frame budget, indent guides, bracket
matching, settings panel with persisted values, sidebar drag-resize, tab
overflow scrolling, large-file guards (highlight/preview/open limits), the
library split (`igui_editor::syntax` etc.).

Delivered (M4): welcome/no-folder state, file & folder drag-and-drop, CLI folder
argument, Quick Open, project search (worker + caps), draggable split divider,
autosave, crash recovery prompt, window size/position restore, session v2.

Delivered (M5): declarative JSON language packs + per-document language
override (persisted by path), replace-in-files with a confirmation modal,
conflict/open-tab skipping and atomic writes, editor/preview scroll sync via
Markdown source-line anchors, session v3.

Delivered (M6): tab drag-reorder + right-click context menu, soft wrap with
visual-row caret/click/navigation mapping and a 2 MB / 20k-line cap, external
file-change detection (clean auto-reload, dirty Keep/Reload), session v4.

Delivered (M7): macOS Finder/`open -a` file opening, Dock Recent/New File/Clear,
quick file creation (`⇧⌘N`), system recent items, and a native dirty-quit guard.

Delivered (M12): generic LSP stdio/TCP client + Rust `rust-analyzer` and Godot
GDScript plugins (diagnostics, hover, completion, definition and symbols), plus
data-only language-pack language-server specs.

Delivered (M13): Go/TypeScript/C-C++/Python language servers with monorepo root
selection and a shared copy-only tooling prompt (the four extra specs were
removed in M15); LSP start/stop in the footer and settings.

Quality pass (see [improvement-plan.md](improvement-plan.md)): keyboard
shortcuts now have one source, saves are atomic with EOL preservation, find and
Replace All are bounded/batched, terminal clipboard commands are focus-aware,
sidebar rows are cached and external URLs are scheme-restricted.

Reduced (M15, Session 5): image-asset preview, Git staging/commit/history,
Material file-type icons and the extra language-server specs were removed;
undo history and crash snapshots are bounded. Session v18 drops `image_preview`.

Delivered (M16): the editor minimap strip — a cached structural thumbnail with a
viewport slider, click/drag scrolling, the View → Minimap toggle (`⌘⌥M`) and
`session.minimap` (session v19).

Next candidates: cross-file undo and incremental preview layout for very large
documents.

Notes:

- The red close control is a quit button (close-to-hide was removed); dirty
  buffers go through the in-app Discard & Quit guard, and the guard runs in the
  app before AppKit terminates. Dock activation still reopens the window.
- Only macOS has been exercised manually; the Linux (XDG portal) and Windows
  dialog paths are wired but untested.
- The macOS sheet path is asynchronous by design (no nested modal loop, no
  re-entrant event dispatch); the app polls the dialog future once per frame
  while it is open.
- Recovery snapshots are written after the 2 s idle delay: a hard kill within
  that window can still lose the last keystrokes (autosave shares the delay).
- Project search skips binary files, files over 1 MB, and stops at the caps
  above.
- Quick Open builds its index on a background worker when opened (no
  filesystem watcher); the one-second poll invalidates that index and refreshes
  the Explorer when files are created or deleted. A closed palette picks up the
  change on its next open, which starts a fresh build; reopening without a
  change reuses the cached index. Project search always scans on demand.
- Soft wrap prefers word boundaries and falls back to character boundaries for
  long tokens; it uses a full-layout cache and is intentionally disabled for
  very large files.

Known engine limitation: `igui_core::Key` has no `PageUp` / `PageDown`, so page
scrolling is not bound yet (`Document::page` is implemented and tested, waiting
for a key variant). `⌘↑` / `⌘↓` cover document navigation meanwhile.
