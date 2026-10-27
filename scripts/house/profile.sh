#!/usr/bin/env bash
# Regenerates the House compatibility profile from every committed inventory
# (plan HE1 Tasks 2, 5 and 8): `merge`, then `report` (profile.toml and
# docs/house/compat-profile.md), then `matrix` (docs/house/compat-matrix.md).
# The three outputs are generated: a mining PR re-runs this script, and a
# conflict in them is resolved by re-running it, never by hand.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
ranking="$(mktemp -d)/ranking.json"
mine() { cargo run --quiet --manifest-path fabric/Cargo.toml -p loams-house-mine -- "$@"; }
mine merge conformance/clickhouse/inventory/*.json --out "$ranking"
mine report --ranking "$ranking"
mine matrix
rm -rf "$(dirname "$ranking")"
