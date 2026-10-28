# Flink SQL on Loams (preview)

Status: **preview, not yet run end to end.** Written against Apache Flink 2.2.1, the Flink SQL
Gateway REST API (v3), Apache Iceberg 1.12.0 and Flink CDC 3.7.0 documentation; the code paths have
unit and golden tests, but no lane has run this walkthrough against a live stack yet (design 81,
ENG81 R20). Commands and outputs below are what the code is written to do.

Managed Flink SQL runs Apache Flink unmodified (the official image plus a pinned bundle of Apache
connector jars) as one session cluster per engine instance, reached only through Loams: the
`loams flink` CLI and the Loams API, or Flink's own SQL Client through loams-gateway (ENG81 R25).

This guide builds one pipeline:

```text
Loams stream "clicks"  ──Kafka wire (Iggy gateway)──▶  Flink SQL: 1-minute tumbling window
                                                             │
                                                             ▼
                              Iceberg table loams.analytics.clicks_per_minute (Lakekeeper)
                                                             │
                                                             ▼
                                         House: SELECT … FROM analytics.clicks_per_minute
```

## What every session gets

Each session the API opens runs a bootstrap script first (decisions D1980 to D1985):

| Name | What | Notes |
|---|---|---|
| `loams` | Iceberg catalog on Lakekeeper's REST endpoint | `base-namespace = ns_<id>`: Flink database `d` is Iceberg namespace `ns_<id>.d`, which the House shows as database `d` |
| `loams_pg` | JDBC catalog on a Loams Postgres database | when the instance or the job names one |
| connector tables | `kafka`, `upsert-kafka`, `postgres-cdc` | temporary, in `default_catalog.default_database` |
| settings | streaming mode, 30 s checkpoints, `UTC`, the instance's savepoint directory | Iceberg commits on checkpoints, so 30 s is also how fresh the lake is |

Catalogs and tables are session-scoped: Flink never stores their options, so credentials live only
as long as the session. Saved jobs keep secret **references** (`{ ref = "<name>" }`), resolved
when a session opens; the engine's credentials are its own scoped service credentials (an Iggy user
for the namespace's topics, a Lakekeeper client), never your API key (ENG81 R5).

See the script for your instance (secrets show as `${secret:<name>}`):

```sh
export LOAMS_URL=http://127.0.0.1:8080 LOAMS_TOKEN=<api key> LOAMS_NAMESPACE=<ns> LOAMS_FLINK_ENGINE=<eng_…>
loams flink bootstrap
```

## 1. Events into a Loams stream

With the `kafka` compose profile on ([Kafka clients](kafka.md)), produce JSON clicks:

```sh
for i in $(seq 1 100); do
  printf '{"user_id":"u%d","url":"/p/%d","ts":"%s"}\n' $((i % 7)) $((i % 3)) "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"
done | kcat -P -b 127.0.0.1:9093 -t clicks
```

## 2. The pipeline as a saved job

A job is a SQL file whose last statement is the pipeline (one `INSERT` or `EXECUTE STATEMENT SET`)
and whose earlier statements are its setup; its connector tables go in a session spec.

`clicks.toml`, the Kafka table (the engine's Iggy credentials are the secret `engine.kafka`):

```toml
[[tables]]
connector = "kafka"
name = "clicks"
stream = "clicks"
startup = { mode = "earliest-offset" }
columns = [
  { name = "user_id", type = "STRING" },
  { name = "url", type = "STRING" },
  { name = "ts", type = "TIMESTAMP_LTZ(3)" },
]
watermark = { column = "ts", delay_ms = 5000 }
gateway = { security = "sasl_plaintext", auth = { username = "engine", password = { ref = "engine.kafka" } } }
```

`clicks_per_minute.sql`:

```sql
CREATE DATABASE IF NOT EXISTS loams.analytics;

CREATE TABLE IF NOT EXISTS loams.analytics.clicks_per_minute (
  window_start TIMESTAMP_LTZ(3),
  window_end   TIMESTAMP_LTZ(3),
  url          STRING,
  clicks       BIGINT
);

INSERT INTO loams.analytics.clicks_per_minute
SELECT window_start, window_end, url, COUNT(*) AS clicks
FROM TABLE(TUMBLE(TABLE clicks, DESCRIPTOR(ts), INTERVAL '1' MINUTE))
GROUP BY window_start, window_end, url;
```

Create and start it:

```sh
loams flink jobs create --name "clicks per minute" --file clicks_per_minute.sql --session clicks.toml
# fj_5c0f…  draft  clicks per minute  flink:-  savepoint:-
loams flink jobs start fj_5c0f…
# fj_5c0f…  running  clicks per minute  flink:3f2a…  savepoint:-
```

A window aggregation over the window TVF emits each window once, when the watermark passes its
end, so the Iceberg table is append-only. Rows appear after the window closes and the next
checkpoint commits (up to about 1 minute + 5 s + 30 s here).

Kafka sinks through the Iggy gateway are **at-least-once** (the gateway has no transactions; the
builders refuse `exactly-once`). Iceberg sinks commit on checkpoints and are exactly-once.

## 3. Look at it from Flink

```sh
loams flink sql
```

```text
Loams Flink SQL (preview). Session fs_….
flink> SELECT url, SUM(clicks) FROM loams.analytics.clicks_per_minute GROUP BY url;
op | url | EXPR$1
+I | /p/0 | 33
-U | /p/0 | 33
+U | /p/0 | 34
…
```

Results stream as a changelog (`+I`, `-U`, `+U`, `-D`); Ctrl-C cancels the statement, `\q` quits.
`loams flink sql -x '…'` or `-f file.sql` runs statements and exits.

## 4. Query it in the House

The House lists every Iceberg table of the namespace it did not create as a read-only external
table, so the Flink output needs no registration:

```sql
SELECT url, sum(clicks) AS clicks
FROM analytics.clicks_per_minute
WHERE window_start >= now() - INTERVAL 1 HOUR
GROUP BY url
ORDER BY clicks DESC;
```

## 5. Stop, change, resume

```sh
loams flink jobs stop fj_5c0f…        # STOP JOB '<id>' WITH SAVEPOINT
# fj_5c0f…  stopped  clicks per minute  flink:-  savepoint:s3://loams-engines/engines/<ns>/<eng>/savepoints/savepoint-…
loams flink jobs resume fj_5c0f…      # re-runs the bootstrap and the INSERT from that savepoint
```

`--no-savepoint` stops without one (the job can then only be started fresh); `--drain` flushes
event-time windows first, for a final stop.

## Postgres CDC

A `postgres-cdc` table reads a Loams Postgres table's changes (Flink CDC, `pgoutput`, the
incremental snapshot, which needs a primary key). The controller creates the publication and the
CDC role; the slot is named `loams_fk_<instance>_<n>` (ENG81 R12). Logical replication on Loams
Postgres is still to be verified.

```toml
[[tables]]
connector = "postgres_cdc"
name = "orders"
host = "compute1"
database = "shop"
table = "orders"
username = "loams_fk_cdc_1"
password = { ref = "engine.pg.cdc1" }
slot = "loams_fk_01jbx3k9q2w7r5t8y6u4i0p3a_1"
primary_key = ["id"]
columns = [
  { name = "id", type = "BIGINT", not_null = true },
  { name = "amount", type = "DECIMAL(10, 2)" },
]
```

## The image and its jars

`loams flink jars` prints the bundle; `deploy/flink/connector-jars.toml` is the pinned list (each
jar's version and sha256) and `deploy/flink/Containerfile` layers it onto the official
`flink:2.2.1-scala_2.12-java21` image. The image is not built or published yet.

| Jar | Version | Licence |
|---|---|---|
| iceberg-flink-runtime-2.2, iceberg-aws-bundle | 1.12.0 | Apache-2.0 |
| flink-sql-connector-kafka | 5.0.0-2.2 | Apache-2.0 |
| flink-connector-jdbc-core, flink-connector-jdbc-postgres | 4.1.0-2.2 | Apache-2.0 |
| postgresql (JDBC driver) | 42.7.14 | BSD-2-Clause |
| flink-sql-connector-postgres-cdc | 3.7.0-2.2 | Apache-2.0 |
| hadoop-client-api, hadoop-client-runtime | 3.4.2 | Apache-2.0 |

## The API

Under `/v1/namespaces/{ns}/engines/{instance}/flink` (bearer auth; reads need view, the rest
operate on the namespace):

| Method | Path | What |
|---|---|---|
| GET | `/bootstrap` | the session script, redacted |
| POST | `/sql` | `{sql, max_rows?, timeout_ms?}`: run in a fresh session, return the last result |
| POST, DELETE | `/sessions`, `/sessions/{s}` | open (bootstrapped), close |
| POST | `/sessions/{s}/statements` | `{statement}` → `{statement: "st_…"}` |
| GET | `/sessions/{s}/statements/{st}/results?max_rows=&wait_ms=` | NDJSON: `columns`, `rows`, `job`, then `end` or `error` |
| DELETE | `/sessions/{s}/statements/{st}` | cancel |
| GET, POST | `/jobs` | list, create |
| GET, DELETE | `/jobs/{j}` | read (refreshed from Flink), delete (not while running) |
| POST | `/jobs/{j}/start`, `/jobs/{j}/stop` (`{savepoint, drain}`), `/jobs/{j}/resume` | lifecycle |

Sessions may not `SET` the instance's state keys (checkpoint and savepoint directories,
`execution.state-recovery.path`, the state backend, HA).

## Known gaps

- Not run end to end yet; the compose profile is FL-a's `compose.flink.yaml` (design 81).
- Saved jobs live in memory until the Postgres-backed store lands with the engines facade's store.
- A session cluster without HA (compose) loses running jobs when the JobManager restarts; the job
  shows `failed` on its next read and resumes from its last savepoint.
- Cross-version Flink upgrades of stateful SQL are not promised in this preview.
