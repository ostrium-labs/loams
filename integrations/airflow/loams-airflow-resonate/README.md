# loams-airflow-resonate (preview)

`ResonateExecutor` is an Apache Airflow 3.3 executor that runs each task instance try as a Resonate durable promise. This package also provides `loams-airflow-resonate-worker`, which claims those promises and runs them through the Airflow Task SDK.

Status: preview. It has unit tests against a fake Resonate. It has not been run end to end yet.

- Guide: [docs/guides/durable/airflow-resonate-executor.md](../../../docs/guides/durable/airflow-resonate-executor.md)
- Design: [§74 §9.3](../../../docs/design/74-durable-execution-compat.md); decisions D2090–D2096
- Compose: [deploy/release/compose.airflow-resonate.yaml](../../../deploy/release/compose.airflow-resonate.yaml)

```
pip install -c https://raw.githubusercontent.com/apache/airflow/constraints-3.3.2/constraints-3.12.txt \
  "apache-airflow==3.3.2" -e ".[test]"
pytest
ruff check . && ruff format --check .
```

Modules:

| Module | What it holds | Needs Airflow |
|---|---|---|
| `ids` | Deterministic promise ids and target groups | no |
| `client` | Resonate protocol client (HTTP + SSE), version 2026-04-01 | no |
| `config` | `[loams_resonate]` settings | no |
| `dispatch` | Executor core: submit, poll, adopt, revoke, outcome mapping | no |
| `worker` | Worker loop and CLI | no (its child process does) |
| `runner` | Child process: `BaseExecutor.run_workload` | yes |
| `executor` | `ResonateExecutor` (alias `LoamsResonateExecutor`) | yes |

Licence: Apache-2.0.
