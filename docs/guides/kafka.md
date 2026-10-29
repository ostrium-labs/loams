# Kafka clients (preview)

Status: **preview**. Optional compose profile, off in the default stack. Built in
[ostrium-labs/iggy](https://github.com/ostrium-labs/iggy), Ostrium's fork of Apache Iggy
(D1796 to D1798, design 32 section 5.10). It is not an Apache Software Foundation release.

## What it is

The `kafka` profile of the release compose bundle (`deploy/release/compose.yaml`) adds two
services from one image, `ghcr.io/ostrium-labs/iggy`:

- `iggy`, an Iggy server: the broker's own storage, on the compose network only.
- `kafka-gateway`, the Kafka wire front end, published on `127.0.0.1:9093`.

The image holds both binaries, so the pair always runs as the one tested pair. The compose file
and `.env.example` pin it by digest (`IGGY_KAFKA_IMAGE`). The fork's CI builds it for amd64 and
arm64, with a provenance and an SBOM attestation:

```sh
gh attestation verify oci://ghcr.io/ostrium-labs/iggy@sha256:<digest> -R ostrium-labs/iggy
```

## Turn it on

Set the profile in `.env` before the first start (with the others, comma-separated):

```sh
COMPOSE_PROFILES=kafka
KAFKA_IGGY_PASSWORD=<a password of your own>   # e.g. openssl rand -hex 24; Iggy keeps it in its volume
docker compose up -d
```

The profile refuses to start while `KAFKA_IGGY_PASSWORD` is empty, still `.env.example`'s
placeholder (`change-me`) or shorter than 12 characters: `iggy` and `kafka-gateway` exit with
`kafka profile: KAFKA_IGGY_PASSWORD is still the .env.example placeholder; refusing to start.`
(`deploy/release/config/kafka-guard.sh`). Set it before the first start: Iggy writes its root
user to the `iggy-data` volume then, and changing `.env` afterwards does not change it.

On a remote machine: `deploy/release/byoc/install.sh --kafka iggy` (or `--profiles kafka`;
it writes a random password into the new `.env`).

## Kafka endpoint: Iggy or Confluent

Your apps need a Kafka endpoint, and Loams can give them one of two (design §80 §4.3, D1820):

| | Iggy (`kafka` profile) | Confluent Cloud (`connectors` profile) |
|---|---|---|
| Where the broker runs | On the Loams VM, next to the engine | Confluent's managed cluster, ideally in the VM's cloud and region |
| Who your apps talk to | `127.0.0.1:9093` on the VM (or over your tailnet) | Confluent's bootstrap endpoint, with your own Confluent keys |
| How data reaches Loams | Directly: it is Loams' own Kafka front | `loams-fabric connect` reads topics into Loams streams and writes streams to topics ([confluent-cloud.md](connectors/confluent-cloud.md)) |
| Cost | The VM's disk and CPU | Confluent's bill; Loams bounds only its own share ([byoc/confluent.md](byoc/confluent.md) §6) |
| Kafka features | All but transactions (preview) | Everything Confluent offers, transactions included |
| Status | Preview | Preview (CC1) |

Pick Iggy when nothing outside the VM needs the topics and you want no second bill. Pick
Confluent when your apps already use Confluent, or need its connectors, Schema Registry or
transactions. Both can run at once (`--kafka iggy,confluent`), each named: there is no shorthand
that turns on both.

With `deploy/release/byoc/install.sh`:

```sh
# Iggy on the VM
install.sh --kafka iggy user@host
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
`secrets/connect/0/confluent-<integration>/{username,password}` yourself.

Try it with kcat:

```sh
echo hello | kcat -P -b 127.0.0.1:9093 -t demo     # needs KAFKA_AUTO_CREATE_TOPICS=true, or create the topic first
kcat -C -b 127.0.0.1:9093 -t demo -o beginning -e
```

`deploy/release/ci/kafka-smoke.sh` does this against a throwaway project and is what CI runs.

## Settings

All in `.env` (see `.env.example`):

| Variable | Default | Meaning |
|---|---|---|
| `IGGY_KAFKA_IMAGE` | a digest | The image pair. Move it only with a release |
| `KAFKA_BIND`, `KAFKA_PORT` | `127.0.0.1`, `9093` | Where the gateway is published |
| `KAFKA_ADVERTISED_HOST` | `127.0.0.1` | The host clients are told to reconnect to. A client on another machine needs a name it can reach, for example the stack's tailnet name |
| `KAFKA_SASL_ENABLED` | `false` | SASL/PLAIN against Iggy users |
| `KAFKA_AUTO_CREATE_TOPICS` | `false` | Create a topic on first use |
| `KAFKA_IGGY_USER`, `KAFKA_IGGY_PASSWORD` | `iggy`, the placeholder `change-me` (refused) | Iggy's root user, which the gateway signs in as. Required: at least 12 characters |

## What to know

- **Not wired to the engine yet.** Records land in Iggy, not in Loams streams or collections.
  The Event Fabric bridge (the `loams_sink` connector, FL1 in design 32) is what carries Iggy
  topics into Loams, and it is planned. Until then this profile is a Kafka-compatible event log
  next to your stack.
- **No authentication by default.** With SASL off, anyone who can reach the port can create and
  delete topics. The default is loopback for that reason. Before you bind it elsewhere, turn SASL
  on, use your tailnet, and note that SASL/PLAIN sends the password in clear.
- **Transactions and exactly-once are not supported** (planned in the fork, not built):
  Flink `EXACTLY_ONCE`, Kafka Streams `exactly_once_v2` and `read_committed` over transactional
  topics will not work.
- **Container flags.** Iggy uses io_uring, which Docker's default seccomp profile refuses, so
  `iggy` runs with `seccomp:unconfined` and `SYS_NICE`, as Iggy's own quick start does.
- **Back up** the `iggy-data` volume; like the default stack, it is one node on one disk.
- Compatibility, per client, is tracked in the fork's plan
  ([`docs/loams/kafka-wire-plan.md`](https://github.com/ostrium-labs/iggy/blob/loams/docs/loams/kafka-wire-plan.md)).
