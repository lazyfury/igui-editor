# macOS packaging

Build a local `.app` from this checkout:

```bash
packaging/package-macos.sh
open dist/igui-editor.app
```

The script builds with `--no-default-features`: the headless self-check,
recording backend and profiler are development-only and are not linked into the
shipped bundle. Release uses fat LTO, `panic = "abort"` and stripped symbols.

## Profiles

| Profile | Settings | Use |
|---|---|---|
| `release` | fat LTO, 1 codegen unit, stripped | the shipped bundle and `scripts/bench.sh` |
| `release-fast` | thin LTO, 16 codegen units, same opt levels | iterating on release-only problems, manual testing of an optimized build |

`release-fast` inherits `release` (including the per-package opt-level
overrides) and only relaxes link-time choices, so a bug that only appears with
optimizations still shows up while relinking costs ~1/3 of the time:

```bash
cargo build --profile release-fast --no-default-features
scripts/check.sh --fast        # gate with the release-fast build step
```

Packaging always uses `release`; never ship a `release-fast` binary — the
recorded size and benchmark numbers assume fat LTO.

Outputs:

- `dist/igui-editor.app` — ad-hoc signed app bundle (run locally);
- `dist/igui-editor-<version>-macos-<arch>.zip` — the app for moving elsewhere;
- `packaging/build/AppIcon.icns` — generated icon;
- `dist/` and `packaging/build/` are git-ignored.

## Icon

`packaging/icon.png` is the 1254×1254 source. `build-icon.sh` resamples every
size required by `iconutil` (16–1024 px, 1x/2x), builds `AppIcon.icns`, and the
package script copies it to `Contents/Resources/AppIcon.icns`.

Regenerate only the icon:

```bash
packaging/build-icon.sh
```

## Customization

```bash
BUNDLE_ID=com.example.editor \
BUILD_NUMBER=42 \
MACOS_MIN=11.0 \
DIST_DIR=/tmp/igui-dist \
packaging/package-macos.sh
```

`CFBundleShortVersionString` comes from `Cargo.toml`; `CFBundleVersion` comes
from `BUILD_NUMBER` (defaults to the version). The bundle is built for the
current host architecture (no universal/lipo step yet).

## Distribution

The script uses an **ad-hoc** signature so the app runs from this machine. For
distribution, replace it with a Developer ID signature, enable hardened runtime,
notarize, and staple:

```bash
codesign --force --deep --options runtime \
  --sign "Developer ID Application: Your Name (TEAMID)" dist/igui-editor.app
ditto -c -k --keepParent dist/igui-editor.app dist/igui-editor.zip
xcrun notarytool submit dist/igui-editor.zip --keychain-profile "notary" --wait
xcrun stapler staple dist/igui-editor.app
```

An ad-hoc build opened from a quarantined download will be rejected by
Gatekeeper; local builds are fine.

## Bundle layout

```text
igui-editor.app/
  Contents/
    Info.plist
    PkgInfo
    MacOS/igui-editor
    Resources/AppIcon.icns
```

`Info.plist` registers text/Markdown document types (txt, md, markdown, mdown,
mkd) with Editor role and `Alternate` handler rank, so “Open With” works without
stealing the default handler. The package script refreshes LaunchServices with
`lsregister -f` so Finder sees a rebuilt bundle immediately.

Finder double-click, `open -a`, and the App's Dock Recent menu feed the same
native request queue; the runtime side lives in `src/macos/`.
