#!/bin/bash
# Cut a release: scripts/release.sh 0.2.0
# Bumps the app version, commits, tags v0.2.0 and pushes; GitHub Actions builds and publishes it.
set -euo pipefail
cd "$(dirname "$0")/.."

new=${1:?usage: scripts/release.sh X.Y.Z}
[[ $new =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first"; exit 1; }
git rev-parse -q --verify "refs/tags/v$new" >/dev/null && { echo "v$new already exists"; exit 1; }

sed -i '' "s/^version = \".*\"/version = \"$new\"/" crates/yamc/Cargo.toml
cargo update -w --offline -q
git commit -qam "Release $new"
git tag "v$new"
git push -q origin HEAD "v$new"
echo "Pushed v$new; the build shows up under Actions, then Releases."
