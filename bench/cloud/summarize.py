#!/usr/bin/env python3
"""CB1 summary (plan §5.3 and §5.4): builds the results README from raw data only.

  summarize.py [--results DIR] [--template FILE] [--check RUN_DIR] [--stdout]

--results  the results root: one directory per run (<date>-<row>) that holds
           meta.json (default bench/results/cloud-free-tier)
--check    verify one run directory is complete (every configured scenario's
           files are there) and exit 1 if not; the dry run uses it
Every run directory also gets a summary.json with the figures the README shows.
Standard library only.
"""
from __future__ import annotations

import argparse
import datetime as dt
import gzip
import json
import re
import statistics
import sys
from pathlib import Path
from string import Template

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
DEFAULT_RESULTS = REPO / "bench/results/cloud-free-tier"
CLOSED = ("ping", "read", "write")
RATE = ("read-rate", "write-rate")
CPU_FLAG = 80.0  # a rep whose loadgen's mean CPU is above this is possibly loadgen-bound


# --- small helpers ---------------------------------------------------------------------
def load(path: Path, default=None):
    try:
        if path.suffix == ".gz":
            return json.loads(gzip.decompress(path.read_bytes()))
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return default


def jsonl(path: Path) -> list[dict]:
    out = []
    try:
        for line in path.read_text().splitlines():
            line = line.strip()
            if line:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass
    except OSError:
        pass
    return out


def ts(s: str | None) -> dt.datetime | None:
    if not s:
        return None
    try:
        t = dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return None
    return t if t.tzinfo else t.replace(tzinfo=dt.timezone.utc)


def ms(seconds) -> float | None:
    return None if seconds is None else round(float(seconds) * 1000, 1)


def fmt(v, digits: int = 1) -> str:
    if v is None:
        return "–"
    if isinstance(v, float):
        return f"{v:,.{digits}f}"
    if isinstance(v, int):
        return f"{v:,}"
    return str(v)


def quantile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    s = sorted(values)
    i = min(len(s) - 1, max(0, int(-(-q * len(s) // 1)) - 1))
    return s[i]


def median(values: list) -> float | None:
    vals = [v for v in values if v is not None]
    return statistics.median(vals) if vals else None


def table(headers: list[str], rows: list[list]) -> str:
    if not rows:
        return "_No data._"
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for r in rows:
        out.append("| " + " | ".join(fmt(c) for c in r) + " |")
    return "\n".join(out)


# --- the loadgen's CPU ---------------------------------------------------------------------
def vmstat_samples(path: Path) -> list[tuple[dt.datetime, float]]:
    """(time, busy %) per second from `vmstat -n -t 1` (TZ=UTC)."""
    samples = []
    idle_col = None
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return samples
    for line in lines:
        cols = line.split()
        if "id" in cols and "us" in cols:
            idle_col = cols.index("id")
            continue
        if idle_col is None or len(cols) < idle_col + 3 or not cols[0].isdigit():
            continue
        try:
            when = dt.datetime.strptime(f"{cols[-2]} {cols[-1]}", "%Y-%m-%d %H:%M:%S").replace(tzinfo=dt.timezone.utc)
            samples.append((when, 100.0 - float(cols[idle_col])))
        except ValueError:
            continue
    return samples


def cpu_in(samples, start, end) -> tuple[float | None, float | None]:
    if not start or not end:
        return None, None
    window = [b for t, b in samples if start <= t <= end]
    if not window:
        return None, None
    return round(statistics.fmean(window), 1), round(max(window), 1)


# --- one oha run -----------------------------------------------------------------------------
def oha_figures(doc: dict) -> dict:
    s = doc.get("summary", {})
    lp = doc.get("latencyPercentiles", {})
    codes = {int(k): int(v) for k, v in (doc.get("statusCodeDistribution") or {}).items() if str(k).isdigit()}
    transport = sum(int(v) for v in (doc.get("errorDistribution") or {}).values())
    ok = sum(n for c, n in codes.items() if 200 <= c < 300)
    total_time = s.get("total") or 0
    attempted = sum(codes.values()) + transport
    return {
        "requests": attempted,
        "ok": ok,
        "p50_ms": ms(lp.get("p50")),
        "p95_ms": ms(lp.get("p95")),
        "p99_ms": ms(lp.get("p99")),
        "max_ms": ms(s.get("slowest")),
        "ok_rps": round(ok / total_time, 1) if total_time else None,
        "err_429": codes.get(429, 0),
        "err_4xx": sum(n for c, n in codes.items() if 400 <= c < 500 and c != 429),
        "err_5xx": sum(n for c, n in codes.items() if 500 <= c < 600),
        "err_transport": transport,
    }


def rep_names(run: Path, scenario: str) -> list[str]:
    if scenario in CLOSED:
        names = []
        for p in run.glob(f"{scenario}-r*.json"):
            m = re.fullmatch(rf"{re.escape(scenario)}-r(\d+)", p.stem)
            if m:
                names.append((int(m.group(1)), p.stem))
        return [n for _, n in sorted(names)]
    if scenario in RATE:
        return [scenario] if (run / f"{scenario}.json").exists() else []
    return []


# --- one run directory ------------------------------------------------------------------------
def summarize_run(run: Path, prices: dict) -> dict:
    meta = load(run / "meta.json", {}) or {}
    done = load(run / "done.json", {}) or {}
    cfg = meta.get("config", {})
    reps_log = {r["name"]: r for r in jsonl(run / "reps.jsonl") if "name" in r}
    cpu = vmstat_samples(run / "loadgen-vmstat.log")
    out: dict = {
        "dir": run.name,
        "cloud": meta.get("cloud"),
        "row": meta.get("row"),
        "run_id": meta.get("run_id"),
        "region": meta.get("region"),
        "commit": meta.get("commit") or (run / "commit").read_text().strip() if (run / "commit").exists() else meta.get("commit"),
        "started": meta.get("started"),
        "finished": done.get("finished"),
        "loadgen": meta.get("loadgen", {}),
        "oha": meta.get("oha"),
        "scenario_status": done.get("scenarios", {}),
        "scenarios": {},
        "flags": [],
    }

    for scenario in CLOSED + RATE:
        reps = []
        for name in rep_names(run, scenario):
            doc = load(run / f"{name}.json")
            if not doc:
                out["flags"].append(f"{name}: oha JSON missing or unreadable")
                continue
            fig = oha_figures(doc)
            fig["rep"] = name
            r = reps_log.get(name, {})
            fig["cpu_mean"], fig["cpu_max"] = cpu_in(cpu, ts(r.get("start")), ts(r.get("end")))
            if fig["cpu_mean"] is not None and fig["cpu_mean"] > CPU_FLAG:
                fig["loadgen_bound"] = True
                out["flags"].append(f"{name}: loadgen mean CPU {fig['cpu_mean']}% (> {CPU_FLAG:.0f}%): possibly loadgen-bound")
            reps.append(fig)
        if reps:
            agg = {k: median([r[k] for r in reps]) for k in ("p50_ms", "p95_ms", "p99_ms", "ok_rps")}
            agg["max_ms"] = max((r["max_ms"] for r in reps if r["max_ms"] is not None), default=None)
            for k in ("requests", "ok", "err_429", "err_4xx", "err_5xx", "err_transport"):
                agg[k] = sum(r[k] for r in reps)
            out["scenarios"][scenario] = {"reps": reps, "summary": agg}

    # cold starts: a probe counts as cold only when the app says it was its instance's first request
    probes = [p for p in jsonl(run / "cold.jsonl") if "probe" in p and "warm_after" not in p]
    warm = [p["warm_after"] for p in jsonl(run / "cold.jsonl") if "warm_after" in p]
    real = [p for p in probes if isinstance(p.get("app"), dict) and p["app"].get("first") is True]
    if probes:
        out["cold"] = {
            "probes": len(probes),
            "cold": len(real),
            "not_cold": len(probes) - len(real),
            "ttfb_p50_ms": ms(median([p.get("ttfb_s") for p in real])),
            "ttfb_max_ms": ms(max((p.get("ttfb_s") for p in real if p.get("ttfb_s") is not None), default=None)),
            "warm_ttfb_p50_ms": ms(median([w.get("ttfb_s") for w in warm])),
            "errors": sum(1 for p in probes if not (200 <= int(p.get("code") or 0) < 300)),
        }

    q = load(run / "queue-stats.json")
    if q:
        lag = q.get("lag_ms") or {}
        out["queue"] = {
            "sent": q.get("sent"),
            "received": q.get("received"),
            "lag_p50_ms": lag.get("p50"),
            "lag_p95_ms": lag.get("p95"),
            "lag_p99_ms": lag.get("p99"),
            "lag_max_ms": lag.get("max"),
        }

    out["cost"] = cost(run, out, prices)
    sweeps = sorted(run.glob("sweep-*.txt"))
    out["sweep"] = [
        {"file": s.name, "clean": "result: clean" in s.read_text(errors="replace")} for s in sweeps
    ]
    if sweeps and not all(s["clean"] for s in out["sweep"]):
        out["flags"].append("sweep not clean: see sweep-*.txt")
    for s, status in out["scenario_status"].items():
        if status != "ok":
            out["flags"].append(f"scenario {s}: {status}")
    return out


def cost(run: Path, summary: dict, prices: dict) -> dict:
    cloud = summary.get("cloud")
    usage_doc = load(run / "metrics/usage.json", {}) or {}
    usage = dict(usage_doc.get("usage") or {})
    table_ = prices.get(cloud or "", {})
    start, end = ts(summary.get("started")), ts(summary.get("finished"))
    if start and end:
        usage["loadgen_hours"] = round((end - start).total_seconds() / 3600, 3)
    lines = []
    list_usd = beyond_usd = 0.0
    for key, price in table_.items():
        if key not in usage or not isinstance(usage[key], (int, float)):
            continue
        cond = price.get("only_if") or {}
        if any(usage.get(k) != v for k, v in cond.items()):
            continue
        amount = float(usage[key])
        lst = amount * price["unit_usd"]
        beyond = max(0.0, amount - price.get("free", 0)) * price["unit_usd"]
        list_usd += lst
        beyond_usd += beyond
        lines.append({"key": key, "usage": amount, "unit_usd": price["unit_usd"], "free": price.get("free", 0),
                      "period": price.get("period"), "list_usd": round(lst, 4), "after_free_usd": round(beyond, 4)})
    requests = usage.get(prices.get("requests_key", {}).get(cloud or "", ""), 0) or 0
    billed = load(run / "billed.json")
    out = {
        "usage_source": "metrics/usage.json" if usage_doc else None,
        "lines": lines,
        "list_usd": round(list_usd, 4) if lines else None,
        "after_free_usd": round(beyond_usd, 4) if lines else None,
        "requests": requests,
        "list_usd_per_million": round(list_usd / requests * 1e6, 4) if requests and lines else None,
        "billed_usd": billed.get("usd") if billed else None,
        "billed_source": billed.get("source") if billed else None,
        "prices_read": prices.get("read"),
    }
    if out["billed_usd"] is not None and out["after_free_usd"] is not None:
        diff = out["billed_usd"] - out["after_free_usd"]
        if abs(diff) > max(0.05, 0.25 * max(out["billed_usd"], out["after_free_usd"])):
            out["disagreement"] = f"billed ${out['billed_usd']:.2f} vs usage x price ${out['after_free_usd']:.2f}: reported, not reconciled"
    return out


# --- completeness (the dry run's gate) ------------------------------------------------------------
def missing(run: Path) -> list[str]:
    meta = load(run / "meta.json")
    if not meta:
        return ["meta.json"]
    cfg = meta.get("config", {})
    scenarios = (cfg.get("scenarios") or "").split()
    reps = int(cfg.get("reps", 3))
    need = ["done.json", "reps.jsonl", "loadgen-vmstat.log", "run.log"]
    for s in scenarios:
        if s == "preload":
            need.append("preload.txt")
        elif s == "cold":
            need.append("cold.jsonl")
        elif s in CLOSED:
            for r in range(1, reps + 1):
                for n in (f"{s}-r{r}", f"{s}-r{r}-warmup"):
                    need += [f"{n}.json"]
        elif s in RATE:
            need.append(f"{s}.json")
        elif s == "queue":
            need += ["queue-send.json", "queue-stats.json"]
    gone = [n for n in need if not (run / n).exists()]
    # per-request SQLite next to every oha JSON (compressed after download)
    for j in run.glob("*.json"):
        if load(j, {}) and "summary" in (load(j, {}) or {}):
            if not ((run / f"{j.stem}.db").exists() or (run / f"{j.stem}.db.gz").exists()):
                gone.append(f"{j.stem}.db")
    done = load(run / "done.json", {}) or {}
    for s in scenarios:
        if done.get("scenarios", {}).get(s) != "ok":
            gone.append(f"scenario {s} not ok ({done.get('scenarios', {}).get(s)})")
    return gone


# --- the README ----------------------------------------------------------------------------------
def render(runs: list[dict], template: Path) -> str:
    latency_rows = []
    for r in runs:
        for s in CLOSED + RATE:
            sc = r["scenarios"].get(s)
            if not sc:
                continue
            a = sc["summary"]
            errors = f"{a['err_429']}/{a['err_4xx']}/{a['err_5xx']}/{a['err_transport']}"
            latency_rows.append([r["row"], s, len(sc["reps"]), a["p50_ms"], a["p95_ms"], a["p99_ms"], a["max_ms"], a["ok_rps"], errors])
    cold_rows = [
        [r["row"], c["probes"], c["cold"], c["ttfb_p50_ms"], c["ttfb_max_ms"], c["warm_ttfb_p50_ms"], c["errors"]]
        for r in runs
        if (c := r.get("cold"))
    ]
    queue_rows = [
        [r["row"], q["sent"], q["received"], q["lag_p50_ms"], q["lag_p95_ms"], q["lag_p99_ms"], q["lag_max_ms"]]
        for r in runs
        if (q := r.get("queue"))
    ]
    cost_rows = []
    for r in runs:
        c = r["cost"]
        cost_rows.append([r["row"], c["requests"] or None, c["list_usd"], c["after_free_usd"], c["list_usd_per_million"],
                          c["billed_usd"], c.get("disagreement", "")])
    run_rows = [
        [f"[{r['dir']}]({r['dir']}/)", r["cloud"], r["region"], r["run_id"], (r["commit"] or "")[:12], r["started"], r["finished"],
         (r["loadgen"] or {}).get("instance_type"), "clean" if r["sweep"] and all(s["clean"] for s in r["sweep"]) else ("not clean" if r["sweep"] else "–")]
        for r in runs
    ]
    flags = [f"- `{r['dir']}`: {f}" for r in runs for f in r["flags"]]
    values = {
        "generated": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d %H:%M UTC"),
        "run_count": str(len(runs)),
        "runs_table": table(["Run", "Cloud", "Region", "Run id", "Commit", "Started", "Finished", "Loadgen", "Sweep"], run_rows),
        "latency_table": table(["Row", "Scenario", "Reps", "p50 ms", "p95 ms", "p99 ms", "max ms", "ok req/s", "429/4xx/5xx/transport"], latency_rows),
        "cold_table": table(["Row", "Probes", "Cold (app says first)", "TTFB p50 ms", "TTFB max ms", "Warm TTFB p50 ms", "Errors"], cold_rows),
        "queue_table": table(["Row", "Sent", "Received", "Lag p50 ms", "Lag p95 ms", "Lag p99 ms", "Lag max ms"], queue_rows),
        "cost_table": table(["Row", "Requests (cloud)", "List USD", "After free tier USD", "List USD / M requests", "Billed USD", "Note"], cost_rows),
        "flags": "\n".join(flags) if flags else "_None._",
    }
    return Template(template.read_text()).safe_substitute(values)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--results", type=Path, default=DEFAULT_RESULTS)
    p.add_argument("--template", type=Path, default=HERE / "report/README.md.tmpl")
    p.add_argument("--prices", type=Path, default=HERE / "prices.json")
    p.add_argument("--check", type=Path)
    p.add_argument("--stdout", action="store_true")
    args = p.parse_args(argv)

    if args.check:
        gone = missing(args.check)
        if gone:
            print(f"{args.check}: incomplete:\n  " + "\n  ".join(gone), file=sys.stderr)
            return 1
        print(f"{args.check}: complete")
        return 0

    prices = load(args.prices, {}) or {}
    runs = []
    for d in sorted(x for x in args.results.iterdir() if x.is_dir() and (x / "meta.json").exists()) if args.results.exists() else []:
        s = summarize_run(d, prices)
        (d / "summary.json").write_text(json.dumps(s, indent=2, default=str) + "\n")
        runs.append(s)
    text = render(runs, args.template)
    if args.stdout:
        sys.stdout.write(text)
    else:
        args.results.mkdir(parents=True, exist_ok=True)
        (args.results / "README.md").write_text(text)
        print(f"{args.results / 'README.md'}: {len(runs)} run(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
