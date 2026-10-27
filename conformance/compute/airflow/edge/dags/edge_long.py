"""The Edge maintenance case: a long task that must finish on its worker after
that worker enters maintenance mode, recording an effect when it does."""

from __future__ import annotations

import time

from _oc_effects import record
from airflow.sdk import Param, dag, task


@dag(
    dag_id="edge_long",
    schedule=None,
    catchup=False,
    params={"sleep": Param(60, type="number")},
    tags=["conformance", "edge"],
)
def edge_long():
    @task
    def long(**context) -> None:
        record(context, phase="start")
        time.sleep(float(context["params"]["sleep"]))
        record(context, phase="done")

    long()


edge_long()
