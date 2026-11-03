#!/usr/bin/env bash
# Build the release binary, generate AppIcon.icns and assemble igui-editor.app.
#
# Usage:
#   packaging/package-macos.sh
#   BUNDLE_ID=com.example.editor BUILD_NUMBER=42 packaging/package-macos.sh
#
# Output: dist/igui-editor.app (ad-hoc signed, no notarization).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="igui-editor"
EXECUTABLE="igui-editor"
VERSION="$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)"
BUILD="${BUILD_NUMBER:-$VERSION}"
BUNDLE_ID="${BUNDLE_ID:-dev.igui.editor}"
MIN_MACOS="${MACOS_MIN:-11.0}"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
APP="$DIST_DIR/$APP_NAME.app"
ICON="$ROOT/packaging/build/AppIcon.icns"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "package-macos.sh: macOS only" >&2
    exit 1
fi
for tool in cargo sips iconutil plutil codesign; do
    command -v "$tool" >/dev/null || { echo "package-macos.sh: missing $tool" >&2; exit 1; }
done

echo "==> release build ($VERSION)"
cargo build --release --locked

echo "==> icon"
"$ROOT/packaging/build-icon.sh" "$ROOT/packaging/icon.png" "$ICON"

echo "==> bundle $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 "target/release/$EXECUTABLE" "$APP/Contents/MacOS/$EXECUTABLE"
cp "$ICON" "$APP/Contents/Resources/AppIcon.icns"
sed \
    -e "s/__EXECUTABLE__/$EXECUTABLE/g" \
    -e "s/__BUNDLE_ID__/$BUNDLE_ID/g" \
    -e "s/__VERSION__/$VERSION/g" \
    -e "s/__BUILD__/$BUILD/g" \
    -e "s/__MIN_MACOS__/$MIN_MACOS/g" \
    "$ROOT/packaging/Info.plist.in" > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Local ad-hoc signature: enough to run unsigned builds from this checkout.
# Distribution outside this machine still needs Developer ID + notarization.
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --deep --strict "$APP"

# Refresh LaunchServices so Finder picks up the document types immediately.
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [[ -x "$LSREGISTER" ]]; then
    "$LSREGISTER" -f "$APP" >/dev/null 2>&1 || true
fi

ARCH="$(uname -m)"
ZIP="$DIST_DIR/$APP_NAME-$VERSION-macos-$ARCH.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "==> ok: $APP"
du -sh "$APP"
echo "==> zip: $ZIP"
