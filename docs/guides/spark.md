# Spark on Loams: PySpark over Spark Connect, Iceberg, then the House

> **Preview.** Managed Spark is design 81 (plan SP1) and is being built now.
> The SDK helpers, the `loams spark` CLI and the job renderer described here
> are in the tree, but nothing in this guide has been run end to end yet.
> The job status, logs and schedule routes are still proposals to the engines
> facade. Expect names to change.

This walkthrough writes a table from PySpark through Spark Connect into an
Iceberg table in the namespace's Lakekeeper catalog, then queries it in the
House. It then runs the same work as a batch job, on a schedule.

## What runs where

```text
your PySpark ──sc://… + Bearer token──▶ loams-gateway (spark route) ──▶ Sail (Spark Connect server, per engine instance)
                                                                          │ Iceberg REST + vended credentials
                                                                          ▼
                                                              Lakekeeper ── bucket (RustFS / S3)
                                                                          ▲
                                         House (ClickHouse SQL) ──────────┘ reads ns_<id>.<db> as database <db>
```

- Loams runs no Spark engine of its own. A Spark *engine instance* belongs to
  one namespace and runs Sail (the default) or Apache Spark 4.1's Connect
  server in its own processes (design 26 §9, design 81). Sail runs tenants'
  Python, so instances are never shared.
- The token never goes in the `sc://` URL. The SDK sends it as gRPC
  metadata (`authorization: Bearer …`).
- Tables go in the Iceberg namespace `ns_<id>.<database>`. The House reads
  every table there as `<database>.<table>`; a table Spark wrote shows up as an
  *external* table, readable with nothing to register (design 49 §5.2–§5.3).

## 1. Install the SDK with the Spark extra

```sh
pip install 'loams[spark]'        # pyspark-client: the Connect-only PySpark, no JVM
export LOAMS_ENDPOINT=https://…   # or http://127.0.0.1:8080 on compose
export LOAMS_TOKEN=…              # an API key or an access token
export LOAMS_NAMESPACE=acme       # the namespace
export LOAMS_SPARK_INSTANCE=etl   # the Spark engine instance in it (name or eng_… id)
export LOAMS_ICEBERG_URI=http://lakekeeper:8181/catalog
export LOAMS_ICEBERG_WAREHOUSE=loams
```

`loams spark session-url` opens a session on the instance and prints its
endpoint: `sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true`
in the cloud, `sc://127.0.0.1:15002` on compose. Set it as
`LOAMS_SPARK_REMOTE` to skip asking the server each time.

## 2. Write an Iceberg table from PySpark

```python
from loams import spark as loams_spark

spark = loams_spark.session(app_name="events-demo").getOrCreate()

NS_ID = 42                                   # your namespace id
events = loams_spark.house_table(NS_ID, "analytics", "events")
# -> "loams.ns_42.analytics.events"

spark.sql(f"CREATE NAMESPACE IF NOT EXISTS loams.ns_{NS_ID}.analytics")
(
    spark.range(1_000_000)
    .selectExpr("id", "id % 7 AS day", "rand() AS score")
    .writeTo(events)
    .using("iceberg")
    .createOrReplace()
)
spark.table(events).groupBy("day").count().show()
```

`session()` returns a `SparkSession.builder` with:

- the remote: `LOAMS_SPARK_REMOTE`, or a new session's `connect_url`;
- the token as gRPC metadata. A `token=` in the URL is refused;
- a catalog named `loams`: Iceberg REST at Lakekeeper with
  `X-Iceberg-Access-Delegation: vended-credentials`, so table data is read
  and written with short-lived credentials scoped to the table. It is also the
  default catalog.

## 3. Query it in the House

```sql
SELECT day, count() AS n, avg(score)
FROM analytics.events
GROUP BY day
ORDER BY day;
```

The House pins the table's current snapshot for each query, so a Spark write
that commits mid-query does not change the result. Spark may write only tables
it owns. The keyed tables Loams' links maintain have one writer, and Spark is
not it (design 17 §6.1).

To check the catalog side from a shell:

```sh
loams spark catalog-check --namespace-id 42 --database analytics --table events
# spark name:   loams.ns_42.analytics.events
# house name:   `analytics`.`events`
# house check:  SELECT count() FROM `analytics`.`events`
# vended:       an S3 session (scoped to s3://…/ns/42/…)
```

## 4. Run it as a batch job

Save the script as `events.py`, upload it to the bucket, and submit it:

```sh
loams spark submit events --py s3://jobs/events.py --max-attempts 3 -- 2026-10-27
loams spark status <run>
loams spark logs <run>              # the last attempt; --attempt n for another
loams spark cancel <run>
```

Or from Python:

```python
run = loams_spark.submit_job("events", pyspark="s3://jobs/events.py",
                             args=["2026-10-27"], max_attempts=3)
print(run.wait().state)
```

Jobs go to `POST /v1/namespaces/{ns}/engines/{instance}/jobs`. A job is one of:

| Kind | Flag | Default backend |
|---|---|---|
| PySpark script | `--py file [--py-file extra]…` | `connect`: a short-lived driver against the namespace's Spark Connect server |
| SQL file (`;`-separated) | `--sql file` | `connect` |
| JAR | `--jar file --class Main [--with-jar extra]…` | `kubernetes`: Apache Spark 4.1 on Kubernetes |

with `--conf key=value`, driver and executor sizes, `--max-attempts` and
`--backoff-secs` (exponential, capped at 10 minutes), and `--timeout-secs`.
Loams sets `spark.master`, the Kubernetes keys and the `loams` catalog itself,
and refuses them in `--conf`.

On the `kubernetes` backend each attempt is one driver pod running
`spark-submit` in client mode. The driver creates its executors, and they are
deleted with it. Use `loams spark render` to see the pod:

```sh
loams spark render rollup --jar s3://jobs/rollup.jar --class dev.acme.Rollup \
  --executors 4 --executor-memory 8g --namespace-id 42 | kubectl apply --dry-run=client -f -
```

The token and the S3 keys come from Kubernetes Secrets, so the rendered pod
holds only references to them. They are never on a command line either: the
catalog token, which Spark only takes as conf, is written at start into a
private properties file that `spark-submit --properties-file` reads. Spark
event logs go to the instance's `…/spark-events/` prefix.

### Locally (compose, a laptop)

```sh
export LOAMS_SPARK_REMOTE=sc://127.0.0.1:15002     # compose's Spark Connect proxy
loams spark submit daily --sql jobs/daily.sql --local
loams spark list --local
loams spark logs <run> --local
```

`--local` runs each attempt as a child process: the Loams driver
(`python -m loams.spark.driver`) for PySpark and SQL when a Connect endpoint
is set, or `$SPARK_HOME/bin/spark-submit --master local[*]` otherwise (always
for a JAR). Statuses and logs are kept under `.loams/spark/runs/<run>/`.

`loams spark` is behind the `spark` cargo feature of the `loams` binary while
it is in preview.

## 5. Schedule it

```sh
loams spark schedule nightly-events --cron "0 2 * * *" --py s3://jobs/events.py
loams spark unschedule nightly-events
```

A schedule is a Resonate schedule on Loams' embedded durable server (design 26
§8.1). Each tick creates one promise, `spark-run-<schedule>-<timestamp>`,
exactly once, and that promise starts exactly one run. Cron rules are five
fields, in UTC.

## Limits in this preview

- Sail does not serve RDDs, `SparkContext`, JVM UDFs, MLlib or
  pandas-on-Spark (design 26 §9.1). Use `--backend kubernetes` for those.
- Sail writes Iceberg copy-on-write only. Tables with deletion vectors are
  read-only to it until the M4 work lands.
- The Spark Connect parity harness (`conformance/compute/spark/`) is written
  but has not run. Expect coverage gaps until its first triaged run.
