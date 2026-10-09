#!/usr/bin/env bash
# no_old_fork_url_left (NF1 Task 1b): check-names.sh passes on the repository,
# fails on a tree that names the old crate, URL or stack, and allows the
# compatibility names.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
check=$here/../check-names.sh
repo=$(cd "$here/../../.." && pwd)

"$check" "$repo"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fails() {
  rm -rf "$tmp/t" && mkdir -p "$tmp/t" && printf '%s\n' "$1" >"$tmp/t/f"
  if "$check" "$tmp/t" >/dev/null 2>&1; then
    echo "FAIL: check-names accepted: $1" >&2
    exit 1
  fi
}
passes() {
  rm -rf "$tmp/t" && mkdir -p "$tmp/t" && printf '%s\n' "$1" >"$tmp/t/f"
  "$check" "$tmp/t" >/dev/null || { echo "FAIL: check-names refused: $1" >&2; exit 1; }
}

fails 'cargo test -p loams-neon --locked'
fails 'members = ["crates/loams-neon"]'
fails 'use loams_neon::pageserver;'
fails '(cd deploy/neon && docker compose up -d)'
fails 'git = "https://github.com/ostrium-labs/neon", rev = "1218fb7a"'
passes 'name: loams-neon'
passes '		dir: "neon",'
passes '"/res/stacks/neon/compose.yaml"'
passes 'git = "https://github.com/ostrium-labs/loams-postgres"'
passes 'deploy/loams-postgres-dev and crates/loams-postgres'
echo "test_check_names: ok"
