#!/bin/sh
# Bump version, commit, tag.
#
# Usage:
#   ./scripts/release.sh 0.2.0
#
# Touches:
#   desktop/src-tauri/Cargo.toml          version = "..."
#   desktop/src-tauri/tauri.conf.json     "version": "..."
#   desktop/src-tauri/Cargo.lock          via `cargo check`
#
# Then commits + tags `vX.Y.Z` locally. Does NOT push — review the diff
# first and push manually:
#   git push origin <branch> vX.Y.Z

set -eu

if [ $# -ne 1 ]; then
    echo "usage: $0 <new-version>     e.g. $0 0.2.0" >&2
    exit 2
fi

new="$1"
# Mirror release.ps1's regex: X.Y.Z plus optional pre-release / build suffix
# (the workflow trigger `tags: ['v*']` already supports e.g. v0.2.0-rc.1).
if ! printf '%s' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.+].+)?$'; then
    echo "version must look like X.Y.Z (got '$new')" >&2
    exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo_toml="$root/desktop/src-tauri/Cargo.toml"
tauri_conf="$root/desktop/src-tauri/tauri.conf.json"

# Refuse if the working tree has uncommitted work — the bump commit below
# would otherwise sweep unrelated changes into the release.
if ! git -C "$root" diff --quiet || ! git -C "$root" diff --cached --quiet; then
    echo "working tree has uncommitted changes; commit or stash first" >&2
    exit 1
fi

# Refuse if the tag already exists; otherwise `git tag` fails AFTER the
# bump commit lands, leaving an orphan commit to clean up by hand.
if [ -n "$(git -C "$root" tag -l "v$new")" ]; then
    echo "tag v$new already exists" >&2
    exit 1
fi

# perl is on macOS / every Linux distro / Git-Bash on Windows; avoids the
# `sed -i ''` macOS-vs-GNU footgun.
perl -pi -e 's/^version = ".*"/version = "'"$new"'"/' "$cargo_toml"
perl -pi -e 's/"version":\s*".*"/"version": "'"$new"'"/' "$tauri_conf"

# Refresh Cargo.lock so the commit is self-consistent.
( cd "$root/desktop/src-tauri" && cargo check )

git -C "$root" add \
    desktop/src-tauri/Cargo.toml \
    desktop/src-tauri/tauri.conf.json \
    desktop/src-tauri/Cargo.lock
git -C "$root" commit -m "chore: release v$new"
git -C "$root" tag "v$new"

branch="$(git -C "$root" rev-parse --abbrev-ref HEAD)"
cat <<EOF

release v$new staged on branch '$branch'.
to publish (triggers .github/workflows/release.yml):
    git push origin $branch v$new
EOF
