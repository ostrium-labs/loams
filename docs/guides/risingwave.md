# RisingWave on Loams: streams to materialized views to Iceberg (preview)

Status: **preview, not yet run end to end.** The builders, the SQL API and the
`loams risingwave` command are written and unit-tested in CI; nobody has run
this walkthrough against a live managed RisingWave yet. Design 81 (managed stream
and batch engines) and plan RW1 cover the whole engine; this guide covers the data
integration and SQL surface (lane RW-b).

RisingWave is Apache-2.0. Loams runs it without a licence key, so nothing here uses
a RisingWave Premium feature: no `CREATE SECRET`, no automatic CDC schema change,
no time travel (design 81 R4).

## What you get

```
Loams stream (Iggy topic)
   │  Kafka wire, SASL/PLAIN as the instance's Iggy user
   ▼
kafka-gateway:9093 ──► CREATE SOURCE clicks (connector = 'kafka')
                          │
                          ▼
                       CREATE MATERIALIZED VIEW clicks_per_minute (TUMBLE, watermark)
                          │
                          ▼
                       CREATE SINK … (connector = 'iceberg', catalog.type = 'rest')
                          │  vended credentials
                          ▼
                       Lakekeeper (warehouse loams) ──► bucket loams-lake ──► House, Spark, Flink read it
```

- **In:** Loams streams through the Iggy Kafka gateway (the `kafka` profile), and
  Loams Postgres tables through RisingWave's `postgres-cdc` connector.
- **Out:** Iceberg through Lakekeeper (exactly-once, committed on checkpoints), Loams
  streams through the gateway (at-least-once: the gateway has no Kafka
  transactions), and Loams Postgres through the `jdbc` or `postgres` sink.
- The House reads what RisingWave writes as Iceberg lake tables. A ClickHouse sink
  straight into the House is not a default path (design 81 R10).

## Before you start

- A compose stack with the `kafka` and `risingwave` profiles (the `risingwave`
  profile brings `compose.lake.yaml`: Lakekeeper and `engine-gateway`).
- A managed RisingWave instance. Its scoped credentials come from the engines
  facade (design 81 R5): an Iggy user for the gateway, a Postgres role per CDC
  source, and (in Kubernetes) a Lakekeeper OAuth2 client. Your own Loams API key is
  only used to reach the instance through `engine-gateway`; it is never written
  into RisingWave SQL.
- The `loams` binary.

```sh
export LOAMS_RISINGWAVE_URL=postgres://loams@127.0.0.1:4566/dev   # engine-gateway, the default
export LOAMS_RISINGWAVE_PASSWORD=<your Loams API key>             # or LOAMS_API_KEY
loams risingwave sql "SELECT version()"
```

## 1. Write the pipeline

`examples/risingwave/clicks-to-iceberg.toml` holds the three statements. A
pipeline is TOML (or JSON) with one `[[statements]]` entry per statement, run in
order. Credentials are `{ env = "VAR" }`: the file never holds a value.

Print the SQL it produces, with credentials redacted (this does not connect):

```sh
loams risingwave render examples/risingwave/clicks-to-iceberg.toml
```

```sql
CREATE SOURCE IF NOT EXISTS "clicks" (
    "user_id" BIGINT,
    "url" VARCHAR,
    "ts" TIMESTAMPTZ,
    WATERMARK FOR "ts" AS "ts" - INTERVAL '5 seconds'
)
WITH (
    connector = 'kafka',
    topic = 'web.clicks',
    properties.bootstrap.server = 'kafka-gateway:9093',
    properties.security.protocol = 'SASL_PLAINTEXT',
    properties.sasl.mechanism = 'PLAIN',
    properties.sasl.username = 'eng_01jbzq7m3k8x2v5n9c4t6w0r1y',
    properties.sasl.password = '<redacted>',
    scan.startup.mode = 'earliest'
)
FORMAT PLAIN ENCODE JSON;

CREATE MATERIALIZED VIEW IF NOT EXISTS "clicks_per_minute" AS
SELECT window_start, url, count(*) AS clicks
FROM TUMBLE(clicks, ts, INTERVAL '1 minute')
GROUP BY window_start, url
EMIT ON WINDOW CLOSE;

CREATE SINK IF NOT EXISTS "clicks_per_minute_lake" FROM "clicks_per_minute"
WITH (
    connector = 'iceberg',
    type = 'append-only',
    catalog.type = 'rest',
    catalog.uri = 'http://lakekeeper:8181/catalog',
    catalog.name = 'loams',
    warehouse.path = 'loams',
    catalog.header = 'X-Iceberg-Access-Delegation=vended-credentials',
    database.name = 'ns_default',
    table.name = 'clicks_per_minute',
    create_table_if_not_exists = 'true',
    commit_checkpoint_interval = '10',
    partition_by = 'day(window_start)'
);
```

Every identifier is double-quoted and every value single-quoted with quotes
doubled, so a topic or table name with odd characters cannot change a statement.

## 2. Apply it

```sh
RW_KAFKA_PASSWORD=<the instance's Iggy password> \
  loams risingwave apply examples/risingwave/clicks-to-iceberg.toml --wait
```

`apply` runs the statements in order and stops at the first error, showing the
redacted statement. `--wait` follows the backfill of each job it started
(`rw_catalog.rw_ddl_progress`) until it finishes. `--dry-run` is the same as
`render`.

## 3. Watch it

```sh
loams risingwave list                 # sources, tables, MVs and sinks in your schemas
loams risingwave list sinks --definitions
loams risingwave progress             # jobs still backfilling, with their progress
loams risingwave progress 1042 --wait
loams risingwave cancel 1042          # CANCEL JOBS 1042
loams risingwave show-create mv clicks_per_minute
loams risingwave sql "SELECT * FROM clicks_per_minute ORDER BY window_start DESC LIMIT 10"
```

Add `--json` to any of them for JSON on stdout.

The Iceberg table appears in Lakekeeper as `ns_default.clicks_per_minute` in the
`loams` warehouse; the House, Spark and Flink read it from there.

## Other building blocks

The pipeline statements, and the Rust builders behind them
(`loams_risingwave::connectors`):

| `statement` | Builder | Notes |
|---|---|---|
| `kafka` | `kafka::KafkaIngest` | `kind = "source"` or `"table"`; `FORMAT UPSERT` needs a table with the included key as its primary key; Avro and Protobuf may take their columns from a schema registry |
| `postgres_cdc_source` | `postgres_cdc::PostgresCdcSource` | one per database; the controller creates the publication (`publication.create.enable = 'false'`) and the slot is `loams_rw_<instance26>_<n>` (`cdc_slot_name`); logical replication on Loams Postgres is (verify) |
| `cdc_table` | `postgres_cdc::CdcTable` | `CREATE TABLE t (*) FROM src TABLE 'public.t'`, or explicit columns with the upstream primary key |
| `materialized_view` | `MaterializedView` | any query |
| `iceberg_sink` | `iceberg::IcebergSink` | `RestCatalog::loams_compose()` (auth off) or `RestCatalog::oauth2(…)` (Kubernetes, Zitadel) |
| `kafka_sink` | `kafka::KafkaSink` | back into a Loams stream; `FORMAT UPSERT` and `DEBEZIUM` need `primary_key` |
| `postgres_sink` | `postgres_sink::PostgresSink` | `connector = "jdbc"` (default) or `"native"`; upserts need the target's primary key |
| `house_sink` | `clickhouse::HouseSink` | not a default path; no default address |
| `drop`, `sql` | `DropObject`, raw SQL | one statement each |

A CDC example:

```toml
[[statements]]
statement = "postgres_cdc_source"
name = "pg_shop"
slot_name = "loams_rw_01jbzq7m3k8x2v5n9c4t6w0r1y_1"
endpoint = { host = "compute1", port = 55433, database = "shop", user = "rw_cdc_1", password = { env = "RW_CDC_PASSWORD" }, ssl_mode = "required" }

[[statements]]
statement = "cdc_table"
name = "orders"
source = "pg_shop"
upstream_table = "public.orders"
schema = { infer = true }
```

## Limits in this preview

- Not yet run end to end against a live managed instance; the conformance harness
  (`conformance/risingwave/`) that runs RisingWave's own sqllogictest subset is
  written but has not run.
- The Kafka gateway has no transactions: Kafka sinks are at-least-once.
- `loams risingwave` uses TLS with a verified certificate for any host that is
  not loopback (a URL with `sslmode=disable` to such a host is refused) and plain
  TCP only to loopback, which is how the compose `engine-gateway` is reached. A
  gateway with a private CA needs `--ca-file` (or `LOAMS_RISINGWAVE_CA_FILE`);
  `--tls` uses TLS to loopback too.
- RisingWave option names follow its documentation and are checked against the
  pinned image only when the conformance harness runs.
