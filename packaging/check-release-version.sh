#!/usr/bin/env bash
# Checks that VERSION is a stable semantic version (no leading "v") and that
# both crates' Cargo.toml files are at exactly that version.
#
# Usage: packaging/check-release-version.sh VERSION
#
# Used by the "Create release" workflow before it tags anything, and by CI.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  sed -n '2,7p' "$0" >&2
  exit 2
fi
VERSION=$1
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(dirname "$HERE")

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "Version must be a stable semantic version such as 0.5.0 (no leading v), got '$VERSION'" >&2
  exit 1
}

status=0
for crate in npack-cli npack-gui; do
  found=$(awk -F'"' '/^version *=/ { print $2; exit }' "$ROOT/$crate/Cargo.toml")
  if [[ "$found" != "$VERSION" ]]; then
    echo "$crate/Cargo.toml is at version '$found', not '$VERSION'" >&2
    status=1
  fi
done
exit $status
