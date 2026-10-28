#!/usr/bin/env bash
# Captures the engine's string and hash fixtures from the pinned oracle,
# clickhouse-server 26.9.2.8 (plan HE1 Task 31):
#
#   tests/fixtures/t31_oracle.jsonl  {"query", "output"} per query of t31_queries.sql,
#                                    output in JSONCompactEachRowWithNamesAndTypes
#
#   LOAMS_ORACLE_URL=http://127.0.0.1:28123/ scripts/house/capture-t31-fixtures.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dir="$root/fabric/crates/loams-house-engine/tests/fixtures"
url="${LOAMS_ORACLE_URL:-http://127.0.0.1:28123/}"
export LOAMS_ORACLE_AUTH="oracle:${LOAMS_ORACLE_PASSWORD:-oracle}"
version="$(curl -sS -u "$LOAMS_ORACLE_AUTH" "$url" --data-binary 'SELECT version()')"
[[ "$version" == 26.9.2.8 ]] || { echo "oracle is $version, not 26.9.2.8" >&2; exit 1; }
python3 -I - "$url" "$dir/t31_queries.sql" "$dir/t31_oracle.jsonl" <<'PY'
import base64, json, os, sys, urllib.request
url, inputs, out = sys.argv[1:4]
auth = base64.b64encode(os.environ["LOAMS_ORACLE_AUTH"].encode()).decode()
with open(out, "w") as f:
    for line in open(inputs, encoding="utf-8"):
        q = line.rstrip("\n")
        if not q or q.startswith("--"):
            continue
        req = urllib.request.Request(
            url + "?default_format=JSONCompactEachRowWithNamesAndTypes",
            data=q.encode(), headers={"Authorization": "Basic " + auth})
        output = urllib.request.urlopen(req).read().decode()
        f.write(json.dumps({"query": q, "output": output}, ensure_ascii=False) + "\n")
PY
echo "captured from clickhouse-server $version"
