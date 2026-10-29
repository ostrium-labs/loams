#!/usr/bin/env python3
"""CFB load driver: Kafka produce/consume latency, throughput, transactions and
partition limits against Confluent Cloud, Apache Kafka or the Loams Iggy Kafka
gateway, with OpenTelemetry traces and metrics.

The client is librdkafka (confluent-kafka-python), the same library the Loams
Kafka connector uses (rdkafka). Every run writes one JSON file.

  driver.py --target loams-cfb-aws-1 --bootstrap HOST:9092 --key-file KEYFILE \\
      --cloud aws --region ap-south-1 --out results/... latency --size 1024 --acks all --rate 20 --seconds 60

Key files hold two lines, id=... and secret=... (SASL_SSL PLAIN). Without one
the driver speaks PLAINTEXT (local baselines).

OpenTelemetry: OTLP/HTTP to $OTEL_EXPORTER_OTLP_ENDPOINT (default
http://127.0.0.1:4318, the SigNoz collector). For a sample of messages
(--trace-sample, default 1%) the driver records a trace with three spans:
kafka.message (send to consume), kafka.produce (send to broker ack) and
kafka.deliver (ack to consume). Each transaction is a kafka.txn span.
"""
from __future__ import annotations

import argparse
import json
import os
import random
import socket
import ssl
import statistics
import sys
import threading
import time
import uuid

from confluent_kafka import Consumer, KafkaError, KafkaException, Producer, TopicPartition
from confluent_kafka.admin import AdminClient, NewTopic

# --- OpenTelemetry ---------------------------------------------------------------------------
try:
    from opentelemetry import metrics, trace
    from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.sdk.metrics import MeterProvider
    from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor
    from opentelemetry.trace import SpanKind, set_span_in_context

    HAVE_OTEL = True
except ImportError:  # the driver still works without it
    HAVE_OTEL = False


class Telemetry:
    def __init__(self, args):
        self.on = HAVE_OTEL and not args.no_otel
        if not self.on:
            return
        ep = os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318").rstrip("/")
        res = Resource.create({
            "service.name": "cfb-driver",
            "service.namespace": "loams-bench",
            "cfb.run": args.run,
            "cfb.target": args.target,
            "cfb.kind": args.kind,
            "cloud.provider": args.cloud,
            "cloud.region": args.region,
            "messaging.system": "kafka",
        })
        tp = TracerProvider(resource=res)
        tp.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(endpoint=ep + "/v1/traces"), max_queue_size=20000))
        self.tp = tp
        self.tracer = tp.get_tracer("cfb-driver")
        reader = PeriodicExportingMetricReader(OTLPMetricExporter(endpoint=ep + "/v1/metrics"), export_interval_millis=5000)
        self.mp = MeterProvider(resource=res, metric_readers=[reader])
        meter = self.mp.get_meter("cfb-driver")
        self.h_produce = meter.create_histogram("cfb.produce.latency", unit="ms", description="send to broker ack")
        self.h_e2e = meter.create_histogram("cfb.e2e.latency", unit="ms", description="send to consume")
        self.h_txn = meter.create_histogram("cfb.txn.commit.latency", unit="ms", description="commit_transaction")
        self.c_bytes = meter.create_counter("cfb.produced.bytes", unit="By")
        self.c_errors = meter.create_counter("cfb.errors")

    def message_trace(self, attrs, t_send, t_ack, t_recv):
        """Three spans for one message, from wall-clock nanoseconds."""
        if not self.on:
            return
        root = self.tracer.start_span("kafka.message", start_time=t_send, kind=SpanKind.INTERNAL, attributes=attrs)
        ctx = set_span_in_context(root)
        p = self.tracer.start_span("kafka.produce", context=ctx, start_time=t_send, kind=SpanKind.PRODUCER, attributes=attrs)
        p.end(end_time=t_ack)
        end = t_recv or t_ack
        if t_recv:
            d = self.tracer.start_span("kafka.deliver", context=ctx, start_time=t_ack, kind=SpanKind.CONSUMER, attributes=attrs)
            d.end(end_time=t_recv)
        root.end(end_time=end)

    def span(self, name, start_ns, end_ns, attrs, error=None):
        if not self.on:
            return
        s = self.tracer.start_span(name, start_time=start_ns, attributes=attrs)
        if error:
            s.set_attribute("error", True)
            s.set_attribute("error.message", str(error)[:500])
        s.end(end_time=end_ns)

    def close(self):
        if self.on:
            self.tp.force_flush(10000)
            self.mp.force_flush(10000)
            self.tp.shutdown()
            self.mp.shutdown()


# --- helpers -----------------------------------------------------------------------------------
def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    k = min(len(xs) - 1, max(0, int(round(p / 100.0 * len(xs) + 0.5)) - 1))
    return round(xs[k], 3)


def summary(xs):
    if not xs:
        return {"n": 0}
    return {"n": len(xs), "min": round(min(xs), 3), "p50": pct(xs, 50), "p90": pct(xs, 90), "p99": pct(xs, 99),
            "p999": pct(xs, 99.9), "max": round(max(xs), 3), "mean": round(statistics.fmean(xs), 3)}


def read_key(path):
    kv = {}
    with open(path) as f:
        for line in f:
            if "=" in line:
                k, v = line.rstrip("\n").split("=", 1)
                kv[k] = v
    return kv["id"], kv["secret"]


def base_conf(args, consume=False):
    """Client config; consume=True points at --consume-bootstrap (cross-cluster e2e:
    produce on one cluster, read the mirror on another)."""
    bs, kf = args.bootstrap, args.key_file
    if consume and getattr(args, "consume_bootstrap", None):
        bs, kf = args.consume_bootstrap, args.consume_key_file
    c = {"bootstrap.servers": bs, "client.id": f"cfb-{args.run}"}
    if kf:
        u, p = read_key(kf)
        c.update({"security.protocol": "SASL_SSL", "sasl.mechanisms": "PLAIN", "sasl.username": u, "sasl.password": p})
    return c


def rf(args):
    return 3 if args.key_file else 1


def ensure_topic(args, name, partitions, config=None):
    admin = AdminClient(base_conf(args))
    cfg = {"retention.ms": str(args.retention_ms)}
    if config:
        cfg.update(config)
    fs = admin.create_topics([NewTopic(name, num_partitions=partitions, replication_factor=rf(args), config=cfg)],
                             request_timeout=60)
    for t, f in fs.items():
        try:
            f.result(90)
        except KafkaException as e:
            if e.args[0].code() != KafkaError.TOPIC_ALREADY_EXISTS:
                raise
    # wait until metadata shows the leaders
    for _ in range(60):
        md = admin.list_topics(name, timeout=30)
        t = md.topics.get(name)
        if t and not t.error and len(t.partitions) == partitions and all(p.leader >= 0 for p in t.partitions.values()):
            return
        time.sleep(1)


def delete_topics(args, names):
    if not names:
        return
    admin = AdminClient(base_conf(args))
    for t, f in admin.delete_topics(list(names), request_timeout=60).items():
        try:
            f.result(90)
        except KafkaException:
            pass


def payload(size):
    # incompressible, so the wire carries `size` bytes per record
    return os.urandom(size)


# --- rtt ---------------------------------------------------------------------------------------
def scen_rtt(args, tel):
    host, port = args.bootstrap.rsplit(":", 1)
    port = int(port)
    tcp, tls = [], []
    for _ in range(args.samples):
        t0 = time.perf_counter()
        try:
            s = socket.create_connection((host, port), timeout=20)
        except OSError as e:
            tcp.append(None)
            continue
        t1 = time.perf_counter()
        tcp.append((t1 - t0) * 1000)
        if args.key_file:
            ctx = ssl.create_default_context()
            try:
                ss = ctx.wrap_socket(s, server_hostname=host)
                tls.append((time.perf_counter() - t1) * 1000)
                ss.close()
            except OSError:
                s.close()
        else:
            s.close()
        time.sleep(args.interval)
    # Kafka request round trip: metadata for one topic over an established connection
    admin = AdminClient(base_conf(args))
    admin.list_topics(timeout=60)
    md = []
    for _ in range(args.samples):
        t0 = time.perf_counter()
        admin.list_topics(timeout=60)
        md.append((time.perf_counter() - t0) * 1000)
        time.sleep(args.interval)
    return {"tcp_connect_ms": summary([x for x in tcp if x is not None]), "tcp_failures": tcp.count(None),
            "tls_handshake_ms": summary(tls), "metadata_rtt_ms": summary(md)}


# --- latency / throughput ----------------------------------------------------------------------
class ConsumerThread(threading.Thread):
    def __init__(self, args, topic, run_tag, isolation="read_uncommitted"):
        super().__init__(daemon=True)
        conf = base_conf(args, consume=True)
        conf.update({"group.id": f"loams-cfb-{run_tag}", "auto.offset.reset": "latest", "enable.auto.commit": False,
                     "isolation.level": isolation, "fetch.wait.max.ms": args.fetch_wait_ms,
                     "fetch.min.bytes": 1})
        self.c = Consumer(conf)
        self.topic = topic
        self.recv = {}  # msg id -> recv wall ns
        self.bytes = 0
        self.count = 0
        self.stop_ev = threading.Event()
        self.assigned = threading.Event()
        self.dup = 0

    def run(self):
        def on_assign(c, parts):
            # start at the end of every partition, then go
            self.assigned.set()
        self.c.subscribe([self.topic], on_assign=on_assign)
        while not self.stop_ev.is_set():
            # poll() returns as soon as one message is there. consume(n, timeout)
            # waits for n messages or the timeout, which added up to 200 ms to
            # every end-to-end sample in the first runs (CFB results before 12:40Z).
            m = self.c.poll(0.1)
            if m is None:
                continue
            now = time.time_ns()
            for m in (m,):
                if m.error():
                    continue
                k = m.key()
                if k in self.recv:
                    self.dup += 1
                else:
                    self.recv[k] = now
                self.bytes += len(m.value() or b"")
                self.count += 1
        self.c.close()


def wait_assigned(ct, timeout=120):
    t0 = time.time()
    while not ct.assigned.is_set() and time.time() - t0 < timeout:
        time.sleep(0.2)
    time.sleep(args_global.settle)  # let the consumer reach the log end


def scen_produce(args, tel, mode):
    topic = args.topic or f"loams-cfb-{args.run}-{mode}"
    created = False
    if not args.topic:
        ensure_topic(args, topic, args.partitions)
        created = True
    tag = uuid.uuid4().hex[:8]
    ct = None
    if not args.no_consume:
        ct = ConsumerThread(args, args.consume_topic or topic, tag)
        ct.start()
        wait_assigned(ct)
    conf = base_conf(args)
    conf.update({"acks": args.acks, "linger.ms": args.linger_ms, "batch.size": args.batch_size,
                 "compression.type": args.compression, "enable.idempotence": args.acks == "all",
                 "queue.buffering.max.kbytes": 1048576, "message.timeout.ms": 120000})
    p = Producer(conf)
    sent = {}  # key -> send wall ns
    acks = {}  # key -> ack wall ns
    errors = []
    lock = threading.Lock()

    def cb(err, msg):
        now = time.time_ns()
        k = msg.key()
        if err:
            errors.append(str(err))
            if tel.on:
                tel.c_errors.add(1, {"op": "produce"})
            return
        with lock:
            acks[k] = now

    data = payload(args.size)
    t_start = time.time()
    deadline = t_start + args.seconds
    i = 0
    max_bytes = args.max_mb * 1024 * 1024
    interval = 1.0 / args.rate if (mode == "latency" and args.rate > 0) else 0
    next_t = time.perf_counter()
    while time.time() < deadline and i * args.size < max_bytes:
        if interval:
            now = time.perf_counter()
            if now < next_t:
                p.poll(min(next_t - now, 0.05))
                continue
            next_t += interval
        k = f"{tag}-{i}".encode()
        try:
            sent[k] = time.time_ns()
            p.produce(topic, value=data, key=k, on_delivery=cb)
            i += 1
        except BufferError:
            sent.pop(k, None)
            p.poll(0.05)
            continue
        if i % 100 == 0 or interval:
            p.poll(0)
    t_sent = time.time()
    remaining = p.flush(180)
    t_flushed = time.time()
    # wait for the consumer to drain
    if ct:
        t_wait = time.time()
        while time.time() - t_wait < args.drain_s and len(ct.recv) < len(acks):
            time.sleep(0.2)
        ct.stop_ev.set()
        ct.join(30)
    produce_ms, e2e_ms = [], []
    attrs = {"cfb.target": args.target, "cfb.size": args.size, "cfb.acks": args.acks, "cfb.mode": mode,
             "messaging.destination.name": topic}
    sample_every = max(1, int(1 / args.trace_sample)) if args.trace_sample > 0 else 0
    for n, (k, ts) in enumerate(sent.items()):
        ta = acks.get(k)
        if ta is None:
            continue
        produce_ms.append((ta - ts) / 1e6)
        tr = ct.recv.get(k) if ct else None
        if tr:
            e2e_ms.append((tr - ts) / 1e6)
        if tel.on:
            tel.h_produce.record((ta - ts) / 1e6, attrs)
            if tr:
                tel.h_e2e.record((tr - ts) / 1e6, attrs)
            if sample_every and n % sample_every == 0:
                tel.message_trace(attrs, ts, ta, tr)
    if tel.on:
        tel.c_bytes.add(len(acks) * args.size, attrs)
    dur = max(1e-6, (t_flushed - t_start))
    res = {
        "topic": topic, "partitions": args.partitions, "size": args.size, "acks": args.acks, "linger_ms": args.linger_ms,
        "rate_target": args.rate if mode == "latency" else None, "seconds": round(dur, 2),
        "sent": len(sent), "acked": len(acks), "unacked_at_flush": remaining, "errors": len(errors),
        "error_samples": sorted(set(errors))[:5],
        "produce_msgs_per_s": round(len(acks) / dur, 1), "produce_mb_per_s": round(len(acks) * args.size / dur / 1e6, 4),
        "send_phase_s": round(t_sent - t_start, 2),
        "produce_latency_ms": summary(produce_ms), "e2e_latency_ms": summary(e2e_ms),
        "consumed": len(ct.recv) if ct else None, "consumer_duplicates": ct.dup if ct else None,
        "lost_after_ack": (len([k for k in acks if k not in ct.recv]) if ct else None),
    }
    if created and not args.keep_topic:
        delete_topics(args, [topic])
    return res


# --- transactions --------------------------------------------------------------------------------
def scen_txn(args, tel):
    topic = args.topic or f"loams-cfb-{args.run}-txn"
    if not args.topic:
        ensure_topic(args, topic, args.partitions)
    tag = uuid.uuid4().hex[:8]
    rc = ConsumerThread(args, topic, tag + "rc", isolation="read_committed")
    ru = ConsumerThread(args, topic, tag + "ru", isolation="read_uncommitted")
    rc.start(); ru.start()
    wait_assigned(rc); wait_assigned(ru)
    conf = base_conf(args)
    conf.update({"transactional.id": f"loams-cfb-{args.run}-{tag}", "acks": "all", "enable.idempotence": True,
                 "linger.ms": args.linger_ms, "transaction.timeout.ms": 60000})
    p = Producer(conf)
    t0 = time.time_ns()
    p.init_transactions(120)
    init_ms = (time.time_ns() - t0) / 1e6
    data = payload(args.size)
    commit_ms, txn_ms, abort_ms = [], [], []
    committed_keys, aborted_keys = set(), set()
    errors = []
    t_start = time.time()
    for n in range(args.txns):
        abort = args.abort_every > 0 and (n + 1) % args.abort_every == 0
        ts = time.time_ns()
        try:
            p.begin_transaction()
            keys = []
            for j in range(args.msgs_per_txn):
                k = f"{tag}-{n}-{j}".encode()
                p.produce(topic, value=data, key=k)
                keys.append(k)
            if abort:
                # Send the records first: an immediate abort lets librdkafka purge
                # them locally, and the broker would never see them (so a
                # read_committed check would prove nothing).
                p.flush(60)
            tc = time.time_ns()
            if abort:
                p.abort_transaction(60)
                aborted_keys.update(keys)
                abort_ms.append((time.time_ns() - tc) / 1e6)
            else:
                p.commit_transaction(60)
                committed_keys.update(keys)
                commit_ms.append((time.time_ns() - tc) / 1e6)
            te = time.time_ns()
            txn_ms.append((te - ts) / 1e6)
            if tel.on:
                tel.span("kafka.txn", ts, te, {"cfb.target": args.target, "cfb.txn.n": n, "cfb.txn.aborted": abort,
                                               "cfb.txn.msgs": args.msgs_per_txn, "cfb.txn.commit_ms": (te - tc) / 1e6})
                if not abort:
                    tel.h_txn.record((te - tc) / 1e6, {"cfb.target": args.target})
        except KafkaException as e:
            errors.append(str(e))
            if tel.on:
                tel.span("kafka.txn", ts, time.time_ns(), {"cfb.target": args.target, "cfb.txn.n": n}, error=e)
            try:
                p.abort_transaction(30)
            except KafkaException:
                pass
    dur = time.time() - t_start
    t_wait = time.time()
    while time.time() - t_wait < args.drain_s and (len(rc.recv) < len(committed_keys)
                                                   or len(ru.recv) < len(committed_keys) + len(aborted_keys)):
        time.sleep(0.2)
    time.sleep(2)
    rc.stop_ev.set(); ru.stop_ev.set(); rc.join(30); ru.join(30)
    rc_keys = set(rc.recv)
    res = {
        "topic": topic, "txns": args.txns, "msgs_per_txn": args.msgs_per_txn, "size": args.size,
        "abort_every": args.abort_every, "seconds": round(dur, 2), "init_transactions_ms": round(init_ms, 1),
        "commit_latency_ms": summary(commit_ms), "abort_latency_ms": summary(abort_ms), "txn_latency_ms": summary(txn_ms),
        "txn_per_s": round(len(txn_ms) / max(dur, 1e-6), 2), "errors": len(errors), "error_samples": errors[:5],
        "eos": {
            "committed": len(committed_keys), "aborted": len(aborted_keys),
            "read_committed_seen": len(rc_keys), "read_committed_missing": len(committed_keys - rc_keys),
            "read_committed_saw_aborted": len(aborted_keys & rc_keys), "read_committed_duplicates": rc.dup,
            "read_uncommitted_seen": len(ru.recv), "read_uncommitted_saw_aborted": len(aborted_keys & set(ru.recv)),
        },
    }
    # pass needs real work: committed records, no errors, and (when aborts were
    # flushed to the broker) the read_uncommitted reader seeing them
    res["eos"]["pass"] = (res["eos"]["committed"] > 0 and not errors
                          and res["eos"]["read_committed_missing"] == 0 and res["eos"]["read_committed_saw_aborted"] == 0
                          and res["eos"]["read_committed_duplicates"] == 0)
    res["eos"]["aborted_reached_broker"] = res["eos"]["read_uncommitted_saw_aborted"] == res["eos"]["aborted"]
    if not args.topic and not args.keep_topic:
        delete_topics(args, [topic])
    return res


# --- partition limit ------------------------------------------------------------------------------
def scen_partitions(args, tel):
    """Create topics of --step partitions until the cluster refuses; record where and why."""
    admin = AdminClient(base_conf(args))
    made, total, refusal = [], 0, None
    times = []
    n = 0
    t_all = time.time()
    while total < args.max_partitions:
        name = f"loams-cfb-{args.run}-p{n}"
        step = min(args.step, args.max_partitions - total)
        ts = time.time_ns()
        f = admin.create_topics([NewTopic(name, num_partitions=step, replication_factor=rf(args),
                                          config={"retention.ms": "3600000"})], request_timeout=120)[name]
        try:
            f.result(180)
            te = time.time_ns()
            times.append((te - ts) / 1e6)
            made.append(name)
            total += step
            if tel.on:
                tel.span("kafka.create_topics", ts, te, {"cfb.target": args.target, "cfb.partitions": step, "cfb.total": total})
        except KafkaException as e:
            refusal = {"at_total": total, "requested": step, "code": e.args[0].name(), "message": e.args[0].str()[:400]}
            if tel.on:
                tel.span("kafka.create_topics", ts, time.time_ns(), {"cfb.target": args.target, "cfb.partitions": step}, error=e)
            if step > 1 and args.bisect:
                args.step = max(1, step // 2)
                if args.step >= 1 and step != 1:
                    refusal = None
                    continue
            break
        n += 1
    res = {"partitions_created": total, "topics": len(made), "refusal": refusal,
           "create_ms": summary(times), "seconds": round(time.time() - t_all, 1)}
    if args.hold_s:
        time.sleep(args.hold_s)
    if not args.keep_topic:
        t0 = time.time()
        for i in range(0, len(made), 50):
            delete_topics(args, made[i:i + 50])
        res["delete_seconds"] = round(time.time() - t0, 1)
    return res


args_global = None


def main():
    global args_global
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    ap.add_argument("--kind", default="confluent-cloud", help="confluent-cloud | apache-kafka-local | iggy-kafka-local")
    ap.add_argument("--bootstrap", required=True)
    ap.add_argument("--key-file")
    ap.add_argument("--cloud", default="local")
    ap.add_argument("--region", default="local")
    ap.add_argument("--run", default=time.strftime("%H%M%S"))
    ap.add_argument("--out", required=True)
    ap.add_argument("--no-otel", action="store_true")
    ap.add_argument("--trace-sample", type=float, default=0.01)
    ap.add_argument("--topic")
    ap.add_argument("--keep-topic", action="store_true")
    ap.add_argument("--partitions", type=int, default=6)
    ap.add_argument("--retention-ms", type=int, default=3600000)
    ap.add_argument("--fetch-wait-ms", type=int, default=50)
    ap.add_argument("--settle", type=float, default=3.0)
    ap.add_argument("--drain-s", type=float, default=60)
    ap.add_argument("--no-consume", action="store_true")
    ap.add_argument("--consume-bootstrap", help="consume from another cluster (mirror / connect sink)")
    ap.add_argument("--consume-key-file")
    ap.add_argument("--consume-topic", help="topic name on the consume side (default: the produce topic)")
    sub = ap.add_subparsers(dest="scenario", required=True)
    r = sub.add_parser("rtt"); r.add_argument("--samples", type=int, default=20); r.add_argument("--interval", type=float, default=0.5)
    for name in ("latency", "throughput"):
        s = sub.add_parser(name)
        s.add_argument("--size", type=int, default=1024)
        s.add_argument("--acks", default="all")
        s.add_argument("--rate", type=float, default=20)
        s.add_argument("--seconds", type=float, default=60)
        s.add_argument("--linger-ms", type=int, default=5 if name == "latency" else 50)
        s.add_argument("--batch-size", type=int, default=1048576)
        s.add_argument("--compression", default="none")
        s.add_argument("--max-mb", type=float, default=200)
    t = sub.add_parser("txn")
    t.add_argument("--txns", type=int, default=50); t.add_argument("--msgs-per-txn", type=int, default=10)
    t.add_argument("--size", type=int, default=512); t.add_argument("--abort-every", type=int, default=5)
    t.add_argument("--linger-ms", type=int, default=5)
    pp = sub.add_parser("partitions")
    pp.add_argument("--step", type=int, default=50); pp.add_argument("--max-partitions", type=int, default=3000)
    pp.add_argument("--bisect", action="store_true"); pp.add_argument("--hold-s", type=float, default=0)
    args = ap.parse_args()
    args_global = args
    tel = Telemetry(args)
    t0 = time.time()
    meta = {"run": args.run, "target": args.target, "kind": args.kind, "cloud": args.cloud, "region": args.region,
            "scenario": args.scenario, "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t0)),
            "client": "librdkafka " + __import__("confluent_kafka").libversion()[0],
            "driver_host": "owner laptop, India (carrier uplink; WAN-bound)" if args.key_file else "owner laptop (local)",
            "params": {k: v for k, v in vars(args).items() if k not in ("key_file", "consume_key_file", "out")}}
    try:
        if args.scenario == "rtt":
            res = scen_rtt(args, tel)
        elif args.scenario in ("latency", "throughput"):
            res = scen_produce(args, tel, args.scenario)
        elif args.scenario == "txn":
            res = scen_txn(args, tel)
        else:
            res = scen_partitions(args, tel)
        meta["ok"] = True
    except Exception as e:  # recorded, not hidden
        res = {"exception": repr(e)[:1000]}
        meta["ok"] = False
    meta["seconds"] = round(time.time() - t0, 1)
    meta["result"] = res
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(meta, f, indent=1)
    tel.close()
    print(json.dumps({"out": args.out, "ok": meta["ok"], **{k: res.get(k) for k in list(res)[:8]}}, default=str)[:1500])
    return 0 if meta["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
