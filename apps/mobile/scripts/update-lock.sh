#!/usr/bin/env bash
# Rewrites tree_sha256 in conformance/proto-ref.lock after proto/ changed.
set -euo pipefail
cd "$(dirname "$0")/.."
sed -i.bak "s/^tree_sha256=.*/tree_sha256=$(scripts/proto-hash.sh)/" conformance/proto-ref.lock
rm -f conformance/proto-ref.lock.bak
grep '^tree_sha256=' conformance/proto-ref.lock
