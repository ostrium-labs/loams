#!/usr/bin/env python3
"""Export driver traces from SigNoz (its ClickHouse, signoz_traces.signoz_index_v3).

  traces.py [--ch http://127.0.0.1:28147] [--day 2026-10-28]

For every cfb.target with kafka.message traces it writes:
  results/<day>/traces/summary.json            span-duration percentiles per target and span name
  results/<day>/traces/<target>-trace.json     the median kafka.message trace (all spans)
  results/<day>/charts/trace-<target>.svg      its waterfall
These are SigNoz data: the same spans the SigNoz trace view shows (http://127.0.0.1:3301).
"""
import argparse
import json
import os
import urllib.parse
import urllib.request

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
SURFACE, INK, INK2, GRID = "#fcfcfb", "#0b0b0b", "#52514e", "#e4e3df"
SPAN_COLOR = {"kafka.message": "#6250d6", "kafka.produce": "#2a78d6", "kafka.deliver": "#eb6834"}


def q(ch, sql):
    url = ch + "/?" + urllib.parse.urlencode({"query": sql + " FORMAT JSONEachRow"})
    with urllib.request.urlopen(url, timeout=60) as r:
        return [json.loads(line) for line in r.read().decode().splitlines() if line.strip()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ch", default="http://127.0.0.1:28147")
    ap.add_argument("--day", default="2026-10-28")
    a = ap.parse_args()
    out = os.path.join(HERE, "results", a.day, "traces")
    charts = os.path.join(HERE, "results", a.day, "charts")
    os.makedirs(out, exist_ok=True)
    os.makedirs(charts, exist_ok=True)
    base = "FROM signoz_traces.signoz_index_v3 WHERE resource_string_service$$name = 'cfb-driver'"
    summ = q(a.ch, f"""SELECT attributes_string['cfb.target'] AS target, name,
        count() AS spans, quantile(0.5)(duration_nano)/1e6 AS p50_ms, quantile(0.99)(duration_nano)/1e6 AS p99_ms
        {base} AND attributes_string['cfb.target'] != '' GROUP BY target, name ORDER BY target, name""")
    with open(os.path.join(out, "summary.json"), "w") as f:
        json.dump(summ, f, indent=1)
    targets = sorted({r["target"] for r in summ if r["name"] == "kafka.message"})
    for t in targets:
        med = q(a.ch, f"""SELECT trace_id FROM (SELECT trace_id, duration_nano {base} AND name = 'kafka.message'
            AND attributes_string['cfb.target'] = '{t}' ORDER BY duration_nano)
            LIMIT 1 OFFSET (SELECT intDiv(count(), 2) {base} AND name = 'kafka.message' AND attributes_string['cfb.target'] = '{t}')""")
        if not med:
            continue
        tid = med[0]["trace_id"]
        spans = q(a.ch, f"""SELECT name, span_id, parent_span_id, toUnixTimestamp64Nano(timestamp) AS start_ns, duration_nano,
            attributes_string AS attrs, resources_string AS res {base} AND trace_id = '{tid}' ORDER BY start_ns""")
        for s in spans:
            s["start_ns"] = int(s["start_ns"]); s["duration_nano"] = int(s["duration_nano"])
        with open(os.path.join(out, f"{t}-trace.json"), "w") as f:
            json.dump({"trace_id": tid, "target": t, "spans": spans}, f, indent=1)
        t0 = min(s["start_ns"] for s in spans)
        order = {"kafka.message": 0, "kafka.produce": 1, "kafka.deliver": 2}
        spans.sort(key=lambda s: order.get(s["name"], 9))
        fig, ax = plt.subplots(figsize=(8, 0.6 * len(spans) + 1.1))
        for i, s in enumerate(spans):
            x0 = (s["start_ns"] - t0) / 1e6
            w = s["duration_nano"] / 1e6
            ax.barh(i, w, left=x0, height=0.5, color=SPAN_COLOR.get(s["name"], INK2))
            ax.text(x0 + w, i, f"  {s['name']}  {w:,.1f} ms", va="center", fontsize=9, color=INK)
        ax.set_yticks([])
        ax.invert_yaxis()
        ax.set_facecolor(SURFACE); fig.set_facecolor(SURFACE)
        for side in ("top", "right", "left"):
            ax.spines[side].set_visible(False)
        ax.spines["bottom"].set_color(GRID)
        ax.tick_params(colors=INK2)
        ax.set_xlabel("ms since send", color=INK2)
        ax.set_title(f"SigNoz trace {tid[:12]}…: median message, {t}", loc="left", fontsize=11, color=INK)
        ax.set_xlim(0, max((s["start_ns"] - t0 + s["duration_nano"]) / 1e6 for s in spans) * 1.45)
        fig.tight_layout()
        fig.savefig(os.path.join(charts, f"trace-{t}.svg"), format="svg", facecolor=SURFACE)
        plt.close(fig)
    print(f"{len(summ)} summary rows; traces for {len(targets)} targets -> {out}")


if __name__ == "__main__":
    main()
