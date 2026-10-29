#!/usr/bin/env python3
"""Turn an iggy-bench log into one JSON line: python3 parse_bench.py <label> <logfile>"""
import json, re, sys
label, path = sys.argv[1], sys.argv[2]
txt = re.sub(r"\x1b\[[0-9;]*m", "", open(path, errors="replace").read())
out = {"label": label}
for m in re.finditer(r"(Producers|Consumers) Results: Total throughput: ([\d.]+) MB/s, (\d+) messages/s.*?p50 latency: ([\d.]+) ms.*?p99 latency: ([\d.]+) ms, p999 latency: ([\d.]+) ms.*?total time: ([\d.]+) s", txt):
    k = m.group(1).lower()
    out[k] = {"mb_per_s": float(m.group(2)), "msgs_per_s": int(m.group(3)), "p50_ms": float(m.group(4)),
              "p99_ms": float(m.group(5)), "p999_ms": float(m.group(6)), "seconds": float(m.group(7))}
b = re.search(r"Benchmark: (.*?)\n", txt)
if b: out["benchmark"] = b.group(1)
print(json.dumps(out))
