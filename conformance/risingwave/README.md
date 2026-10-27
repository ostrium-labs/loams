# RisingWave conformance: the managed instance against RisingWave's own tests

Design 81 (managed stream and batch engines), plan RW1, lane RW-b.

Status: **the harness only, not yet run.** It needs a running managed RisingWave
instance (RW-a's `risingwave` compose profile), which no lane has started yet.

A managed RisingWave is judged by RisingWave's own end-to-end sqllogictest files,
run unmodified against the instance through `engine-gateway`, plus a few cases of
Loams' own. Every result is checked against `xfail.toml` strictly, as in
`conformance/compute/`: an unlisted failure, an unexpected pass or a stale entry
fails the run.

## Layout

| Path | What |
|---|---|
| `pins.env` | the RisingWave tag, the SHA-256 of its source tarball (recorded on the first run) and the `sqllogictest-bin` version |
| `subset.txt` | globs under the tag's `e2e_test/`: suites that need only the frontend (no Kafka, no external connectors, no Premium features) |
| `loams/*.slt` | Loams' cases: no licence key (`rw_test_paid_tier()` must fail, design 81 R4), the `rw_catalog` reads `loams risingwave` relies on, a materialized view shaped as the builders render it |
| `xfail.toml` | expected failures, format `conformance/compute/xfail.schema.json` |
| `run.sh` | installs the runner, runs both sets, writes JUnit, runs `scripts/compute/xfail_check.py` |

The upstream files are read from the source tarball of the pinned tag
(`codeload.github.com`, digest-checked), extracted to the work directory. Nothing
is vendored and nothing is cloned. RisingWave is Apache-2.0.

## Running

```sh
RW_PASSWORD=<Loams API key> conformance/risingwave/run.sh                 # both sets
RW_PASSWORD=<Loams API key> conformance/risingwave/run.sh --loams-only    # Loams' cases only
```

The work directory defaults to `target/conformance-risingwave` (never `/tmp`).
The first run stops after downloading the tarball because `pins.env` has no digest
yet; rerun with `RW_CONFORMANCE_RECORD_PIN=1`, then commit the digest it prints.

## Not covered yet

- Connector suites (`e2e_test/source_inline`, `e2e_test/sink`): they need Kafka,
  Postgres and Iceberg services wired the way RisingWave's CI wires them. The Loams
  equivalents (Iggy Kafka gateway, Loams Postgres CDC, Lakekeeper) come as a second
  subset once the managed profile runs.
- The `subset.txt` paths are (verify) against the pinned tag.
