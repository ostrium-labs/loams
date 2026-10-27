#!/usr/bin/env bash
# Fetches <owner/repo> at <commit> into <dest>, unmodified, from GitHub's
# archive (no clone, nothing kept in this repository).
# Usage: fetch.sh <owner/repo> <commit> <dest>
set -euo pipefail
repo="$1" commit="$2" dest="$3"
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "fetch: $commit is not a full commit" >&2; exit 2; }
mkdir -p "$dest"
curl -fsSL --retry 3 "https://codeload.github.com/$repo/tar.gz/$commit" |
  tar -xz --strip-components=1 -C "$dest"
echo "fetch: $repo@$commit -> $dest"
