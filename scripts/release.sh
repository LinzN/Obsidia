#!/usr/bin/env bash
# Prepare a release: set the version, commit and tag.
#   scripts/release.sh 0.5.0
# Then `git push origin main v0.5.0` - CI builds the installers and
# publishes the GitHub release that Obsidia's updater picks up.
set -euo pipefail
version="${1:?usage: scripts/release.sh X.Y.Z}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z"; exit 1; }
cd "$(dirname "$0")/.."
git diff --quiet || { echo "working tree not clean"; exit 1; }

sed -i -E "0,/^version = \"[^\"]+\"/s//version = \"$version\"/" Cargo.toml
# keep Cargo.lock in sync without needing network access
awk -v v="$version" '
  /^name = "obsidia"$/ { print; found = 1; next }
  found && /^version = / { print "version = \"" v "\""; found = 0; next }
  { print }
' Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock

git commit -am "Release $version"
git tag -a "v$version" -m "Obsidia $version"
echo "Tagged v$version. Push with: git push origin main v$version"
