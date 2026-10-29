#!/usr/bin/env python3
"""Build the CFB report's charts (SVG) and tables (markdown) from results/<day>/.

  report.py [--day 2026-10-28]

Writes results/<day>/charts/*.svg and results/<day>/tables.md. The report
(docs/benchmarks/confluent-cloud-2026-10.md) embeds the charts and pastes the tables.

Colors follow entities, never rank (dataviz skill; validated palette, light surface).
Every chart has a table next to it, because three of the hues are under 3:1
contrast against the surface.
"""
from __future__ import annotations

import argparse
import glob
import json
import os
from datetime import datetime

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
SURFACE = "#fcfcfb"
INK = "#0b0b0b"
INK2 = "#52514e"
GRID = "#e4e3df"
COLOR = {  # entity -> fixed slot
    "aws": "#2a78d6", "gcp": "#eb6834", "azure": "#1baf7a",
    "local-kafka": "#eda100", "local-iggy": "#e87ba4", "aws-us": "#6250d6",
}


def entity(target: str, region: str = "") -> str:
    if "iggy" in target:
        return "local-iggy"
    if "apache-kafka" in target:
        return "local-kafka"
    if "aws-us" in target or region == "us-east-1":
        return "aws-us"
    for k in ("aws", "gcp", "azure"):
        if f"-{k}-" in target:
            return k
    return "aws"


def label(target: str) -> str:
    return (target.replace("loams-cfb-", "").replace("apache-kafka-4.3.1-local", "Kafka 4.3.1 (local)")
            .replace("iggy-kafka-gateway-local", "Iggy gateway (local)"))


def style(ax, title, ylabel):
    ax.set_facecolor(SURFACE)
    ax.figure.set_facecolor(SURFACE)
    ax.set_title(title, loc="left", color=INK, fontsize=12, fontweight="bold")
    ax.set_ylabel(ylabel, color=INK2)
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    for s in ("left", "bottom"):
        ax.spines[s].set_color(GRID)
    ax.tick_params(colors=INK2)
    ax.grid(axis="y", color=GRID, linewidth=0.8)
    ax.set_axisbelow(True)


def load(day, sub):
    out = []
    for f in sorted(glob.glob(os.path.join(HERE, "results", day, sub, "*.json"))):
        with open(f) as fh:
            d = json.load(fh)
        d["_file"] = os.path.relpath(f, os.path.join(HERE, "results", day))
        out.append(d)
    return out


def fmt(x, nd=1):
    return "–" if x is None else (f"{x:,.{nd}f}")


def save(fig, path):
    fig.tight_layout()
    fig.savefig(path, format="svg", facecolor=SURFACE)
    plt.close(fig)


# --- RTT ----------------------------------------------------------------------------------------
def rtt(day, charts, md):
    rows = load(day, "rtt")
    if not rows:
        return
    md.append("### WAN round trips from the driver laptop (p50 / p99, ms)\n")
    md.append("| Cluster | Cloud / region | Type | TCP connect | TLS handshake | Kafka metadata request |")
    md.append("| --- | --- | --- | --- | --- | --- |")
    for d in rows:
        r = d["result"]
        md.append(f"| {label(d['target'])} | {d['cloud']} {d['region']} | {d['kind'].split('-')[-1]} | "
                  f"{fmt(r['tcp_connect_ms'].get('p50'))} / {fmt(r['tcp_connect_ms'].get('p99'))} | "
                  f"{fmt(r['tls_handshake_ms'].get('p50'))} / {fmt(r['tls_handshake_ms'].get('p99'))} | "
                  f"{fmt(r['metadata_rtt_ms'].get('p50'))} / {fmt(r['metadata_rtt_ms'].get('p99'))} |")
    md.append("")
    fig, ax = plt.subplots(figsize=(8, 3.6))
    names = [label(d["target"]) for d in rows]
    vals = [d["result"]["metadata_rtt_ms"].get("p50") or 0 for d in rows]
    cols = [COLOR[entity(d["target"], d["region"])] for d in rows]
    bars = ax.bar(names, vals, color=cols, width=0.6)
    for b, v in zip(bars, vals):
        ax.text(b.get_x() + b.get_width() / 2, v, f"{v:.0f}", ha="center", va="bottom", color=INK, fontsize=9)
    style(ax, "Kafka metadata round trip from the laptop, p50", "ms")
    ax.tick_params(axis="x", rotation=20)
    save(fig, os.path.join(charts, "rtt.svg"))


# --- latency -------------------------------------------------------------------------------------
def latency(day, charts, md):
    rows = load(day, "latency") + load(day, "local/latency")
    # Kafka 4.3.1 local: only the first run exists (its container was removed before
    # the rerun), and that run's end-to-end column carries the consumer artefact.
    sup = [d for d in load(day, "superseded/local-latency-batch-consume") if "apache-kafka" in d["target"]]
    for d in sup:
        d["_e2e_artefact"] = True
    rows += sup
    if not rows:
        return
    md.append("### Produce and end-to-end latency at 10 msg/s (ms; p50 / p99 / p99.9)\n")
    md.append("End to end marked † includes a driver artefact of 0–200 ms: the first runs read with "
              "`consume(1000, timeout=0.2)`, which waits for a full batch. Produce latency is not affected. "
              "Runs after 12:40Z (Iggy local, Cluster Linking) poll per message.\n")
    md.append("| Target | Size | acks | Produce p50 / p99 / p99.9 | End to end p50 / p99 / p99.9 | Sent | Lost after ack | Errors |")
    md.append("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for d in sorted(rows, key=lambda d: (d["target"], d["result"].get("size", 0), str(d["result"].get("acks")))):
        r = d["result"]
        if "exception" in r:
            md.append(f"| {label(d['target'])} | – | – | failed: {r['exception'][:80]} | | | | |")
            continue
        p, e = r["produce_latency_ms"], r["e2e_latency_ms"]
        dag = " †" if (d.get("_e2e_artefact") or d.get("started_utc", "") < "2026-10-28T12:40") else ""
        md.append(f"| {label(d['target'])} | {r['size']} B | {r['acks']} | {fmt(p.get('p50'))} / {fmt(p.get('p99'))} / {fmt(p.get('p999'))} | "
                  f"{fmt(e.get('p50'))} / {fmt(e.get('p99'))} / {fmt(e.get('p999'))}{dag} | {r['sent']} | {r['lost_after_ack']} | {r['errors']} |")
    md.append("")
    sel = [d for d in rows if d["result"].get("size") == 1024 and str(d["result"].get("acks")) == "all"]
    if sel:
        fig, ax = plt.subplots(figsize=(8.5, 3.8))
        xs = range(len(sel))
        p50 = [d["result"]["produce_latency_ms"].get("p50") or 0 for d in sel]
        p99 = [d["result"]["produce_latency_ms"].get("p99") or 0 for d in sel]
        cols = [COLOR[entity(d["target"], d["region"])] for d in sel]
        ax.bar([x - 0.18 for x in xs], p50, width=0.34, color=cols, label="p50")
        ax.bar([x + 0.18 for x in xs], p99, width=0.34, color=cols, alpha=0.45, hatch="//", edgecolor=SURFACE, label="p99")
        for x, a, b in zip(xs, p50, p99):
            ax.text(x - 0.18, a, f"{a:.0f}", ha="center", va="bottom", fontsize=8, color=INK)
            ax.text(x + 0.18, b, f"{b:.0f}", ha="center", va="bottom", fontsize=8, color=INK)
        ax.set_xticks(list(xs), [label(d["target"]) for d in sel], rotation=20)
        ax.set_yscale("log")
        style(ax, "Produce latency, 1 KiB, acks=all (solid p50, hatched p99), log scale", "ms (log)")
        ax.legend(frameon=False, labelcolor=INK2)
        save(fig, os.path.join(charts, "latency-1k-acks-all.svg"))


# --- transactions ---------------------------------------------------------------------------------
def txn(day, charts, md):
    rows = load(day, "txn") + load(day, "local/txn")
    if not rows:
        return
    md.append("### Transactions (60 transactions x 10 records of 512 B, every 5th aborted)\n")
    md.append("| Target | init_transactions | Commit p50 / p99 | Abort p50 | Txn/s | read_committed: missing / saw aborted / duplicates | read_uncommitted saw aborted | EOS |")
    md.append("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for d in rows:
        r = d["result"]
        if "exception" in r:
            md.append(f"| {label(d['target'])} | failed: {r['exception'][:100]} | | | | | | |")
            continue
        e = r["eos"]
        md.append(f"| {label(d['target'])} | {fmt(r['init_transactions_ms'], 0)} ms | {fmt(r['commit_latency_ms'].get('p50'))} / {fmt(r['commit_latency_ms'].get('p99'))} ms | "
                  f"{fmt(r['abort_latency_ms'].get('p50'))} ms | {r['txn_per_s']} | {e['read_committed_missing']} / {e['read_committed_saw_aborted']} / {e['read_committed_duplicates']} | "
                  f"{e['read_uncommitted_saw_aborted']} of {e['aborted']} | {'pass' if e['pass'] else 'FAIL'} |")
    md.append("")


# --- throughput (WAN and local) -------------------------------------------------------------------------
def tput(day, charts, md):
    rows = load(day, "tput") + load(day, "local/tput") + load(day, "parallel/tput")
    if not rows:
        return
    md.append("### Producer throughput (acks=all, no compression)\n")
    md.append("| Target | Where | Record size | MB/s | records/s | Produce p50 / p99 (ms) |")
    md.append("| --- | --- | --- | --- | --- | --- |")
    for d in rows:
        r = d["result"]
        if "exception" in r:
            continue
        where = "laptop, loopback" if d["cloud"] == "local" else ("laptop over WAN, in parallel on all clusters" if "parallel" in d["_file"] else "laptop over WAN")
        md.append(f"| {label(d['target'])} | {where} | {r['size']} B | {r['produce_mb_per_s']} | {r['produce_msgs_per_s']:,} | "
                  f"{fmt(r['produce_latency_ms'].get('p50'))} / {fmt(r['produce_latency_ms'].get('p99'))} |")
    md.append("")


# --- in-region push --------------------------------------------------------------------------------
def push(day, charts, md):
    pdir = os.path.join(HERE, "results", day, "push")
    series = sorted(glob.glob(os.path.join(pdir, "*-series-full.json")) or glob.glob(os.path.join(pdir, "*-series-*.json")))
    if not series:
        return
    md.append("### In-region push (Flink generator in the same region; Metrics API, per minute)\n")
    md.append("| Cluster | Type | max eCKU step | Nominal ingress (MB/s) | Measured, steady minutes (MB/s, median) | Peak minute (MB/s) |")
    md.append("| --- | --- | --- | --- | --- | --- |")
    figs = []
    for f in series:
        name = os.path.basename(f).split("-series-")[0]
        with open(f) as fh:
            s = json.load(fh)
        steps_f = os.path.join(pdir, f"{name}-steps.tsv")
        steps = []
        if os.path.exists(steps_f):
            for line in open(steps_f):
                t, e = line.split()
                steps.append((datetime.strptime(t, "%Y-%m-%dT%H:%M:%SZ"), int(e)))
        pts = [(datetime.strptime(p["t"][:19], "%Y-%m-%dT%H:%M:%S"), p["v"] / 60 / 1e6) for p in s["received_bytes"]]
        kind = "standard" if name.endswith("-2") else "basic"
        per = 25 if kind == "standard" else 5
        for i, (t0, e) in enumerate(steps):
            t1 = steps[i + 1][0] if i + 1 < len(steps) else None
            # whole minutes inside the step, skipping the first (ramp) minute
            inside = [v for (t, v) in pts if t >= t0.replace(second=0) and (t1 is None or t < t1.replace(second=0))]
            steady = sorted(inside[1:]) if len(inside) > 2 else sorted(inside)
            med = steady[len(steady) // 2] if steady else None
            md.append(f"| {name.replace('loams-cfb-', '')} | {kind} | {e} | {e * per} | {fmt(med)} | {fmt(max(inside) if inside else None)} |")
        figs.append((name, pts, steps, per))
    md.append("")
    if figs:
        fig, axes = plt.subplots(len(figs), 1, figsize=(9, 2.6 * len(figs)), sharex=False)
        if len(figs) == 1:
            axes = [axes]
        for ax, (name, pts, steps, per) in zip(axes, figs):
            ent = entity(name)
            ax.plot([t for t, _ in pts], [v for _, v in pts], color=COLOR[ent], linewidth=2, label="measured ingress")
            # nominal cap as a step line
            if steps:
                xs, ys = [], []
                for i, (t0, e) in enumerate(steps):
                    t1 = steps[i + 1][0] if i + 1 < len(steps) else (pts[-1][0] if pts else t0)
                    xs += [t0, t1]
                    ys += [e * per, e * per]
                ax.plot(xs, ys, color=INK2, linewidth=1, linestyle="--", label="nominal max (eCKU x per-eCKU ingress)")
                for t0, e in steps:
                    ax.annotate(f"{e} eCKU", (t0, e * per), textcoords="offset points", xytext=(2, 4), fontsize=8, color=INK2)
            style(ax, f"{name.replace('loams-cfb-', '')}: produce ingress vs max eCKU", "MB/s")
            ax.legend(frameon=False, fontsize=8, labelcolor=INK2, loc="upper left")
        save(fig, os.path.join(charts, "push.svg"))


# --- Dedicated per-CKU -------------------------------------------------------------------------
def _ts(x):
    return datetime.strptime(x[:19], "%Y-%m-%dT%H:%M:%S")


def dedicated(day, charts, md):
    d = os.path.join(HERE, "results", day, "dedicated")
    rows = []
    for ph in sorted(glob.glob(os.path.join(d, "*-phases.tsv"))):
        name = os.path.basename(ph).replace("-phases.tsv", "")
        ser = sorted(glob.glob(os.path.join(d, f"{name}-series-*.json")))
        if not ser:
            continue
        with open(ser[-1]) as fh:
            s = json.load(fh)
        marks = []
        for line in open(ph):
            t, what = line.rstrip("\n").split("\t", 1)
            marks.append((_ts(t), what.split()[0]))
        def window(start, stop):
            t0 = next((t for t, w in marks if w == start), None)
            t1 = next((t for t, w in marks if w == stop), None)
            return t0, t1
        def steady(metric, start, stop, per=60.0, skip=2):
            t0, t1 = window(start, stop)
            if not t0 or not t1:
                return None, None
            vals = [p["v"] / per for p in s.get(metric, []) if t0 <= _ts(p["t"]) < t1.replace(second=0)]
            vals = vals[skip:] if len(vals) > skip + 1 else vals
            if not vals:
                return None, None
            v = sorted(vals)
            return v[len(v) // 2], max(v)
        cku = int(name.rsplit("-d", 1)[1]) if "-d" in name else 1
        a_mb, a_pk = steady("received_bytes", "A-start", "A-stop", per=60e6)
        b_rec, _ = steady("received_records", "B-start", "B-stop")
        b_req, _ = steady("request_count", "B-start", "B-stop")
        a_req, _ = steady("request_count", "A-start", "A-stop")
        c_mb, c_pk = steady("sent_bytes", "C-start", "C-stop", per=60e6)
        rows.append(dict(name=name.replace("loams-cfb-", ""), cku=cku, a_mb=a_mb, a_pk=a_pk, a_req=a_req, b_rec=b_rec,
                         b_req=b_req, c_mb=c_mb, c_pk=c_pk))
    if not rows:
        return
    md.append("### Dedicated clusters: measured capacity per CKU (in-region Flink load; Metrics API medians of steady minutes)\n")
    md.append("| Cluster | CKU | Produce 1 KB: MB/s (peak) | per CKU | Produce 100 B: records/s | per CKU | Requests/s (100 B phase) | per CKU | Consume MB/s (peak) | per CKU |")
    md.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for r in sorted(rows, key=lambda r: (r["name"].split("-")[0], r["cku"])):
        c = r["cku"]
        pc = lambda v, nd=1: fmt(v / c, nd) if v else "–"
        md.append(f"| {r['name']} | {c} | {fmt(r['a_mb'])} ({fmt(r['a_pk'])}) | {pc(r['a_mb'])} | {fmt(r['b_rec'], 0)} | {pc(r['b_rec'], 0)} | "
                  f"{fmt(r['b_req'], 0)} | {pc(r['b_req'], 0)} | {fmt(r['c_mb'])} ({fmt(r['c_pk'])}) | {pc(r['c_mb'])} |")
    md.append("")
    # 1B records/s projection from the best measured per-CKU record rate
    best = max((r for r in rows if r["b_rec"]), key=lambda r: r["b_rec"] / r["cku"], default=None)
    if best:
        per = best["b_rec"] / best["cku"]
        ckus = 1e9 / per
        md.append("### Projection to 1 billion records/s (PROJECTED, not measured)\n")
        md.append(f"- Best measured rate: **{per:,.0f} records/s per CKU** (100 B records, {best['name']}, Flink generator in-region).")
        md.append(f"- 1,000,000,000 / {per:,.0f} = **{ckus:,.0f} CKU**. At the guard's assumed $4.00/CKU-hour that is **${ckus * 4:,.0f}/hour** for CKUs alone, before data in/out.")
        md.append("- Assumes linear scaling past the largest size measured here (4 CKU), even partition spread, and a generator that is not the bottleneck; see the caveats.")
        md.append("")
    fig, ax = plt.subplots(figsize=(7.5, 3.8))
    for ent in ("aws", "gcp", "azure"):
        pts = sorted([(r["cku"], r["a_mb"]) for r in rows if r["name"].startswith(ent) and r["a_mb"]])
        if pts:
            ax.plot([p[0] for p in pts], [p[1] for p in pts], marker="o", markersize=8, linewidth=2, color=COLOR[ent], label=ent.upper())
            for x, y in pts:
                ax.annotate(f"{y:.0f}", (x, y), textcoords="offset points", xytext=(6, -2), fontsize=9, color=INK)
    style(ax, "Dedicated produce throughput (1 KB records) by CKU", "MB/s (median steady minute)")
    ax.set_xlabel("CKU", color=INK2)
    ax.set_xticks([1, 2, 4])
    ax.legend(frameon=False, labelcolor=INK2)
    save(fig, os.path.join(charts, "dedicated-scaling.svg"))


def link(day, charts, md):
    rows = load(day, "link")
    if not rows:
        return
    md.append("### Cross-cloud Cluster Linking (produce on the source cloud, consume the mirror on AWS; laptop driver)\n")
    md.append("| Link | Produce p50 / p99 (ms) | End to end through the link p50 / p99 / p99.9 (ms) | Sent | Consumed | Lost after ack |")
    md.append("| --- | --- | --- | --- | --- | --- |")
    for d in rows:
        r = d["result"]
        if "exception" in r:
            md.append(f"| {d['target']} | failed: {r['exception'][:80]} | | | | |"); continue
        p, e = r["produce_latency_ms"], r["e2e_latency_ms"]
        md.append(f"| {d['target']} | {fmt(p.get('p50'))} / {fmt(p.get('p99'))} | {fmt(e.get('p50'))} / {fmt(e.get('p99'))} / {fmt(e.get('p999'))} | {r['sent']} | {r['consumed']} | {r['lost_after_ack']} |")
    md.append("")


# --- aggregate bursts ----------------------------------------------------------------------------
def burst(day, charts, md):
    d = os.path.join(HERE, "results", day, "burst")
    tags = sorted({os.path.basename(f).split("-events.tsv")[0] for f in glob.glob(os.path.join(d, "*-events.tsv"))})
    if not tags:
        return
    md.append("### Aggregate bursts: tiny records across 4-CKU Dedicated clusters (in-region Flink; Metrics API)\n")
    md.append("| Burst | Clusters (CKU) | Statements | Record payload | Peak minute: records/s | Steady median: records/s | per CKU | Requests/s (steady) | Ingress MB/s (steady) | Flink CFU used / max (peak) |")
    md.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    figs = []
    for tag in tags:
        ev = [l.rstrip("\n").split("\t", 1) for l in open(os.path.join(d, f"{tag}-events.tsv"))]
        t_start = next((_ts(t) for t, w in ev if w.startswith("submitted")), None)
        t_stop = next((_ts(t) for t, w in ev if w.startswith("stopping")), None)
        size = next((w.split("size=")[1].split()[0] for t, w in ev if "size=" in w), "?")
        nst = next((w.split()[1] for t, w in ev if w.startswith("submitted")), "?")
        series = sorted(glob.glob(os.path.join(d, f"{tag}-loams-cfb-*-series.json")))
        if not series or not t_start or not t_stop:
            continue
        agg_rec, agg_req, agg_b = {}, {}, {}
        ncl = 0
        for f in series:
            with open(f) as fh:
                sj = json.load(fh)
            ncl += 1
            for key, acc in (("received_records", agg_rec), ("request_count", agg_req), ("received_bytes", agg_b)):
                for pt in sj.get(key, []):
                    t = _ts(pt["t"])
                    acc[t] = acc.get(t, 0) + pt["v"] / 60
        mins = sorted(t for t in agg_rec if t_start <= t < t_stop.replace(second=0))
        steady = mins[2:] if len(mins) > 3 else mins
        def med(acc):
            v = sorted(acc.get(t, 0) for t in steady)
            return v[len(v) // 2] if v else None
        peak = max((agg_rec[t] for t in mins), default=None)
        cfu_used, cfu_max = 0, 0
        cf = os.path.join(d, f"{tag}-cfu.tsv")
        if os.path.exists(cf):
            per_t = {}
            for line in open(cf):
                t, pool, js = line.rstrip("\n").split("\t")
                j = json.loads(js) if js.startswith("{") else {}
                u, m = per_t.get(t[:16], (0, 0))
                per_t[t[:16]] = (u + (j.get("cfu") or 0), m + (j.get("max") or 0))
            if per_t:
                cfu_used, cfu_max = max(per_t.values())
        cku = 4 * ncl
        mr = med(agg_rec)
        md.append(f"| {tag} | {ncl} ({cku}) | {nst} | {size} B | {fmt(peak, 0)} | {fmt(mr, 0)} | {fmt(mr / cku if mr else None, 0)} | "
                  f"{fmt(med(agg_req), 0)} | {fmt((med(agg_b) or 0) / 1e6)} | {cfu_used} / {cfu_max} |")
        figs.append((tag, cku, mr))
    md.append("")
    pts = sorted((cku, mr) for _, cku, mr in figs if mr)
    if pts:
        fig, ax = plt.subplots(figsize=(7.5, 3.8))
        ax.plot([p[0] for p in pts], [p[1] / 1e6 for p in pts], marker="o", markersize=8, linewidth=2, color=COLOR["aws"])
        for x, y in pts:
            ax.annotate(f"{y / 1e6:.1f}M", (x, y / 1e6), textcoords="offset points", xytext=(6, -2), fontsize=9, color=INK)
        style(ax, "Aggregate records/s vs CKU (tiny records, in-region Flink)", "million records/s (steady median)")
        ax.set_xlabel("CKU (sum over 4-CKU clusters)", color=INK2)
        save(fig, os.path.join(charts, "burst-scaling.svg"))


# --- cost -------------------------------------------------------------------------------------------
def cost(day, charts, md):
    f = os.path.join(HERE, "results", day, "cost.json")
    if not os.path.exists(f):
        return
    with open(f) as fh:
        c = json.load(fh)
    md.append("### Cost per test (ledger: list price x billed hours at the cap; data at $0.05/GB)\n")
    md.append("| Test | Resources | Estimated USD |")
    md.append("| --- | --- | --- |")
    for t in c["tests"]:
        md.append(f"| {t['test']} | {t['resources']} | ${t['usd']:.2f} |")
    md.append(f"| **Total (estimate)** | | **${c['total_usd']:.2f}** |")
    md.append("")
    fig, ax = plt.subplots(figsize=(8, 0.5 * len(c["tests"]) + 1.2))
    names = [t["test"] for t in c["tests"]][::-1]
    vals = [t["usd"] for t in c["tests"]][::-1]
    bars = ax.barh(names, vals, color=COLOR["aws"], height=0.55)
    for b, v in zip(bars, vals):
        ax.text(v, b.get_y() + b.get_height() / 2, f" ${v:.2f}", va="center", fontsize=8, color=INK)
    style(ax, "Estimated cost per test (USD)", "")
    ax.grid(axis="x", color=GRID)
    ax.grid(axis="y", visible=False)
    save(fig, os.path.join(charts, "cost.svg"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--day", default="2026-10-28")
    a = ap.parse_args()
    charts = os.path.join(HERE, "results", a.day, "charts")
    os.makedirs(charts, exist_ok=True)
    md = []
    for fn in (rtt, latency, txn, tput, push, dedicated, burst, link, cost):
        fn(a.day, charts, md)
    with open(os.path.join(HERE, "results", a.day, "tables.md"), "w") as fh:
        fh.write("\n".join(md) + "\n")
    print(f"wrote {charts} and tables.md ({len(md)} lines)")


if __name__ == "__main__":
    main()
