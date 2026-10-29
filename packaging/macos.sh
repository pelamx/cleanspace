#!/usr/bin/env bash
# macOS builds, Apple Silicon + Intel in one app:
#   packaging/macos.sh                 website DMG  → dist/deskMonk-<version>-macos.dmg (ad-hoc signed)
#   packaging/macos.sh appstore-test   sandboxed App Store build, ad-hoc signed, to try locally → dist/appstore/out/deskMonk.app
#   packaging/macos.sh appstore        signed upload package for App Store Connect → dist/deskMonk-<version>-appstore.pkg
#                                      (needs packaging/appstore/embedded.provisionprofile; see packaging/appstore/README.md)
# Needs both Rust targets: rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -euo pipefail
cd "$(dirname "$0")/.."
MODE=${1:-dmg}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case $MODE in
  dmg) MINOS=10.13; FEATURES=(); BUILD=${BUILD:-$VERSION}; WORK=dist/macos ;;
  appstore | appstore-test)
    MINOS=12.0 # StoreKit 2
    FEATURES=(--features appstore)
    # Every upload needs a higher build number than the last one.
    BUILD=${BUILD:-$(date +%Y%m%d.%H%M)}
    WORK=dist/appstore
    # Own target folder, so the App Store binary never replaces the regular one in target/release.
    export CARGO_TARGET_DIR=target/appstore
    # Full Xcode, not the Command Line Tools: only its Swift back-deployment libraries include Intel (x86_64).
    export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer} ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
# Also C code inside dependencies (e.g. ring) is built for this macOS version, not the build machine's.
export MACOSX_DEPLOYMENT_TARGET=$MINOS
TARGET_DIR=${CARGO_TARGET_DIR:-target}
build() { cargo build --release --bin deskmonk --target "$1" "${FEATURES[@]}"; }
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  # A fresh build on macOS 27 sometimes cannot load a proc-macro it just built ("can't find crate");
  # building again continues where it stopped.
  build "$target" || build "$target" || build "$target"
done

APP=$WORK/out/deskMonk.app
rm -rf "$WORK" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$WORK/icon.iconset"
lipo -create -output "$APP/Contents/MacOS/deskmonk" "$TARGET_DIR"/{aarch64,x86_64}-apple-darwin/release/deskmonk
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUILD@/$BUILD/g" -e "s/@MINOS@/$MINOS/g" packaging/Info.plist > "$APP/Contents/Info.plist"
# Every icon size from the 1024 px Dock icon (padded to Apple's icon grid).
for size in 16 32 128 256 512; do
  sips -z $size $size assets/dock-icon.png --out "$WORK/icon.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) assets/dock-icon.png --out "$WORK/icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/icon.iconset" -o "$APP/Contents/Resources/icon.icns"

if [ "$MODE" = dmg ]; then
  # shortcut: ad-hoc signature, not notarized; users confirm "Open Anyway" once. The Developer ID certificate
  # in the keychain plus notarytool would remove that step.
  codesign --force --deep --sign - "$APP"
  ln -s /Applications "$WORK/out/Applications"
  # hdiutil sometimes fails with "resource busy" right after the files were written; one retry.
  DMG="dist/deskMonk-$VERSION-macos.dmg"
  hdiutil create -volname deskMonk -srcfolder "$WORK/out" -ov -format UDZO "$DMG" || { sleep 3; hdiutil create -volname deskMonk -srcfolder "$WORK/out" -ov -format UDZO "$DMG"; }
  echo "$DMG"
  exit
fi

cp packaging/appstore/PrivacyInfo.xcprivacy "$APP/Contents/Resources/"
if [ "$MODE" = appstore-test ]; then
  # The App ID entitlements are only valid with a provisioning profile; without them the sandbox still applies.
  ENT=$WORK/test.entitlements
  cp packaging/appstore/deskMonk.entitlements "$ENT"
  /usr/libexec/PlistBuddy -c 'Delete :com.apple.application-identifier' -c 'Delete :com.apple.developer.team-identifier' "$ENT"
  codesign --force --sign - --entitlements "$ENT" "$APP"
  echo "$APP"
  exit
fi

PROFILE=packaging/appstore/embedded.provisionprofile
[ -f "$PROFILE" ] || { echo "missing $PROFILE (Mac App Store profile for com.pelamx.deskmonk, see packaging/appstore/README.md)" >&2; exit 1; }
cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
codesign --force --timestamp --sign "Apple Distribution" --entitlements packaging/appstore/deskMonk.entitlements "$APP"
PKG="dist/deskMonk-$VERSION-appstore.pkg"
productbuild --component "$APP" /Applications --sign "3rd Party Mac Developer Installer" "$PKG"
echo "$PKG (build $BUILD)"
