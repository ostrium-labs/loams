#!/usr/bin/env python3
"""Kafka-wire load generator for the Iggy gateway (BQ1-iggy). Uses librdkafka (confluent-kafka)
for produce/consume and a raw pipelined Metadata/ApiVersions client for request QPS.
Run under benchslot.sh and taskset. One JSON line per scenario on stdout.
  produce  --procs N --size B --seconds S --acks 1|all --linger-ms --batch-size
  consume  (preloads --preload msgs then reads them back)  --procs N
  metadata --procs N --conns C --depth D --seconds S
"""
import argparse, asyncio, json, multiprocessing as mp, os, struct, sys, time

def admin_topic(bs, topic, parts):
    from confluent_kafka.admin import AdminClient, NewTopic
    a = AdminClient({"bootstrap.servers": bs})
    for t, f in a.create_topics([NewTopic(topic, num_partitions=parts, replication_factor=1)]).items():
        try: f.result(60)
        except Exception as e:
            if "EXISTS" not in str(e).upper(): raise
    for _ in range(60):
        md = a.list_topics(topic, timeout=10).topics.get(topic)
        if md and not md.error and len(md.partitions) == parts: return
        time.sleep(0.5)

def produce_worker(a, idx, q, start_evt):
    from confluent_kafka import Producer
    stats = {"tx_reqs": 0, "txmsgs": 0}
    def scb(js):
        d = json.loads(js); stats["tx_reqs"] = d.get("tx", 0); stats["txmsgs"] = d.get("txmsgs", 0)
    p = Producer({"bootstrap.servers": a.bootstrap, "acks": a.acks, "linger.ms": a.linger_ms,
                  "batch.size": a.batch_size, "queue.buffering.max.messages": 500000,
                  "max.in.flight.requests.per.connection": a.in_flight,
                  "queue.buffering.max.kbytes": 1048576, "compression.type": a.compression,
                  "enable.idempotence": False, "statistics.interval.ms": 1000, "stats_cb": scb,
                  "client.id": f"bq1-p{idx}"})
    payload = os.urandom(a.size); acked = [0]; errs = [0]
    def cb(err, msg):
        if err: errs[0] += 1
        else: acked[0] += 1
    start_evt.wait(); t0 = time.time(); n = 0
    while time.time() - t0 < a.seconds:
        for _ in range(500):
            try: p.produce(a.topic, payload, key=None, on_delivery=cb); n += 1
            except BufferError: p.poll(0.01)
        p.poll(0)
    p.flush(60); dur = time.time() - t0; p.poll(1.2)
    q.put({"sent": n, "acked": acked[0], "errors": errs[0], "dur": dur, **stats})

def consume_worker(a, idx, q, start_evt, expect):
    from confluent_kafka import Consumer, TopicPartition
    c = Consumer({"bootstrap.servers": a.bootstrap, "group.id": f"bq1-{a.run}-{idx}", "enable.auto.commit": False,
                  "fetch.min.bytes": 1, "fetch.wait.max.ms": 50, "fetch.message.max.bytes": 4194304,
                  "max.partition.fetch.bytes": 4194304, "queued.max.messages.kbytes": 262144, "client.id": f"bq1-c{idx}"})
    got = 0; bytes_ = 0
    c.assign([TopicPartition(a.topic, p, -2) for p in range(idx, a.partitions, a.procs)])  # beginning; no group rebalances
    start_evt.wait(); t0 = time.time(); last = t0
    while time.time() - t0 < a.seconds + 60:
        ms = c.consume(num_messages=10000, timeout=0.5)
        for m in ms:
            if not m.error(): got += 1; bytes_ += len(m.value() or b"")
        if ms: last = time.time()
        if got >= expect[idx] or time.time() - last > 5: break
    dur = (last - t0) if got else 0.0
    c.close(); q.put({"got": got, "bytes": bytes_, "dur": dur})

def req(api, ver, corr, body):
    h = struct.pack(">hhi", api, ver, corr) + struct.pack(">h", 6) + b"bq1met"
    m = h + body; return struct.pack(">i", len(m)) + m

async def meta_conn(host, port, topic, depth, t_end, counter):
    r, w = await asyncio.open_connection(host, port)
    tb = topic.encode()
    body = struct.pack(">i", 1) + struct.pack(">h", len(tb)) + tb          # Metadata v1, one topic
    corr = 0; inflight = 0
    async def send(): 
        nonlocal corr, inflight
        corr += 1; w.write(req(3, 1, corr, body)); inflight += 1
    for _ in range(depth): await send()
    await w.drain()
    while time.time() < t_end:
        n = struct.unpack(">i", await r.readexactly(4))[0]; await r.readexactly(n)
        inflight -= 1; counter[0] += 1; await send()
        if counter[0] % 32 == 0: await w.drain()
    w.close()

def meta_worker(a, idx, q, start_evt):
    host, port = a.bootstrap.split(":"); port = int(port)
    start_evt.wait(); t0 = time.time(); c = [0]
    async def main():
        await asyncio.gather(*[meta_conn(host, port, a.topic, a.depth, t0 + a.seconds, c) for _ in range(a.conns)],
                             return_exceptions=True)
    try: asyncio.run(main())
    except Exception as e: print("meta err", e, file=sys.stderr)
    q.put({"reqs": c[0], "dur": time.time() - t0})

def run(a, target, extra=()):
    q = mp.Queue(); evt = mp.Event(); ps = []
    for i in range(a.procs):
        ps.append(mp.Process(target=target, args=(a, i, q, evt) + ((extra[0],) if target is consume_worker else ())))
    for p in ps: p.start()
    time.sleep(3 + a.procs * 0.3); evt.set()
    res = [q.get() for _ in ps]
    for p in ps: p.join()
    return res

def pct(xs, p): return xs[min(len(xs) - 1, int(len(xs) * p))] if xs else None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scenario", choices=["produce", "consume", "metadata"])
    ap.add_argument("--bootstrap", default="127.0.0.1:49093"); ap.add_argument("--topic", default="bq1")
    ap.add_argument("--partitions", type=int, default=1); ap.add_argument("--procs", type=int, default=4)
    ap.add_argument("--size", type=int, default=100); ap.add_argument("--seconds", type=float, default=20)
    ap.add_argument("--acks", default="1"); ap.add_argument("--linger-ms", type=int, default=5)
    ap.add_argument("--batch-size", type=int, default=1048576); ap.add_argument("--preload", type=int, default=2000000)
    ap.add_argument("--in-flight", type=int, default=5); ap.add_argument("--compression", default="none")
    ap.add_argument("--conns", type=int, default=4); ap.add_argument("--depth", type=int, default=32)
    ap.add_argument("--run", default=time.strftime("%H%M%S")); ap.add_argument("--no-create", action="store_true")
    a = ap.parse_args()
    if not a.no_create: admin_topic(a.bootstrap, a.topic, a.partitions)
    out = {"in_flight": a.in_flight, "compression": a.compression, "scenario": a.scenario, "partitions": a.partitions, "procs": a.procs, "size": a.size, "acks": a.acks}
    if a.scenario == "consume": a.procs = min(a.procs, a.partitions)
    if a.scenario == "produce":
        r = run(a, produce_worker); dur = max(x["dur"] for x in r)
        acked = sum(x["acked"] for x in r)
        out.update(acked=acked, errors=sum(x["errors"] for x in r), seconds=round(dur, 2),
                   msgs_per_s=round(acked / dur), mb_per_s=round(acked * a.size / dur / 1e6, 1),
                   produce_requests=sum(x["tx_reqs"] for x in r),
                   produce_req_per_s=round(sum(x["tx_reqs"] for x in r) / dur),
                   msgs_per_request=round(acked / max(1, sum(x["tx_reqs"] for x in r)), 1))
    elif a.scenario == "consume":
        pa = argparse.Namespace(**{**vars(a), "procs": 4, "seconds": max(5, a.preload / 4 / 200000)})
        pr = run(pa, produce_worker)
        # each preload proc sends for pa.seconds; whatever landed is what we read back
        total = sum(x["acked"] for x in pr)
        r = run(a, consume_worker, extra=([total] * a.procs,))   # stop on idle; expect is an upper bound
        got = sum(x["got"] for x in r); dur = max(x["dur"] for x in r)
        out.update(preloaded=total, consumed=got, seconds=round(dur, 2), msgs_per_s=round(got / max(dur, 1e-6)),
                   mb_per_s=round(sum(x["bytes"] for x in r) / max(dur, 1e-6) / 1e6, 1))
    else:
        r = run(a, meta_worker); dur = max(x["dur"] for x in r); tot = sum(x["reqs"] for x in r)
        out.update(requests=tot, seconds=round(dur, 2), req_per_s=round(tot / dur), conns=a.procs * a.conns, depth=a.depth)
    print(json.dumps(out))

if __name__ == "__main__": main()
