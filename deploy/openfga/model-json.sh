#!/usr/bin/env bash
# Model v1 (fga.mod: core.fga and graph.fga) as the JSON that OpenFGA's write API takes, for
# the release bundle's openfga-init (deploy/release/config/openfga-model.json). Uses the pinned
# OpenFGA CLI (conformance/identity/fga.sh) and jq.
#
#   deploy/openfga/model-json.sh           # regenerate
#   deploy/openfga/model-json.sh --check   # fail when the committed JSON is stale (CI)
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/deploy/release/config/openfga-model.json"
fresh=$("$root/conformance/identity/fga.sh" model transform --file /model/fga.mod --output-format json | jq .)
if [ "${1:-}" = --check ]; then
  if ! diff -u <(jq -S . "$out") <(jq -S . <<<"$fresh"); then
    echo "deploy/release/config/openfga-model.json is stale: run deploy/openfga/model-json.sh" >&2
    exit 1
  fi
  exit 0
fi
printf '%s\n' "$fresh" > "$out"
