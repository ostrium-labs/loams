# Confluent Cloud benchmark harness (lane CFB)

This harness measures Confluent Cloud clusters on AWS, Google Cloud and Azure. It compares them with Apache Kafka 4.3.1 and the Loams Iggy Kafka gateway, both run locally. The report is
[docs/benchmarks/confluent-cloud-2026-10.md](../../docs/benchmarks/confluent-cloud-2026-10.md), and the
results are in `results/<day>/`.

Everything runs from one laptop. The scripts never print credentials. They keep their state outside the repo,
in `~/.cfb-state`: resource ids, key files (mode 0600) and the guard's ledger.

## Spend control (Confluent has no budget API)

| Piece | What it does |
| --- | --- |
| `lib/cc.sh` | Cloud API helper. It changes `env-3kqygo` only, refuses the Kafka oracle (`env-3kqddw`, `lkc-120zwvj`), and refuses a DELETE of anything this harness did not create. Keys go to curl on stdin. |
| `cfb.sh` | Creates and resizes clusters (Basic, Standard, Dedicated) and Flink pools. Before every create it checks the budget: the projected cost of everything to its deadline must stay at or under the cap (`~/.cfb-state/stop_usd`). It writes the shared `confluent-ci-created.tsv`. |
| `guard.sh`, `guard-timer.sh` | Runs as a `systemd --user` timer (`guard-timer.sh start`, one `guard.sh --once` tick a minute), so it does not depend on any shell surviving; a detached loop died on 2026-10-28. Every minute it deletes resources whose deadline has passed. It also deletes everything when the estimate reaches the cap, when `~/.cfb-state/ABORT` exists, or at the hard deadline. Every 5 minutes it deletes any paid cluster with under 1 MB of ingress in the last 30 minutes (no live generator). Every 10 minutes it refreshes the data cost from the Metrics API. |
| `collect.sh` | Metrics API (per cluster, and per minute for push tests) and Costs API. The per-cluster byte totals never go down, because a deleted cluster answers 403. |

The estimate counts each resource at its cap (max eCKU, CKU or max CFU) for every started hour, plus data
at $0.05/GB. That is an upper bound. The Costs API, which lags up to 72 hours, gives the real figure.

## Load

| Piece | What it does |
| --- | --- |
| `driver.py` | librdkafka (confluent-kafka-python) driver, with these scenarios: `rtt` (TCP, TLS and Kafka metadata), `latency` (produce and end-to-end at a fixed rate), `throughput`, `txn` (EOS: read_committed must see no aborted record), and `partitions`. It can consume from a different cluster (`--consume-bootstrap`), which is used for Cluster Linking. It sends OpenTelemetry traces and metrics over OTLP/HTTP; a sample of messages becomes a `kafka.message` trace with `kafka.produce` and `kafka.deliver` spans. |
| `run-wan.sh` | The laptop matrix against every live cluster, sequentially or in parallel. The laptop's uplink bounds it. |
| `run-local.sh` | The same scenarios against local Kafka 4.3.1 and the Iggy gateway, run under `.he0-shared/benchslot.sh`. |
| `flink.sh`, `flink-setup.sh` | In-region load generators on Confluent Cloud for Apache Flink. A `faker` id table (100k rows/s, bounded to 100,000 rows) feeds an UNNEST fan-out of raw records. Each statement is bounded (by default 30M records, about 30 GB). |
| `push.sh` | Steps max eCKU under in-region load and records the per-minute ingress (the throttle points). |
| `dedicated.sh`, `consume.sh` | Per-CKU capacity: phase A (1 KB produce MB/s), B (100 B records/s and requests/s) and C (consume MB/s). |
| `run-connect.sh`, `loams-connect/` | Loams' own path: a Kafka topic on one cluster, then `loams-fabric connect`, a local Loams stream, the exactly-once sink, and a topic on another cluster or cloud (`SINK_KAFKA_BOOTSTRAP`). |
| `cost.py`, `traces.py` | The per-test cost breakdown, and trace export from SigNoz's ClickHouse. |
| `link.sh` | Cross-cloud Cluster Linking: a link initiated by the Dedicated destination, a mirror topic, and an end-to-end latency run. |
| `signoz/` | SigNoz community build (MIT image `signoz/signoz-community`), as an overlay on the repo's `conformance/clickhouse/apps/signoz` compose. UI on 127.0.0.1:3301, OTLP on 4317/4318. |
| `report.py` | Charts (SVG) and tables from `results/<day>/`. |

## Run

```sh
cd bench/confluent
uv venv .venv && VIRTUAL_ENV=.venv uv pip install confluent-kafka opentelemetry-sdk opentelemetry-exporter-otlp-proto-http matplotlib
echo 1900 > ~/.cfb-state/stop_usd; date -d '+24 hours' +%s > ~/.cfb-state/hard_deadline
./guard-timer.sh start                                          # BEFORE creating anything
./cfb.sh cluster loams-cfb-aws-1 aws ap-south-1 basic 1 6        # name cloud region type cap ttl-hours
./cfb.sh pool loams-cfb-flink-aws aws ap-south-1 20 2
./signoz/up.sh
./run-wan.sh rtt; ./run-wan.sh latency; ./run-wan.sh txn
./dedicated.sh loams-cfb-flink-aws loams-cfb-aws-d4 4
.venv/bin/python report.py
touch ~/.cfb-state/ABORT                                         # stop: the guard deletes everything
```

Names are always `loams-cfb-*`. The CI reaper deletes `loams-ci-*` resources.
