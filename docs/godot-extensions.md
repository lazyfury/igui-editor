# Godot extension investigation: GDScript addon vs GDExtension

Status: **investigation complete; Phase 0 document links landed**. This note
records what a project-local Godot editor addon and a native GDExtension can add
to the current attach-only GDScript LSP, and what they cannot. It is the
reference for [`docs/features.md`](features.md) / [`docs/services.md`](services.md)
claims about Godot.

## Question

Can a Godot-side extension make GDScript editing in `igui-editor` better than
the current arrangement (attach to the running editor's LSP on `127.0.0.1:6005`),
without forking or rebuilding the engine?

The two candidates:

1. **GDScript addon** — a project-local `EditorPlugin` written in GDScript.
2. **GDExtension** — a native shared library built against `godot-cpp`.

## Test setup

- Godot `4.7.2-stable (official, ed1daf0bf)`, macOS 26.7.2, arm64.
- A temporary project (`/tmp/gdproj`) with `project.godot`, a `main.tscn`
  (`Main` → `World` → `Player` → `Sprite`, plus `World/Camera`), `player.gd`
  attached to `Main`, and a global class `Foo`.
- A headless editor (`--headless --editor --path …`) served the real LSP on a
  non-default port (6015) so the user's running editor on 6005 was untouched.
- The addon prototype listened on loopback port 6016 with newline-delimited
  JSON; a Python client drove requests and observed events.
- The GDExtension prototype was built with SCons 4.11.1 and `godot-cpp` 4.5
  (the newest 4.x branch published at the time), then loaded by Godot 4.7.2.

The official documentation used:

- [Using an external text editor](https://docs.godotengine.org/en/stable/tutorials/editor/external_editor.html)
  — LSP 6005 / DAP 6006, a running editor is required.
- [Editor plugins](https://docs.godotengine.org/en/stable/tutorials/plugins/editor/index.html)
  and [`EditorPlugin`](https://docs.godotengine.org/en/stable/classes/class_editorplugin.html).
- [`EditorInterface`](https://docs.godotengine.org/en/stable/classes/class_editorinterface.html)
  and [`EditorFileSystem`](https://docs.godotengine.org/en/stable/classes/class_editorfilesystem.html).
- [What is GDExtension?](https://docs.godotengine.org/en/stable/engine_details/engine_api/gdextension/what_is_gdextension.html)
  and [the godot-cpp example](https://docs.godotengine.org/en/stable/tutorials/scripting/cpp/gdextension_cpp_example.html).
- The 4.7 LSP source (`modules/gdscript/language_server/`) for the actual method
  table and capability flags.

## What the LSP already gives us (Godot 4.7)

`initialize` returned:

```text
textDocumentSync            full (open/close/save, willSaveWaitUntil=true)
completionProvider          triggerCharacters [".", "$", "'", "\""]
signatureHelpProvider       triggerCharacters [",", "("]
hover/definition/declaration/references/documentHighlight/documentSymbol  true
documentLinkProvider        {}                     (available)
renameProvider              {prepareProvider: true}
foldingRangeProvider        false
workspaceSymbolProvider     false
documentFormattingProvider  false
documentRangeFormattingProvider false
codeActionProvider          false
semanticTokensProvider      absent
implementation/typeDefinition  false
```

Measured behavior:

- **Node/group/signal/input-action completion works already.** With
  `player.gd` attached to `main.tscn`, completion after `$` returned
  `World`, `World/Player`, `World/Player/Sprite`, `World/Camera`. Godot 4.7's
  `SceneCache` loads the owning scene for this. On a script that is not attached
  to a scene, the same request fell back to the ordinary completion list, as
  expected. `connect("…")`, `call("…")`, input actions, groups, resource paths
  and built-in types are produced by the engine's own `complete_code` path.
- **Document links work for existing path strings.** A saved script containing
  `preload("res://foo.gd")` and `load("res://player.gd")` returned
  `textDocument/documentLink` ranges whose targets were the real files. The
  client currently does not request or use them.
- **Folding is not usable.** `foldingRangeProvider` is `false`, and
  `textDocument/foldingRange` is implemented as an empty array. This is not a
  `Method not found` error.
- **These methods are genuinely missing**: `textDocument/formatting`,
  `textDocument/rangeFormatting`, `textDocument/codeAction`,
  `workspace/symbol`, `textDocument/semanticTokens/full`.
- **These are implemented but empty/no-op**: `textDocument/foldingRange`,
  `textDocument/codeLens`, `textDocument/colorPresentation`;
  `textDocument/willSaveWaitUntil` returns `null`.
- **No watched-file handling.** The class registry behind `class_name` is updated
  by the editor's project scan. In a headless test, a newly written
  `bar.gd` with `class_name Bar` stayed invisible to completion after 1.5 s.
  Godot's default setting only auto-scans/imports while unfocused when
  `interface/editor/behavior/import_resources_when_unfocused` is enabled; the
  normal trigger is focusing the editor.

## GDScript addon capability test

A ~200-line `EditorPlugin` proved the following, all through public editor APIs:

| Request | Result |
|---|---|
| `ping` | Editor version and project path. |
| `status` | Edited scene, selection paths, open scripts, scanning state/progress. |
| `scene_tree` | Full node tree with name, type, unique-name flag, script and children. |
| `open_script` | `EditorInterface.edit_script(script, line)` opens the script at a line in Godot. |
| `play_scene` | `EditorInterface.play_current_scene()` / `play_custom_scene(path)` (capability exists; running games is outside this editor's scope). |
| `update_file` | `EditorFileSystem.update_file(path)` registers a new file, updates a changed file, and drops a deleted one. |
| `scan` | `EditorFileSystem.scan()` forces a project scan. |

Measured freshness results:

- Create `baz.gd` externally → call `update_file` → `Baz` appeared in LSP
  completion immediately (next request).
- Create `qux.gd` externally → call `scan` → `Qux` appeared immediately.
- Delete `delme.gd` → call `update_file` → the class was gone from completion.
- The addon's `EditorFileSystem.script_classes_updated` and
  `filesystem_changed` signals fired after both `update_file` and `scan`, so the
  bridge can **push** an event to the editor instead of relying only on the
  one-second class-cache file poll.
- `EditorPlugin.scene_changed`, `EditorSelection.selection_changed`, and
  `EditorFileSystem.filesystem_changed` are all connectable from GDScript.

The addon **cannot** extend the built-in LSP. Godot's
`GDScriptLanguageProtocol` exposes only `get_text_document`, `get_workspace`,
`is_smart_resolve_enabled` and `is_initialized` (plus deprecated helpers); its
request table is fixed in C++. A project addon can only start a **side server**
with its own protocol, not add `textDocument/formatting` or a new LSP method to
the server on 6005.

### No formatter/parser access

The engine's `GDScript` class exposes script *loading*, methods, constants,
signals and properties, but not the tokenizer/parser or a source formatter.
Nothing in the GDScript API returns formatted source. A GDScript addon therefore
cannot implement a real "Format Document" by calling the engine; it would need
its own formatter. That is a separate Rust (or external tool) project, not an
addon capability.

## GDExtension capability test

A minimal C++ extension (`IguiCppProbe`, built against `godot-cpp` 4.5) was
loaded by Godot 4.7.2:

```text
IGUI_CPP ping=cpp-ok 4.7.2-stable (official) add=42
IGUI_CPP scan=true
```

- Cold build incl. the `godot-cpp` static library: **86 s** (8 jobs).
- Incremental source compile: **~3 s**.
- Release output for this trivial class: **213 KB `.dylib`** (macOS arm64).
- Calling `EditorInterface.get_singleton()->get_resource_filesystem()->scan()`
  and `EditorFileSystem` from C++ worked.
- The 4.5-built extension loaded in 4.7.2. This matches the documented rule:
  extensions target an older minor and work in later minors, not the reverse.
  A 4.7-only build would not load in 4.6.
- GDExtension does **not** replace the project entry point: `addons/*/plugin.cfg`
  still needs a script resource (normally a small GDScript wrapper), and the
  `.gdextension` file and platform binaries must be installed per project.
- Internal engine classes (the GDScript tokenizer/parser, the LSP protocol
  object) are not part of the GDExtension API. A native extension cannot expose
  Godot's parser/formatter or extend the LSP request table either. It would have
  to implement its own parser/formatter.

Conclusion: a GDExtension is technically capable of the same editor-API calls
as the GDScript addon, plus native CPU work. For the features this editor needs
it has **no unique advantage**, and it adds a platform build matrix, ABI
versioning, signing/distribution and per-project installation.

## Decision matrix

| Need | Godot 4.7 LSP | GDScript addon | GDExtension | Recommendation |
|---|---|---|---|---|
| Diagnostics, hover, completion, definition, references, rename, signature help | yes | not needed | not needed | Keep using LSP. |
| Node/group/signal/input/resource completion | yes (scene cache) | could add context | could add context | Use LSP; add coverage/docs. |
| `preload`/`load` path navigation | `documentLink` yes | not needed | not needed | Wired: Ctrl/Cmd-click opens the target (Phase 0). |
| Folding | empty/false | would need its own scanner; editor has no folding UI | same | Native editor feature, not a Godot extension. |
| Project scan after external edits | only on editor focus | `update_file` / `scan` + push events | same, overkill | Optional minimal bridge (Phase 1), or a native class index. |
| Scene tree / selected node / open in Godot | no | yes (public APIs) | yes, overkill | Only if scene-aware editing becomes a priority (Phase 2). |
| Format Document | no | no parser/formatter access | no parser access | Implement in Rust in `igui-editor`, or use an explicitly configured external tool. |
| Debug (DAP 6006) | DAP exists | not relevant | not relevant | Explicit non-goal in `AGENTS.md`. |

## Recommendation

**Do not ship a GDExtension.** The native-extension cost buys nothing for the
current scope, and it cannot reach the parts of Godot a formatter would need.

**Do not ship a GDScript bridge just to force project scans.** It works, but it
costs a per-project addon install/enable step, a trust boundary, a protocol and
long-term compatibility work for a gap that is currently one focus click in
Godot. Prefer a native fallback first.

Suggested order, following the product priority in `AGENTS.md`:

### Phase 0 — consume what the LSP already offers (no addon)

1. **Document links — done.** Request `textDocument/documentLink`, store ranges
   per document, and make Ctrl/Cmd-click over a linked range open the target
   file. This is the cheapest real GDScript navigation win (`preload`, `load`,
   resource path strings) and is already implemented by Godot 4.7. The client
   drops cached ranges on any local edit and ignores a response whose document
   revision moved while the request was in flight.
2. **Verify the existing node/signal completion.** Add a selfcheck/manual smoke
   case for `$` node paths, `connect("…")`, groups, input actions and
   `preload("res://…")` so a future LSP change cannot silently remove them.
3. **Native GDScript folding** only as part of a general text-editing folding
   change; the LSP will never provide it.
4. If focus latency remains annoying, add a **native `class_name` index** for
   immediate local completion/definition. It avoids the addon install and keeps
   working when Godot's registry is stale; the LSP remains the richer source.

### Phase 1 — optional minimal Godot bridge (only if needed)

If the editor decides that "external `class_name` should appear without focusing
Godot" is worth an addon:

- One `EditorPlugin`, loopback only, newline-delimited JSON, no new Rust
  dependency.
- Methods: `ping`, `update_file`, `scan`; notifications:
  `script_classes_updated`, `filesystem_changed`.
- Send `update_file` for created/changed/deleted files in existing directories;
  coalesce requests (for example 200 ms) and use `scan` only for directory
  structure changes.
- Keep the existing `.godot/global_script_class_cache.cfg` watch as a fallback,
  and trigger the existing document re-send immediately when a bridge event
  arrives.
- Ship the addon as source with a **copy-only** install instruction or an
  explicit install command; never enable it behind the user's back and never
  auto-run project code. Detect it from `addons/igui_bridge/plugin.cfg` plus the
  enabled list in `project.godot`.
- Use a handshake file (for example `.godot/igui_bridge.json` with a port and
  PID) or a small port range, because a fixed port collides when two Godot
  editors are open.
- Acceptance: an external `class_name` appears in completion/outline without
  focusing Godot, no LSP restart, fake-server unit tests, and a manual smoke
  test with a real editor.

### Phase 2 — scene-aware editing (only if priority 2 grows)

The addon is the only route to live editor state. Candidate features, in order
of editing value:

- Ctrl/Cmd-click a `$Node/Path` reference to select/reveal that node in Godot's
  open scene (and report its type on hover).
- "Open in Godot" for the current script/scene.
- Signal connection inspection (which scene nodes connect to which methods).

Not candidates: formatting, code actions, workspace symbols, debugger, task
runner, run/play controls. Those either need a formatter/index that belongs in
Rust, or are outside the product scope.

## Risks and constraints

- **Install friction.** Every addon is project-local; teams must commit it or
  each developer must install it. This is the main reason not to make a bridge
  mandatory.
- **Trust.** The addon runs arbitrary GDScript inside the editor. The bridge
  must restrict commands to project paths, expose no file-writing or arbitrary
  command execution, and bind to loopback only. The editor already has a
  workspace-trust guard for local binaries; the addon install prompt must be
  explicit.
- **Main thread.** `update_file`/`scan` run on Godot's main thread. Coalesce,
  debounce and avoid full scans for single-file saves.
- **Version drift.** GDScript addons are source-compatible across 4.x but editor
  APIs can change; include an addon protocol version and Godot version in the
  handshake. GDExtension has a stricter ABI rule (build against the oldest
  supported minor).
- **Single LSP client.** The bridge does not change Godot's one-client LSP
  limit; it only triggers the scan that makes the attached client fresh.

## Evidence

The probes were run against a temporary project with a headless editor on a
separate port; no user project or running editor was touched. The LSP probe
printed the `initialize` result, completion lists, `documentLink` results and
the missing/empty methods. The addon probe exercised `ping`, `status`,
`scene_tree`, `open_script`, `update_file`, `scan`, and the push signals. The
GDExtension probe built a class against `godot-cpp` 4.5, loaded it in 4.7.2, and
called both a registered method and `EditorFileSystem::scan()`.
