#!/usr/bin/env bash
# Captures the ClickHouse type fixtures of loams-house-types (plan HE1 Task
# 21) from the pinned oracle, clickhouse-server 26.9.2.8 (R0.3):
#
#   data_type_families.tsv  system.data_type_families (names, case rule, alias)
#   type_names.tsv          <input> TAB <canonical toTypeName, \n as \\n> | ERROR <code>
#   defaults.tsv            <type> TAB <defaultValueOfTypeName in TSV>
#   values.tsv              <id> TAB <style> TAB <output of the case, as JSON>
#
#   LOAMS_ORACLE_URL=http://127.0.0.1:28123/ scripts/house/capture-type-fixtures.sh
#
# The server is the oracle compose's (`scripts/house/oracle.sh up`).
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
crate="$root/fabric/crates/loams-house-types"
fx="$crate/tests/fixtures"
url="${LOAMS_ORACLE_URL:-http://127.0.0.1:28123/}"
auth="oracle:${LOAMS_ORACLE_PASSWORD:-oracle}"
q() { curl -sS -u "$auth" "$url" --data-binary "$1"; }

version="$(q 'SELECT version()')"
[[ "$version" == 26.9.2.8 ]] || { echo "oracle is $version, not 26.9.2.8" >&2; exit 1; }

q "SELECT name, case_insensitive, alias_to FROM system.data_type_families ORDER BY name FORMAT TSV" \
  >"$crate/src/data_type_families.tsv"

sql_quote() { python3 -I -c 'import sys; s=sys.argv[1]; print("\x27"+s.replace("\\","\\\\").replace("\x27","\\\x27")+"\x27")' "$1"; }

: >"$fx/type_names.tsv"
: >"$fx/defaults.tsv"
while IFS= read -r t; do
  [[ -z "$t" ]] && continue
  lit="$(sql_quote "$t")"
  out="$(q "SELECT toTypeName(defaultValueOfTypeName($lit)) FORMAT TSVRaw" 2>&1 || true)"
  if [[ "$out" == Code:* ]]; then
    code="${out#Code: }"; code="${code%%.*}"
    printf '%s\tERROR %s\n' "$t" "$code" >>"$fx/type_names.tsv"
    continue
  fi
  printf '%s\t%s\n' "$t" "$(printf '%s' "$out" | python3 -I -c 'import sys; print(sys.stdin.read().replace("\n","\\n"), end="")')" >>"$fx/type_names.tsv"
  d="$(q "SELECT defaultValueOfTypeName($lit) FORMAT TSV" 2>/dev/null || true)"
  [[ "$d" == Code:* ]] || printf '%s\t%s\n' "$t" "$d" >>"$fx/defaults.tsv"
done <"$fx/type_inputs.txt"

: >"$fx/values.tsv"
while IFS=$'\t' read -r id expr; do
  [[ -z "$id" || "$id" == \#* ]] && continue
  for style in TSV CSV JSONEachRow Values; do
    # Piped, not substituted: bash drops NUL bytes (a FixedString's padding).
    out="$(q "SELECT $expr AS v FORMAT $style" | python3 -I -c 'import sys,json; print(json.dumps(sys.stdin.buffer.read().decode("utf-8", "surrogateescape")), end="")')"
    printf '%s\t%s\t%s\n' "$id" "$style" "$out" >>"$fx/values.tsv"
  done
done <"$fx/value_cases.tsv"
echo "captured from clickhouse-server $version"
