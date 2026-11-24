#!/bin/bash
# Builds "Photo Editor.app" for both Apple Silicon and Intel Macs and packs it
# into dist/PhotoEditor-mac.dmg. Run on a Mac, from anywhere in the project:
#
#   bash packaging/macos/bundle.sh
#
# The app is not signed with an Apple developer certificate (that needs a paid
# account), so macOS asks the person to allow it the first time. See README.md.
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
export MACOSX_DEPLOYMENT_TARGET=11.0

cargo build --release -p photo-editor --target aarch64-apple-darwin
cargo build --release -p photo-editor --target x86_64-apple-darwin

APP="dist/dmg/Photo Editor.app"
rm -rf dist/dmg dist/PhotoEditor-mac.dmg
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# One program that runs on both kinds of Mac.
lipo -create -output "$APP/Contents/MacOS/photo-editor" \
  target/aarch64-apple-darwin/release/photo-editor \
  target/x86_64-apple-darwin/release/photo-editor

sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# The icon, in the sizes macOS asks for.
ICONSET="$(mktemp -d)/icon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256; do
  sips -z "$size" "$size" crates/app/assets/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" crates/app/assets/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

# An "ad-hoc" signature: required for the app to start on Apple Silicon. It
# does not identify a developer, so the first-launch warning still appears.
codesign --force --deep --sign - "$APP"

ln -s /Applications dist/dmg/Applications
hdiutil create -volname "Photo Editor" -srcfolder dist/dmg -ov -format UDZO dist/PhotoEditor-mac.dmg
echo "Built dist/PhotoEditor-mac.dmg (version $VERSION)"
lipo -info "$APP/Contents/MacOS/photo-editor"
