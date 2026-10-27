# Loams provider for Apache Airflow

Status: **preview**. The package has unit tests that use mocks. It has **not yet been run end to
end** against a Loams stack. It targets Apache Airflow 3.3 (tested on 3.3.2) and Python 3.10 or
later. It is real Airflow with a provider: Loams does not ship its own Airflow (design 68 §4, D1256).

Where it sits in the designs:

- §68 §4.4 lists the provider's contents.
- §74 §9.4 adds `LoamsPostgresHook` and the executor.
- The executor, `LoamsResonateExecutor`, is in this package under `executors/` (preview). See
  [the executor guide](../../../docs/guides/durable/airflow-resonate-executor.md).
- The package lives in the monorepo at `integrations/airflow/` (D2100).

## Install

```sh
pip install 'airflow-provider-loams[all]'
```

The extras are optional and each one adds a group of hooks:

| Extra | What it adds |
|---|---|
| `kafka` | `confluent-kafka`, for the streams hooks |
| `s3` | `boto3` and `s3fs`, for object storage and `loams-s3://` |
| `iceberg` | `pyiceberg[pyarrow]`, for the Iceberg hooks |

Postgres, House and Graph need none of them.

## The `loams` connection

One connection describes one Loams instance and a namespace in it.

| Field | Value |
|---|---|
| Connection type | `loams` |
| Endpoint (host) | The instance's base URL, for example `https://acme.loams.dev` or `http://127.0.0.1:8080`. Without a scheme, `https://` is assumed |
| Port | Optional |
| API key (password) | A Loams API key, sent as `Authorization: Bearer` |
| Namespace (schema) | The Loams namespace |
| Extra | The per-service settings below, all optional |

As a URI (`AIRFLOW_CONN_LOAMS_DEFAULT`). URL-encode the extras when you use this form:

```
loams://:lk_yourkey@loams.example.com:443/acme?pg_project=prj-01J...&kafka_bootstrap_servers=gw%3A9093
```

As JSON:

```json
{
  "conn_type": "loams",
  "host": "http://127.0.0.1:8080",
  "password": "lk_...",
  "schema": "acme",
  "extra": {
    "postgres_control_endpoint": "http://127.0.0.1:7680",
    "pg_project": "prj-01J...",
    "kafka_bootstrap_servers": "127.0.0.1:9093",
    "s3_endpoint": "http://127.0.0.1:9000",
    "s3_access_key_id": "...",
    "s3_secret_access_key": "...",
    "s3_bucket": "lake",
    "iceberg_catalog_uri": "http://127.0.0.1:8181/catalog",
    "iceberg_warehouse": "lake",
    "graph": "social"
  }
}
```

| Extra | Used by | Meaning |
|---|---|---|
| `scheme`, `verify`, `timeout` | all HTTP | Scheme for a bare host; TLS verification (`false` or a CA bundle path); seconds |
| `postgres_control_endpoint`, `postgres_control_api_key` | Postgres | `pg-control`'s URL (`loams.postgres.v1`) and its key, if different from the API key |
| `pg_project`, `pg_branch`, `pg_endpoint`, `pg_role`, `pg_database`, `pg_read_only`, `pg_credential_ttl_seconds` | Postgres | Defaults for discovery |
| `pg_host`, `pg_port`, `pg_user`, `pg_password`, `pg_sslmode` | Postgres | A direct address, which skips discovery |
| `house_endpoint`, `house_database` | House | Where `loams.house.v1` is served (default: the endpoint, which proxies it) and the default database |
| `graph` | Graph | The default graph |
| `kafka_bootstrap_servers`, `kafka_security_protocol`, `kafka_sasl_mechanism`, `kafka_sasl_username`, `kafka_sasl_password`, `kafka_config` | Streams | The Kafka gateway, SASL/PLAIN, and any librdkafka settings |
| `s3_endpoint`, `s3_region`, `s3_access_key_id`, `s3_secret_access_key`, `s3_bucket`, `s3_path_style` | Object storage, Iceberg FileIO | The bucket's S3 endpoint and keys. Path-style addressing is on by default |
| `iceberg_catalog_uri`, `iceberg_warehouse`, `iceberg_token`, `iceberg_credential`, `iceberg_oauth2_server_uri`, `iceberg_scope`, `iceberg_properties` | Iceberg | Lakekeeper. The bearer defaults to the API key. `iceberg_properties` is passed to PyIceberg as is |

Airflow masks extras whose names contain `password` or `secret` in logs and in the UI.

## Loams Postgres

`LoamsPostgresHook` is Airflow's `PostgresHook`. The one difference is where its connection comes
from. For each hook, it calls `loams.postgres.v1.PostgresService/IssueConnectCredential` on
`pg-control`. That call returns a short-lived login (15 minutes by default, at most 1 hour) and where
to connect: PgDog's host and port, the routed database name, `sslmode=verify-full` and the CA bundle.
With `pg_database` or `read_only`, the hook also calls `GetConnectionInfo` for that database's routed
name, or for the branch's read-only endpoints. Nothing is stored. The CA bundle goes to a private
temporary file.

Everything else from `PostgresHook` and `DbApiHook` works: `run`, `get_records`, `get_df`,
`insert_rows`, SQLAlchemy and OpenLineage. `LoamsPostgresOperator` is `SQLExecuteQueryOperator`
with this hook.

```python
LoamsPostgresOperator(task_id="load", sql="sql/load.sql", project="prj-01J...", branch="br-...")
```

## Loams House

`LoamsHouseHook` speaks `loams.house.v1.HouseService`. The engine's main port proxies it, the API key
signs in, and every statement runs through the same checks as the ClickHouse HTTP interface. The
Python SDK has no `house` module yet, so the hook calls the RPCs over Connect's JSON encoding.

- `run(sql, params=..., settings=..., read_only=True, max_rows=...)` answers columns, rows and the
  summary. The summary includes `snapshot_ids` after a write to a lake table.
- `get_query(id)` and `cancel(id)` read and cancel a query.

`LoamsHouseQueryOperator` takes one statement or a list, because the House takes one statement per
call. `params_` fills `{name:Type}` placeholders. Set `read_only=False` for writes.

- **Query ids.** Each query gets an id fixed by the task try:
  `airflow:<dag>:<task>:<run>:<map>:<try>`.
- **Deferrable mode.** With `deferrable=True`, the last statement runs from the triggerer and the
  worker slot is freed. If the triggerer restarts, the trigger first looks the id up and follows a
  query that is already running instead of starting it again.

`LoamsHouseQuerySensor` waits for a query id, in a worker or deferred.

## Streams (Kafka gateway)

Loams's Kafka wire front end is the Iggy Kafka gateway, in the `kafka` compose profile (see
`docs/guides/kafka.md`). The streams hooks are confluent-kafka clients configured from the connection.

- `LoamsStreamProduceOperator` produces records and waits for every acknowledgement. Each record is
  `{"key", "value", "headers", "partition"}` or a bare value. Text is sent as UTF-8; anything else is
  sent as JSON.
- `LoamsStreamConsumeOperator` reads up to `max_messages` as a consumer group, then commits. With
  `apply_function`, it commits only after the function has returned.
- `LoamsStreamOffsetSensor` and `LoamsStreamOffsetTrigger` wait for one of three conditions:
  - `mode_="offset"`: every partition in `target_offsets` has reached its offset;
  - `mode_="new_records"`: the group has at least `min_records` records it has not read;
  - `mode_="caught_up"`: the group has read everything.
- `LoamsStreamMessageTrigger` is a `BaseEventTrigger` for `AssetWatcher`:

```python
Asset(
    "loams://acme/streams/table-commits",
    watchers=[
        AssetWatcher(
            name="commits",
            trigger=LoamsStreamMessageTrigger(
                topics="table-commits",
                group_id="airflow-commits",
                match={"type": "io.loams.dev.table.commit.v1"},
            ),
        )
    ],
)
```

It fires at most once per record: the offset is committed before the event is yielded.

**Not yet:**

- Records produced through the gateway land in Iggy. The Event Fabric bridge that carries them into
  Loams streams is planned, not built.
- The gateway has no transactions and no exactly-once.

## Object storage

`LoamsObjectStorageHook` is boto3 against the bucket's S3 endpoint: RustFS in the default stack, or
the cloud bucket in a BYOC install. The operators upload, list, copy and delete. A delete by prefix
refuses an empty prefix. `LoamsObjectKeySensor` waits for keys, or for shell patterns with
`wildcard=True`.

The `loams-s3://` ObjectStoragePath scheme uses the same settings:

```python
path = ObjectStoragePath("loams-s3://loams_default@lake/exports/orders.csv")
path.write_text(csv_text)
```

## Iceberg tables

`LoamsIcebergHook` is PyIceberg's `RestCatalog` on Lakekeeper.

- **Bearer.** It uses `iceberg_credential` (OAuth2 client credentials) if set, else `iceberg_token`,
  else the API key.
- **Storage.** It uses the `s3_*` extras if set. Otherwise it asks Lakekeeper for vended credentials.

The operators create a namespace, append or overwrite rows, and scan. `LoamsIcebergSnapshotSensor`
waits for a table to get a snapshot, or a snapshot other than the one an earlier run saw.

## Loams Graph

`LoamsGraphHook` uses the Loams Python SDK's `graph` module. The SDK marks that module unstable.

- **Parameters.** They are bound as `$name` values and never put into the statement text.
- **Results.** Values come back as plain Python, ready for XCom: nodes, relationships and paths as
  dicts, and temporal values as ISO 8601 text.
- **Retries.** A statement that writes is sent with an idempotency key fixed by the task, so a retry
  after a lost answer does not write twice.

## Assets: `loams://`

`loams://<namespace>/<kind>/<name>`. The kinds are `streams`, `tables` (Iceberg, `<ns>.<table>`),
`house` (`<db>.<table>`), `postgres` (`<project>/<schema>.<table>`), `graphs` and `objects`
(`<bucket>/<key>`).

The handler normalises a URI. It refuses a URI without a namespace or an object, or with a port or
credentials. `loams_uri()` and the factory `create_asset()` build URIs.

## Executor (preview)

`airflow.providers.loams.executors.resonate_executor.LoamsResonateExecutor` runs each task instance
try as a Resonate durable promise, and `loams-airflow-resonate-worker` runs the tries. Configuration,
routing, failure reporting and the compose profile `airflow-resonate` are in
[the executor guide](../../../docs/guides/durable/airflow-resonate-executor.md).

## Not built yet

- An end-to-end run of `LoamsResonateExecutor` and its worker (`executors/`; unit-tested only).
- The Temporal and Resonate operators of §74 §9.4.
- `LoamsSecretsBackend`, remote logging, the auth manager.
- OpenLineage converters for `loams://`.
- An end-to-end test against the compose stack.
