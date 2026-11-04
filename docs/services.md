# Native services and built-in plugins

Status: Git status/diff/stage/unstage/commit, the first native PTY terminal slice,
and LSP for Rust, Go, TypeScript/JavaScript, C/C++, Python and Godot GDScript are
implemented. Extension host remains deferred.

## Intent

- Git and Terminal are native Rust services, not JS extensions.
- Built-in features are **compile-time plugins**, never dynamically loaded
  dylibs. Dynamic native ABI loading is explicitly out of scope.
- Built-in plugins call native service methods directly; the manifest,
  capabilities and DTOs remain serializable so a future external extension host
  can map the same surface over JSON-RPC or WASM.
- Extension host is not implemented yet.

```text
Host
 ├─ ServiceRegistry (manifests + commands + sidebar tabs)
 ├─ GitState / GitWorker (native)
 └─ later: TerminalState / PtyWorker (native)

BuiltinPlugin (compile time)
  git  -> ServiceManifest("git"), sidebar tab "GIT", commands
```

## Layers

- **Plugin**: registration unit (`BuiltinPlugin`). Registered at startup by
  `services::install_builtins`.
- **Service**: capability implementation (`model::git` worker). Owns expensive
  resources; UI holds immutable snapshots.
- **Contribution**: sidebar tab, command descriptor, later panels/status items.
- **Protocol DTO**: `RepoStatus`, `GitEntry`, `GitDiff`, command/event types;
  already serializable-friendly (plain data), no UI types.

## Capability ids

```text
git.read.status
git.read.diff
git.read.log
git.read.commit
git.write.stage
git.write.commit
terminal.create
terminal.write
terminal.resize
terminal.close
lsp.diagnostics
lsp.hover
lsp.completion
lsp.definition
lsp.symbols
```

Git read + stage/unstage/commit are wired. Terminal capabilities are declared;
the first panel exposes one session with a draggable, persisted height,
open state, vt100 scrollback and cell selection/copy. Multiple sessions are next.

## Git service

- Calls system `git` via `std::process::Command`, never a shell.
- Worker thread (`model::git::GitWorker`) serializes status/diff requests.
- `--porcelain=v2 -z --branch` parsing is pure and unit-tested.
- `git diff --no-ext-diff --no-color --unified=3`, capped at 2 MB.
- UI triggers refresh on open/save/tab/folder changes and manually; no polling.
- Read-only diff overlay; stage/unstage/commit is M8.2.

Not exposed: arbitrary git command strings, network/auth UI, libgit2.

## Terminal service (first slice implemented)

- `portable-pty` + `vt100`; native cell-grid renderer, no WebView/JS.
- Font: the panel names a fixed-pitch family per text draw
  (`src/fonts.rs`: `Menlo` / `SF Mono` / `.SF NS Mono` / `Monaco` / …), probed against
  the backend measurer and cached in `TerminalState`. The editor uses the same
  resolved family for narrow runs (cached in `EditorUi`) and the UI family for
  wide glyphs. The engine supports this with `DrawCommand::DrawText::family`,
  `PaintContext::draw_text_in` and `TextMeasurer::advance_in`.
- Cells are placed at `col * cell_w`; wide glyphs use the UI family at the same
  em and are centered in their two columns, and the parser screen (not the
  cached grid) decides when to resize the parser/PTY.
- One PTY worker per session; reader thread + writer; resize updates PTY and
  parser; drop kills the child. The first UI has one session and a draggable
  panel whose open/height state persists; multiple sessions are next.
- Focus is a separate `Focus::Terminal`; key encoding is pure and testable.
- Panel lives below the editor inside `main`; its top divider extends the shared
  `ui::handle` with a horizontal axis.
- Raw terminal data is not exposed to extensions by default; only a gated
  command/event bridge later.

## Built-in plugin rule

A first-party plugin may use native types directly. It may **not** reach into
`AppState` mutably from a worker or paint thread; it exposes snapshots and
commands. Future external plugins see only the serialized projection of the
same service.

## Next

1. Terminal: multiple sessions.
2. Git: discard, branch and network operations (network stays in the terminal).
3. Workspace Trust UI (the opt-in local-binary guard is implemented).
4. Extension host (deferred): JSON-RPC/WASM mapping of the same capabilities.

## LSP service (Rust, Go, TS/JS, C/C++, Python, Godot)

Status: diagnostics, hover, completion, definition, document symbols and
semantic tokens are implemented for every built-in spec. The client is
language-agnostic; each plugin registers a declarative `LanguageServerSpec`.

```text
PluginHost
 └─ language_server(LanguageServerSpec)
       id, language, extensions, language_ids,
       project_markers, workspace_markers, loose_files,
       command / command_env, args, transport, port_arg, install
```

### Built-in servers

Repository-local `node_modules/.bin` shims are only considered when
`IGUI_EDITOR_TRUST_WORKSPACE=1`; the default is PATH plus the user/global
toolchain directories. This keeps opening an untrusted project from executing
its own language-server binary.

| Spec | Command (`IGUI_EDITOR_*`) | Transport | Project markers | Workspace markers |
|---|---|---|---|---|
| `rust` | `rust-analyzer` (`…_RUST_ANALYZER`) | stdio | `Cargo.toml` | `Cargo.lock`, `rust-toolchain.toml` |
| `go` | `gopls` (`…_GOPLS`) | stdio | `go.mod` | `go.work` |
| `typescript` | `typescript-language-server --stdio` (`…_TS_SERVER`) | stdio | `package.json`, `tsconfig.json`, `jsconfig.json` | `pnpm-workspace.yaml`, `lerna.json`, `nx.json`, `turbo.json`, `rush.json`, `tsconfig.base.json` |
| `clangd` | `clangd` (`…_CLANGD`) | stdio | `compile_commands.json`, `CMakeLists.txt`, `.clangd`, `.git` | `.clangd`, `.git` |
| `python` | `pyright-langserver --stdio` (`…_PYRIGHT`) | stdio | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`, `.git` | `pyrightconfig.json` |
| `gdscript` | Godot editor (`…_GODOT`) | TCP | `project.godot` | — |

- `language_ids` maps extensions to LSP `languageId`: `.tsx → typescriptreact`,
  `.jsx → javascriptreact`, `.h → c`, `.hpp → cpp`, everything else falls back
  to `language`. `didOpen` uses the per-file value.
- `loose_files` lets a server start outside a project (TypeScript, clangd,
  Python); Rust/Go/Godot keep the old “project required” rule.
- **Root choice**: the outermost workspace marker wins, then the nearest project
  marker, then (loose-file specs) the open folder or the file's folder. Markers
  are searched above the open folder as well, so opening one file inside a
  monorepo still finds `go.work`/`pnpm-workspace.yaml`. Roots are cached per
  `(spec, file)` and invalidated when the file is activated or the folder
  changes; `node_modules/.bin`, `~/go/bin`, pnpm/npm global bins, Homebrew LLVM
  and the Xcode toolchain are searched for commands.
- **Sessions**: one JSON-RPC child per `(spec, project root)`. Up to four live
  sessions are kept (LRU, 10 idle minutes), and every open document is synced to
  the session that owns its root. Switching tabs between languages reuses the
  child instead of restarting it; diagnostics/semantic tokens/symbols are keyed
  by path, so background tabs keep their state.
- **Enable/disable**: `Settings::lsp` (settings row **Language Intelligence** and
  the footer `LSP` dot) stops every child and clears transient state; re-enabling
  starts the active project lazily. Persisted in the session (format v13).
- **Missing tools**: a spawn error with `ErrorKind::NotFound` opens the shared
  tooling prompt (`state/tooling.rs` + `ui/tooling.rs`) with the spec's
  `ToolInstall` hint. It offers **COPY COMMAND**, **RETRY** and **CLOSE** only —
  the editor never installs or executes the hint. The same prompt is generic and
  reusable by future native services.
- **Process**: shell-free `Command` with argument vectors; `stderr` discarded;
  `PATH` augmented for Finder launches. Godot is launched headless with a fresh
  loopback port and the client connects on a background thread.
- **Sync**: full-text `didOpen`/`didChange`/`didClose`; edits are debounced
  (140 ms) and use LSP UTF-16 positions.
- **Server requests**: `workspace/configuration`, `workspace/workspaceFolders`,
  `client/registerCapability` and progress requests are answered defensively so
  a server cannot stall the UI.
- **UI state**: diagnostics are keyed by path and painted as underlines + a
  gutter severity rail. The footer LSP chip is severity-colored and opens a
  compact PROBLEMS popup (message, line:column, click to jump). Hovering a
  diagnostic shows its message immediately; normal code falls back to the
  server hover. Godot native hovers expose an **OPEN GODOT DOCS** action built
  from the LSP declaration (`<Native> class ...` / `func Class.member`); the
  `gdscript/capabilities` notification supplies native class names for
  Ctrl/Cmd-click documentation. The docs view requests
  `textDocument/nativeSymbol`, caches the response, and renders it as a normal
  read-only editor tab (`model/docs.rs` + `Document::ViewKind::Docs`). BBCode
  links become byte ranges on the tab text; clicking one updates the docs tab.
  Completion activation is debounced (140 ms) and the selected item is resolved
  with `completionItem/resolve` to show a hover-style preview; native previews
  open the docs tab, while local methods use a plain resolved docs tab.
  Ctrl/Cmd-click performs a hover probe for classes, methods, virtual methods,
  globals and properties before falling back to definition. Completion is prefix-filtered before the UI
  cap so global methods such as `print` are not lost in Godot's ~2000-item
  result. Semantic tokens
  are requested on open/change and painted over the syntax pass with
  server-aware variable/parameter/field/type colors. Document symbols feed the
  right outline panel. Hover and completion are transient overlays. Nothing is
  persisted.

Capabilities:

```text
lsp.diagnostics
lsp.hover
lsp.completion
lsp.definition
lsp.symbols
```

### Dynamic/data-only contributions

`LanguageServerSpec` uses owned strings, and `LanguagePack` accepts an optional
`lsp` object. A language pack can therefore add a server without a rebuild:

```json
"lsp": {
  "id": "lua-ls",
  "language": "lua",
  "extensions": ["lua"],
  "project_markers": ["stylua.toml"],
  "workspace_markers": [],
  "language_ids": {},
  "loose_files": false,
  "command": "lua-language-server",
  "args": ["--stdio"],
  "install": {
    "tool": "lua-language-server",
    "language": "Lua",
    "command": "brew install lua-language-server"
  }
}
```

Built-in server ids win collisions. A future external extension host can
deserialize the same spec and expose the same capabilities; native code still
ships only as compile-time built-in plugins (no Rust dylib ABI). Running a
server command executes a local process, so future UI should add an explicit
trust/permission prompt for third-party packs.
