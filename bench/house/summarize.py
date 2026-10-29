#!/usr/bin/env python3
"""Summarise a BQ1-house run directory into one small JSON file.

Usage: summarize.py <run_dir> <out.json> [--label TEXT] [--meta k=v ...]

Reads oha's JSON outputs (http_*.json), `clickhouse benchmark` text outputs
(native_*.txt) and the native insert counters (native_ins_*.count/.time).
Latencies are in milliseconds.
"""

import glob
import json
import os
import re
import sys


def oha(path):
    with open(path) as f:
        d = json.load(f)
    s = d.get("summary", {})
    lat = d.get("latencyPercentiles", {})
    codes = d.get("statusCodeDistribution", {})
    ok = sum(v for k, v in codes.items() if str(k).startswith("2"))
    total = sum(codes.values()) if codes else 0
    ms = lambda v: None if v is None else round(v * 1000, 3)
    return {
        "qps": round(s.get("requestsPerSec", 0.0), 1),
        "ok": ok,
        "total": total,
        "errors": d.get("errorDistribution", {}),
        "p50_ms": ms(lat.get("p50")),
        "p99_ms": ms(lat.get("p99")),
        "p99_9_ms": ms(lat.get("p99.9")),
    }


def chbench(path):
    text = open(path).read()
    out = {}
    # One line per host in the final report (several with several --port):
    # the last line of each host, summed.
    per_host = {}
    for host, n, qps in re.findall(r"^(\S+?), queries:? (\d+), QPS: ([\d.]+)", text, re.M):
        per_host[host] = (int(n), float(qps))
    if per_host:
        out["queries"] = sum(n for n, _ in per_host.values())
        out["qps"] = round(sum(q for _, q in per_host.values()), 1)
        out["hosts"] = len(per_host)
    pct = {}
    for p, v in re.findall(r"^([\d.]+)%\s+([\d.]+) sec", text, re.M):
        pct[float(p)] = float(v) * 1000
    for key, p in (("p50_ms", 50.0), ("p99_ms", 99.0), ("p99_9_ms", 99.9)):
        if p in pct:
            out[key] = round(pct[p], 3)
    err = re.findall(r"(Code: \d+\..{0,160})", text)
    if err:
        out["error"] = err[0]
    if not out:
        out["error"] = text.strip()[-300:]
    return out


def native_inserts(run, batch_rows):
    out = {}
    for tf in glob.glob(os.path.join(run, "native_ins_*_c*.time")):
        stem = os.path.basename(tf)[: -len(".time")]
        t0, t1 = map(float, open(tf).read().split())
        n = 0
        for cf in glob.glob(os.path.join(run, stem + "_*.count")):
            try:
                n += int(open(cf).read().strip() or 0)
            except ValueError:
                pass
        secs = t1 - t0
        out[stem] = {
            "batches": n,
            "seconds": round(secs, 2),
            "batches_per_s": round(n / secs, 2) if secs else None,
            "rows_per_s": round(n * batch_rows / secs) if secs else None,
        }
    return out


def main():
    run, dest = sys.argv[1], sys.argv[2]
    label, meta = None, {}
    args = sys.argv[3:]
    while args:
        a = args.pop(0)
        if a == "--label":
            label = args.pop(0)
        elif a == "--meta":
            k, v = args.pop(0).split("=", 1)
            meta[k] = v
    batch_rows = 0
    br = os.path.join(run, "batch_rows")
    if os.path.exists(br):
        batch_rows = int(open(br).read().strip())
    results = {}
    for p in sorted(glob.glob(os.path.join(run, "http_*.json"))):
        r = oha(p)
        name = os.path.basename(p)[:-5]
        if name.startswith("http_ins_") and batch_rows:
            r["rows_per_s"] = round(r["qps"] * batch_rows)
        rows_file = p[:-5] + ".rows"
        if os.path.exists(rows_file):
            r["batch_rows"] = int(open(rows_file).read())
            r["rows_per_s"] = round(r["qps"] * r["ok"] / max(r["total"], 1) * r["batch_rows"])
        results[name] = r
    for p in sorted(glob.glob(os.path.join(run, "pipe_*.json"))):
        results[os.path.basename(p)[:-5]] = json.load(open(p))
    for p in sorted(glob.glob(os.path.join(run, "native_*.txt"))):
        results[os.path.basename(p)[:-4]] = chbench(p)
    if batch_rows:
        results.update(native_inserts(run, batch_rows))
        meta["batch_rows"] = str(batch_rows)
    doc = {"label": label, "meta": meta, "results": results}
    os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
    with open(dest, "w") as f:
        json.dump(doc, f, indent=1, sort_keys=True)
    print(json.dumps(doc, indent=1, sort_keys=True)[:4000])


if __name__ == "__main__":
    main()
