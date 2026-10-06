#!/usr/bin/env bash
# Prints the SHA-256 of the vendored proto tree, the value conformance/proto-ref.lock
# records as tree_sha256. The algorithm, which the Android and iOS tests reimplement:
# for every *.proto under proto/, sorted by path in byte order, the line
# "<sha256 hex of the file>  <path relative to the repository root>\n"; then the
# SHA-256 of all lines concatenated.
set -euo pipefail
cd "$(dirname "$0")/.."
find proto -type f -name '*.proto' | LC_ALL=C sort | while read -r f; do
  printf '%s  %s\n' "$(sha256sum "$f" | cut -d' ' -f1)" "$f"
done | sha256sum | cut -d' ' -f1
