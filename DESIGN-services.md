# Native services and built-in plugins

Status: Git status/diff/stage/unstage/commit, the first native PTY terminal slice,
Rust LSP (rust-analyzer), and Godot GDScript LSP (TCP) are implemented. Extension
host remains deferred.

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
the first panel currently exposes one session, fixed height, no persistence.

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

1. Git M8.2: stage/unstage/commit/discard, status-bar branch chip.
2. Terminal service + panel.
3. Extension host (deferred): JSON-RPC/WASM mapping of the same capabilities.

## LSP service (Rust + Godot)

Status: Rust (`rust-analyzer`) and Godot GDScript diagnostics, hover, completion,
definition and document symbols are implemented. The client is language-agnostic;
the built-in plugins register declarative `LanguageServerSpec` values.

```text
PluginHost
 └─ language_server(LanguageServerSpec)
       id, language, extensions, project_markers,
       command / command_env, args, transport, port_arg
```

- **Process**: one JSON-RPC child per project root, lazily started when an owned
  file is active and a `project_markers` file exists (`Cargo.toml` for Rust,
  `project.godot` for Godot). Rust uses the child's stdio. Godot is launched
  headless with a fresh loopback port, then the client connects over TCP (the
  connection is established on a background thread, so startup never blocks a
  frame). Frames are read/written on threads; `stderr` is discarded.
- **Sync**: full-text `didOpen` / `didChange` / `didClose`; edits are debounced
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
  Ctrl/Cmd-click documentation. Completion is prefix-filtered before the UI
  cap so global methods such as `print` are not lost in Godot's ~2000-item
  result. Semantic tokens
  are requested on open/change and painted over the syntax pass with
  server-aware variable/parameter/field/type colors. Document symbols feed the
  right outline panel. Hover and completion are transient overlays. Nothing is
  persisted.
- **Rust spec**: command `rust-analyzer`, overridable with
  `IGUI_EDITOR_RUST_ANALYZER`; stdio transport; arguments are passed directly,
  never through a shell.
- **Godot spec**: command `/Applications/Godot.app/Contents/MacOS/Godot` on
  macOS (or `godot` elsewhere), overridable with `IGUI_EDITOR_GODOT`; TCP
  transport with `--lsp-port <dynamic>`; the process is killed with the client.

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
  "command": "lua-language-server",
  "args": ["--stdio"]
}
```

Built-in server ids win collisions. A future external extension host can
deserialize the same spec and expose the same capabilities; native code still
ships only as compile-time built-in plugins (no Rust dylib ABI). Running a
server command executes a local process, so future UI should add an explicit
trust/permission prompt for third-party packs.
