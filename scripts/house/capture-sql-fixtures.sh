#!/usr/bin/env bash
# Captures loams-house-sql's fixtures from the pinned oracle,
# clickhouse-server 26.9.2.8 (plan HE1 Task 22):
#
#   src/functions.tsv          system.functions: name, is_aggregate, case_insensitive, alias_to
#   tests/fixtures/names.tsv   <expression> TAB <its output column name>, for each line of name_inputs.txt
#
#   LOAMS_ORACLE_URL=http://127.0.0.1:28123/ scripts/house/capture-sql-fixtures.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
crate="$root/fabric/crates/loams-house-sql"
url="${LOAMS_ORACLE_URL:-http://127.0.0.1:28123/}"
export LOAMS_ORACLE_AUTH="oracle:${LOAMS_ORACLE_PASSWORD:-oracle}"
version="$(curl -sS -u "$LOAMS_ORACLE_AUTH" "$url" --data-binary 'SELECT version()')"
[[ "$version" == 26.9.2.8 ]] || { echo "oracle is $version, not 26.9.2.8" >&2; exit 1; }
curl -sS -u "$LOAMS_ORACLE_AUTH" "$url" --data-binary \
  "SELECT name, is_aggregate, case_insensitive, alias_to FROM system.functions ORDER BY name FORMAT TSV" \
  >"$crate/src/functions.tsv"
python3 -I - "$url" "$crate/tests/fixtures/name_inputs.txt" "$crate/tests/fixtures/names.tsv" <<'PY'
import base64, json, os, sys, urllib.request, urllib.error
url, inputs, out = sys.argv[1:4]
auth = base64.b64encode(os.environ["LOAMS_ORACLE_AUTH"].encode()).decode()
with open(out, "w") as f:
    for line in open(inputs):
        e = line.rstrip("\n")
        if not e:
            continue
        sql = f"SELECT {e} FROM numbers(1) FORMAT JSONCompactEachRowWithNames"
        req = urllib.request.Request(url + "?param_p=7", data=sql.encode(), headers={"Authorization": "Basic " + auth})
        try:
            name = json.loads(urllib.request.urlopen(req).read().decode().splitlines()[0])[0]
        except urllib.error.HTTPError:
            name = "ERROR"
        f.write(f"{e}\t{json.dumps(name)}\n")
PY
echo "captured from clickhouse-server $version"
