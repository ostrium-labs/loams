# SPDX-License-Identifier: Apache-2.0
"""Loams object storage: ObjectStoragePath on `loams-s3://`, a sensor and the operators."""

from __future__ import annotations

import csv
import io

import pendulum
from airflow.sdk import DAG, ObjectStoragePath, task

from airflow.providers.loams.operators.object_storage import (
    LoamsObjectCopyOperator,
    LoamsObjectUploadOperator,
)
from airflow.providers.loams.sensors.object_storage import LoamsObjectKeySensor

# Built inside the task: a path attaches its filesystem (and reads the
# connection) when used, which should not happen while the DAG is parsed.
BASE = "loams-s3://loams_default@lake/airflow-example/"

with DAG(
    dag_id="example_loams_object_storage",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=None,
    catchup=False,
    tags=["example", "loams"],
) as dag:
    marker = LoamsObjectUploadOperator(
        task_id="write_marker",
        bucket="lake",
        key="airflow-example/{{ ds }}/_STARTED",
        data="{{ run_id }}",
        content_type="text/plain",
    )

    @task
    def write_csv(ds: str | None = None) -> str:
        path = ObjectStoragePath(BASE) / str(ds) / "orders.csv"
        buffer = io.StringIO()
        writer = csv.writer(buffer)
        writer.writerow(["id", "customer", "amount"])
        writer.writerows([[1, "acme", 42.5], [2, "globex", 7.0]])
        path.write_text(buffer.getvalue())
        return str(path)

    wait = LoamsObjectKeySensor(
        task_id="wait_for_csv",
        bucket="lake",
        keys="airflow-example/{{ ds }}/*.csv",
        wildcard=True,
        poke_interval=10,
        timeout=600,
    )

    archive = LoamsObjectCopyOperator(
        task_id="archive",
        bucket="lake",
        source_key="airflow-example/{{ ds }}/orders.csv",
        dest_key="archive/{{ ds }}/orders.csv",
    )

    marker >> write_csv() >> wait >> archive
