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
KAFKA_IGGY_PASSWORD=<a password of your own>   # Iggy keeps it in its volume
docker compose up -d
```

On a remote machine: `deploy/release/byoc/install.sh --profiles kafka`.

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
| `KAFKA_IGGY_USER`, `KAFKA_IGGY_PASSWORD` | `iggy`, a placeholder | Iggy's root user, which the gateway signs in as |

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
