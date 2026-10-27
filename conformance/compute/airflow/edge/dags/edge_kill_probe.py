"""The Edge worker-kill case: the first try records that it started and then
waits for the harness to `kill -9` its worker; the retry, on the other
worker, records and finishes. One retry is allowed, so the run succeeds with
exactly two tries."""

from __future__ import annotations

import time
from datetime import timedelta

from _oc_effects import record
from airflow.sdk import dag, task


@dag(dag_id="edge_kill_probe", schedule=None, catchup=False, tags=["conformance", "edge"])
def edge_kill_probe():
    @task(retries=1, retry_delay=timedelta(seconds=5))
    def victim(**context) -> None:
        record(context, phase="start")
        if context["ti"].try_number == 1:
            time.sleep(900)
        record(context, phase="done")

    victim()


edge_kill_probe()
