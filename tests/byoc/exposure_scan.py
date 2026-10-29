#!/usr/bin/env python3
"""TCP connect scan used by exposure.sh. Exit 1 if any scanned port is open."""
import argparse
import asyncio
import sys

COMMON = [
    1080, 1194, 1433, 1521, 1723, 2049, 2181, 2375, 2376, 2379, 2380, 3000, 3306, 3389,
    4000, 4222, 4369, 5000, 5432, 5601, 5672, 5900, 5984, 6000, 6379, 6443, 6666, 7000,
    7001, 7077, 7474, 7680, 7687, 7781, 8000, 8001, 8008, 8080, 8081, 8086, 8088, 8090,
    8091, 8123, 8200, 8443, 8500, 8888, 9000, 9042, 9090, 9092, 9100, 9200, 9300, 9418,
    9999, 10000, 10250, 11211, 15672, 20160, 20180, 27017, 41641, 50000, 50051, 50070,
]
ENGINE = [8080, 8443, 6333, 9200, 5432, 4000, 7781, 3000, 7680, 9000, 8090]


async def probe(host, port, timeout, sem):
    async with sem:
        try:
            _, w = await asyncio.wait_for(asyncio.open_connection(host, port), timeout)
        except (OSError, asyncio.TimeoutError):
            return None
        w.close()
        try:
            await w.wait_closed()
        except OSError:
            pass
        return port


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("host")
    ap.add_argument("--timeout", type=float, default=2.0)
    ap.add_argument("--extra", default="")
    ap.add_argument("--concurrency", type=int, default=200)
    a = ap.parse_args()
    ports = set(range(1, 1101)) | set(COMMON) | set(ENGINE)
    ports |= {int(p) for p in a.extra.split() if p.isdigit()}
    sem = asyncio.Semaphore(a.concurrency)
    found = sorted(p for p in await asyncio.gather(*[probe(a.host, p, a.timeout, sem) for p in sorted(ports)]) if p)
    print(f"exposure: scanned {len(ports)} TCP ports on {a.host}")
    if found:
        print(f"FAIL: open ports: {' '.join(map(str, found))}")
        return 1
    print("ok: no open port")
    return 0


sys.exit(asyncio.run(main()))
