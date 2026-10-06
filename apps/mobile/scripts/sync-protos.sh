#!/usr/bin/env bash
# Vendors the app protos from the main repository at a git ref, then updates
# conformance/proto-ref.lock and regenerates every client.
#
#   scripts/sync-protos.sh <ref> [repo-url]
#
# <ref> is a commit, tag or branch (a tag or commit for anything merged to main).
# The default repo is the lock's `repo`. Only the packages in the lock's
# `packages` line are copied; the rest of upstream proto/ (Live, Stream) is
# not used by the phones (design §37 §8.1). A package listed in `placeholder`
# that now exists upstream replaces the local placeholder and leaves that list.
#
# The clone goes to .cache/upstream inside this repository (gitignored), never /tmp.
set -euo pipefail
cd "$(dirname "$0")/.."
lock=conformance/proto-ref.lock
ref=${1:?usage: scripts/sync-protos.sh <ref> [repo-url]}
repo=${2:-$(sed -n 's/^repo=//p' "$lock")}
dir=.cache/upstream
if [[ ! -d "$dir/.git" ]]; then
  git clone --filter=blob:none --no-checkout "$repo" "$dir"
fi
git -C "$dir" fetch --quiet origin "$ref"
sha=$(git -C "$dir" rev-parse FETCH_HEAD)
read -r -a packages <<<"$(sed -n 's/^packages=//p' "$lock")"
read -r -a placeholders <<<"$(sed -n 's/^placeholder=//p' "$lock")"
still=()
for p in "${packages[@]}"; do
  if git -C "$dir" cat-file -e "$sha:proto/$p" 2>/dev/null; then
    rm -rf "proto/$p"
    mkdir -p "proto/$p"
    git -C "$dir" archive "$sha" "proto/$p" | tar -x -C .
    echo "vendored $p from $sha"
  elif [[ " ${placeholders[*]} " == *" $p "* ]]; then
    still+=("$p")
    echo "kept placeholder $p (not upstream at $sha)"
  else
    echo "error: $p is neither upstream at $sha nor a placeholder" >&2
    exit 1
  fi
done
sed -i.bak -e "s|^ref=.*|ref=$sha|" -e "s|^placeholder=.*|placeholder=${still[*]:-}|" "$lock"
rm -f "$lock.bak"
scripts/update-lock.sh
scripts/generate.sh
