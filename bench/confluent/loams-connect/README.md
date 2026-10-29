# Loams connect: Kafka <-> Loams stream, end to end

A scripted local stack for the CFB benchmark: a Loams engine (`loams dev`, native on the host) and the `loams-fabric connect` role (#1299) with two instances:

- `cfb-in`, a source: Kafka topic `$CFB_TOPIC_IN` -> Loams stream `$CFB_STREAM`;
- `cfb-out`, an exactly-once sink: Loams stream `$CFB_STREAM` -> Kafka topic `$CFB_TOPIC_OUT`. Each batch is written in one Kafka transaction together with its checkpoint in `__loams_checkpoints`, with transactional id `loams-cfb-cfb-out-<split>`.

Kafka is a local Apache Kafka 4.3.1 container (`docker.io/apache/kafka:4.3.1`, KRaft, PLAINTEXT on 127.0.0.1:19092) unless `KAFKA_BOOTSTRAP` is set. With `KAFKA_KEY_FILE` set, the instances use the `confluent-cloud` profile (SASL_SSL, PLAIN, `verify_full` against the system roots).

## Run

```sh
./up.sh                      # Kafka container, topics, engine, connect, instances
./e2e.sh                     # 10 000 records at 1 000/s, kill -9 connect halfway
./down.sh                    # stop; ./down.sh --purge also deletes .run/ and the container
```

Against a Confluent Cloud cluster:

```sh
# key file: two lines, id=<API key id> and secret=<API secret>, mode 0600
KAFKA_BOOTSTRAP=SASL_SSL://pkc-xxxxx.us-east-1.aws.confluent.cloud:9092 \
KAFKA_KEY_FILE=~/confluent-cfb.key CFB_TOPIC_IN=cfb-in CFB_TOPIC_OUT=cfb-out ./up.sh
KAFKA_BOOTSTRAP=... KAFKA_KEY_FILE=... ./e2e.sh
```

`up.sh` tries to create the topics: both data topics with `message.timestamp.type=LogAppendTime`, and `__loams_checkpoints` compacted with one partition. If the key has no CREATE, it warns. Create them yourself in that case, with the same settings. The key's ACLs must allow consumer groups and transactional ids with the prefix `loams-cfb-`. The key never appears in the instance files: `up.sh` writes it to `.run/secrets/0/confluent-$CFB_INTEGRATION/{username,password}` (0600), and the CLI tools read it from `.run/kafka-client/client.properties` (0600).

`./connect.sh kill9` or `./connect.sh restart` restarts only the connect role.

## Settings (environment)

| Variable | Default | |
|---|---|---|
| `KAFKA_BOOTSTRAP` | unset: local container on 127.0.0.1:19092 | a `SASL_SSL://` scheme is stripped |
| `KAFKA_KEY_FILE` | unset: PLAINTEXT | `id=` and `secret=` lines -> SASL_SSL PLAIN, profile `confluent-cloud` |
| `CFB_TOPIC_IN`, `CFB_TOPIC_OUT` | `cfb-in`, `cfb-out` | |
| `CFB_STREAM`, `CFB_NAMESPACE` | `cfb`, `default` | the Loams stream |
| `CFB_PARTITIONS` | `1` | topic and stream partitions |
| `CFB_PREFIX` | `loams-cfb-` | consumer group and transactional id prefix |
| `CFB_DEBEZIUM` | `0` | `1` adds source `cfb-dbz` (`format: debezium-json`, topic `cfb-dbz` -> stream `cfb-dbz`); check it with `./dbz.sh` |
| `CFB_MAX_BATCH_ROWS` | `1000` | the source's `pipeline.max_batch_rows`; empty leaves it out. Needed with `loams-fabric` builds without #1496, which dead-letter any batch over 1000 events |
| `CFB_CONNECT_VIA` | `socket` | `tcp` points connect at the engine's main port instead of `--connect-socket` |
| `CFB_N`, `CFB_RATE` | `10000`, `1000` | e2e.sh: records and records per second |
| `CFB_KILL`, `CFB_KILL_AT`, `CFB_KILL_DOWN_S` | `1`, `0.5`, `2` | e2e.sh: kill -9 connect after that share of the records is sent, restart after N seconds |
| `CFB_TIMEOUT_S` | `180` | e2e.sh: wait for the output topic |
| `LOAMS_PORT`, `CONNECT_ADMIN` | `18080`, `127.0.0.1:17790` | engine main API, connect admin API |
| `LOAMS_BIN`, `FABRIC_BIN` | `<repo>/bin/…`, else `/mnt/Projects/rust-cache/target/release/…` | `loams-fabric` must be built with `--features connectors` |
| `CFB_RUN` | `./.run` | engine data, connect state, secrets, logs, e2e work dirs |

## What e2e.sh measures

Each record's value is `{"run","seq","ts"}`. Both topics stamp `LogAppendTime`. The source carries the input record's timestamp into the event's `time`, so the legs are:

- `producer_to_input_topic`: input append time minus the producer's `ts`. This includes the console producer's batching.
- `input_topic_to_loams_stream`: when a long-polling `FetchCloudEvents` reader on the engine's main port first sees the event, minus the input append time.
- `loams_stream_to_output_topic`: output append time minus that observation.
- `end_to_end`: output append time minus input append time. Both are broker clocks.

`dlq_events` counts the events added to `cfb-in-dlq` and `cfb-out-dlq` during the run. `exactly_once` is true only when there are no duplicates, nothing is missing and the DLQs are empty. Loss and duplicates are counted per `seq` on the Loams stream and on the output topic. The output topic is read with `read_committed`, so aborted transactions don't count. The latencies include the kill -9 gap. Set `CFB_KILL=0` for a clean latency run.

## Binaries

`loams` (default features) and `loams-fabric --features connectors`, both release builds. Copy them to `<repo>/bin` so another agent's rebuild can't swap them mid-run:

```sh
buildslot.sh cargo build --release -j 4 -p loams
(cd fabric && buildslot.sh cargo build --release -j 4 -p loams-fabric --features connectors)
```

## OpenTelemetry

The scripts export `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://127.0.0.1:4318`), and set `OTEL_SERVICE_NAME=loams-engine` for the engine and `loams-connect` for connect. **Neither binary exports anything over OTLP today**: neither the engine (`crates/loams`) nor `loams-fabric` (connect, connector SDK, Kafka family) has an OpenTelemetry dependency. Both log through `tracing` to stderr (`.run/logs/`). The engine has no Prometheus endpoint either. Connect's per-instance state is at `GET http://$CONNECT_ADMIN/v1/connect/instances`, with a bearer token from `.run/secrets/connect-admin-token`. The variables are set so that OTLP export works without script changes once it lands.

## Known issues (2026-10-28)

- **Fixed in #1496.** Connect sent a source batch with more than 1000 events to the engine whole. The engine refused it, and every event went to the DLQ. The kill -9 run then lost 9368 of 10 000 records.
- **Open.** A `ProduceCloudEvents` request that is in flight when connect is killed is cancelled in the engine. The append has committed by then, but its idempotency claims are never completed. They lapse after 120 s (`CLAIM_TTL_MS`), and the re-sent batch is appended a second time. The kill -9 run shows about one batch of duplicates, 323 records, plus a stall of about 120 s.
