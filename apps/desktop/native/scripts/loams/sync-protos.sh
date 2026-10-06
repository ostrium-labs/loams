#!/usr/bin/env bash
# Copies the protos the desktop uses from a checkout of the Loams repository
# into crates/loams-desktop-link/proto and records the ref in proto/PIN.
# Usage: scripts/loams/sync-protos.sh /path/to/loams-checkout
set -euo pipefail
src=${1:?usage: sync-protos.sh /path/to/loams-checkout}
root=$(cd "$(dirname "$0")/../.." && pwd)
dest="$root/crates/loams-desktop-link/proto"
protos=(loams/instance/v1/instance.proto loams/errors/v1/errors.proto)
for p in "${protos[@]}"; do
  mkdir -p "$dest/$(dirname "$p")"
  cp "$src/proto/$p" "$dest/$p"
done
printf 'repo: ostrium-labs/loams\nref: %s\nfiles: %s\n' \
  "$(git -C "$src" rev-parse HEAD)" "${protos[*]}" > "$dest/PIN"
"$root/scripts/loams/gen-protos.sh"
echo "synced; review the diff and commit"
