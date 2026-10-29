#!/usr/bin/env python3
"""Cost per million operations, from measured throughput and stated prices.

Inputs (all in the repo):
  bench/results/bq1/all/*-d<depth>.json   the BQ1 combined runs (QPS, busy cores)
  bench/confluent/results/2026-10-28/...  Confluent Dedicated and burst series
  bench/cost/prices.toml                  every price, with its source and label

Formula, per engine and depth:
  ops per core-second  = mean QPS / mean busy cores        (measured)
  $ per core-hour      = price per vCPU-hour x vCPU per core   (published / assumption)
  $ per 1M ops         = $ per core-hour x 1e6 / (ops per core-second x 3600)

Usage:
  python3 bench/cost/compute.py            print every table
  python3 bench/cost/compute.py --update   rewrite the tables in the report
  python3 bench/cost/compute.py --check    exit 1 if the report is out of date

Standard library only (Python 3.11+ for tomllib).
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import os
import re
import sys
import tomllib
from datetime import datetime

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
PRICES = os.path.join(ROOT, "bench", "cost", "prices.toml")
RUNS = os.path.join(ROOT, "bench", "results", "bq1", "all")
REPORT = os.path.join(ROOT, "docs", "benchmarks", "2026-10-29-cost-per-op", "README.md")
HOURS_PER_YEAR = 8760

# Network engines whose server cores are not shared with another slot, so a
# server-only figure (without the load generator) is meaningful. sql is left
# out: its TiKV store sits on kv-tikv's core and is not in sql's busy time.
SERVER_ONLY = ("pg", "iggy", "gateway", "durable", "house")

ORDER = ("meter", "iggy", "kv", "gateway", "pg", "sql", "kv-tikv", "git", "graph", "durable", "house")
WHAT = {
    "meter": "Meter gate check + sink emit, in-process",
    "iggy": "Iggy native TCP produce, 100 B messages",
    "kv": "loams-kv embedded (redb, in memory) get/put",
    "gateway": "loams-gateway HTTP/2 GET /healthz",
    "pg": "Postgres 17.10 point select (stock compute)",
    "sql": "TiDB (keyspace) point select, MySQL wire",
    "kv-tikv": "loams-kv on TiKV get/put",
    "git": "git-on-S3 WAL push, file store",
    "graph": "Grafeo point lookup",
    "durable": "Resonate promise.create on SQLite",
    "house": "House (chDB worker) point query",
}


# ---------------------------------------------------------------- formatting

def money(v: float | None) -> str:
    """Three significant figures, never scientific notation."""
    if v is None or (isinstance(v, float) and math.isnan(v)):
        return "–"
    if v == 0:
        return "$0"
    if v >= 100:
        return f"${v:,.0f}"
    if v >= 1:
        return f"${v:,.2f}"
    digits = 2 - math.floor(math.log10(v))
    return f"${v:.{digits}f}"


def num(v: float | None, nd: int = 0) -> str:
    if v is None:
        return "–"
    return f"{v:,.{nd}f}"


def ratio(v: float | None) -> str:
    if v is None:
        return "–"
    return f"{v:,.0f}×" if v >= 10 else f"{v:,.1f}×"


def table(head: list[str], rows: list[list[str]], right: set[int] | None = None) -> str:
    right = right if right is not None else set(range(1, len(head)))
    out = ["| " + " | ".join(head) + " |",
           "|" + "|".join("---:" if i in right else "---" for i in range(len(head))) + "|"]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(out)


# ---------------------------------------------------------------- prices

def core_hour(p: dict, vcpu_per_core: float = 1.0, watts: float | None = None) -> float:
    """$ per busy core-hour for one price entry."""
    if p["kind"] == "cloud":
        return p["usd_per_hour"] / p["vcpus"] * vcpu_per_core
    hours = p["years"] * HOURS_PER_YEAR
    capex = (p["hardware_usd"] + p["extra_usd"]) / hours / p["cores"]
    w = p["watts"] if watts is None else watts
    power = w / 1000.0 * p["usd_per_kwh"] / p["cores"]
    return capex + power


def per_million(usd_core_hour: float, ops_per_core_s: float | None) -> float | None:
    if not ops_per_core_s:
        return None
    return usd_core_hour * 1e6 / (ops_per_core_s * 3600.0)


# ---------------------------------------------------------------- BQ1 runs

def load_runs() -> dict[int, dict]:
    """Mean of runs A and B per depth: QPS, busy cores, server-core busy."""
    by_depth: dict[int, list[dict]] = {}
    for f in sorted(glob.glob(os.path.join(RUNS, "*-d*.json"))):
        with open(f) as fh:
            d = json.load(fh)
        # runs A and B of the combined report
        if d.get("label") not in ("final", "final-repeat"):
            continue
        by_depth.setdefault(int(d["depth"]), []).append(d)
    out: dict[int, dict] = {}
    for depth, runs in by_depth.items():
        slots: dict[str, dict] = {}
        for r in runs:
            busy = r["host"]["per_core_busy"]
            for e in r["engines"]:
                s = slots.setdefault(e["slot"], {"qps": [], "busy": [], "server": []})
                s["qps"].append(e["qps"])
                s["busy"].append(e["busy_cores"])
                own = [c for c in str(e["own_cores"]).split(",") if c]
                s["server"].append(sum(busy[c] for c in own))
        engines = {}
        for slot, s in slots.items():
            n = len(s["qps"])
            qps = sum(s["qps"]) / n
            busy = sum(s["busy"]) / n
            server = sum(s["server"]) / n
            engines[slot] = {
                "qps": qps,
                "busy": busy,
                "per_core": qps / busy if qps else None,
                "server": server if slot in SERVER_ONLY else None,
                "per_server_core": qps / server if qps and slot in SERVER_ONLY else None,
            }
        tq = sum(e["qps"] for e in engines.values())
        tb = sum(e["busy"] for e in engines.values())
        out[depth] = {
            "n": len(runs),
            "cpu": runs[0]["host"]["cpu"],
            "commit": runs[0]["commit"],
            "engines": engines,
            "total": {"qps": tq, "busy": tb, "per_core": tq / tb},
        }
    return out


# ---------------------------------------------------------------- Confluent

def _ts(x: str) -> datetime:
    return datetime.strptime(x[:19], "%Y-%m-%dT%H:%M:%S")


def confluent_dedicated(prefix: str) -> dict:
    """Steady-minute medians, the same rule as bench/confluent/report.py."""
    series = sorted(glob.glob(os.path.join(ROOT, prefix + "-series-*.json")))[-1]
    with open(series) as fh:
        s = json.load(fh)
    marks = []
    with open(os.path.join(ROOT, prefix + "-phases.tsv")) as fh:
        for line in fh:
            t, what = line.rstrip("\n").split("\t", 1)
            marks.append((_ts(t), what.split()[0]))

    def window(start: str, stop: str):
        t0 = next(t for t, w in marks if w == start)
        t1 = next(t for t, w in marks if w == stop)
        return t0, t1.replace(second=0)

    def steady(metric: str, start: str, stop: str, skip: int = 2) -> list[tuple[str, float]]:
        t0, t1 = window(start, stop)
        pts = [(p["t"], p["v"] / 60.0) for p in s.get(metric, []) if t0 <= _ts(p["t"]) < t1]
        return pts[skip:] if len(pts) > skip + 1 else pts

    def median(pts):
        v = sorted(x for _, x in pts)
        return v[len(v) // 2]

    a_bytes = steady("received_bytes", "A-start", "A-stop")
    b_recs = steady("received_records", "B-start", "B-stop")
    b_bytes = dict(steady("received_bytes", "B-start", "B-stop"))
    # bytes per record over the same steady minutes of phase B
    tot_b = sum(b_bytes.get(t, 0) for t, _ in b_recs)
    tot_r = sum(v for _, v in b_recs)
    return {
        "series": os.path.relpath(series, ROOT),
        "a_mb_s": median(a_bytes) / 1e6,
        "b_rec_s": median(b_recs),
        "b_bytes_per_rec": tot_b / tot_r,
    }


def confluent_burst(path: str) -> dict:
    with open(os.path.join(ROOT, path)) as fh:
        s = json.load(fh)
    recs = {p["t"]: p["v"] / 60.0 for p in s["received_records"]}
    byts = {p["t"]: p["v"] / 60.0 for p in s["received_bytes"]}
    t = max(recs, key=recs.get)
    return {"t": t, "rec_s": recs[t], "bytes_per_rec": byts[t] / recs[t]}


# ---------------------------------------------------------------- sections

def section_prices(P: dict, R: dict) -> str:
    vpc = P["mapping"]["vcpu_per_core"]
    rows = []
    for key in ("on_demand", "reserved_3y", "owned"):
        p = P["price"][key]
        if p["kind"] == "cloud":
            basis = f"${p['usd_per_hour']} / h ÷ {p['vcpus']} vCPU × {vpc} vCPU per core"
            src = p["source"]
        else:
            basis = (f"(${p['hardware_usd']:,.2f} + ${p['extra_usd']:,.0f}) ÷ ({p['years']} y × {HOURS_PER_YEAR:,} h) ÷ "
                     f"{p['cores']} cores + {p['watts']:.0f} W × ${p['usd_per_kwh']}/kWh ÷ {p['cores']}")
            src = f"{p['hardware_source']}; {p['kwh_source']}"
        rows.append([p["name"], f"{money(core_hour(p, vpc))} [{p['label']}]", basis, src])
    return table(["Reference", "$ per busy core-hour", "Basis", "Source"], rows, right={1})


def section_engines(P: dict, R: dict) -> str:
    vpc = P["mapping"]["vcpu_per_core"]
    prices = [core_hour(P["price"][k], vpc) for k in ("on_demand", "reserved_3y", "owned")]
    rows = []
    for slot in ORDER:
        e1 = R[1]["engines"].get(slot)
        e16 = R[16]["engines"].get(slot)
        if e1 is None or e16 is None:
            continue
        row = [slot, WHAT[slot], num(e1["per_core"]), num(e16["per_core"])]
        for pr in prices:
            row += [money(per_million(pr, e1["per_core"])), money(per_million(pr, e16["per_core"]))]
        rows.append(row)
    t1, t16 = R[1]["total"], R[16]["total"]
    row = ["**whole mix**", "all eleven slots at once", num(t1["per_core"]), num(t16["per_core"])]
    for pr in prices:
        row += [money(per_million(pr, t1["per_core"])), money(per_million(pr, t16["per_core"]))]
    rows.append(row)
    head = ["Engine", "Operation", "ops/s per busy core, d1 [M]", "ops/s per busy core, d16 [M]",
            "On-demand d1 [P]", "On-demand d16 [P]", "3-y plan d1 [P]", "3-y plan d16 [P]",
            "Owned d1 [P]", "Owned d16 [P]"]
    return table(head, rows, right=set(range(2, len(head))))


def section_server_only(P: dict, R: dict) -> str:
    vpc = P["mapping"]["vcpu_per_core"]
    od = core_hour(P["price"]["on_demand"], vpc)
    own = core_hour(P["price"]["owned"], vpc)
    rows = []
    for slot in ORDER:
        if slot not in SERVER_ONLY:
            continue
        e = R[16]["engines"][slot]
        if not e["qps"]:
            rows.append([slot, num(e["server"], 2), "–", "–", "–", "–"])
            continue
        rows.append([slot, num(e["server"], 2), num(e["per_server_core"]),
                     ratio(e["per_server_core"] / e["per_core"]),
                     money(per_million(od, e["per_server_core"])), money(per_million(own, e["per_server_core"]))])
    return table(["Engine (d16)", "Server cores busy [M]", "ops/s per server core [M]",
                  "vs busy-core figure [M]", "On-demand $/1M [P]", "Owned $/1M [P]"], rows)


def section_sensitivity(P: dict, R: dict) -> str:
    picks = ("meter", "iggy", "gateway", "pg", "sql", "durable")
    scen: list[tuple[str, float, str]] = []
    for vpc in P["mapping"]["vcpu_per_core_sensitivity"]:
        for key in ("on_demand", "reserved_3y"):
            p = P["price"][key]
            scen.append((f"{p['name']}, {vpc} vCPU per core", core_hour(p, vpc), "published × assumption"))
    for key, p in P["sensitivity"].items():
        scen.append((f"{p['name']}, 1 vCPU per core", core_hour(p, 1), "published × assumption"))
    o = P["price"]["owned"]
    scen.append((f"Owned, {o['watts']:.0f} W (PSU rating)", core_hour(o), "published + assumption"))
    scen.append((f"Owned, {o['watts_low']:.0f} W (CPU base power)", core_hour(o, watts=o["watts_low"]), "published + assumption"))
    o2 = dict(o, years=5)
    scen.append(("Owned, 5-year life, 100 W", core_hour(o2), "assumption"))
    o3 = dict(o, extra_usd=o["extra_usd"] * 2)
    scen.append(("Owned, RAM + SSD at 2× the assumption", core_hour(o3), "assumption"))
    scen.sort(key=lambda s: s[1])
    rows = []
    for name, pr, lab in scen:
        rows.append([name, money(pr)] + [money(per_million(pr, R[16]["engines"][s]["per_core"])) for s in picks]
                    + [money(per_million(pr, R[16]["total"]["per_core"])), lab])
    head = ["$ per core-hour scenario", "$/core-h"] + [f"{s} d16" for s in picks] + ["whole mix d16", "Label"]
    return table(head, rows, right=set(range(1, len(head) - 1)))


def section_confluent(P: dict, R: dict) -> str:
    C = P["confluent"]
    I = P["iggy"]
    ded = confluent_dedicated(C["dedicated_cluster"])
    bur = confluent_burst(C["burst_series"])
    k = C["dedicated_cku"]
    cku, cku_lo, dat = C["cku_usd_per_hour"], C["cku_usd_per_hour_low"], C["data_in_usd_per_gb"]

    mb_cku = ded["a_mb_s"] / k
    rec100_cku = ded["b_rec_s"] / k
    rec10_cku = bur["rec_s"] / C["burst_cku"]

    def per_m_rec(price_h: float, rec_s: float, bpr: float) -> tuple[float, float]:
        return price_h * 1e6 / (rec_s * 3600.0), bpr * 1e6 / 1e9 * dat

    def per_gb(price_h: float, mb_s: float) -> float:
        return price_h / (mb_s * 3600.0 / 1000.0)

    vpc = P["mapping"]["vcpu_per_core"]
    od = core_hour(P["price"]["on_demand"], vpc)
    own = core_hour(P["price"]["owned"], vpc)
    rf = I["replication_factor"]
    mb = I["message_bytes"]
    ig = R[16]["engines"]["iggy"]
    ig1 = R[1]["engines"]["iggy"]

    lines = []
    # Table A: what one CKU does and costs
    rows = []
    c100, d100 = per_m_rec(cku, rec100_cku, ded["b_bytes_per_rec"])
    c100l, _ = per_m_rec(cku_lo, rec100_cku, ded["b_bytes_per_rec"])
    c10, d10 = per_m_rec(cku, rec10_cku, bur["bytes_per_rec"])
    c10l, _ = per_m_rec(cku_lo, rec10_cku, bur["bytes_per_rec"])
    rows.append(["1 KB records, cluster-bound", f"{mb_cku:,.1f} MB/s", "–",
                 money(per_gb(cku, mb_cku)) + " /GB", money(dat) + " /GB",
                 money(per_gb(cku, mb_cku) + dat) + " /GB", money(per_gb(cku_lo, mb_cku) + dat) + " /GB"])
    rows.append(["100 B records, generator-bound (lower bound)", f"{rec100_cku:,.0f} rec/s",
                 f"{ded['b_bytes_per_rec']:,.1f}", money(c100), money(d100), money(c100 + d100), money(c100l + d100)])
    rows.append([f"{C['burst_payload_bytes']} B payload burst, peak minute (lower bound)", f"{rec10_cku:,.0f} rec/s",
                 f"{bur['bytes_per_rec']:,.1f}", money(c10), money(d10), money(c10 + d10), money(c10l + d10)])
    lines.append(table(["Confluent Dedicated, per CKU", "Throughput per CKU [M]", "Wire bytes per record [M]",
                        f"CKU at ${cku:.2f}/h, per 1M records [A]", f"Data in at ${dat}/GB [published]",
                        "Total per 1M records [P]", f"Total at ${cku_lo:.2f}/CKU-h [P]"], rows))

    # Table B: what the benchmark actually cost
    lines.append("")
    rows = [
        ["Whole CFB lane (Basic, Standard, Dedicated, Flink generators, idle hours, phase 3)",
         money(C["spend_upper_usd"]), f"{C['ingress_gb']:,}", money(C["spend_upper_usd"] / C["ingress_gb"])],
        ["Dedicated test only (per-test recount)", money(C["dedicated_test_usd"]), f"{C['dedicated_test_gb']:,}",
         money(C["dedicated_test_usd"] / C["dedicated_test_gb"])],
    ]
    lines.append(table(["Actual bill (guard estimate, upper bound) [M]", "USD [M]", "GB ingested [M]", "$ per GB ingested [M]"], rows))

    # Table C: the like-for-like attempt
    lines.append("")
    ig_mb_core = ig["per_core"] * mb / 1e6
    ig_mb_srv = ig["per_server_core"] * mb / 1e6
    rows = []
    conf_100 = c100 + d100
    for name, ops in (
        ("Iggy, d16, per busy core (server + load generator)", ig["per_core"]),
        ("Iggy, d16, per server core only", ig["per_server_core"]),
        ("Iggy, d1, per busy core", ig1["per_core"]),
    ):
        a = per_million(od, ops)
        b = per_million(own, ops)
        rows.append([name, f"{ops:,.0f}", money(a), money(a * rf), money(b), money(b * rf),
                     ratio(conf_100 / (a * rf)), ratio(c100 / (a * rf))])
    lines.append(table(["100 B messages", "msgs/s per core [M]", "On-demand $/1M, RF 1 [P]", f"On-demand $/1M, ×{rf} [P,A]",
                        "Owned $/1M, RF 1 [P]", f"Owned $/1M, ×{rf} [P,A]",
                        f"Confluent {money(conf_100)} ÷ on-demand ×{rf} [P]",
                        f"Confluent CKU only {money(c100)} ÷ on-demand ×{rf} [P]"], rows))

    lines.append("")
    rows = [
        ["Iggy per busy core, d16 (100 B payload)", f"{ig_mb_core:,.1f}", money(per_gb(od, ig_mb_core)),
         money(per_gb(od, ig_mb_core) * rf), money(per_gb(own, ig_mb_core) * rf)],
        ["Iggy per server core, d16 (100 B payload)", f"{ig_mb_srv:,.1f}", money(per_gb(od, ig_mb_srv)),
         money(per_gb(od, ig_mb_srv) * rf), money(per_gb(own, ig_mb_srv) * rf)],
        ["Confluent per CKU, 1 KB records (CKU + data in; replication included; record size differs)", f"{mb_cku:,.1f}", money(per_gb(cku, mb_cku) + dat), "–", "–"],
    ]
    lines.append(table(["Per GB of payload", "MB/s per unit [M]", "On-demand $/GB, RF 1 [P]",
                        f"On-demand $/GB, ×{rf} [P,A]", f"Owned $/GB, ×{rf} [P,A]"], rows))
    return "\n".join(lines)


def section_inputs(P: dict, R: dict) -> str:
    C = P["confluent"]
    ded = confluent_dedicated(C["dedicated_cluster"])
    bur = confluent_burst(C["burst_series"])
    rows = [
        ["BQ1 combined runs", f"{R[1]['n']} runs at depth 1, {R[16]['n']} at depth 16; {R[1]['cpu']}; commit {R[1]['commit']}",
         "bench/results/bq1/all/*.json"],
        ["Confluent Dedicated (aws-d4)", f"phase A {ded['a_mb_s']:,.1f} MB/s, phase B {ded['b_rec_s']:,.0f} rec/s on {C['dedicated_cku']} CKU",
         ded["series"]],
        ["Confluent burst (aws-x2)", f"peak minute {bur['t'][11:16]}Z: {bur['rec_s']:,.0f} rec/s on {C['burst_cku']} CKU", C["burst_series"]],
    ]
    return table(["Input", "What the script read [M]", "File"], rows, right=set())


SECTIONS = {
    "inputs": section_inputs,
    "prices": section_prices,
    "engines": section_engines,
    "server-only": section_server_only,
    "sensitivity": section_sensitivity,
    "confluent": section_confluent,
}


def render(P: dict, R: dict) -> dict[str, str]:
    return {k: f(P, R) for k, f in SECTIONS.items()}


def splice(text: str, parts: dict[str, str]) -> str:
    for name, body in parts.items():
        pat = re.compile(rf"(<!-- cost:{re.escape(name)} -->\n).*?(<!-- /cost:{re.escape(name)} -->)", re.S)
        if not pat.search(text):
            raise SystemExit(f"marker cost:{name} missing in {REPORT}")
        text = pat.sub(lambda m: m.group(1) + body + "\n" + m.group(2), text)
    return text


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--update", action="store_true", help="rewrite the tables in the report")
    ap.add_argument("--check", action="store_true", help="exit 1 if the report's tables are stale")
    ap.add_argument("--prices", default=PRICES)
    a = ap.parse_args()
    with open(a.prices, "rb") as fh:
        P = tomllib.load(fh)
    R = load_runs()
    parts = render(P, R)
    if a.update or a.check:
        with open(REPORT) as fh:
            old = fh.read()
        new = splice(old, parts)
        if a.check:
            if new != old:
                print("report tables are stale: run bench/cost/compute.py --update", file=sys.stderr)
                return 1
            return 0
        with open(REPORT, "w") as fh:
            fh.write(new)
        return 0
    for k, v in parts.items():
        print(f"## {k}\n\n{v}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
