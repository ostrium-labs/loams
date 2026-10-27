# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import json
import os
import tempfile

# Airflow reads its home and config on first import: keep both private to the run.
os.environ.setdefault("AIRFLOW_HOME", tempfile.mkdtemp(prefix="airflow-loams-tests-"))
os.environ.setdefault("AIRFLOW__CORE__UNIT_TEST_MODE", "True")
os.environ.setdefault("AIRFLOW__CORE__LOAD_EXAMPLES", "False")

from types import SimpleNamespace
from typing import Any

import pytest
from airflow.sdk import Connection

from airflow.providers.loams.hooks.base import LoamsHook

DEFAULT_EXTRA: dict[str, Any] = {
    "postgres_control_endpoint": "http://pg-control:7680",
    "pg_project": "prj-01TEST",
    "kafka_bootstrap_servers": "127.0.0.1:9093",
    "s3_endpoint": "http://rustfs:9000",
    "s3_access_key_id": "ak",
    "s3_secret_access_key": "sk",
    "s3_bucket": "lake",
    "iceberg_catalog_uri": "http://lakekeeper:8181/catalog",
    "iceberg_warehouse": "wh",
    "graph": "social",
}


def make_connection(extra: dict[str, Any] | None = None, **fields: Any) -> Connection:
    values: dict[str, Any] = {
        "conn_id": "loams_default",
        "conn_type": "loams",
        "host": "http://loams:8080",
        "password": "lk_test",
        "schema": "acme",
    }
    values.update(fields)
    return Connection(extra=json.dumps(DEFAULT_EXTRA if extra is None else extra), **values)


@pytest.fixture
def loams_connection(monkeypatch: pytest.MonkeyPatch):
    """Every Loams hook resolves `loams_default` (or any id) to this connection.

    Tests change it with `loams_connection(extra=..., host=...)`.
    """
    state = {"conn": make_connection()}

    def get_connection(cls: type, conn_id: str) -> Connection:
        return state["conn"]

    monkeypatch.setattr(LoamsHook, "get_connection", classmethod(get_connection))

    def set_connection(extra: dict[str, Any] | None = None, **fields: Any) -> Connection:
        state["conn"] = make_connection(extra, **fields)
        return state["conn"]

    return set_connection


@pytest.fixture
def context() -> dict[str, Any]:
    ti = SimpleNamespace(
        dag_id="dag", task_id="task", run_id="manual__2026-10-27T00:00:00+00:00", map_index=-1, try_number=1
    )
    return {"ti": ti, "ds": "2026-10-27"}
