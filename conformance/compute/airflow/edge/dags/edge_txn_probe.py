"""The transaction-mode leg: a deferrable sensor (the triggerer's work) and
mapped task instances (the schedulers'), run while the schedulers and the
triggerer reach Loams Postgres through PgDog in transaction pooling."""

from __future__ import annotations

from datetime import timedelta

from _oc_effects import record
from airflow.providers.standard.sensors.time_delta import TimeDeltaSensor
from airflow.sdk import Param, dag, task


@dag(
    dag_id="edge_txn_probe",
    schedule=None,
    catchup=False,
    params={"n": Param(30, type="integer")},
    tags=["conformance", "edge"],
)
def edge_txn_probe():
    wait = TimeDeltaSensor(task_id="deferred_wait", delta=timedelta(seconds=5), deferrable=True)

    @task
    def items(**context) -> list[int]:
        return list(range(int(context["params"]["n"])))

    @task
    def work(i: int, **context) -> None:
        record(context, i=i)

    wait >> work.expand(i=items())


edge_txn_probe()
