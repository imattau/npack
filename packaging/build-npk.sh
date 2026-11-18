#!/usr/bin/env bash
# Builds the .npk archives and their manifests that a release publishes to
# Nostr: `npack` and `npack-gui` (the Electron window around the web store). Both manifests carry a store listing (summary,
# description, categories) so they show up in npack's own Browse view the way any
# other app does; the GUI's also has an icon and desktop entry.
#
# Usage: packaging/build-npk.sh VERSION PUBLISHER BIN_DIR OUT_DIR GUI_DIR
#   VERSION    release version without a leading "v", e.g. 0.5.0
#   PUBLISHER  the Nostr publisher key (hex or npub) that will sign the release
#   BIN_DIR    a directory holding the built `npack` binary
#   OUT_DIR    where the .npk files, manifests and SHA256SUMS are written
#   GUI_DIR    the unpacked Electron app (npack-electron/release/linux-unpacked)
#
# Needs jq. Run it locally with debug binaries to check changes before a release.
set -euo pipefail

if [[ $# -ne 5 ]]; then
  sed -n '2,15p' "$0" >&2
  exit 2
fi
VERSION=$1
PUBLISHER=$2
BIN=$(cd "$3" && pwd)
mkdir -p "$4"
OUT=$(cd "$4" && pwd)
GUI=$(cd "$5" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
RELEASE_DATE=${RELEASE_DATE:-$(date -u +%F)}
NPACK=$BIN/npack

[[ -x "$BIN/npack" ]] || { echo "missing $BIN/npack" >&2; exit 1; }
[[ -x "$GUI/npack-gui" ]] || { echo "missing $GUI/npack-gui (build npack-electron first)" >&2; exit 1; }
command -v jq > /dev/null || { echo "jq is required" >&2; exit 1; }

WORK=$(mktemp -d)
cleanup() { [[ -n "${WORK:-}" && -d "$WORK" ]] && rm -rf -- "$WORK"; }
trap cleanup EXIT

# The linker-level dependencies of a binary, as a JSON array.
needed() {
  "$NPACK" inspect "$1" \
    | awk '/^needed: / { sub(/^needed: /, ""); if ($0 != "none") print }' \
    | jq -R -s 'split("\n") | map(select(length > 0))'
}

# Libraries the Electron app loads from the system (it bundles Chromium itself).
# They are not ordinary linker dependencies of any file in the package, so they
# are declared by hand. Installing on a system without them fails up front with
# a clear message instead of an app that will not start.
GUI_RUNTIME_LIBS='["libgtk-3.so.0","libnss3.so","libgbm.so.1","libasound.so.2","libX11.so.6","libxkbcommon.so.0","libatspi.so.0","libXtst.so.6","libXss.so.1","libnotify.so.4"]'

# finish NAME MANIFEST_JSON_FILE ROOT_DIR: pack ROOT_DIR into NAME's .npk and
# write the standalone manifest next to it.
finish() {
  local name=$1 manifest=$2 root=$3
  local artifact="${name}-${VERSION}-linux-x86_64.npk"
  mkdir -p "$root/.npack"
  cp "$manifest" "$root/.npack/manifest.json"
  # --strict: our own packages must not hard-code install paths (see docs).
  "$NPACK" pack "$root" --output "$OUT/$artifact" --strict > /dev/null
  "$NPACK" manifest "$OUT/$artifact" --output "$OUT/${name}-${VERSION}.manifest.json" > /dev/null
  (cd "$OUT" && sha256sum "$artifact") >> "$OUT/SHA256SUMS"
}

: > "$OUT/SHA256SUMS"

# --- npack -------------------------------------------------------------------
root=$WORK/npack-root
mkdir -p "$root/bin" "$root/share/man/man1"
install -m 0755 "$BIN/npack" "$root/bin/npack"
"$NPACK" man > "$root/share/man/man1/npack.1"
jq -n \
  --arg publisher "$PUBLISHER" --arg version "$VERSION" \
  --arg artifact "npack-${VERSION}-linux-x86_64.npk" \
  --arg date "$RELEASE_DATE" \
  --argjson runtime_requires "$(needed "$BIN/npack")" \
  '{publisher:$publisher,name:"npack",version:$version,artifact:$artifact,sha256:"",
    dependencies:[],conflicts:[],os:"linux",arch:"x86_64",format:"npk",
    runtime_requires:$runtime_requires,provides:[],
    app:{
      summary:"Find, install and update software published by people you trust",
      description:"npack installs software that publishers have signed and announced on Nostr. Every download is checked against a signed checksum before anything is installed, and nobody is trusted until you say so.",
      homepage:"https://github.com/imattau/npack",
      license:"MIT",
      categories:["System","PackageManager"],
      release_date:$date
    }}' > "$WORK/npack.json"
finish npack "$WORK/npack.json" "$root"

# --- npack-gui ---------------------------------------------------------------
root=$WORK/gui-root
mkdir -p "$root/bin" "$root/share/applications" \
  "$root/share/icons/hicolor/scalable/apps" "$root/share/metainfo"
cp -a "$GUI" "$root/share/npack-gui"
# A user-local install cannot make Chromium's sandbox helper setuid root, so
# the app relies on unprivileged user namespaces. Where the system forbids those
# (Ubuntu 24.04+ by default), the launcher turns the sandbox off rather than
# leaving an app that aborts at start; the system packages (.deb/.rpm) install
# the helper setuid root and keep the sandbox on.
cat > "$root/bin/npack-gui" <<'LAUNCHER'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
app="$here/../share/npack-gui"
sandbox_args=
helper="$app/chrome-sandbox"
if [ -u "$helper" ] && [ "$(stat -c %u "$helper" 2>/dev/null)" = 0 ]; then
  :
elif [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null)" = 1 ] \
  || [ "$(cat /proc/sys/kernel/unprivileged_userns_clone 2>/dev/null)" = 0 ] \
  || [ "$(cat /proc/sys/user/max_user_namespaces 2>/dev/null)" = 0 ]; then
  sandbox_args=--no-sandbox
fi
exec "$app/npack-gui" $sandbox_args "$@"
LAUNCHER
chmod 0755 "$root/bin/npack-gui"
install -m 0644 "$HERE/desktop/io.npack.Gui.desktop" "$root/share/applications/"
install -m 0644 "$HERE/desktop/io.npack.Gui.svg" "$root/share/icons/hicolor/scalable/apps/"
sed -e "s/@VERSION@/${VERSION}/" -e "s/@DATE@/${RELEASE_DATE}/" \
  "$HERE/desktop/io.npack.Gui.metainfo.xml" > "$root/share/metainfo/io.npack.Gui.metainfo.xml"
jq -n \
  --arg publisher "$PUBLISHER" --arg version "$VERSION" \
  --arg artifact "npack-gui-${VERSION}-linux-x86_64.npk" \
  --arg date "$RELEASE_DATE" \
  --argjson runtime_libs "$GUI_RUNTIME_LIBS" \
  '{publisher:$publisher,name:"npack-gui",version:$version,artifact:$artifact,sha256:"",
    dependencies:[{publisher:$publisher,name:"npack",requirement:("=" + $version)}],
    conflicts:[],os:"linux",arch:"x86_64",format:"npk",
    runtime_requires:$runtime_libs,provides:[],
    app:{
      summary:"Find, install and update software published by people you trust",
      description:"npack installs software that publishers have signed and announced on Nostr. Every download is checked against a signed checksum before anything is installed, and nobody is trusted until you say so.",
      homepage:"https://github.com/imattau/npack",
      license:"MIT",
      categories:["System","PackageManager"],
      icon:"share/icons/hicolor/scalable/apps/io.npack.Gui.svg",
      desktop_file:"share/applications/io.npack.Gui.desktop",
      release_date:$date
    }}' > "$WORK/gui.json"
finish npack-gui "$WORK/gui.json" "$root"

echo "Built:"
ls -1 "$OUT" | sed 's/^/  /'
