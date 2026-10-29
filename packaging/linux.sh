#!/usr/bin/env bash
# Linux build for this machine's CPU (x86_64 or aarch64):
#   dist/deskMonk-<version>-linux-<arch>.AppImage   one file, runs on any distro with a glibc at least as new as
#                                                   the build machine's (CI builds on AlmaLinux 8: glibc 2.28)
#   dist/deskmonk-linux-<arch>.tar.gz               the same app, plain
# Graphics libraries (X11, Wayland, OpenGL) are the system's, loaded at runtime, so nothing is bundled.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)
cargo build --release --bin deskmonk

WORK=dist/linux
APPDIR=$WORK/AppDir
rm -rf "$WORK" && mkdir -p "$APPDIR/usr/bin" "$WORK/deskmonk-linux-$ARCH"
cp target/release/deskmonk "$APPDIR/usr/bin/"
cp assets/deskmonk.desktop "$APPDIR/"
cp assets/icon.png "$APPDIR/deskmonk.png"
ln -s usr/bin/deskmonk "$APPDIR/AppRun"
curl -fsSL -o "$WORK/appimagetool" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
chmod +x "$WORK/appimagetool"
# No FUSE in containers: the tool unpacks itself instead of mounting.
ARCH=$ARCH APPIMAGE_EXTRACT_AND_RUN=1 "$WORK/appimagetool" --no-appstream "$APPDIR" "dist/deskMonk-$VERSION-linux-$ARCH.AppImage"

cp target/release/deskmonk assets/icon.png assets/deskmonk.desktop "$WORK/deskmonk-linux-$ARCH/"
tar czf "dist/deskmonk-linux-$ARCH.tar.gz" -C "$WORK" "deskmonk-linux-$ARCH"
ls -l dist/*linux*
