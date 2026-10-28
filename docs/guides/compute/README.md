# Open-protocol compute on Loams

Status: **preview**. Design [§68](../../design/68-open-protocol-compute.md);
plan [OC1](../../plans/2026-10-20-oc1-open-protocol-compute.md) (OC0).

Loams runs other people's engines **unmodified**, exactly as released, on its
own primitives: Loams Postgres for metadata and state, and the bucket (RustFS
or any S3) for logs and large values. Each product is judged by its own test
suite plus the few cases that Loams' primitives must survive, strictly against
a list of expected failures. The harness is `conformance/compute/`, and the
latest results are in the [compatibility matrix](compatibility.md).

| Recipe | Status |
|---|---|
| [Airflow 3.3](airflow.md) | preview, written but not yet run end to end |
| [Durable execution](../durable/README.md): Temporal compat mode, DBOS, Airflow with the Edge executor | see that page |

The RisingWave, Sail and Postgres-queue recipes (OC0 Tasks 3–5, 8–10) have not
been written yet.
