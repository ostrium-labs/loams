#!/usr/bin/env python3
"""Helpers for e2e.sh (standard library only).

  cfb.py produce RUN N RATE            lines "key<TAB>value" for kafka-console-producer
  cfb.py watermarks                    the Loams stream's high watermarks (JSON)
  cfb.py poll RUN N OUT START_JSON     long-polls the Loams stream, writes observations
  cfb.py report RUN N LOAMS OUT META   the JSON summary

Engine settings come from the environment: ENGINE_URL, ENGINE_KEY_FILE,
CFB_NAMESPACE, CFB_STREAM, CFB_PARTITIONS.
"""
import base64
import datetime as dt
import json
import os
import sys
import threading
import time
import urllib.request


def now_ms():
    return time.time_ns() // 1_000_000


def produce(run, n, rate):
    n, rate = int(n), float(rate)
    start = time.monotonic()
    out = sys.stdout
    for i in range(n):
        due = start + i / rate
        d = due - time.monotonic()
        if d > 0:
            time.sleep(d)
        out.write(f"{run}-{i}\t" + json.dumps({"run": run, "seq": i, "ts": now_ms()}, separators=(",", ":")) + "\n")
        out.flush()


def engine(method, body, timeout=40):
    key = open(os.environ["ENGINE_KEY_FILE"]).read().strip()
    req = urllib.request.Request(
        f"{os.environ['ENGINE_URL']}/loams.stream.v1.StreamService/{method}",
        data=json.dumps(body).encode(),
        headers={
            "content-type": "application/json",
            "connect-protocol-version": "1",
            "authorization": f"Bearer {key}",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def stream_args():
    return os.environ.get("CFB_NAMESPACE", "default"), os.environ["CFB_STREAM"], int(os.environ.get("CFB_PARTITIONS", "1"))


def watermarks():
    ns, stream, parts = stream_args()
    hw = {}
    for p in range(parts):
        v = engine("FetchCloudEvents", {"namespace": ns, "stream": stream, "partition": p, "offset": "0", "maxBytes": 1, "maxWaitMs": 0})
        hw[p] = int(v.get("highWatermark", 0))
    print(json.dumps(hw))


def parse_time(s):
    if not s:
        return None
    s = s.replace("Z", "+00:00")
    return int(dt.datetime.fromisoformat(s).timestamp() * 1000)


def event_payload(ev):
    if "data_base64" in ev:
        raw = base64.b64decode(ev["data_base64"])
    elif isinstance(ev.get("data"), str):
        raw = ev["data"].encode()
    else:
        raw = json.dumps(ev.get("data")).encode()
    try:
        return json.loads(raw)
    except ValueError:
        return None


def poll(run, n, out_path, start_json):
    ns, stream, parts = stream_args()
    start = {int(k): v for k, v in json.loads(start_json).items()}
    n = int(n)
    seen = set()
    lock = threading.Lock()
    stop = threading.Event()
    out = open(out_path, "w", buffering=1)

    def worker(p):
        off = start.get(p, 0)
        while not stop.is_set():
            try:
                v = engine("FetchCloudEvents", {"namespace": ns, "stream": stream, "partition": p,
                                                "offset": str(off), "maxBytes": 4 << 20, "maxWaitMs": 1000})
            except Exception as e:  # noqa: BLE001 - keep polling through engine hiccups
                print(f"poll p{p}: {e}", file=sys.stderr)
                time.sleep(0.2)
                continue
            t = now_ms()
            batch = json.loads(base64.b64decode(v.get("jsonBatch", "")) or b"[]")
            for k, ev in enumerate(batch):
                d = event_payload(ev)
                if not isinstance(d, dict) or d.get("run") != run:
                    continue
                with lock:
                    out.write(json.dumps({"seq": d["seq"], "ts": d["ts"], "obs": t, "time": parse_time(ev.get("time")),
                                          "p": p, "off": off + k, "id": ev.get("id")}) + "\n")
                    seen.add(d["seq"])
            off = int(v.get("nextOffset", off))

    ts = [threading.Thread(target=worker, args=(p,), daemon=True) for p in range(parts)]
    for t in ts:
        t.start()
    stopfile = out_path + ".stop"
    while not stop.is_set():
        if os.path.exists(stopfile):
            stop.set()
        time.sleep(0.1)
    time.sleep(0.2)
    out.close()


def pct(xs, q):
    if not xs:
        return None
    xs = sorted(xs)
    i = min(len(xs) - 1, max(0, int(round(q / 100 * (len(xs) - 1)))))
    return xs[i]


def stats(xs):
    return {"n": len(xs), "p50_ms": pct(xs, 50), "p99_ms": pct(xs, 99), "max_ms": max(xs) if xs else None}


def report(run, n, loams_path, out_path, meta_json):
    n = int(n)
    meta = json.loads(meta_json)
    loams = {}
    loams_dups = 0
    for line in open(loams_path):
        r = json.loads(line)
        if r["seq"] in loams:
            loams_dups += 1
            continue
        loams[r["seq"]] = r
    out = {}
    out_dups = 0
    for line in open(out_path, errors="replace"):
        line = line.rstrip("\n")
        if "\t" not in line:
            continue
        tsf, val = line.split("\t", 1)
        try:
            kind, ms = tsf.split(":", 1)
            d = json.loads(val)
        except ValueError:
            continue
        if not isinstance(d, dict) or d.get("run") != run:
            continue
        if d["seq"] in out:
            out_dups += 1
            continue
        out[d["seq"]] = {"append": int(ms), "kind": kind}
    want = set(range(n))
    in_loams, in_out = set(loams), set(out)
    k2l = [loams[s]["obs"] - loams[s]["time"] for s in loams if loams[s]["time"]]
    prod = [loams[s]["time"] - loams[s]["ts"] for s in loams if loams[s]["time"]]
    l2k = [out[s]["append"] - loams[s]["obs"] for s in in_out & in_loams]
    e2e = [out[s]["append"] - loams[s]["time"] for s in in_out & in_loams if loams[s]["time"]]
    kinds = sorted({v["kind"] for v in out.values()})
    summary = {
        "run": run,
        "kafka": meta.get("kafka"),
        "records_produced": n,
        "loams_stream": {"count": len(in_loams), "duplicates": loams_dups, "missing": len(want - in_loams)},
        "output_topic": {"count": len(in_out), "duplicates": out_dups, "missing": len(want - in_out),
                         "isolation": "read_committed", "timestamp_type": kinds},
        "exactly_once": loams_dups == 0 and out_dups == 0 and not (want - in_loams) and not (want - in_out)
                        and not any((meta.get("dlq_events") or {}).values()),
        "latency_ms": {
            "producer_to_input_topic": stats(prod),
            "input_topic_to_loams_stream": stats(k2l),
            "loams_stream_to_output_topic": stats(l2k),
            "end_to_end": stats(e2e),
        },
        **{k: v for k, v in meta.items() if k != "kafka"},
    }
    if want - in_out:
        summary["output_topic"]["missing_sample"] = sorted(want - in_out)[:10]
    print(json.dumps(summary, indent=2))
    return 0 if summary["exactly_once"] else 1


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    rc = {"produce": produce, "watermarks": watermarks, "poll": poll, "report": report}[cmd](*args)
    sys.exit(rc or 0)
