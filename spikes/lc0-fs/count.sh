#!/usr/bin/env bash
# Summarise the counting proxy's log between two epoch timestamps.
# Usage: count.sh <proxy.jsonl> <from-epoch> [<to-epoch>]
# Writes = PUT/POST/PATCH/DELETE to *.drive9.ai (POST fs:batch-stat is a
# read); reads = GET/HEAD. Object-store part uploads go direct to S3 and
# show only as CONNECTs.
# Long-lived event streams (/v1/events*, SSE) are listed apart.
set -euo pipefail
log=$1 from=$2 to=${3:-9999999999}
python3 -I - "$log" "$from" "$to" <<'PY'
import json, sys, collections
log, lo, hi = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
w = collections.Counter(); r = collections.Counter(); other = collections.Counter()
wbytes = 0
for line in open(log):
    d = json.loads(line)
    if not (lo <= d["t"] <= hi):
        continue
    m = d["method"]
    if m == "CONNECT":
        other[f"CONNECT {d['host']}"] += 1
        continue
    p = d["path"]
    # group by the API family, not the file path
    parts = p.split("/")
    fam = "/".join(parts[:3]) if len(parts) > 2 else p
    if m in ("PUT", "POST", "PATCH", "DELETE") and not p.endswith(":batch-stat"):
        w[f"{m} {fam}"] += 1; wbytes += d.get("reqBytes", 0)
    else:
        r[f"{m} {fam}"] += 1
print(json.dumps({"writes": sum(w.values()), "reads": sum(r.values()),
                  "write_bytes": wbytes, "writes_by_kind": dict(w.most_common()),
                  "reads_by_kind": dict(r.most_common()), "other": dict(other)}, indent=1))
PY
