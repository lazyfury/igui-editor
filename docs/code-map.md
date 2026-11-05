# Code map

```
src/
  model/doc.rs        text buffer, caret/selection, movement, delta undo stack
  model/fileicon.rs   Explorer file-type table: name/extension -> FileKind
  model/find.rs       plain-text matcher (case / whole-word)
  model/git.rs        git status/diff/stage/commit worker + porcelain parser
  model/pty.rs        portable-pty PTY session
  model/fuzzy.rs      Quick Open ranking
  model/workspace_index.rs  ignored-dir file index (Quick Open)
  model/search.rs     background project search (worker + caps)
  wake.rs             cross-thread frame wakeups + deadline clock
  fonts.rs            family policy (Menlo-first mono, CJK UI face)
  keymap.rs           chord parser, profiles, user override resolver
  vim.rs              Normal/Insert/Visual key handling (Vim profile)
  syntax/pack.rs      declarative JSON language packs
  model/lsp.rs        stdio/TCP JSON-RPC client, per-root sessions, diagnostics,
                      hover/completion, monorepo root selection, native Godot symbols
  model/godot.rs      GDScript block rules (smart newline indent)
  model/typing.rs     auto-pair decisions (brackets/quotes) + Markdown list continuation
  model/bbcode.rs     Godot BBCode -> text spans + clickable references
  model/docs.rs       native symbol -> docs view text + link ranges
  services/lsp.rs     built-in Rust language-server spec (rust-analyzer)
  services/go.rs      built-in Go language-server spec (gopls)
  services/typescript.rs  built-in TS/JS language-server spec
  services/clang.rs   built-in C/C++ language-server spec (clangd)
  services/python.rs  built-in Python language-server spec (pyright)
  services/godot.rs   built-in Godot GDScript spec (attach to the running editor, else spawn headless with the rescan profile)
  model/recovery.rs   crash snapshots (txt + metadata)
  model/replace.rs    replace-in-files planning/apply (conflict + open skip)
  model/fs_tree.rs    lazy sidebar tree (dirs first, visibility filter per session)
  model/ignore.rs     Explorer ignore rules + OFF/DIM/HIDE mode for tool files
  model/fsutil.rs     atomic temp+fsync+rename writer shared by model IO
  model/markdown.rs   Markdown event stream -> flat block model
  model/session.rs    key=value session file (atomic write, escaping, v16)
  model/watch.rs      directory snapshots, change diffs, watcher globs
  model/workspace.rs  tabs + file IO (open/save/move/close folder)
  model/wrap.rs       soft-wrap display rows + visual row/column mapping
  dialogs.rs          `Dialogs` trait + scripted test double
  drop.rs             shared file-drop target (winit observer -> update)
  native.rs           Finder/Dock/quit request queue (main-thread)
  macos/              Apple Event open, Dock menu, recent, quit guard, chrome
  services/           native service registry + built-in plugin descriptors
  ui/git.rs           GIT sidebar tab + diff overlay + commit prompt
  ui/lsp.rs           hover card + completion list overlays
  ui/dock.rs          shared dock tab strip (layout, paint, hit tests, close)
  ui/problems.rs      bottom-dock PROBLEMS list (workspace diagnostics, jump)
  ui/tooling.rs       missing-tool prompt (copy-only install command)
  ui/docs_view.rs     dedicated read-only docs tab view (wrap + link hit-test)
  ui/outline.rs       right document-symbol outline
  ui/tree.rs          shared Explorer/outline tree row renderer
  ui/icon.rs          embedded Lucide icons, cached igui_svg documents + painter
  ui/fileicon.rs      Material file-type icons: fill spans, cache + painter
  ui/tree_menu.rs     file-tree context menu
  ui/settings.rs      settings + Keymap row
  ui/references.rs    Fn/Shift+F12 references overlay (geometry shared by paint/click)
  ui/rename_symbol.rs  F2 symbol-rename prompt
  ui/terminal.rs      vt100 cell renderer, shell tabs and terminal key encoding
  input.rs            keyboard dispatch: modal -> find -> shortcuts -> tree
  state/mod.rs        AppState value, constructors, menus and hit rects
  state/actions.rs    command dispatch (menus, shortcuts, keymap actions)
  state/{tabs,files,find,modal,workspace}.rs  document lifecycle and dialogs
  state/{terminal,git,lsp,search}.rs  native services and workers
  state/tooling.rs    missing-tool prompt state, LSP on/off entry point
  state/dock.rs       dock/panel table, open state, tab commands, badges
  state/{settings,view,reliability,services,new_file,quick_open,references,rename_symbol}.rs
                      settings, layout, autosave/recovery and prompt state
  state/watch.rs      external-change poll: visible directories -> FileEvents
  state/schedule.rs   time-based wakeup deadlines (status/hover/autosave/poll)
  theme.rs            geek palette + compact metrics
  kit.rs              control-node construction and text/line helpers
  ui/menu.rs          menu bar + app-level drop-down overlay
  ui/sidebar.rs       file tree
  ui/tabs.rs          tab strip (drag reorder, close targets)
  ui/tab_menu.rs      right-click tab menu (close variants / path / reveal)
  ui/editor.rs        gutter, text, selection, caret, scrollbars, IME preedit
  ui/find_bar.rs      find / replace strip
  ui/quick_open.rs    Quick Open palette
  ui/new_file.rs      quick-create-file prompt
  ui/project_search.rs  project search panel
  ui/split.rs         draggable editor/preview divider
  ui/recovery.rs      startup recovery prompt
  ui/drop.rs          drop overlay
  ui/preview.rs       rendered Markdown pane (cached primitive layout)
  ui/modal.rs         close-dirty / close-many / external / replace modals
  ui/status.rs        file / position / facts / messages
  app.rs              shell construction, input routing, igui_app glue
  host.rs             winit + wgpu plugins, native dialogs, CJK-capable face
  selfcheck.rs        --selfcheck
  bench.rs            benchmark entry points (`--features bench`)
benches/
  markdown.rs         Markdown parse/layout/paint/typing benchmark
assets/icons/lucide/  vendored Lucide SVGs (ISC), embedded via include_str!
assets/icons/material/ vendored Material Icon Theme SVGs (MIT), embedded via
                      include_str! and painted as fill spans
docs/                 progressive-disclosure documentation (see docs/README.md)
AGENTS.md             agent rules, file routing, mandatory checks
packaging/
  icon.png            1254×1254 app-icon source
  build-icon.sh       sips/iconutil -> AppIcon.icns
  package-macos.sh    release build + .app bundle + ad-hoc signature
  Info.plist.in       bundle metadata template
```
