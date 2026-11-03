#!/usr/bin/env bash
# Build a macOS .icns from packaging/icon.png (1254×1254 source).
# Usage: packaging/build-icon.sh [source.png] [output.icns]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="${1:-$ROOT/packaging/icon.png}"
OUTPUT="${2:-$ROOT/packaging/build/AppIcon.icns}"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "build-icon.sh: iconutil/sips require macOS" >&2
    exit 1
fi
for tool in sips iconutil; do
    command -v "$tool" >/dev/null || { echo "build-icon.sh: missing $tool" >&2; exit 1; }
done
[[ -f "$SOURCE" ]] || { echo "build-icon.sh: no source $SOURCE" >&2; exit 1; }

ICONSET="$(mktemp -d)/AppIcon.iconset"
trap 'rm -rf "$(dirname "$ICONSET")"' EXIT
mkdir -p "$ICONSET" "$(dirname "$OUTPUT")"

# size:iconset-name pairs required by iconutil.
for spec in \
    16:icon_16x16.png \
    32:icon_16x16@2x.png \
    32:icon_32x32.png \
    64:icon_32x32@2x.png \
    128:icon_128x128.png \
    256:icon_128x128@2x.png \
    256:icon_256x256.png \
    512:icon_256x256@2x.png \
    512:icon_512x512.png \
    1024:icon_512x512@2x.png
do
    size="${spec%%:*}"
    name="${spec#*:}"
    sips -s format png -z "$size" "$size" "$SOURCE" --out "$ICONSET/$name" >/dev/null
done

iconutil -c icns "$ICONSET" -o "$OUTPUT"
echo "icon: $OUTPUT"
