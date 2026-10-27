"""OC0's XCom case: an XCom far above the object-storage threshold goes to the
bucket, and the next task reads it back whole."""

from __future__ import annotations

import hashlib

from airflow.sdk import dag, task

SIZE = 200_000


@dag(dag_id="oc_xcom_big", schedule=None, catchup=False, tags=["conformance"])
def oc_xcom_big():
    @task
    def produce() -> dict:
        blob = "".join(chr(97 + (i * 7) % 26) for i in range(SIZE))
        return {"blob": blob, "sha256": hashlib.sha256(blob.encode()).hexdigest()}

    @task
    def consume(value: dict) -> str:
        got = hashlib.sha256(value["blob"].encode()).hexdigest()
        if len(value["blob"]) != SIZE or got != value["sha256"]:
            raise ValueError(f"XCom came back changed: {len(value['blob'])} bytes, sha256 {got}")
        print(f"OC-XCOM-OK {got}")
        return got

    consume(produce())


oc_xcom_big()
