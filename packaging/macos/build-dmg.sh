#!/usr/bin/env bash
# Builds Obsidia.app and wraps it in a .dmg.
# Usage: build-dmg.sh <path to obsidia binary> <version> <output.dmg>
set -euo pipefail

bin="$1"
version="$2"
out="$3"
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d)"

app="$work/dmg/Obsidia.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/obsidia"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"

iconset="$work/obsidia.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$root/assets/logo-512.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  [ $double -le 512 ] && sips -z $double $double "$root/assets/logo-512.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
cp "$root/assets/logo-512.png" "$iconset/icon_256x256@2x.png"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/obsidia.icns"

ln -s /Applications "$work/dmg/Applications"
hdiutil create -volname "Obsidia" -srcfolder "$work/dmg" -ov -format UDZO "$out"
