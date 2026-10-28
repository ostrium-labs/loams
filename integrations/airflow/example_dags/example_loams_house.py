# SPDX-License-Identifier: Apache-2.0
"""Loams House: a ClickHouse rollup, run deferred, then a check on it."""

from __future__ import annotations

import pendulum
from airflow.sdk import DAG, Asset

from airflow.providers.loams.assets.loams import loams_uri
from airflow.providers.loams.operators.house import LoamsHouseQueryOperator

DAILY = Asset(loams_uri("default", "house", "analytics.daily_events"))

with DAG(
    dag_id="example_loams_house",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule="@daily",
    catchup=False,
    tags=["example", "loams"],
) as dag:
    rollup = LoamsHouseQueryOperator(
        task_id="rollup",
        database="analytics",
        sql=[
            "CREATE TABLE IF NOT EXISTS daily_events (day Date, kind String, n UInt64) "
            "ENGINE = MergeTree ORDER BY (day, kind)",
            "INSERT INTO daily_events SELECT toDate(ts), kind, count() FROM events "
            "WHERE toDate(ts) = {day:Date} GROUP BY toDate(ts), kind",
        ],
        params_={"day": "{{ ds }}"},
        read_only=False,
        deferrable=True,
        outlets=[DAILY],
    )

    check = LoamsHouseQueryOperator(
        task_id="check",
        database="analytics",
        sql="SELECT kind, n FROM daily_events WHERE day = {day:Date} ORDER BY n DESC LIMIT 10",
        params_={"day": "{{ ds }}"},
        max_rows=10,
    )

    rollup >> check
