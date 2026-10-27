# SPDX-License-Identifier: Apache-2.0
"""Iceberg tables in the Loams catalog: create a table, append, wait for the commit, read."""

from __future__ import annotations

import pendulum
from airflow.sdk import DAG, Asset, task

from airflow.providers.loams.assets.loams import loams_uri
from airflow.providers.loams.hooks.iceberg import LoamsIcebergHook
from airflow.providers.loams.operators.iceberg import (
    LoamsIcebergAppendOperator,
    LoamsIcebergCreateNamespaceOperator,
    LoamsIcebergScanOperator,
)
from airflow.providers.loams.sensors.iceberg import LoamsIcebergSnapshotSensor

TABLE = "sales.orders"
ORDERS = Asset(loams_uri("default", "tables", TABLE))

with DAG(
    dag_id="example_loams_iceberg",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=None,
    catchup=False,
    render_template_as_native_obj=True,
    tags=["example", "loams"],
) as dag:
    namespace = LoamsIcebergCreateNamespaceOperator(task_id="namespace", namespace="sales")

    @task
    def create_table() -> None:
        from pyiceberg.schema import Schema
        from pyiceberg.types import DoubleType, LongType, NestedField, StringType

        hook = LoamsIcebergHook()
        catalog = hook.get_catalog()
        catalog.create_table_if_not_exists(
            TABLE,
            schema=Schema(
                NestedField(1, "id", LongType(), required=False),
                NestedField(2, "customer", StringType(), required=False),
                NestedField(3, "amount", DoubleType(), required=False),
                NestedField(4, "day", StringType(), required=False),
            ),
        )

    @task
    def extract(ds: str | None = None) -> list[dict]:
        return [
            {"id": 1, "customer": "acme", "amount": 42.5, "day": ds},
            {"id": 2, "customer": "globex", "amount": 7.0, "day": ds},
        ]

    rows = extract()

    append = LoamsIcebergAppendOperator(
        task_id="append",
        table=TABLE,
        rows=rows,
        outlets=[ORDERS],
    )

    committed = LoamsIcebergSnapshotSensor(task_id="committed", table=TABLE, poke_interval=10, timeout=600)

    read = LoamsIcebergScanOperator(
        task_id="read",
        table=TABLE,
        row_filter="day == '{{ ds }}'",
        limit=100,
    )

    namespace >> create_table() >> rows >> append >> committed >> read
