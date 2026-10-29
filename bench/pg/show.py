#!/usr/bin/env python3
"""Prints the last result line of a BQ1-pg JSONL file (progress on stderr)."""
import json
import sys

r = json.loads(open(sys.argv[1]).read().splitlines()[-1])
lat = r.get("latency_ms", {})
gw = f" gw_us/q={r['gateway_us_per_query']}" if r.get("gateway_us_per_query") else ""
print(f"bq1-pg: {r['label']:<26} {r['mode']:<13} {r['workload']:<11} c={r['clients']:<3} "
      f"tps={r['tps']:>9.0f} p50={lat.get('p50')} p99={lat.get('p99')} p99.9={lat.get('p99.9')}{gw}",
      file=sys.stderr)
