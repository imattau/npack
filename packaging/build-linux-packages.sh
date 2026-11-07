#!/usr/bin/env bash
# Builds the Linux packages for a release from already-built binaries:
#
#   npack          the command line tool and the npackd service, with the man
#                  page, systemd units and the PolicyKit policy. No graphics
#                  libraries: it installs on servers and containers.
#   npack-gui      the desktop app, its launcher entry, icon and AppStream
#                  description. Depends on the matching npack.
#   npack-desktop  an empty meta-package that pulls in both, for desktop
#                  installers and documentation to point at.
#
# Usage: packaging/build-linux-packages.sh VERSION BIN_DIR OUT_DIR
#   VERSION  the release version without a leading "v", e.g. 0.5.0
#   BIN_DIR  a directory holding the built `npack` and `npack-gui` binaries
#   OUT_DIR  where the .deb and .rpm files are written
#
# Run it locally with debug binaries to check packaging changes before a
# release; the release workflows call it with the release binaries.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  sed -n '2,20p' "$0" >&2
  exit 2
fi
VERSION=$1
BIN=$(cd "$2" && pwd)
mkdir -p "$3"
OUT=$(cd "$3" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
RPM_VERSION=${VERSION//-/_}
RELEASE_DATE=${RELEASE_DATE:-$(date -u +%F)}
# rpm wants "Sat Nov 07 2026" and rejects a date whose weekday is wrong.
CHANGELOG_DATE=$(date -u -d "$RELEASE_DATE" '+%a %b %d %Y')

for binary in npack npack-gui; do
  [[ -x "$BIN/$binary" ]] || { echo "missing $BIN/$binary" >&2; exit 1; }
done

WORK=$(mktemp -d)
cleanup() { [[ -n "${WORK:-}" && -d "$WORK" ]] && rm -rf -- "$WORK"; }
trap cleanup EXIT

# Libraries the desktop app loads at run time (found by running it and
# listing what it opened). They never show up as ordinary linker
# dependencies, so they have to be declared by hand.
GUI_DEB_DEPENDS="libc6, libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libegl1, libvulkan1"
GUI_DEB_RECOMMENDS="mesa-vulkan-drivers, libgl1-mesa-dri"
GUI_RPM_SONAMES=(
  "libX11.so.6()(64bit)" "libX11-xcb.so.1()(64bit)" "libxcb.so.1()(64bit)"
  "libXcursor.so.1()(64bit)" "libXi.so.6()(64bit)" "libxkbcommon.so.0()(64bit)"
  "libxkbcommon-x11.so.0()(64bit)" "libwayland-client.so.0()(64bit)"
  "libEGL.so.1()(64bit)" "libvulkan.so.1()(64bit)"
)

# --- stage the files of each package under $WORK/<package>/ -----------------

# Strips the staged copy (never the build output). Debug symbols are most of
# the size of a release binary; skipped quietly if strip is not installed.
strip_binary() {
  command -v strip > /dev/null && strip --strip-unneeded "$1" || true
}

stage_npack() {
  local root=$WORK/npack
  install -d -m 0755 "$root/usr/bin" "$root/usr/share/man/man1" \
    "$root/usr/lib/systemd/user" "$root/usr/lib/systemd/system" \
    "$root/usr/share/polkit-1/actions"
  install -m 0755 "$BIN/npack" "$root/usr/bin/npack"
  strip_binary "$root/usr/bin/npack"
  "$BIN/npack" man | gzip -n -9 > "$root/usr/share/man/man1/npack.1.gz"
  chmod 0644 "$root/usr/share/man/man1/npack.1.gz"
  install -m 0644 "$HERE/npack-update-check.service" "$HERE/npack-update-check.timer" \
    "$HERE/npackd.service" "$root/usr/lib/systemd/user/"
  install -m 0644 "$HERE/npackd-system.service" "$root/usr/lib/systemd/system/"
  install -m 0644 "$HERE/io.npack.policy" "$root/usr/share/polkit-1/actions/"
}

stage_gui() {
  local root=$WORK/npack-gui
  install -d -m 0755 "$root/usr/bin" "$root/usr/share/applications" \
    "$root/usr/share/icons/hicolor/scalable/apps" "$root/usr/share/metainfo"
  install -m 0755 "$BIN/npack-gui" "$root/usr/bin/npack-gui"
  strip_binary "$root/usr/bin/npack-gui"
  install -m 0644 "$HERE/desktop/io.npack.Gui.desktop" "$root/usr/share/applications/"
  install -m 0644 "$HERE/desktop/io.npack.Gui.svg" \
    "$root/usr/share/icons/hicolor/scalable/apps/io.npack.Gui.svg"
  sed -e "s/@VERSION@/${VERSION}/" -e "s/@DATE@/${RELEASE_DATE}/" \
    "$HERE/desktop/io.npack.Gui.metainfo.xml" \
    > "$root/usr/share/metainfo/io.npack.Gui.metainfo.xml"
  chmod 0644 "$root/usr/share/metainfo/io.npack.Gui.metainfo.xml"
}

# --- .deb --------------------------------------------------------------------

# build_deb NAME STAGE_DIR SECTION DEPENDS RECOMMENDS SUMMARY LONG_DESCRIPTION
build_deb() {
  local name=$1 stage=$2 section=$3 depends=$4 recommends=$5 summary=$6 long=$7
  # Build from a copy, so the DEBIAN control directory never ends up in the
  # staged tree that the .rpm is built from.
  local root=$WORK/deb-$name
  mkdir -p "$root"
  cp -a "$stage"/. "$root"/
  install -d -m 0755 "$root/DEBIAN"
  {
    echo "Package: $name"
    echo "Version: $VERSION"
    echo "Section: $section"
    echo "Priority: optional"
    echo "Architecture: amd64"
    echo "Depends: $depends"
    [[ -n "$recommends" ]] && echo "Recommends: $recommends"
    echo "Maintainer: npack contributors"
    echo "Homepage: https://github.com/imattau/npack"
    echo "Description: $summary"
    echo " $long"
  } > "$root/DEBIAN/control"
  dpkg-deb --root-owner-group --build "$root" "$OUT/${name}_${VERSION}_amd64.deb" > /dev/null
}

# --- .rpm --------------------------------------------------------------------

# build_rpm NAME STAGE_DIR SUMMARY DESCRIPTION REQUIRES... (one per argument,
# prefix a weak dependency with "recommends:")
build_rpm() {
  local name=$1 stage=$2 summary=$3 description=$4
  shift 4
  local topdir=$WORK/rpm-$name
  mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
  local spec=$topdir/SPECS/$name.spec
  {
    echo "Name:           $name"
    echo "Version:        ${RPM_VERSION}"
    echo "Release:        1%{?dist}"
    echo "Summary:        $summary"
    echo "License:        MIT"
    echo "URL:            https://github.com/imattau/npack"
    echo "BuildArch:      x86_64"
    local requirement
    for requirement in "$@"; do
      if [[ "$requirement" == recommends:* ]]; then
        echo "Recommends:     ${requirement#recommends:}"
      else
        echo "Requires:       $requirement"
      fi
    done
    echo
    echo "%description"
    echo "$description"
    echo
    echo "%install"
    echo "mkdir -p %{buildroot}"
    echo "if [ -d '$stage' ]; then cp -a '$stage'/. %{buildroot}/; fi"
    echo
    echo "%files"
    if [[ -d "$stage" ]]; then
      (cd "$stage" && find . \( -type f -o -type l \) ! -path './DEBIAN/*' | sort | sed 's|^\.||')
    fi
    echo
    echo "%changelog"
    echo "* ${CHANGELOG_DATE} npack contributors - ${RPM_VERSION}-1"
    echo "- Release build"
  } > "$spec"
  rpmbuild -bb --define "_topdir $topdir" --define "_builddir $topdir/BUILD" \
    --define "debug_package %{nil}" --define "__os_install_post %{nil}" \
    --define "_build_id_links none" \
    "$spec" > "$topdir/rpmbuild.log" 2>&1 || { cat "$topdir/rpmbuild.log" >&2; exit 1; }
  cp "$topdir"/RPMS/x86_64/*.rpm "$OUT/"
}

stage_npack
stage_gui
install -d -m 0755 "$WORK/npack-desktop"   # the meta-package has no files

build_deb npack "$WORK/npack" utils "libc6, ca-certificates" "libnotify-bin" \
  "A Nostr-native package manager" \
  "Discovers, verifies, and installs signed package releases."
build_deb npack-gui "$WORK/npack-gui" x11 "npack (= ${VERSION}), ${GUI_DEB_DEPENDS}" \
  "${GUI_DEB_RECOMMENDS}" \
  "Desktop app for the npack package manager" \
  "Browse, install and update software published by people you trust, from a desktop window."
build_deb npack-desktop "$WORK/npack-desktop" metapackages \
  "npack (= ${VERSION}), npack-gui (= ${VERSION})" "libnotify-bin" \
  "npack with its desktop app" \
  "Installs npack and its desktop app together."

build_rpm npack "$WORK/npack" "A Nostr-native package manager" \
  "Discovers, verifies, and installs signed package releases." \
  "glibc" "ca-certificates" "recommends:libnotify"
build_rpm npack-gui "$WORK/npack-gui" "Desktop app for the npack package manager" \
  "Browse, install and update software published by people you trust, from a desktop window." \
  "npack = ${RPM_VERSION}" "${GUI_RPM_SONAMES[@]}" "recommends:mesa-vulkan-drivers"
build_rpm npack-desktop "$WORK/npack-desktop" "npack with its desktop app" \
  "Installs npack and its desktop app together." \
  "npack = ${RPM_VERSION}" "npack-gui = ${RPM_VERSION}" "recommends:libnotify"

echo "Built:"
ls -1 "$OUT" | sed 's/^/  /'
