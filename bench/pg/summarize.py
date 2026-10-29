#!/usr/bin/env python3
"""One BQ1-pg run as a JSON line: pgbench's own TPS, the latency
percentiles from its sampled per-transaction log, and CPU use per core set.

pgbench's -l format: client_id transaction_no time script_no time_epoch
time_us [schedule_lag]; `time` is the transaction's latency in microseconds.
"""

import argparse
import glob
import json
import os
import re
import sys

STATEMENTS = {"select-only": 1, "tpcb": 7}


def percentile(sorted_vals, q):
    if not sorted_vals:
        return None
    k = (len(sorted_vals) - 1) * q
    lo = int(k)
    hi = min(lo + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (k - lo)


def latencies(log_dir):
    vals = []
    for path in glob.glob(os.path.join(log_dir, "bq1.*")):
        with open(path) as f:
            for line in f:
                parts = line.split()
                if len(parts) >= 3 and parts[2].isdigit():
                    vals.append(int(parts[2]))
    vals.sort()
    return vals


def summary_tps(text):
    m = re.search(r"^tps = ([0-9.]+) \(without initial connection time\)", text, re.M)
    if not m:
        sys.exit(f"summarize: no tps line in pgbench output:\n{text}")
    failed = re.search(r"^number of failed transactions: (\d+)", text, re.M)
    avg = re.search(r"^latency average = ([0-9.]+) ms", text, re.M)
    return float(m.group(1)), int(failed.group(1)) if failed else 0, float(avg.group(1)) if avg else None


def cpu(spec, duration, hz):
    # name=cores:busy0 total0:busy1 total1
    name, rest = spec.split("=", 1)
    cores, s0, s1 = rest.split(":")
    b0, t0 = map(int, s0.split())
    b1, t1 = map(int, s1.split())
    n = 0
    for part in cores.split(","):
        a, _, b = part.partition("-")
        n += int(b or a) - int(a) + 1
    # Busy CPU-seconds over the measured interval (pgbench's -T), so the
    # container start before it does not dilute the figure.
    busy_cores = (b1 - b0) / hz / duration
    return name, {"cores": cores, "n": n, "busy_cores": round(busy_cores, 3),
                  "busy_frac": round(busy_cores / n, 4)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--mode", required=True)
    ap.add_argument("--workload", required=True)
    ap.add_argument("--clients", type=int, required=True)
    ap.add_argument("--threads", type=int, required=True)
    ap.add_argument("--duration", type=float, required=True)
    ap.add_argument("--summary", required=True)
    ap.add_argument("--logs", required=True)
    ap.add_argument("--cpu", action="append", default=[])
    ap.add_argument("--gw-jiffies", default="0:0")
    ap.add_argument("--wall", required=True)
    ap.add_argument("--extra", default="{}")
    a = ap.parse_args()

    text = open(a.summary).read()
    tps, failed, avg = summary_tps(text)
    lat = latencies(a.logs)
    w0, w1 = map(float, a.wall.split(":"))
    hz = os.sysconf("SC_CLK_TCK")
    cpus = dict(cpu(s, a.duration, hz) for s in a.cpu)
    g0, g1 = map(int, a.gw_jiffies.split(":"))
    gw_cpu_s = (g1 - g0) / hz
    per_tx = STATEMENTS.get(a.workload) or int(a.workload.removeprefix("pipe"))
    qps = tps * per_tx
    rec = {
        "label": a.label,
        "mode": a.mode,
        "workload": a.workload,
        "clients": a.clients,
        "threads": a.threads,
        "duration_s": a.duration,
        "wall_s": round(w1 - w0, 2),
        "tps": round(tps, 1),
        "statements_per_tx": per_tx,
        "pipeline_depth": per_tx if a.workload.startswith("pipe") else 1,
        "qps": round(qps, 1),
        "failed_tx": failed,
        "latency_ms": {
            "avg": avg,
            "samples": len(lat),
            "p50": round(percentile(lat, 0.50) / 1000, 3) if lat else None,
            "p99": round(percentile(lat, 0.99) / 1000, 3) if lat else None,
            "p99.9": round(percentile(lat, 0.999) / 1000, 3) if lat else None,
            "max": round(lat[-1] / 1000, 3) if lat else None,
        },
        "cpu": cpus,
    }
    if gw_cpu_s > 0:
        # The gateway's own CPU (its process, not its cores), per query.
        rec["gateway_cpu_s"] = round(gw_cpu_s, 2)
        rec["gateway_cpu_cores"] = round(gw_cpu_s / a.duration, 3)
        rec["gateway_us_per_query"] = round(gw_cpu_s / (qps * a.duration) * 1e6, 2) if qps else None
    pg = cpus.get("pg")
    if pg and pg["busy_cores"] > 0:
        rec["qps_per_pg_core"] = round(qps / pg["busy_cores"], 1)
    rec.update(json.loads(a.extra))
    print(json.dumps(rec, sort_keys=True))


if __name__ == "__main__":
    main()
