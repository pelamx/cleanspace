#!/bin/sh
# Installs deskMonk from this folder: for this user (~/.local) by default, for everyone with sudo (/usr/local).
#   ./install.sh            install        ./install.sh --uninstall   remove
set -eu
cd "$(dirname "$0")"
if [ "$(id -u)" = 0 ]; then PREFIX=/usr/local; else PREFIX="$HOME/.local"; fi
BIN="$PREFIX/bin" APPS="$PREFIX/share/applications" ICON="$PREFIX/share/icons/hicolor/256x256/apps"
if [ "${1:-}" = --uninstall ]; then
  rm -f "$BIN/deskmonk" "$BIN/deskmonk-tui" "$APPS/deskmonk.desktop" "$ICON/deskmonk.png"
  echo "deskMonk removed from $PREFIX"
  exit 0
fi
mkdir -p "$BIN" "$APPS" "$ICON"
install -m 755 deskmonk deskmonk-tui "$BIN/"
install -m 644 icon.png "$ICON/deskmonk.png"
# The menu entry runs the installed binary even if that bin folder is not on PATH.
sed "s|^Exec=deskmonk|Exec=$BIN/deskmonk|" deskmonk.desktop > "$APPS/deskmonk.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
echo "deskMonk installed to $BIN; it is in your app menu."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Add $BIN to PATH to run deskmonk from a terminal." ;; esac
