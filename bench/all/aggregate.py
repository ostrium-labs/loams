#!/usr/bin/env python3
"""Aggregate one round of the BQ1 combined run (bench/all/run.sh).

Reads the round directory (slots.tsv, <slot>.jsonl, <slot>.started/.ended,
samples.txt) and writes <out>.json and <out>.md: per-engine QPS, the total
aggregate QPS, and efficiency per budgeted core and per busy core.

CPU accounting: every engine is pinned to its own cores (budget.sh), so an
engine's CPU is the busy time of its cores over the window, read from
/proc/stat samples. A core that several slots share (a load core, or the
in-process engines' low-power cores) is split evenly between them.
"""
import argparse
import json
import os
import platform
from collections import defaultdict


def cpu_list(spec):
    out = []
    for part in str(spec).split(","):
        part = part.strip()
        if not part:
            continue
        a, _, b = part.partition("-")
        out.extend(range(int(a), int(b or a) + 1))
    return out


def read_samples(path):
    """[(t, load1, {cpu_name: (busy, total, iowait)})]"""
    samples, cur = [], None
    if not os.path.exists(path):
        return samples
    for line in open(path):
        f = line.split()
        if not f:
            continue
        if not f[0].startswith("cpu"):
            cur = (float(f[0]), float(f[1]), {}, float(f[2]) if len(f) > 2 else None)
            samples.append(cur)
            continue
        v = list(map(int, f[1:9]))
        idle, iowait = v[3] + v[4], v[4]
        total = sum(v)
        cur[2][f[0]] = (total - idle, total, iowait)
    return [s for s in samples if "cpu" in s[2]]


def window_stats(samples, t0, t1):
    inside = [s for s in samples if t0 - 0.5 <= s[0] <= t1 + 0.5]
    if len(inside) < 2:
        return None
    a, b = inside[0], inside[-1]
    secs = b[0] - a[0]
    per_core = {}
    for name, (busy1, tot1, _) in b[2].items():
        if name == "cpu" or name not in a[2]:
            continue
        busy0, tot0, _ = a[2][name]
        per_core[int(name[3:])] = (busy1 - busy0) / max(1, tot1 - tot0)
    busy0, tot0, io0 = a[2]["cpu"]
    busy1, tot1, io1 = b[2]["cpu"]
    loads = [s[1] for s in inside]
    temps = [s[3] for s in inside if len(s) > 3 and s[3]]
    thermal = {}
    if temps:
        thermal = {"cpu_temp_c_mean": round(sum(temps) / len(temps), 1),
                   "cpu_temp_c_max": max(temps)}
    return {**thermal,
        "seconds": round(secs, 1),
        "per_core_busy": {str(k): round(v, 3) for k, v in sorted(per_core.items())},
        "busy_pct": round(100 * (busy1 - busy0) / max(1, tot1 - tot0), 1),
        "iowait_pct": round(100 * (io1 - io0) / max(1, tot1 - tot0), 1),
        "loadavg_1m_mean": round(sum(loads) / len(loads), 2),
        "loadavg_1m_max": round(max(loads), 2),
    }


def read_time(path):
    try:
        parts = open(path).read().split()
    except OSError:
        return None, None
    if len(parts) == 1:
        return None, float(parts[0])
    return int(parts[0]), float(parts[1])


def fmt(v, digits=0):
    if v is None:
        return "–"
    if isinstance(v, (int, float)):
        if digits == 0:
            return f"{v:,.0f}"
        return f"{v:,.{digits}f}"
    return str(v)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--t0", type=float, required=True)
    ap.add_argument("--duration", type=float, required=True)
    ap.add_argument("--depth", type=int, required=True)
    ap.add_argument("--commit", default="")
    ap.add_argument("--label", default="")
    ap.add_argument("--start-temp", type=float, default=None)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    t0, t1 = a.t0, a.t0 + a.duration

    rows = []
    for line in open(os.path.join(a.dir, "slots.tsv")):
        f = line.rstrip("\n").split("\t")
        rows.append(dict(slot=f[0], engine=f[1], script=f[2], own=f[3], load=f[4],
                         align=f[5], sha=f[6], args=f[7] if len(f) > 7 else ""))

    # Who uses each core.
    users = defaultdict(set)
    for r in rows:
        if r["script"].startswith("missing:"):
            continue
        for c in set(cpu_list(r["own"]) + cpu_list(r["load"])):
            users[c].add(r["slot"])

    samples = read_samples(os.path.join(a.dir, "samples.txt"))
    host = window_stats(samples, t0, t1) or {}
    # A run that started at 70 C or more, or peaked at 95 C or more, ran
    # throttled (or close to it): its numbers are not headline numbers.
    host["cpu_temp_c_start"] = a.start_temp
    hot = (a.start_temp is not None and a.start_temp >= 70) or (host.get("cpu_temp_c_max") or 0) >= 95
    host["thermal"] = "throttled" if hot else ("ok" if host.get("cpu_temp_c_max") else "unknown")
    per_core = {int(k): v for k, v in host.get("per_core_busy", {}).items()}

    engines, missing, failed = [], [], []
    for r in rows:
        if r["script"].startswith("missing:"):
            missing.append({"slot": r["slot"], "script": r["script"][8:]})
            continue
        cores = set(cpu_list(r["own"]) + cpu_list(r["load"]))
        budget = sum(1 / len(users[c]) for c in cores)
        busy = sum(per_core.get(c, 0) / len(users[c]) for c in cores) if per_core else None
        _, started = read_time(os.path.join(a.dir, r["slot"] + ".started"))
        rc, ended = read_time(os.path.join(a.dir, r["slot"] + ".ended"))
        lines = []
        path = os.path.join(a.dir, r["slot"] + ".jsonl")
        if os.path.exists(path):
            for raw in open(path):
                raw = raw.strip()
                if raw.startswith("{"):
                    try:
                        lines.append(json.loads(raw))
                    except json.JSONDecodeError:
                        pass
        if not lines:
            tail = ""
            log = os.path.join(a.dir, r["slot"] + ".log")
            if os.path.exists(log):
                tail = "".join(open(log, errors="replace").readlines()[-5:])[-600:]
            failed.append({"slot": r["slot"], "rc": rc, "log_tail": tail})
            continue
        # The measured window: the script's own, else [end - duration, end].
        ws = [l.get("t_start") for l in lines if l.get("t_start")]
        we = [l.get("t_end") for l in lines if l.get("t_end")]
        if ws and we:
            w0, w1, wsrc = min(ws), max(we), "reported"
        elif ended:
            w0, w1, wsrc = ended - a.duration, ended, "estimated from exit"
        else:
            w0 = w1 = None
            wsrc = "unknown"
        # Overlap: intersection over union of the engine's window and
        # [T0, T0 + duration]; 1.0 means it ran exactly in the common window.
        overlap = None
        if w0 is not None:
            inter = max(0.0, min(w1, t1) - max(w0, t0))
            union = max(w1, t1) - min(w0, t0)
            overlap = inter / union if union > 0 else None
        native = [l for l in lines if l.get("note") != "compat"]
        qps = sum(float(l.get("qps") or 0) for l in native)
        engines.append({
            "slot": r["slot"], "engine": r["engine"], "script": r["script"], "script_sha256_12": r["sha"],
            "args": r["args"], "own_cores": r["own"], "load_cores": r["load"],
            "budget_cores": round(budget, 2),
            "busy_cores": round(busy, 2) if busy is not None else None,
            "busy_core_seconds": round(busy * host.get("seconds", a.duration), 1) if busy is not None else None,
            "qps": round(qps),
            "qps_per_budget_core": round(qps / budget) if budget else None,
            "qps_per_busy_core": round(qps / busy) if busy else None,
            "window": {"start": w0, "end": w1, "source": wsrc,
                       "overlap_with_t0_window": round(overlap, 2) if overlap is not None else None},
            "started": started, "ended": ended, "rc": rc,
            "ops": lines,
        })

    total_qps = sum(e["qps"] for e in engines)
    total_busy = sum(e["busy_cores"] or 0 for e in engines)
    result = {
        "bench": "bq1-all combined run",
        "label": a.label, "commit": a.commit, "depth": a.depth, "duration_s": a.duration,
        "t0": t0, "host": dict(host, cpu=platform.processor() or "Intel Core Ultra 5 225H",
                                kernel=platform.release(), cores=14),
        "total": {
            "qps": round(total_qps),
            "qps_per_core": round(total_qps / 14),
            "busy_cores": round(total_busy, 2),
            "qps_per_busy_core": round(total_qps / total_busy) if total_busy else None,
            "engines_measured": len(engines),
        },
        "engines": engines, "missing": missing, "failed": failed,
    }
    with open(a.out + ".json", "w") as fh:
        json.dump(result, fh, indent=1)
        fh.write("\n")

    md = [f"# BQ1 combined run, depth {a.depth} ({os.path.basename(a.out)})", "",
          f"Commit `{a.commit}`. Window {a.duration:.0f} s. "
          f"Host load average over the window: {fmt(host.get('loadavg_1m_mean'), 2)} mean, "
          f"{fmt(host.get('loadavg_1m_max'), 2)} max; iowait {fmt(host.get('iowait_pct'), 1)} %; "
          f"CPU {fmt(host.get('cpu_temp_c_start'), 0)} °C at the start, {fmt(host.get('cpu_temp_c_mean'), 0)} °C mean, "
          f"{fmt(host.get('cpu_temp_c_max'), 0)} °C max (thermal: {host.get('thermal')}); "
          f"all 14 cores {fmt(host.get('busy_pct'), 1)} % busy.", "",
          "| Engine (slot) | Op | QPS | p50 ms | p99 ms | p99.9 ms | Errors | Budget cores | Busy cores | QPS per busy core | Window overlap |",
          "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for e in engines:
        for i, l in enumerate(e["ops"]):
            first = i == 0
            md.append("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |".format(
                f"{e['engine']} ({e['slot']})" if first else "", l.get("op"), fmt(l.get("qps")),
                fmt(l.get("p50_ms"), 3), fmt(l.get("p99_ms"), 3), fmt(l.get("p999_ms"), 3),
                fmt(l.get("errors")), fmt(e["budget_cores"], 2) if first else "",
                fmt(e["busy_cores"], 2) if first else "", fmt(e["qps_per_busy_core"]) if first else "",
                fmt(e["window"]["overlap_with_t0_window"], 2) if first else ""))
    md.append("| **Total** | | **{}** | | | | | 14 | {} | {} | |".format(
        fmt(total_qps), fmt(total_busy, 2), fmt(result["total"]["qps_per_busy_core"])))
    if missing or failed:
        md.append("")
        for m in missing:
            md.append(f"- Not run: {m['slot']} (no `{m['script']}`).")
        for f in failed:
            md.append(f"- Failed: {f['slot']} (exit {f['rc']}), no result line.")
    with open(a.out + ".md", "w") as fh:
        fh.write("\n".join(md) + "\n")
    print("\n".join(md))


if __name__ == "__main__":
    main()
