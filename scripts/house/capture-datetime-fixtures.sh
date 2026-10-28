#!/usr/bin/env bash
# Captures the date and time fixtures of the Rust engine (plan HE1 Task 30)
# from the pinned oracle, clickhouse-server 26.9.2.8:
#
#   datetime.tsv   <expression> TAB <toTypeName> TAB <value in TSV>
#
# for every line of datetime_inputs.txt. The test
# `datetime_functions_match_the_oracle` runs each through the engine.
#
#   LOAMS_ORACLE_URL=http://127.0.0.1:28123/ scripts/house/capture-datetime-fixtures.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fx="$root/fabric/crates/loams-house-engine/tests/fixtures"
url="${LOAMS_ORACLE_URL:-http://127.0.0.1:28123/}"
auth="oracle:${LOAMS_ORACLE_PASSWORD:-oracle}"
q() { curl -sS -u "$auth" "$url" --data-binary "$1"; }

version="$(q 'SELECT version()')"
[[ "$version" == 26.9.2.8 ]] || { echo "oracle is $version, not 26.9.2.8" >&2; exit 1; }

: >"$fx/datetime.tsv"
while IFS= read -r expr; do
  [[ -z "$expr" || "$expr" == \#* ]] && continue
  out="$(q "SELECT toTypeName($expr), $expr FORMAT TSV")"
  [[ "$out" == Code:* ]] && { echo "$expr: $out" >&2; exit 1; }
  printf '%s\t%s\n' "$expr" "$out" >>"$fx/datetime.tsv"
done <"$fx/datetime_inputs.txt"
echo "captured from clickhouse-server $version"
