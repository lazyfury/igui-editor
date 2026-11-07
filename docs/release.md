# Releasing on GitHub

The macOS bundle is built locally (there is no CI) and published as a GitHub
release from the same clean commit. [`packaging/release-macos.sh`](../packaging/release-macos.sh)
automates the flow; this page records what it does and how to do it by hand.

## Preconditions

- macOS with `cargo`, `gh` (`gh auth login`), `codesign`, `shasum`.
- Clean working tree and `master` pushed; the release tag points at `HEAD`.
- Network access through the usual proxy: export `all_proxy`/`https_proxy`
  before `git push`/`gh` if your network needs it.
- `scripts/check.sh --release` green (the script runs it).

## One command

```bash
export all_proxy=http://127.0.0.1:7890 https_proxy=http://127.0.0.1:7890
packaging/release-macos.sh
```

Overrides:

| Variable | Default | Meaning |
|---|---|---|
| `RELEASE_TAG` | `v<Cargo version>` | tag / release name |
| `RELEASE_TITLE` | same as tag | GitHub release title |
| `RELEASE_NOTES` | `docs/release-notes/<tag>.md` | notes file (falls back to one line) |
| `SKIP_PACKAGE` | `0` | reuse an existing `dist/` zip (no gate/rebuild) |
| `DIST_DIR` | `./dist` | output directory |
| `ALLOW_DIRTY` | `0` | allow a dirty tree (not recommended) |

The script:

1. runs `scripts/check.sh --release` and `packaging/package-macos.sh`;
2. verifies the bundle signature;
3. writes `dist/igui-editor-<version>-macos-<arch>.zip.sha256`;
4. creates the annotated tag if missing and pushes it;
5. `gh release create` (or `gh release upload --clobber` when the release
   already exists) with the zip and checksum;
6. prints the release URL and asset list.

## Manual equivalent

```bash
scripts/check.sh --release
packaging/package-macos.sh
codesign --verify --deep --strict dist/igui-editor.app
(cd dist && shasum -a 256 igui-editor-0.1.0-macos-arm64.zip > igui-editor-0.1.0-macos-arm64.zip.sha256)

git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
gh release create v0.1.0 \
  dist/igui-editor-0.1.0-macos-arm64.zip \
  dist/igui-editor-0.1.0-macos-arm64.zip.sha256 \
  --title v0.1.0 \
  --notes-file docs/release-notes/v0.1.0.md \
  --verify-tag
```

Refreshing an existing release after a rebuild:

```bash
SKIP_PACKAGE=0 packaging/package-macos.sh
gh release upload v0.1.0 dist/*.zip dist/*.sha256 --clobber
```

## Verify the published assets

```bash
gh release view v0.1.0 --json url,assets
tmp=$(mktemp -d)
gh release download v0.1.0 --pattern '*macos-arm64.zip*' --dir "$tmp"
(cd "$tmp" && shasum -a 256 -c *.sha256)
unzip -q "$tmp"/*.zip -d "$tmp/app"
codesign --verify --deep --strict "$tmp/app/igui-editor.app"
```

## Notes

- The bundle is **ad-hoc signed**, not notarized. Locally built copies run;
  downloaded copies may need `xattr -dr com.apple.quarantine` or
  System Settings → Privacy & Security → Open Anyway. Developer ID signing and
  notarization remain a separate step (see [`packaging.md`](packaging.md)).
- The release is built from the current host architecture only (no universal
  binary yet).
- Release notes live in `docs/release-notes/<tag>.md` so the published text is
  versioned with the source.
- Never upload `target/`, `packaging/build/`, or an unsigned intermediate
  bundle. `dist/` is git-ignored and only the zip + sha256 are assets.

## Release history

| Tag | Date | Assets | Notes |
|---|---|---|---|
| `v0.1.0` | 2026-11-07 | `igui-editor-0.1.0-macos-arm64.zip`, `.sha256` | First tagged macOS arm64 build (emoji, VS Code file icons). |
