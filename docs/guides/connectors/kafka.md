# Kafka

The native Kafka connector (`loams-connector-kafka`, CN2 Task 9). The same
code also serves [Redpanda](redpanda.md),
[Azure Event Hubs](azure-event-hubs.md) and
[Confluent Cloud](confluent-cloud.md) as target profiles.

## Tested against

| System | Version | Where | Suites |
|---|---|---|---|
| Apache Kafka | 4.3.1 (`apache/kafka`, KRaft, single node) | `deploy/fabric/compose.yaml`, profile `connectors-p2`; CI job `connectors-p2` | contract, roundtrip, kill-restart, dup-check, exactly-once |

Client: `rdkafka` 0.39.0 over librdkafka 2.12.1, built from source with
OpenSSL 3.6 (vendored), zlib and zstd. Every compression codec Kafka uses
(gzip, snappy, lz4, zstd) can be read.

## Source

* One split per topic-partition (`orders/3`). New partitions are picked up
  at the next discovery, with no restart.
* Each split resumes from its checkpoint. Without one, it starts from the
  consumer group's committed offset, and failing that from
  `auto_offset_reset` (`latest` by default). If a checkpointed offset has
  been deleted by retention, the instance stops with `PositionLost`. It never
  skips data silently.
* The consumer group's offset is committed only after the Fabric
  acknowledges the batch. Records are read `read_committed`.
* Events: a record carrying CloudEvents `ce_` headers (binary mode) or
  `content-type: application/cloudevents+json` (structured mode) is passed
  through with its `id`, `type` and `source`. Any other record becomes
  `io.loams.dev.flow.kafka.record.v1` with the id
  `<topic>/<partition>/<offset>`. The record key becomes the `partitionkey`
  extension, or `loamskeyb64` when the key is not UTF-8.

## Sink

* `delivery: effectively_once` (the default) uses the idempotent producer
  (`enable.idempotence=true`, `acks=all`). Each event becomes a binary-mode
  record (D270): `ce_` headers, `content-type`, the payload as the value,
  and `partitionkey` as the key.
* `delivery: exactly_once` uses Kafka transactions with
  `transactional.id = loams-<ns>-<instance>-fabric`. The batch and the
  instance's checkpoint (written to the compacted topic
  `__loams_checkpoints`, key `<ns>/<instance>/fabric`) are committed in one
  transaction. On start, the sink fences older producers, reads its
  checkpoint `read_committed`, and the Fabric reader resumes there. The
  checkpoint topic is created (compacted, one partition) if it is missing.
  Set `create_checkpoint_topic: false` to require it to exist already.
* A sink writes exactly one topic. An event larger than `max_record_bytes`
  goes to the DLQ as poison, as does an Arrow batch.

## Configuration

`connectors/schemas/kafka.config.json`. `sasl.password` and `tls.key_pem`
are names of secrets, never the secrets themselves. SASL mechanisms:
`plain`, `scram-sha-256` and `scram-sha-512`. mTLS uses `tls.cert_pem` and
`tls.key_pem`. OAUTHBEARER (and with it AWS MSK IAM) is not supported by the
native connector yet. The consumer group is `group_id`, else
`group_id_prefix`, else `loams-<ns>-<instance>`.

CC1 Task 2 (D1822) added:

* `sasl.username_secret`: the SASL username as a secret's name, in place of
  `sasl.username` (exactly one of the two). Confluent Cloud's API key id
  arrives this way.
* `brokers` may carry a listener scheme per entry (`SASL_SSL://`, `SSL://`,
  `PLAINTEXT://`, `SASL_PLAINTEXT://`, any case), as Confluent Cloud's
  bootstrap endpoint sometimes does. It is removed.
* `rate_limit: { bytes_per_sec, requests_per_sec }`: the instance's rates,
  enforced by the SDK's rate limiter across its splits. Absent means
  unlimited.
* `schema_registry.basic_auth: { username | username_secret, password }`:
  HTTP Basic credentials for the registry, `password` a secret's name.

[Confluent Cloud](confluent-cloud.md) runs on this connector as a target
profile.

## Narrowings and fallback

None against Apache Kafka. There is no fallback runtime: Kafka is native
(OR-1).
