![Loams — Your data. Your bucket.](https://raw.githubusercontent.com/ostrium-labs/loams/dev/docs/assets/loams-banner.svg)

# Loams SDK for Python

> **0.1.2.** `loams` 0.1.2 tracks Loams 0.1.2. In 0.x the API may change between
> minor versions: pin `loams~=0.1.2` to stay on the 0.1 line.

One `Loams` object over one Loams instance, speaking the unified Connect API.
The module surface is generated from `proto/` by `scripts/sdk/gen.sh python`;
the runtime below it is hand-written and obeys the contract in
[`docs/sdk/runtime-contract.md`](https://github.com/ostrium-labs/loams/blob/dev/docs/sdk/runtime-contract.md).

## Install

```
pip install loams
```

From a checkout instead:

```
uv sync --all-extras --dev     # from sdks/python
```

The runtime dependency is [`connectrpc`](https://pypi.org/project/connectrpc/)
0.12.1 (connectrpc/connect-py), pinned in `scripts/sdk/pins.lock`. Up to 0.9.0
the same project was published as `connect-python`; the import name has always
been `connectrpc`.

## Use

```python
from loams import Loams

loams = Loams("https://acme.loams.dev", api_key="...")
print(loams.instance.version())
```

Four modules hang off the client: `loams.instance` (what this instance is),
`loams.live` (the live-sync session half, whose package is `unstable`),
`loams.tables` (the table half of the same service), and `loams.system` (the
module catalogue, feature detection and the version check).

### Auth

Pass `api_key` for a script or CI job that holds one — it does not expire, so
there is nothing to refresh. Pass `auth` for everything else: `env_token()`,
`oidc_exchange()`, or your own `TokenSource`. The two are mutually exclusive
because they answer the same question, and passing both raises.

### Protocol and encoding

`protocol` is `"connect"` (default), `"grpc"` or `"grpc-web"` — the one port
speaks all three; this picks which one this client uses. `proto_json=True`
sends the proto3 JSON mapping that `curl` sends; the default is protobuf,
which is smaller.

### Consistency

`session_consistency=False` by default: every read is then `STRONG` on its own,
which is correct but does not give read-your-writes across processes. With it on
(D609, R4), every consistency token a response carries — the
`consistency_token`, `token` and `read_token` fields and the
`loams-consistency-token` header — merges into `loams.consistency` (the max
offset per stream and partition), and every later read whose request has a
`consistency` field is sent `at_least` the session. `ConsistencyToken.parse`,
`merge` and `str` work on tokens directly.

## Build an app

The app-building layer (SDKP): the modules `loams.namespaces`, `collections`,
`documents`, `search`, `vector` and `graph` on both `Loams` and `AsyncLoams`,
generated from the protos' facade annotations with the hand-written
conveniences below attached; builders; a collection handle; and the runtime an
app needs. `loams.sql` is generated from `loams.sql.v1` the same way:
`sql.query` answers `columns` and `rows`, `sql.records(statement, [$1, …])`
answers one dict per row, and `loams.sql_records(answer)` reads an answer in
hand; `sql.query_arrow` answers Arrow IPC bytes (`arrow_ipc`), which
`pyarrow.ipc.open_stream` reads (the `arrow` extra). `loams.streams` is
generated from `loams.stream.v1`: `create`, `get`, `list` (and `list_all`),
`produce` (pass `producer_id` and `producer_seq` so a retry cannot append
twice), `fetch`, `produce_events` and `fetch_events` (CloudEvents as a JSON
batch), plus `streams.subscribe(...)` (R14), an iterator over `fetch` whose
`offset` is where to resume. `examples/notes/main.py`
is the whole app scenario as a program, type-checked in CI. The session's
token is used only for collection and SQL reads: a graph's token names one
graph (SDKP-17).

```python
from loams import Filter, Loams, Schema, Search, Sort, rrf

loams = Loams.from_env()   # LOAMS_ENDPOINT, LOAMS_API_KEY or LOAMS_TOKEN, LOAMS_NAMESPACE
# or: Loams(endpoint, api_key=key, namespace="app", session_consistency=True)

schema = Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4)

notes = loams.collection("notes")          # calls with `collection` filled in
notes.upsert([
    {"id": 1, "source": {"title": "Refunds", "body": "refund policy", "tags": "policy", "n": 1},
     "vectors": {"embedding": [1, 0, 0, 0]}},
])

hits = notes.search(
    notes.query()
    .text("refund", fields=["body"])
    .vector("embedding", [1, 0, 0, 0], k=10)
    .fuse(rrf(60))
    .filter(Filter.and_(Filter.eq("tags", "policy"), Filter.range("n", gte=1)))
    .highlight("body")
    .select(include=["title"])
    .limit(5)
)

for hit in loams.search.query_all(
    Search("notes").text("refund").sort(Sort.score(), Sort.pk()).limit(100)
):
    ...                                     # every hit, page by page

notes.write_batch(rows)                     # 10 000 rows / 4 MiB a chunk, one key per chunk
```

`AsyncLoams` has the same surface, awaited: `await notes.upsert(...)`,
`async for hit in loams.search.query_all(...)`,
`async for record in loams.streams.subscribe("events")`.

- **Default namespace (R11).** `namespace=` fills every app request
  (`loams.collection.v1`, `graph`, `sql`, `stream`) that leaves `namespace`
  empty; an explicit one wins, and neither raises `InvalidArgumentError` before
  any network call.
- **Deadlines (R12).** Unary calls default to 30 s (`timeout_ms=` on the client
  or `CallOptions(timeout_ms=...)` per call; `0` disables). One budget covers a
  call's retries; running out raises `DeadlineExceededError` with reason
  `client_deadline_exceeded`, which is never retried. Streams have no default.
- **Bulk writes (R13).** `documents.write_batch` sends one chunk at a time, each
  with its own idempotency key, returns a `BatchResult` with the merged token,
  and stops at the first failed chunk with a `BatchError` (`chunks_applied`,
  `rows_applied`, `token`).
- **Subscriptions (R14).** `streams.subscribe("events", partition=0,
  from_offset=1)` is an iterator over `Fetch` that exposes `offset` for
  resuming and `cancel()`.

Durable workflows use the Resonate SDK pointed at `LOAMS_ENDPOINT/durable/`
(SDKP-1).

## Spark (preview)

`loams.spark` opens Spark Connect sessions and runs Spark batch jobs (design 81,
SP-b). It is **preview**: the job API and its JSON may change.

```
pip install 'loams[spark]'      # pyspark-client, for sessions; jobs need no extra
```

```python
from loams import spark as loams_spark

# A session on the namespace's Spark Connect endpoint: LOAMS_SPARK_REMOTE, else
# asked of LOAMS_ENDPOINT. The token goes as gRPC metadata, never in the sc:// URI.
spark = loams_spark.session(app_name="etl").getOrCreate()
spark.sql(f"SELECT count(*) FROM {loams_spark.house_table(42, 'sales', 'orders')}").show()

# A batch job: exactly one of pyspark=, jar= (with main_class=) or sql=.
run = loams_spark.submit_job("etl", pyspark="s3://bucket/jobs/etl.py", args=["--day", "2026-10-27"],
                             max_attempts=3)
final = run.wait()                      # polls until succeeded, failed or canceled
print(final.state, run.logs())

loams_spark.schedule("nightly", "0 2 * * *", loams_spark.job_spec("etl", sql="s3://bucket/report.sql"))
```

- The `loams` Iceberg catalog (Lakekeeper REST, vended credentials) is set from
  `CatalogOptions` or `LOAMS_ICEBERG_URI` / `LOAMS_ICEBERG_WAREHOUSE`;
  `catalog_conf()` returns the same conf for a JVM `spark-submit`. Tables in
  `loams.ns_<id>.<db>` are the House's database `<db>` with no registration.
- `python -m loams.spark.driver --app <file-or-uri> [--sql] -- args...` is the
  entrypoint the Loams driver container runs for a Spark Connect job.

## What is not here yet

- **Bulk ingest and bulk results over Arrow Flight SQL.** They go through ADBC,
  which is why the `flight`, `arrow` and `polars` extras already exist.

## Development

```
uv sync --all-extras --dev
uv run pytest -q
uv run mypy --strict src
./scripts/sdk/gen.sh python    # regenerate the facade from proto/
```

Generated code is never hand-edited: CI regenerates and fails on a diff, so a
proto change shows up as a dependency bump rather than a silent drift.
