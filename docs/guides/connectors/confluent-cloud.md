# Confluent Cloud

Confluent Cloud speaks the Kafka protocol. It runs on the native
[Kafka connector](kafka.md) through the target profile
`connectors/profiles/confluent-cloud.yaml` (CC1 Task 2, D1822, design §80
§4.2). There is no Confluent SDK in Loams: the connector talks to the
cluster's bootstrap endpoint over SASL_SSL with a Kafka API key.

How a BYOC VM gets the key and runs the connector is in the BYOC guide
(CC1 Task 9). This page covers the connector side.

## Tested against

| System | Version | Where | Suites |
|---|---|---|---|
| Apache Kafka with SASL_SSL, PLAIN users and a test CA | 4.3.1 (`apache/kafka`, KRaft, single node) | `deploy/fabric/compose.yaml`, service `kafka-sasl`, profile `connectors-p2`; CI job `connectors-p2` (kafka shard), `tests/kafka_sasl.rs` | contract, roundtrip, kill-restart, dup-check, exactly-once, wrong password and untrusted CA refused |
| Confluent Cloud Basic and Standard | `us-east-1`, `ap-south-1` | CC0 (owner-run, 2026-10-26, R0.1); nightly from CC1 Task 10 | roundtrip, kill-restart, exactly-once |

The SASL_SSL leg runs the same suites as Apache Kafka, through this profile,
with a bootstrap string carrying Confluent's `SASL_SSL://` scheme and the
username and password given as secret names.

## What the profile sets

| Key | Value | Why |
|---|---|---|
| `security_protocol` | `sasl_ssl` | Confluent Cloud's only listener for API keys |
| `sasl.mechanism` | `plain` | the API key id is the username, its secret the password |
| `tls.enabled`, `tls.verify_full` | `true`, `true` | hostname verification against the system roots (`ssl.ca.location=probe`; CC0 R0.3). No CA is shipped |
| `max_record_bytes` | `8000000` | Basic and Standard accept 8 MB records |
| `create_checkpoint_topic` | `false` | Loams' data account has no CREATE (D1826): the `confluent` module or the user creates `__loams_checkpoints` (compacted, one partition) before an exactly-once sink starts |

Nothing is narrowed: transactions, the idempotent producer and compaction
passed on Basic and Standard (CC0 R0.1), so `delivery: exactly_once` is
allowed. OAuth (keyless) is not built; it comes in CC3, for Standard and
above (R0.5).

## An instance

```yaml
connector: kafka
profile: confluent-cloud
config:
  brokers: SASL_SSL://pkc-xxxxx.us-east-1.aws.confluent.cloud:9092   # the scheme is optional; it is removed
  topics: [orders]
  group_id_prefix: loams-main-
  group_id: loams-main-orders-in               # unique per source instance
  transactional_id_prefix: loams-main-         # exactly-once sinks: <prefix><instance>-<split>
  sasl:
    username_secret: confluent-main/username   # the API key id, from the secret store
    password: confluent-main/password          # the API secret, from the secret store
  rate_limit: { bytes_per_sec: 1048576 }       # optional: Loams' share of the cluster's throughput
  schema_registry:                             # optional
    url: https://psrc-xxxxx.us-east-1.aws.confluent.cloud
    basic_auth:
      username_secret: confluent-main/sr-username
      password: confluent-main/sr-password
```

`sasl.username` (a literal key id) can replace `sasl.username_secret`; one
of the two is required. Consumer groups and transactional ids must sit under
the prefixes the `confluent` module grants (`loams-<name>-` by default, its
`consumer_group_prefix` and `transactional_id_prefix` outputs). Without
`transactional_id_prefix`, an exactly-once sink's id is `loams-<namespace_id>-…`,
which those ACLs refuse.
