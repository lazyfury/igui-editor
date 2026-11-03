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

Next candidates: native macOS top menu bar (Window/Services/Help), window-close
guard + Dock reopen, code-plugin loading (dylib/wasm), per-hit replace and
cross-file undo, incremental preview layout for very large documents, and
word-boundary wrapping.

Notes:

- Native macOS top menu, a close-window dirty guard and Dock reopen are not
  implemented yet; closing the window still follows winit's event-loop exit.
- Only macOS has been exercised manually; the Linux (XDG portal) and Windows
  dialog paths are wired but untested.
- The macOS sheet path is asynchronous by design (no nested modal loop, no
  re-entrant event dispatch); the app polls the dialog future once per frame
  while it is open.
- Recovery snapshots are written after the 2 s idle delay: a hard kill within
  that window can still lose the last keystrokes (autosave shares the delay).
- Project search skips binary files, files over 1 MB, and stops at the caps
  above.
- Quick Open builds its index when opened (no filesystem watcher), so files
  created while it is closed appear on the next open. Open files are polled for
  external changes, but the sidebar/search index is not automatically refreshed.
- Soft wrap breaks at character boundaries, not words, and uses a full-layout
  cache; it is intentionally disabled for very large files.

Known engine limitation: `igui_core::Key` has no `PageUp` / `PageDown`, so page
scrolling is not bound yet (`Document::page` is implemented and tested, waiting
for a key variant). `⌘↑` / `⌘↓` cover document navigation meanwhile.
