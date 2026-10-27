# Connectors on a Loams VM: `loams-fabric connect`

Status: **preview** (CC1 Task 1, design [§80 §4.4](../../design/80-confluent-byoc.md), D1821).

`loams-fabric connect` runs native connector instances on the machine that runs your Loams stack: a BYOC VM or your own server. Each instance either reads an outside system into a Loams stream (a source) or writes a Loams stream out to one (a sink). It is the `connectors` compose profile and is off by default.

What it runs: the native families compiled into the `loams-fabric` image: Kafka (with its target profiles Confluent Cloud, Redpanda and Azure Event Hubs), Postgres (with Aurora, CockroachDB and pgvector) and MySQL (with MariaDB). Connectors that need a fallback runtime (Camel, Debezium Server) run on Kubernetes only. An instance file that names one is refused with `runtime_not_supported_on_vm`.

## Turn it on

On a remote machine, `deploy/release/byoc/install.sh --kafka confluent …` does steps 1, 2 and 4 for Confluent Cloud ([kafka.md](../kafka.md), "Kafka endpoint: Iggy or Confluent"). By hand:

1. In `.env`, add `connectors` to `COMPOSE_PROFILES`, for example `COMPOSE_PROFILES=connectors`, and make sure `LOAMS_AUTH_MODE` takes API keys (`tailscale,apikey`, `apikey`, or `oidc` with `secrets/api-keys`): the connect socket accepts nothing else.
2. Create a Loams API key for connect (`loams auth new-key --label connect --append secrets/api-keys`) and write the key it prints to `secrets/connect/engine-key` (mode 0600, owner uid 10001). Optionally write a random token to `secrets/connect/connect-admin-token` for the admin API.
3. Put the connectors' secrets under `secrets/connect/0/<secret>/<key>` (mode 0600, owner uid 10001). On a BYOC VM the secrets fetcher writes the Confluent ones there (`secrets/connect/0/confluent-<integration>/{username,password,…}`).
4. Write one instance file per connector in `connect/instances/` (below).
5. `docker compose up -d`.

## How connect reaches the engine

Connect reaches the engine only through the engine's **connect socket**: a Unix socket at `/run/loams-connect/engine.sock` in the `connect-run` volume, which only the engine and connect mount (CC1 ruling R1.1). The socket takes a Loams API key and nothing else, whatever `LOAMS_AUTH_MODE` is. Connect does not join the engine's network namespace and publishes no port.

Through the socket, a source writes its events with `ProduceCloudEvents`. The engine appends each event once per `source` + `id`, so after a crash the events a source reads again are dropped as duplicates. A sink reads its stream with `FetchCloudEvents`.

## Instance files

```yaml
name: orders-in              # unique; 1-63 characters of [a-z0-9-]
connector: kafka             # a registry manifest id (connectors/registry)
profile: confluent-cloud     # optional: a target profile of the connector's family
direction: source            # source: outside -> Loams stream; sink: Loams stream -> outside
namespace: default           # the stream's engine namespace
namespace_id: 0              # the secrets directory: secrets/connect/<namespace_id>/
stream: orders               # a source creates it if missing (stream_partitions, default 1)
delivery: effectively_once   # optional; must be within the manifest's capabilities
dead_letter_stream: orders-in-dlq   # the default is <name>-dlq; "" turns the DLQ off
config:                      # the connector's own config (connectors/schemas/<id>.config.json)
  brokers: pkc-xxxxx.us-east-1.aws.confluent.cloud:9092
  topics: [orders]
  group_id_prefix: loams-main-
  sasl: { mechanism: plain, username: KEYID, password: confluent-main/password }
  rate_limit: { bytes_per_sec: 1048576 }    # optional, enforced by connect
  pipeline: { max_batch_rows: 10000 }       # optional SDK tuning
```

Secrets are named as `<secret>/<key>` and resolved from `secrets/connect/<namespace_id>/<secret>/<key>`. A secret's value never appears in an instance file, a status, an error or a log line: connect scrubs every value it has resolved from all of them.

Connect refuses to start if any instance file is invalid, and lists every problem.

## The admin API

The admin API listens on `127.0.0.1:7790` inside the connect container. Every route except `/health` needs `Authorization: Bearer <secrets/connect/connect-admin-token>` when that file exists, and it must exist when `--admin-listen` is not a loopback address.

| Call | What it does |
|---|---|
| `GET /v1/connect/instances` | Lists every instance's status: `running`, `paused`, `failed`, `stopped`, `fenced` or `start_failed`, with its last error (scrubbed), splits and rate limit. |
| `GET /v1/connect/instances/{name}` | Returns one instance's status. |
| `POST /v1/connect/instances/{name}:pause` | Stops reading within one poll. The pause is kept across restarts. |
| `POST /v1/connect/instances/{name}:resume` | Continues from the last checkpoint. |
| `POST /v1/connect/instances/{name}:restart` | Stops the instance gracefully and starts it again, for example after a key rotation. |
| `PATCH /v1/connect/instances/{name}` | Sets the rate limit with `{"rate_limit": {"bytes_per_sec": 65536}}`, or returns to the instance file's limit with `{"rate_limit": null}`. |

## State and the single-writer rule

Connect keeps its state under `_meta/connect/` in its store: the `connect-state` volume by default, or your bucket if `LOAMS_CONNECT_STORE=s3://…` (put the `AWS_*` settings in `secrets/connect.env`). The state is:

- source checkpoints;
- sink positions;
- operator controls (pause and rate);
- statuses.

Each process start writes a new lock epoch with a create-only write. If a second connect process starts against the same store, the first one sees the newer epoch and stops its instances without writing another checkpoint (exit code 75). At most one process per install writes positions.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Stopped by a signal. |
| 69 | The store could not be reached. |
| 75 | Fenced by a newer process. |
| 78 | An invalid instance file or flag. |
