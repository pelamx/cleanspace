#!/usr/bin/env bash
# Linux build for this machine's CPU (x86_64 or aarch64):
#   dist/deskMonk-<version>-linux-<arch>.AppImage   one file, runs on any distro with a glibc at least as new as
#                                                   the build machine's (CI builds on AlmaLinux 8: glibc 2.28)
#   dist/deskmonk-linux-<arch>.tar.gz               the same app, plain, with an install.sh
#   dist/deskmonk_<version>_<arch>.deb / deskmonk-<version>-1.<arch>.rpm / deskmonk-<version>-1-<arch>.pkg.tar.zst
#                                                   native packages with the TUI too (see nfpm.yaml)
# Graphics libraries (X11, Wayland, OpenGL) are the system's, loaded at runtime, so nothing is bundled.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)
cargo build --release --bin deskmonk --bin deskmonk-tui

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

cp target/release/deskmonk target/release/deskmonk-tui assets/icon.png assets/deskmonk.desktop packaging/install.sh "$WORK/deskmonk-linux-$ARCH/"
tar czf "dist/deskmonk-linux-$ARCH.tar.gz" -C "$WORK" "deskmonk-linux-$ARCH"

NFPM=2.48.0
if [ "$ARCH" = aarch64 ]; then NFPM_TAR=arm64 NFPM_ARCH=arm64; else NFPM_TAR=x86_64 NFPM_ARCH=amd64; fi
curl -fsSL "https://github.com/goreleaser/nfpm/releases/download/v$NFPM/nfpm_${NFPM}_Linux_$NFPM_TAR.tar.gz" | tar xz -C "$WORK" nfpm
for format in deb rpm archlinux; do
  NFPM_ARCH=$NFPM_ARCH VERSION=$VERSION "$WORK/nfpm" package -f packaging/nfpm.yaml -p "$format" -t dist/
done
ls -l dist/*linux* dist/*.deb dist/*.rpm dist/*.pkg.tar.zst
