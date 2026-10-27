# SPDX-License-Identifier: Apache-2.0
"""Loams streams through the Kafka gateway: produce, wait, consume, and an asset watcher.

Two DAGs:

- `example_loams_streams` produces a few records, waits (deferred) until its
  consumer group has records to read, and reads them;
- `example_loams_stream_watcher` is scheduled on an asset fed by
  `LoamsStreamMessageTrigger`: each matching record on the `table-commits`
  topic starts a run.
"""

from __future__ import annotations

import pendulum
from airflow.sdk import DAG, Asset, AssetWatcher, task

from airflow.providers.loams.assets.loams import loams_uri
from airflow.providers.loams.operators.stream import (
    LoamsStreamConsumeOperator,
    LoamsStreamProduceOperator,
)
from airflow.providers.loams.sensors.stream import LoamsStreamOffsetSensor
from airflow.providers.loams.triggers.stream import LoamsStreamMessageTrigger

with DAG(
    dag_id="example_loams_streams",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=None,
    catchup=False,
    tags=["example", "loams"],
) as dag:
    produce = LoamsStreamProduceOperator(
        task_id="produce",
        topic="orders",
        records=[
            {"key": "acme", "value": {"id": 1, "amount": 42.5, "day": "{{ ds }}"}},
            {"key": "globex", "value": {"id": 2, "amount": 7.0, "day": "{{ ds }}"}},
        ],
    )

    wait = LoamsStreamOffsetSensor(
        task_id="wait_for_orders",
        topic="orders",
        mode_="new_records",
        group_id="airflow-example-orders",
        deferrable=True,
        poke_interval=15,
        timeout=3600,
    )

    consume = LoamsStreamConsumeOperator(
        task_id="consume",
        topics="orders",
        group_id="airflow-example-orders",
        decode="json",
        max_messages=500,
        apply_function=lambda records: {"orders": len(records)},
    )

    produce >> wait >> consume


TABLE_COMMITS = Asset(
    loams_uri("default", "streams", "table-commits"),
    watchers=[
        AssetWatcher(
            name="loams_table_commits",
            trigger=LoamsStreamMessageTrigger(
                topics="table-commits",
                group_id="airflow-example-watcher",
                match={"type": "io.loams.dev.table.commit.v1"},
            ),
        )
    ],
)

with DAG(
    dag_id="example_loams_stream_watcher",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=[TABLE_COMMITS],
    catchup=False,
    tags=["example", "loams"],
) as watcher_dag:

    @task
    def on_commit(**context) -> None:
        for event in context["triggering_asset_events"][TABLE_COMMITS]:
            print("commit:", event.extra)

    on_commit()
