# Upgrade fixtures

The upgrade gate's fixture data (design §54 §14, UP1 Task 19). The job
`Upgrade: previous release → candidate` writes these on the previous
release, upgrades, and reads them back. It reads them again after the
rollback.

```
run.py write     --out state.json      # on the previous release
run.py check     --state state.json    # after the upgrade
run.py write-new --state state.json    # one new item per product
run.py check     --state state.json --expect-new-writes present   # after the upgrade, or rollback --keep-writes
run.py check     --state state.json --expect-new-writes absent    # after a rollback from the snapshot
```

The gate runs `run.py` in a `python:3.12-slim` container that shares the `loams` container's network
namespace (`--network container:<loams>`). From there, every engine listener is reachable at its own loopback
default:
- the engine on `:8080`;
- Qdrant on `:6333`;
- Elasticsearch on `:9200`;
- Durable on `:8001`.

TiDB is `tidb:4000` on the compose network. The stack runs with `LOAMS_AUTH_MODE=apikey`, and `--key` (or
`LOAMS_API_KEY`) is a key from its `secrets/api-keys`.

| Product | Fixture | Notes |
|---|---|---|
| `collections` | two documents through the native REST API; a strong get and a text search | |
| `streams_links` | two records on a stream; a counter link over it | |
| `qdrant` | two points through the Qdrant REST API | skipped without the Qdrant listener |
| `es` | two documents through the Elasticsearch REST API | skipped without the ES listener |
| `live` | Live is served and answers the same way | no deployed app yet; Live's stores are read by `loams migrate verify` |
| `graph` | an owned graph `(:Person)-[:KNOWS]->(:Person)` | skipped without `loams.graph.v1` |
| `sql` | a TiDB database, a table, two rows | skipped without TiDB |
| `postgres` | a table and rows on a compute (`--pg DSN`) | skipped without a compute |
| `house` | DDL (a table) until the House announces `house.insert`; rows after | skipped without `loams.house.v1` |
| `durable` | a pending and a resolved promise | skipped without the Durable listener |
| `keys` | a call with the key is let in; one without is refused | skipped without `--key` |

A product the previous release lacks is written as `skipped` with the reason, and is not checked.

`run.sh` runs the fixtures' own tests (pytest, through `uv`) against an in-memory fake engine.
`LOAMS_UPGRADE_ENGINE` also runs them against a real stack.
