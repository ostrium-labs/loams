"""Effects the conformance DAGs leave behind, so the harness judges what ran,
not only what Airflow recorded: one object per execution of a task, in the
test bucket. A task that ran twice leaves two objects, whatever the metadata
database says. Not a DAG: the DAG processor parses it and finds none. The
DAG files import it from the bundle's root, which Airflow puts on sys.path
both when it parses a bundle and when a task runs. The copy under
edge/dags/ must stay identical (selftest.sh: airflow_effects_copies_match).
"""

from __future__ import annotations

import json
import socket
import time
import uuid

EFFECTS_BUCKET = "airflow-effects"


def record(context, **extra) -> None:
    from airflow.providers.amazon.aws.hooks.s3 import S3Hook

    ti = context["ti"]
    key = f"{ti.dag_id}/{ti.run_id}/{ti.task_id}/{ti.map_index}/{uuid.uuid4().hex}.json"
    body = {
        "try_number": ti.try_number,
        "host": socket.gethostname(),
        "at": time.time(),
        **extra,
    }
    S3Hook(aws_conn_id="loams_s3").load_string(json.dumps(body), key, bucket_name=EFFECTS_BUCKET)
