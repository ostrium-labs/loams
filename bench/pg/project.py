#!/usr/bin/env python3
"""BQ1-pg: tables and the 1B QPS projection from bench/results/bq1/pg/*.jsonl.

  bench/pg/project.py [--results DIR] [--json OUT]

Prints markdown (the report's measured tables and the projection) and, with
--json, writes the projection's inputs and outputs. The projection is
arithmetic on the measured numbers and the assumptions below; it is
"projected, not measured".
"""

import argparse
import glob
import json
import os
import statistics

TARGET_QPS = 1_000_000_000
# Assumptions (written into the report with the result).
NODE_CORES = 64          # cores per server in the projected fleet
TARGET_UTIL = 0.6        # run each tier at 60% CPU (headroom for p99 and failover)


def load(results):
    runs = []
    for path in sorted(glob.glob(os.path.join(results, "*.jsonl"))):
        with open(path) as f:
            lines = [json.loads(line) for line in f if line.strip()]
        meta = lines[0] if lines and lines[0].get("meta") else {}
        for r in lines[1:] if meta else lines:
            r["_file"] = os.path.basename(path)
            r["_session"] = meta.get("session")
            runs.append(r)
    return runs


def pick(runs, label, mode=None):
    """Every run of a label (and mode), for medians over repeats."""
    return [r for r in runs if r.get("label") == label and (mode is None or r.get("mode") == mode)]


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def fmt(x, nd=0):
    if x is None:
        return "-"
    return f"{x:,.{nd}f}"


def row_paths(runs, label, modes):
    out = []
    for m in modes:
        rs = pick(runs, label, m)
        if not rs:
            continue
        out.append({
            "mode": m,
            "n": len(rs),
            "qps": med([r["qps"] for r in rs]),
            "tps": med([r["tps"] for r in rs]),
            "p50": med([r["latency_ms"]["p50"] for r in rs]),
            "p99": med([r["latency_ms"]["p99"] for r in rs]),
            "p999": med([r["latency_ms"]["p99.9"] for r in rs]),
            "pg_cores": med([r["cpu"]["pg"]["busy_cores"] for r in rs]),
            "gw_us": med([r.get("gateway_us_per_query") for r in rs]),
            "gw_cores": med([r.get("gateway_cpu_cores") for r in rs]),
            "bg": med([r.get("background_busy_cores") for r in rs]),
        })
    return out


def table(rows, title):
    print(f"\n{title}\n")
    print("| path | runs | QPS | p50 ms | p99 ms | p99.9 ms | server cores busy | gateway cores | gateway µs/query | background cores |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        print(f"| {r['mode']} | {r['n']} | {fmt(r['qps'])} | {fmt(r['p50'], 3)} | {fmt(r['p99'], 3)} | {fmt(r['p999'], 3)} "
              f"| {fmt(r['pg_cores'], 2)} | {fmt(r['gw_cores'], 2)} | {fmt(r['gw_us'], 1)} | {fmt(r['bg'], 2)} |")


def main():
    ap = argparse.ArgumentParser()
    here = os.path.dirname(os.path.abspath(__file__))
    ap.add_argument("--results", default=os.path.join(here, "..", "results", "bq1", "pg"))
    ap.add_argument("--json")
    a = ap.parse_args()
    runs = load(a.results)
    modes = ["plain", "tls", "gw-splice", "gw-terminate"]

    sel = row_paths(runs, "paths-select-c64", modes)
    table(sel, "Select-only, 64 clients (stock Postgres 17.10, 6 cores)")
    table(row_paths(runs, "paths-select-c1", modes), "Select-only, 1 client (latency of the hop)")
    table(row_paths(runs, "paths-tpcb-c32", modes), "TPC-B, 32 clients (QPS = 7 statements per transaction)")
    for scale in (10, 50):
        rows = []
        for lab in (f"compute-select-c64-s{scale}", f"compute-select-c1-s{scale}", f"compute-tpcb-c32-s{scale}"):
            for r in row_paths(runs, lab, ["plain"]):
                r["mode"] = lab
                rows.append(r)
        if rows:
            table(rows, f"Loams Postgres compute (compute-v17), scale {scale}")

    # Scaling.
    print("\nScaling\n")
    print("| series | 1 | 2 | 4 | efficiency at 4 |")
    print("|---|---|---|---|---|")
    series = {}
    for name, lab in (("one server, cores", "pg-cores-{n}"), ("independent 1-core servers (shards)", "shards-{n}")):
        vals = {n: med([r["qps"] for r in pick(runs, lab.format(n=n))]) for n in (1, 2, 4)}
        eff = vals[4] / (4 * vals[1]) if vals.get(1) and vals.get(4) else None
        series[name] = {"qps": vals, "efficiency_4": eff}
        print(f"| {name} | {fmt(vals[1])} | {fmt(vals[2])} | {fmt(vals[4])} | {fmt(eff * 100 if eff else None, 0)}% |")
    for mode in ("gw-splice", "gw-terminate"):
        vals = {n: med([r["qps"] for r in pick(runs, f"gw-cores-{n}", mode)]) for n in (1, 2, 4)}
        us = {n: med([r.get("gateway_us_per_query") for r in pick(runs, f"gw-cores-{n}", mode)]) for n in (1, 2, 4)}
        series[f"gateway {mode}"] = {"qps": vals, "us_per_query": us}
        print(f"| gateway {mode}, QPS (µs CPU/query) | {fmt(vals[1])} ({fmt(us[1], 1)}) | {fmt(vals[2])} ({fmt(us[2], 1)}) | {fmt(vals[4])} ({fmt(us[4], 1)}) | - |")

    # Projection.
    def by(mode, label="paths-select-c64"):
        return next((r for r in sel if r["mode"] == mode), None) if label == "paths-select-c64" else None

    tls = by("tls")
    splice = by("gw-splice")
    term = by("gw-terminate")
    shard1 = series["independent 1-core servers (shards)"]["qps"].get(1)
    shard_eff = series["independent 1-core servers (shards)"]["efficiency_4"]
    per_core = None
    if tls and tls["pg_cores"]:
        per_core = tls["qps"] / tls["pg_cores"]
    proj = {"target_qps": TARGET_QPS, "node_cores": NODE_CORES, "target_util": TARGET_UTIL,
            "pg_qps_per_busy_core_tls": per_core, "shard_qps_1core": shard1, "shard_efficiency_4": shard_eff,
            "gateway_us_per_query_splice": splice and splice["gw_us"],
            "gateway_us_per_query_terminate": term and term["gw_us"]}
    if per_core:
        eff = shard_eff or 1.0
        pg_cores = TARGET_QPS / (per_core * min(eff, 1.0) * TARGET_UTIL)
        proj["pg_cores"] = pg_cores
        proj["pg_nodes"] = pg_cores / NODE_CORES
    for key, r in (("splice", splice), ("terminate", term)):
        if r and r["gw_us"]:
            cores = TARGET_QPS * r["gw_us"] / 1e6 / TARGET_UTIL
            proj[f"gateway_cores_{key}"] = cores
            proj[f"gateway_nodes_{key}"] = cores / NODE_CORES
    print("\nProjection (projected, not measured)\n")
    for k, v in proj.items():
        print(f"- {k}: {fmt(v, 2) if isinstance(v, float) else v}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"projection": proj, "series": series}, f, indent=2, sort_keys=True, default=str)


if __name__ == "__main__":
    main()
