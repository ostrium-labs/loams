#!/usr/bin/env bash
# The Rust engine against the oracle (plan HE1 Task 23, D1804): builds
# `loams-house-engine-dev` and `loams-house-mine`, starts the dev server on
# 127.0.0.1:28126, runs every HE1 corpus (`--case he1-`) on the pinned
# clickhouse-server 26.9.2.8 and on the engine, then stops the server.
#
#   scripts/house/oracle.sh up                 # the baseline, once (28123)
#   scripts/house/engine-oracle.sh             # writes the triage
#   scripts/house/engine-oracle.sh --check --task 24   # what CI and a task's exit run
#
# Extra arguments go to `loams-house-mine oracle`. Heavy builds on a shared
# machine: BUILD_WRAPPER=~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root/fabric"
${BUILD_WRAPPER:-} cargo build --locked -j "${JOBS:-4}" -p loams-house-engine -p loams-house-mine --bin loams-house-engine-dev --bin loams-house-mine
target="$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug"
cd "$root"
export LOAMS_ORACLE_PASSWORD="${LOAMS_ORACLE_PASSWORD:-oracle}"
curl -fsS http://127.0.0.1:28123/ping >/dev/null || { echo "the oracle is not up on 127.0.0.1:28123 (scripts/house/oracle.sh up)" >&2; exit 1; }
# Task 26's lake fixtures shadow the seeded copies of the same tables.
lakes=()
for dir in "$root"/conformance/clickhouse/oracle/corpus/he1/*/lake; do
  [ -d "$dir" ] && lakes+=(--lake "$dir")
done
"$target/loams-house-engine-dev" --listen 127.0.0.1:28126 "${lakes[@]}" >"${TMPDIR:-/tmp}/loams-house-engine-dev.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 30); do curl -fsS http://127.0.0.1:28126/ping >/dev/null 2>&1 && break; sleep 1; done
"$target/loams-house-mine" oracle --only house-rust --case he1- "$@"
