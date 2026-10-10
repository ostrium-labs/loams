#!/usr/bin/env bash
# Seed cargo's git database with shallow copies of the Neon fork's Postgres
# submodules (PG2 Task 31). Cargo checks out every submodule of a git
# dependency, and the fork's vendor/postgres-v14..v17 point at
# ostrium-labs/postgres, whose full history is several hundred MB. The
# decoder's build never reads them (postgres_ffi takes its headers from
# POSTGRES_INSTALL_DIR), so one commit each is enough: cargo finds the
# commits in its database and fetches nothing more.
#
#   scripts/pg2/seed-neon-submodules.sh crates/loams-wal-decoder/Cargo.lock
#
# The database directory name is cargo's hash of the submodule URL
# (postgres-3118924c31a8989c for https://github.com/ostrium-labs/postgres.git
# with cargo 1.97). If a cargo release names it differently, the seed is
# ignored and cargo fetches the full history: slower, still correct.
set -euo pipefail

lock=${1:?usage: seed-neon-submodules.sh <Cargo.lock>}
db=${CARGO_HOME:-$HOME/.cargo}/git/db/postgres-3118924c31a8989c
url=https://github.com/ostrium-labs/postgres.git

rev=$(sed -n 's#^source = "git+https://github.com/ostrium-labs/loams-postgres?rev=\([0-9a-f]\{40\}\)\#.*#\1#p' "$lock" | head -1)
[ -n "$rev" ] || { echo "seed: no ostrium-labs/loams-postgres rev in $lock" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git init -q --bare "$tmp/neon"
git -C "$tmp/neon" fetch -q --depth=1 --filter=blob:none https://github.com/ostrium-labs/loams-postgres "$rev"
mapfile -t subs < <(git -C "$tmp/neon" ls-tree "$rev" vendor/ | awk '$2 == "commit" { print $3 }')
[ "${#subs[@]}" -gt 0 ] || { echo "seed: no submodules at $rev" >&2; exit 1; }

[ -d "$db" ] || git init -q --bare "$db"
for sha in "${subs[@]}"; do
  if [ "$(git -C "$db" cat-file -t "$sha" 2>/dev/null)" != commit ]; then
    git -C "$db" fetch -q --depth=1 "$url" "+${sha}:refs/commit/${sha}"
  fi
  echo "seed: $sha"
done
