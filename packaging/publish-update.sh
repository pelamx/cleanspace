#!/usr/bin/env bash
# Tells installed copies about a new version: sets one platform's entry in docs/update.json and signs the file.
#   packaging/publish-update.sh macos          0.5.0 docs/downloads/deskMonk-0.5.0-macos.dmg
#   packaging/publish-update.sh windows-x86_64 0.5.0 docs/downloads/update/deskmonk-0.5.0-windows-x86_64.tar.gz
#   packaging/publish-update.sh linux-x86_64   0.5.0 docs/downloads/update/deskmonk-0.5.0-linux-x86_64.tar.gz
# The file must already be in docs/ (it is served from there); deploy afterwards. Needs the release key
# (~/.deskmonk-release/update-signing.key, made by `cargo run --example sign_update -- keygen`), see src/update.rs.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# = 3 ] || { sed -n 2,6p "$0"; exit 2; }
PLATFORM=$1 VERSION=$2 FILE=$3
KEY=${DESKMONK_UPDATE_KEY:-$HOME/.deskmonk-release/update-signing.key}
case $FILE in docs/*) ;; *) echo "$FILE is not under docs/" >&2; exit 1 ;; esac
[ -f "$FILE" ] && [ -f "$KEY" ]
SHA=$(shasum -a 256 "$FILE" | cut -d' ' -f1)
python3 - "$PLATFORM" "$VERSION" "/${FILE#docs/}" "$SHA" <<'PY'
import json, sys, os
platform, version, url, sha = sys.argv[1:]
path = "docs/update.json"
m = json.load(open(path)) if os.path.exists(path) else {}
m[platform] = {"version": version, "url": url, "sha256": sha}
open(path, "w").write(json.dumps(m, indent=2, sort_keys=True) + "\n")
PY
PATH=$HOME/.cargo/bin:$PATH cargo run -q --example sign_update -- sign "$KEY" docs/update.json
echo "$PLATFORM → $VERSION ($SHA)"
