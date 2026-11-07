#!/usr/bin/env bash
# Package the macOS app and publish (or refresh) a GitHub release.
#
# Usage:
#   packaging/release-macos.sh
#   RELEASE_TAG=v0.2.0 packaging/release-macos.sh
#   SKIP_PACKAGE=1 packaging/release-macos.sh      # reuse dist/ artifacts
#
# Requires `gh auth login` and a clean, pushed `master` (override with
# ALLOW_DIRTY=1 at your own risk). See docs/release.md for the manual workflow.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)"
TAG="${RELEASE_TAG:-v$VERSION}"
TITLE="${RELEASE_TITLE:-$TAG}"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
ARCH="$(uname -m)"
ZIP="$DIST_DIR/igui-editor-${VERSION}-macos-${ARCH}.zip"
SHA="$ZIP.sha256"
NOTES="${RELEASE_NOTES:-$ROOT/docs/release-notes/$TAG.md}"
SKIP_PACKAGE="${SKIP_PACKAGE:-0}"
ALLOW_DIRTY="${ALLOW_DIRTY:-0}"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "release-macos.sh: macOS only" >&2
    exit 1
fi
for tool in cargo gh shasum codesign; do
    command -v "$tool" >/dev/null || { echo "release-macos.sh: missing $tool" >&2; exit 1; }
done

if [[ "$ALLOW_DIRTY" != 1 && -n "$(git status --porcelain)" ]]; then
    echo "release-macos.sh: working tree is dirty; commit/stash or set ALLOW_DIRTY=1" >&2
    exit 1
fi

if [[ "$SKIP_PACKAGE" != 1 ]]; then
    echo "==> gate + release build"
    scripts/check.sh --release
    packaging/package-macos.sh
fi

[[ -f "$ZIP" ]] || { echo "release-macos.sh: missing $ZIP" >&2; exit 1; }
codesign --verify --deep --strict "$DIST_DIR/igui-editor.app"

echo "==> checksum"
(cd "$DIST_DIR" && shasum -a 256 "$(basename "$ZIP")" > "$(basename "$SHA")")
cat "$SHA"

echo "==> tag $TAG"
if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    git tag -a "$TAG" -m "Release $TAG"
fi
git push origin "$TAG"

NOTES_ARGS=(--notes "Release $TAG")
if [[ -f "$NOTES" ]]; then
    NOTES_ARGS=(--notes-file "$NOTES")
fi

echo "==> GitHub release"
if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "$ZIP" "$SHA" --clobber
else
    gh release create "$TAG" "$ZIP" "$SHA" \
        --title "$TITLE" "${NOTES_ARGS[@]}" --verify-tag
fi

gh release view "$TAG" --json url,isDraft,isPrerelease,assets \
    --template '# {{.url}}{{"\n"}}draft={{.isDraft}} prerelease={{.isPrerelease}}{{"\n"}}{{range .assets}}- {{.name}} ({{.size}} bytes){{"\n"}}{{end}}'
