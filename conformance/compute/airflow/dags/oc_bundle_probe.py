"""OC0's GitDagBundle case: the harness pushes a second commit that changes
VERSION while a run of the first is in progress. The running task must still
run the first commit's code, and the run must record that commit."""

from __future__ import annotations

import time

from airflow.sdk import Param, dag, task

VERSION = "v1"


@dag(
    dag_id="oc_bundle_probe",
    schedule=None,
    catchup=False,
    params={"sleep": Param(60, type="number")},
    tags=["conformance"],
)
def oc_bundle_probe():
    @task
    def wait(**context) -> None:
        time.sleep(float(context["params"]["sleep"]))

    @task
    def which() -> str:
        print(f"OC-BUNDLE-VERSION {VERSION}")
        return VERSION

    wait() >> which()


oc_bundle_probe()
