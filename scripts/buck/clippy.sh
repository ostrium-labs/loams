#!/usr/bin/env bash
# Clippy with Buck2, as `cargo clippy --all-targets -- -D warnings` runs it:
#
#   scripts/buck/clippy.sh [target patterns...]    default: every crate
#
# Builds each crate's `<package>-clippy` target (buck/rust.bzl), which holds
# the clippy diagnostics of all its targets, prints every non-empty one and
# exits 1 if there are any. Clippy's own exit status is not used: Buck2 keeps
# lint output as an artifact so that a cached run reports it too.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo"
buck2=${BUCK2:-$repo/.tools/bin/buck2}

patterns=("$@")
[[ ${#patterns[@]} -eq 0 ]] && patterns=(//...)
mapfile -t targets < <("$buck2" uquery "kind(filegroup, attrregexfilter(name, '-clippy\$', set(${patterns[*]})))" 2> /dev/null)
if [[ ${#targets[@]} -eq 0 ]]; then
  echo "clippy.sh: no -clippy targets in ${patterns[*]}" >&2
  exit 2
fi
out=$("$buck2" build "${targets[@]}" --show-full-simple-output ${BUCK2_ARGS:-})
status=0
for dir in $out; do
  for f in "$dir"/*.clippy.txt; do
    if [[ -s "$f" ]]; then
      cat "$f"
      status=1
    fi
  done
done
if [[ $status -ne 0 ]]; then
  echo "clippy.sh: clippy reported diagnostics (above)" >&2
else
  echo "clippy.sh: ${#targets[@]} crates clean"
fi
exit $status
