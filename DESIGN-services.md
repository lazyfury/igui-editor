# Native services and built-in plugins

Status: Git status/diff/stage/unstage/commit and the first native PTY terminal slice are implemented. Extension host remains deferred.

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
git.write.stage
git.write.commit
terminal.create
terminal.write
terminal.resize
terminal.close
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
