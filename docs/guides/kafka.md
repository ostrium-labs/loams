# Kafka and Iggy (preview)

Status: **preview**. **Kafka-compatible access through Iggy is gone (D2450, 2026-11-02).** Loams
used to run Ostrium's fork of Apache Iggy, whose Kafka wire gateway let Kafka clients (kcat,
Kafka Connect, Flink, librdkafka) talk to Iggy on `127.0.0.1:9093`. The owner decided that every
`ostrium-labs` fork is superseded by its original project, so Loams now runs the **official Apache
Iggy** server image (`docker.io/apache/iggy`), which speaks Iggy's own protocol only. The fork's
patches, including the Kafka gateway, are not carried over. Nothing in Loams starts a Kafka
listener in front of Iggy any more.

## What is left

- The `iggy` profile of the release compose bundle (`deploy/release/compose.yaml`) runs one
  service, `iggy`, the official Apache Iggy server: on the compose network only, no published
  port. Use it through an Iggy SDK or the `iggy` CLI (`docker compose exec iggy iggy ...`).
- `loams_sink` (`crates/loams-iggy-sink`) reads Iggy topics into Loams streams with the official
  Iggy Rust SDK.
- For a Kafka endpoint, use a real Kafka-protocol broker: Confluent Cloud
  ([byoc/confluent.md](byoc/confluent.md)), Redpanda ([connectors/redpanda.md](connectors/redpanda.md))
  or Apache Kafka ([connectors/kafka.md](connectors/kafka.md)), reached by `loams-fabric connect`.

The image is pinned by version and digest in `IGGY_IMAGE` (`.env.example`): Apache Iggy server
0.9.0 (released 2026-09-18). Pins are releases at least 14 days old.

## Turn it on

Set the profile in `.env` before the first start (with the others, comma-separated):

```sh
COMPOSE_PROFILES=iggy
IGGY_PASSWORD=<a password of your own>   # e.g. openssl rand -hex 24; Iggy keeps it in its volume
docker compose up -d
```

The profile refuses to start while `IGGY_PASSWORD` is empty, still `.env.example`'s placeholder
(`change-me`) or shorter than 12 characters: `iggy` exits with
`iggy profile: IGGY_PASSWORD is still the .env.example placeholder; refusing to start.`
(`deploy/release/config/iggy-guard.sh`). Set it before the first start: Iggy writes its root user
to the `iggy-data` volume then, and changing `.env` afterwards does not change it.

On a remote machine: `deploy/release/byoc/install.sh --kafka iggy` (or `--profiles iggy`; it
writes a random password into the new `.env`). The flag keeps its name for compatibility; it no
longer opens a Kafka port.

Moving from an earlier preview: the profile was `kafka` and the variables were `IGGY_KAFKA_IMAGE`,
`KAFKA_IGGY_USER` and `KAFKA_IGGY_PASSWORD`, plus `KAFKA_BIND`, `KAFKA_PORT`,
`KAFKA_ADVERTISED_HOST`, `KAFKA_SASL_ENABLED` and `KAFKA_AUTO_CREATE_TOPICS`, which are gone.
Rename the profile and the three variables that remain (`IGGY_IMAGE`, `IGGY_USER`,
`IGGY_PASSWORD`). The `iggy-data` volume written by the fork's server is not guaranteed to open
with the official server: treat it as disposable.

## Kafka endpoint: Confluent

Your apps need a Kafka endpoint, and Loams can give them one (design §80 §4.3, D1820): Confluent
Cloud, through the `connectors` profile.

| | Confluent Cloud (`connectors` profile) |
|---|---|
| Where the broker runs | Confluent's managed cluster, ideally in the VM's cloud and region |
| Who your apps talk to | Confluent's bootstrap endpoint, with your own Confluent keys |
| How data reaches Loams | `loams-fabric connect` reads topics into Loams streams and writes streams to topics ([confluent-cloud.md](connectors/confluent-cloud.md)) |
| Cost | Confluent's bill; Loams bounds only its own share ([byoc/confluent.md](byoc/confluent.md) §6) |
| Kafka features | Everything Confluent offers, transactions included |
| Status | Preview (CC1) |

With `deploy/release/byoc/install.sh`:

```sh
# Loams' connectors to and from a Confluent Cloud cluster
install.sh --kafka confluent \
  --confluent-bootstrap pkc-xxxxx.us-east-1.aws.confluent.cloud:9092 \
  --confluent-topics orders,cdc.app.users=users --confluent-sinks events=loams-prod-events \
  --confluent-prefix loams-prod- user@host
```

With `confluent`, install.sh turns on the `connectors` profile, adds `apikey` to
`LOAMS_AUTH_MODE` (the connect socket takes API keys only), writes one instance file per topic
and per sink into `connect/instances/` from `deploy/release/connect/templates/`, and makes the
connect role's API key and admin token on the host. It does not create the Confluent key:
`loams byoc confluent key create` puts it in your cloud's secret store, and with `--cloud` the
VM fetches it at boot ([byoc/confluent.md](byoc/confluent.md) §5). Without `--cloud`, write it to
`secrets/connect/0/confluent-<integration>/{username,password}` yourself. `--kafka iggy,confluent`
still turns on both profiles, each named.

## Settings

All in `.env` (see `.env.example`):

| Variable | Default | Meaning |
|---|---|---|
| `IGGY_IMAGE` | a version and digest | The official Apache Iggy image. Move it only with a release |
| `IGGY_USER`, `IGGY_PASSWORD` | `iggy`, the placeholder `change-me` (refused) | Iggy's root user. The password is required: at least 12 characters |

## What to know

- **No Kafka wire protocol.** Kafka clients cannot reach Iggy. Code in Loams that builds Kafka
  connector settings (the Flink and RisingWave Kafka builders, `loams-fabric` Kafka connectors)
  works against any real Kafka-protocol broker, but their old default bootstrap
  (`kafka-gateway:9093`) has no service behind it any more: set the bootstrap explicitly.
- **Not wired to the engine yet.** Records land in Iggy, not in Loams streams or collections.
  The Event Fabric bridge (the `loams_sink` connector, FL1 in design 32) carries Iggy topics
  into Loams, and it is planned.
- **Container flags.** Iggy uses io_uring, which Docker's default seccomp profile refuses, so
  `iggy` runs with `seccomp:unconfined` and `SYS_NICE`, as Iggy's own quick start does.
- **Back up** the `iggy-data` volume; like the default stack, it is one node on one disk.
