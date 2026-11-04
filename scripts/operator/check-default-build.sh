#!/usr/bin/env bash
# The default build of `loams` must not pull any operator crate (OP2-A Global
# Constraints, test `default_build_excludes_operator`). `cargo tree` reads
# metadata only; it builds nothing.
set -euo pipefail

cd "$(dirname "$0")/../.."

forbidden='loams-operator|loams-kube|loams-operator-api|loc-harness|loc-spec'
tree="$(cargo tree -p loams -e normal --prefix none --format '{p}')"

# Package names are the first word of each line.
hits="$(printf '%s\n' "$tree" | awk '{print $1}' | grep -E -x "$forbidden" | sort -u || true)"
if [ -n "$hits" ]; then
  echo "error: default build of loams pulls operator crates:" >&2
  printf '  %s\n' $hits >&2
  exit 1
fi
echo "ok: default build excludes operator crates"
