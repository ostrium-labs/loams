"""OC0's HA case (design 68 §4.3): `n` mapped task instances (500 by default)
under two schedulers; each records one effect. The harness checks that every
task instance has exactly one try and exactly one effect."""

from __future__ import annotations

import time

from _oc_effects import record
from airflow.sdk import Param, dag, task


@dag(
    dag_id="oc_ha_mapped",
    schedule=None,
    catchup=False,
    max_active_tasks=64,
    params={"n": Param(500, type="integer"), "sleep": Param(0, type="number")},
    tags=["conformance"],
)
def oc_ha_mapped():
    @task
    def items(**context) -> list[int]:
        return list(range(int(context["params"]["n"])))

    @task
    def work(i: int, **context) -> None:
        time.sleep(float(context["params"]["sleep"]))
        record(context, i=i)

    work.expand(i=items())


oc_ha_mapped()
