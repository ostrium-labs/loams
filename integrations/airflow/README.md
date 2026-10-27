# airflow-provider-loams

An Apache Airflow 3.3 provider for [Loams](https://loams.dev): use Loams Postgres, Loams House,
Loams streams, object storage, Iceberg tables and Loams Graph from your DAGs.

**Status: preview.** The code has unit tests that use mocks. It has **not yet been run end to end**
against a Loams stack. It is not published to PyPI yet. Not an Apache Software Foundation release.

| Area | Hook | Operators | Sensors / triggers |
|---|---|---|---|
| Connection | `LoamsHook` (connection type `loams`: endpoint, API key, namespace) | | |
| Loams Postgres | `LoamsPostgresHook`: `PostgresHook`, with endpoint discovery and a short-lived login from `pg-control` | `LoamsPostgresOperator` | |
| Loams House (ClickHouse SQL) | `LoamsHouseHook` (`loams.house.v1`) | `LoamsHouseQueryOperator` (deferrable) | `LoamsHouseQuerySensor`, `LoamsHouseQueryTrigger` |
| Streams (Iggy Kafka gateway) | `LoamsStreamHook` (confluent-kafka) | `LoamsStreamProduceOperator`, `LoamsStreamConsumeOperator`, `LoamsStreamWaitOperator` | `LoamsStreamOffsetSensor`, `LoamsStreamOffsetTrigger`, `LoamsStreamMessageTrigger` (for `AssetWatcher`) |
| Object storage (S3-compatible) | `LoamsObjectStorageHook`, `ObjectStoragePath("loams-s3://...")` | upload, list, copy, delete | `LoamsObjectKeySensor` |
| Iceberg (Lakekeeper REST catalog) | `LoamsIcebergHook` (PyIceberg) | create namespace, append/overwrite, scan | `LoamsIcebergSnapshotSensor` |
| Loams Graph | `LoamsGraphHook` (the Loams Python SDK) | `LoamsGraphQueryOperator` | |
| Assets | `loams://<namespace>/<kind>/<name>` URI scheme | | |

```sh
pip install 'airflow-provider-loams[all]'   # or [kafka], [s3], [iceberg]
```

The full guide covers the connection fields, each service and what is not built yet:
[docs/index.md](docs/index.md). Examples are in [example_dags/](example_dags/).

## Development

```sh
cd integrations/airflow
uv sync --all-extras --dev
uv run ruff check . && uv run ruff format --check .
uv run pytest -q
```

CI runs the same three commands (`.github/workflows/airflow-provider.yml`). Dependencies resolve with
`exclude-newer` set 14 days back, and Airflow is pinned to 3.3.2.
