"""OC0's remote-log case: one task prints a marker unique to its run; the
harness reads it back through /api/v2 after the worker that ran it is gone."""

from __future__ import annotations

from airflow.sdk import dag, task


@dag(dag_id="oc_log_probe", schedule=None, catchup=False, tags=["conformance"])
def oc_log_probe():
    @task
    def say(**context) -> None:
        print(f"OC-LOG-MARKER {context['run_id']}")

    say()


oc_log_probe()
