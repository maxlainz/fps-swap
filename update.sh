#!/bin/bash
# Internal: installs or updates fps-swap to the latest GitHub release. No sudo.
#   curl -fsSL https://raw.githubusercontent.com/maxlainz/fps-swap/main/update.sh | bash
set -euo pipefail

repo="maxlainz/fps-swap"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# FPS_SWAP_ZIP_URL replaces the release zip URL (testing, e.g. file:///path/fps-swap-vX-macos-universal.zip).
url=${FPS_SWAP_ZIP_URL:-}
if [ -z "$url" ]; then
  json=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest") || {
    echo "error: could not fetch the latest release of $repo (no network, or no release published yet)" >&2
    exit 1
  }
  tag=$(printf '%s\n' "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  if [ -z "$tag" ]; then
    echo "error: no release tag in the GitHub API response for $repo" >&2
    exit 1
  fi
  url="https://github.com/$repo/releases/download/$tag/fps-swap-$tag-macos-universal.zip"
fi

curl -fsSL -o "$tmp/release.zip" "$url" || {
  echo "error: could not download $url" >&2
  exit 1
}
ditto -x -k "$tmp/release.zip" "$tmp/x"

pkg=$(echo "$tmp"/x/fps-swap-*-macos-universal)
if [ ! -f "$pkg/install.sh" ]; then
  echo "error: unexpected zip layout (no fps-swap-*-macos-universal/install.sh)" >&2
  exit 1
fi

bash "$pkg/install.sh"
tag=${pkg##*/fps-swap-}
echo "fps-swap ${tag%-macos-universal} installed."
