# SPDX-License-Identifier: Apache-2.0
"""Loams Graph: write nodes with GQL, then query them with parameters."""

from __future__ import annotations

import pendulum
from airflow.sdk import DAG

from airflow.providers.loams.operators.graph import LoamsGraphQueryOperator

with DAG(
    dag_id="example_loams_graph",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=None,
    catchup=False,
    tags=["example", "loams"],
) as dag:
    write = LoamsGraphQueryOperator(
        task_id="write",
        graph="social",
        statement=("INSERT (:Person {name: $a})-[:KNOWS {since: $since}]->(:Person {name: $b})"),
        parameters={"a": "Ada", "b": "Grace", "since": 2026},
    )

    read = LoamsGraphQueryOperator(
        task_id="read",
        graph="social",
        statement="MATCH (p:Person)-[k:KNOWS]->(q:Person) WHERE p.name = $name "
        "RETURN q.name AS friend, k.since AS since",
        parameters={"name": "Ada"},
        read_only=True,
        max_rows=100,
    )

    write >> read
