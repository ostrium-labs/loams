#!/usr/bin/env bash
# Seed cargo's git database with shallow copies of Neon's Postgres
# submodules (PG2 Task 31). Cargo checks out every submodule of a git
# dependency, and Neon's vendor/postgres-v14..v17 point at
# neondatabase/postgres, whose full history is several hundred MB. The
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
url=https://github.com/neondatabase/postgres.git

rev=$(sed -n 's#^source = "git+https://github.com/neondatabase/neon?rev=\([0-9a-f]\{40\}\)\#.*#\1#p' "$lock" | head -1)
[ -n "$rev" ] || { echo "seed: no neondatabase/neon rev in $lock" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git init -q --bare "$tmp/neon"
git -C "$tmp/neon" fetch -q --depth=1 --filter=blob:none https://github.com/neondatabase/neon "$rev"
mapfile -t subs < <(git -C "$tmp/neon" ls-tree "$rev" vendor/ | awk '$2 == "commit" { print $3 }')
[ "${#subs[@]}" -gt 0 ] || { echo "seed: no submodules at $rev" >&2; exit 1; }

[ -d "$db" ] || git init -q --bare "$db"
# One fetch for every missing commit, with auto gc and maintenance off: a
# fetch per commit let the previous fetch's detached gc rewrite the shallow
# file under the next one ("shallow file has changed since we read it").
want=()
for sha in "${subs[@]}"; do
  if [ "$(git -C "$db" cat-file -t "$sha" 2>/dev/null)" != commit ]; then
    want+=("+${sha}:refs/commit/${sha}")
  fi
  echo "seed: $sha"
done
if [ "${#want[@]}" -gt 0 ]; then
  git -C "$db" -c gc.auto=0 -c maintenance.auto=false fetch -q --depth=1 "$url" "${want[@]}"
fi
