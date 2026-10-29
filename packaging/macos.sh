#!/usr/bin/env bash
# Universal macOS build (Apple Silicon + Intel in one app): dist/deskMonk-<version>-macos.dmg
# Needs both Rust targets: rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --bin deskmonk --target "$target"
done

WORK=dist/macos
APP=$WORK/dmg/deskMonk.app
rm -rf "$WORK" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$WORK/icon.iconset"
lipo -create -output "$APP/Contents/MacOS/deskmonk" target/aarch64-apple-darwin/release/deskmonk target/x86_64-apple-darwin/release/deskmonk
sed "s/@VERSION@/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"
# Every icon size from the 1024 px Dock icon (padded to Apple's icon grid).
for size in 16 32 128 256 512; do
  sips -z $size $size assets/dock-icon.png --out "$WORK/icon.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) assets/dock-icon.png --out "$WORK/icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/icon.iconset" -o "$APP/Contents/Resources/icon.icns"
# shortcut: ad-hoc signature, not notarized; users confirm "Open Anyway" once. An Apple Developer ID fixes that.
codesign --force --deep --sign - "$APP"
ln -s /Applications "$WORK/dmg/Applications"
# hdiutil sometimes fails with "resource busy" right after the files were written; one retry.
DMG="dist/deskMonk-$VERSION-macos.dmg"
hdiutil create -volname deskMonk -srcfolder "$WORK/dmg" -ov -format UDZO "$DMG" || { sleep 3; hdiutil create -volname deskMonk -srcfolder "$WORK/dmg" -ov -format UDZO "$DMG"; }
echo "$DMG"
