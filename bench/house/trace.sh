#!/usr/bin/env bash
# Diagnosis: where a point lookup's time goes, from the House's own spans
# (house.query > house.admission, house.worker per worker execution,
# house.cache.read per object read). Not a reported number.
source "$(dirname "$0")/lib.sh"
out="$DATA/runs/trace"; rm -rf "$out" "$DATA/tracenode"; mkdir -p "$out"
python3 "$HERE/otlp_sink.py" 38318 "$out/spans.jsonl" & sink=$!
OTLP_ENDPOINT=http://127.0.0.1:38318 pid=$(OTLP_ENDPOINT=http://127.0.0.1:38318 start_house tracenode "$SERVER_CORES" 38123 39000 38125 "${WORKERS:-2}")
trap 'stop_house "$pid"; kill $sink' EXIT
q tracenode 38123 "${DDL:-CREATE TABLE kv (id UInt64, v UInt64, s String) ENGINE = MergeTree ORDER BY id}" > /dev/null
q tracenode 38123 "INSERT INTO kv FORMAT Native" "$DATA/kv.native" > /dev/null
sleep 2; : > "$out/spans.jsonl"
for i in $(seq 1 ${N:-20}); do q tracenode 38123 "${SQL:-SELECT v, s FROM kv WHERE id = 424242}" > /dev/null; done
sleep 6
stop_house "$pid"; kill $sink; trap - EXIT
python3 - "$out/spans.jsonl" <<'PY'
import json, sys, collections
spans = [json.loads(l) for l in open(sys.argv[1])]
by_trace = collections.defaultdict(list)
for s in spans: by_trace[s["trace"]].append(s)
agg = collections.defaultdict(list)
for t, ss in by_trace.items():
    root = [s for s in ss if s["name"] == "house.query"]
    if not root: continue
    r = root[0]
    agg["total"].append((r["end"] - r["start"]) / 1e6)
    for s in ss:
        if s is r: continue
        key = s["name"] + " " + str({k: v for k, v in s["attrs"].items() if k in ("house.execution", "house.kind", "kind", "house.object", "house.phase")})
        agg[key].append((s["end"] - s["start"]) / 1e6)
for k, v in sorted(agg.items(), key=lambda kv: -sum(kv[1])):
    v.sort()
    print(f"{k[:110]:110s} n={len(v):4d} median={v[len(v)//2]:8.2f} ms sum={sum(v):9.1f}")
print("attribute keys seen:", sorted({k for s in spans for k in s["attrs"]}))
PY
