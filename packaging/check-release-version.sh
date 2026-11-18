#!/usr/bin/env bash
# Checks that VERSION is a stable semantic version (no leading "v") and that
# npack-cli/Cargo.toml and npack-electron/package.json are at exactly that version.
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
found=$(awk -F'"' '/^version *=/ { print $2; exit }' "$ROOT/npack-cli/Cargo.toml")
if [[ "$found" != "$VERSION" ]]; then
  echo "npack-cli/Cargo.toml is at version '$found', not '$VERSION'" >&2
  status=1
fi
found=$(awk -F'"' '/^ *"version":/ { print $4; exit }' "$ROOT/npack-electron/package.json")
if [[ "$found" != "$VERSION" ]]; then
  echo "npack-electron/package.json is at version '$found', not '$VERSION'" >&2
  status=1
fi
exit $status
