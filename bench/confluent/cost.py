#!/usr/bin/env python3
"""Per-test cost estimate for the CFB run (writes results/<day>/cost.json for report.py).

Upper bound, the same rule as the guard: every started hour of a resource at its
cap (max eCKU, CKU or max CFU) at list price (Dedicated: the assumed $4.00/CKU-hour),
plus data at $0.05/GB from the Metrics API (bytes per cluster in ~/.cfb-state/data_bytes).
The Costs API (72 h lag) replaces this when it lands.

  cost.py [--day 2026-10-28]
"""
import argparse
import json
import math
import os
from datetime import datetime, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
STATE = os.path.expanduser(os.environ.get("CFB_STATE", "~/.cfb-state"))
DATA_USD_PER_GB = 0.05


def h(a, b):
    """Started hours between two HH:MM (UTC, 2026-10-27/28; a later than 20:00 is the 27th)."""
    def t(x):
        hh, mm = map(int, x.split(":"))
        day = 27 if hh >= 20 else 28
        return datetime(2026, 10, day, hh, mm, tzinfo=timezone.utc)
    return max(1, math.ceil((t(b) - t(a)).total_seconds() / 3600))


def data_gb(cluster_ids):
    tot = 0.0
    for c in cluster_ids:
        p = os.path.join(STATE, "data_bytes", c)
        if os.path.exists(p):
            i, o = open(p).read().split()
            tot += (float(i) + float(o)) / 1e9
    return tot


# (test, [(resource label, usd/hour at cap, start, end)], [cluster ids for data])
TESTS = [
    ("Basic x4: WAN matrix + in-region push to 25/50 eCKU (AWS, GCP, Azure Mumbai/Pune; AWS us-east-1)", [
        ("aws-1 25 eCKU, 02:19-07:20 (left at 25 by a dead push script)", 24 * 0.14, "02:19", "07:20"),
        ("gcp-1 50 eCKU, 02:19-07:20 (left at 50 after a state-file race)", 49 * 0.14, "02:19", "07:20"),
        ("azure-1 50 eCKU, 02:39-02:45", 49 * 0.14, "02:39", "02:45"),
        ("Flink pools x3, 20 CFU, ~4 h", 3 * 20 * 0.21, "02:04", "06:15"),
    ], ["lkc-81dm7dq", "lkc-k8wg2w2", "lkc-zmk3zk7", "lkc-o36276o"]),
    ("Standard x3: WAN latency/txn/throughput matrix, azure-2 throttle push", [
        ("aws-2, gcp-2, azure-2 at 4 eCKU", 3 * 4 * 0.75, "02:03", "07:22"),
        ("the same at 1 eCKU", 3 * 0.75, "07:22", "09:04"),
        ("Flink pool azure2, 20 CFU", 20 * 0.21, "07:23", "08:24"),
    ], ["lkc-k8wgvy6", "lkc-57ymjvz", "lkc-dozgnyz"]),
    ("Dedicated x4 (1, 4, 2, 2 CKU): per-CKU produce/consume/request rate (incl. ~4.5 h idle in a session pause)", [
        ("aws-d1 1 CKU", 1 * 4.0, "07:27", "13:30"),
        ("aws-d4 4 CKU", 4 * 4.0, "07:27", "13:30"),
        ("gcp-d2 2 CKU", 2 * 4.0, "07:29", "13:30"),
        ("azure-d2 2 CKU", 2 * 4.0, "07:40", "13:30"),
        ("aws-d2 2 CKU, deleted while provisioning", 2 * 4.0, "07:27", "07:29"),
        ("Flink pools 50+20+30+30 CFU (to the guard's 15:13Z pool deadline)", 130 * 0.21, "12:13", "15:14"),
    ], ["lkc-12ywwyj", "lkc-o36xx6x", "lkc-k8wkkwv", "lkc-xqk77nz"]),
    ("Cluster Linking GCP->AWS and Azure->AWS (2 links, both sides)", [
        ("4 link-sides x $0.25", 1.00, "12:26", "13:30"),
    ], []),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--day", default="2026-10-28")
    ap.add_argument("--end", default=None, help="override every 13:30 end (HH:MM) with the real teardown time")
    a = ap.parse_args()
    tests, total = [], 0.0
    for name, res, ids in TESTS:
        usd = 0.0
        parts = []
        for label, rate, s, e in res:
            e = a.end if (a.end and e == "13:30") else e
            hours = h(s, e)
            usd += rate * hours
            parts.append(f"{label}: ${rate:.2f}/h x {hours} h")
        gb = data_gb(ids)
        usd += gb * DATA_USD_PER_GB
        if gb:
            parts.append(f"data {gb:,.0f} GB x ${DATA_USD_PER_GB}")
        tests.append({"test": name, "resources": "; ".join(parts), "usd": round(usd, 2)})
        total += usd
    out = {"rule": "upper bound: cap x started hours at list (Dedicated assumed $4.00/CKU-h) + Metrics API data at $0.05/GB",
           "tests": tests, "total_usd": round(total, 2),
           "guard_estimate_usd": float(open(os.path.join(STATE, "estimate_usd")).read().strip())}
    path = os.path.join(HERE, "results", a.day, "cost.json")
    with open(path, "w") as f:
        json.dump(out, f, indent=1)
    print(json.dumps({"total_usd": out["total_usd"], "guard_estimate_usd": out["guard_estimate_usd"]}))


if __name__ == "__main__":
    main()
