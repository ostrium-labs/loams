#!/usr/bin/env python3
"""HTTP/1.1 pipelining load generator for the House (BQ1 pipelining rule).

Opens C keep-alive connections; on each, keeps up to DEPTH requests in flight
(written back to back, read in order), for SECS seconds. Requests are GETs of
point lookups with random ids. Prints JSON: requests/s, ok, errors, and
latency percentiles (request write to response end, ms).

Usage: pipeline.py <port> <key> <conns> <depth> <secs> <max_id> [sql_template]
"""
import asyncio, json, random, sys, time, urllib.parse

port, key = int(sys.argv[1]), sys.argv[2]
conns, depth, secs, max_id = map(int, sys.argv[3:7])
tmpl = sys.argv[7] if len(sys.argv) > 7 else "SELECT v, s FROM kv WHERE id = {}"
lat, ok, bad = [], 0, 0

def request():
    q = urllib.parse.quote(tmpl.format(random.randrange(max_id)))
    return (f"GET /?query={q} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ClickHouse-User: default\r\n"
            f"X-ClickHouse-Key: {key}\r\n\r\n").encode()

async def read_response(r):
    status = int((await r.readline()).split()[1])
    length, chunked = None, False
    while True:
        line = await r.readline()
        if line in (b"\r\n", b""):
            break
        k, _, v = line.decode().partition(":")
        k = k.strip().lower()
        if k == "content-length":
            length = int(v)
        elif k == "transfer-encoding" and "chunked" in v:
            chunked = True
    if chunked:
        while True:
            n = int((await r.readline()).strip(), 16)
            await r.readexactly(n + 2)
            if n == 0:
                break
    elif length:
        await r.readexactly(length)
    return status

async def conn(deadline):
    global ok, bad
    r, w = await asyncio.open_connection("127.0.0.1", port)
    sent = []
    while time.monotonic() < deadline or sent:
        while len(sent) < depth and time.monotonic() < deadline:
            w.write(request()); sent.append(time.monotonic())
        await w.drain()
        status = await read_response(r)
        t0 = sent.pop(0)
        lat.append((time.monotonic() - t0) * 1000)
        if status == 200:
            ok += 1
        else:
            bad += 1
    w.close()

async def main():
    deadline = time.monotonic() + secs
    t0 = time.monotonic()
    await asyncio.gather(*(conn(deadline) for _ in range(conns)))
    el = time.monotonic() - t0
    lat.sort()
    p = lambda q: round(lat[min(len(lat) - 1, int(len(lat) * q))], 3) if lat else None
    print(json.dumps({"conns": conns, "depth": depth, "seconds": round(el, 2),
                      "qps": round((ok + bad) / el, 1), "ok": ok, "errors": bad,
                      "p50_ms": p(0.5), "p99_ms": p(0.99), "p99_9_ms": p(0.999)}))

asyncio.run(main())
