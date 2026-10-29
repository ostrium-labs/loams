# Spark Connect parity (skeleton)

Design 81 SP-b, D2009. **Status: a skeleton.** The harness is written and has
not run yet, in CI or locally. Its first run belongs to the SP1 task that
brings up a Spark Connect endpoint in compose (SP-a's proxy in front of Sail).

What it runs, against a Loams Spark Connect endpoint (`LOAMS_SPARK_REMOTE`):

| Part | What | Judged by |
|---|---|---|
| `parity` | the PySpark Spark Connect parity modules in `modules.txt`, from the Apache Spark source at `SPARK_TAG`, with `pyspark-client==PYSPARK_CLIENT` | their own assertions |
| `loams` | `loams_iceberg_test.py`: a table written through the `loams` Iceberg catalog (Lakekeeper, vended credentials) reads back from the House's namespace `ns_<id>.<db>` | its assertions |

Results go to `$OC_OUT/spark.junit.xml` and through the strict expected-failure
gate (`xfail.toml`): an unlisted failure, an unexpected pass, or a stale entry
fails the run. `xfail.toml` is empty until the first run is triaged; Sail's own
CI suggests most failures will be in `test_parity_udf` and the catalog suite
(design 26 §9.1 reports 90.7 % on the Spark 3.5 Connect tests and 71.3 % on
Spark 4.2).

Before it runs in CI:

- move `versions.env` into `../pins.env`, with entries in `../licences.toml`
  (PySpark and Spark are Apache-2.0);
- decide how PySpark's own suites authenticate (they cannot add Loams' gRPC
  header): a loopback-only proxy port or a test token the proxy maps (SP-a);
- add the `spark` profile to `deploy/release/compose.compute.yaml` (SP-a/ENG81).
