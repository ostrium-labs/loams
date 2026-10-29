# Vendored: Spark Connect protos, `apache/spark` v4.1.3

The protos under `proto/spark/connect/` are copied **unchanged** from
[`apache/spark`](https://github.com/apache/spark)
`sql/connect/common/src/main/protobuf/spark/connect/` (Apache-2.0; the full
text, with Spark's third-party notices, is `LICENSE-apache-spark`).

| | |
|---|---|
| Tag | `v4.1.3` (design 81 R15 / SP-a D1990: the Apache Spark backend's version; Iceberg 1.12.0 ships `iceberg-spark-runtime-4.1_2.13`, none for 4.2) |
| Commit | `77bbf77e86ad48f58b5dfbc6ac882b3e70cf1989` (2026-07-11) |
| Source | `https://raw.githubusercontent.com/apache/spark/77bbf77e86ad48f58b5dfbc6ac882b3e70cf1989/sql/connect/common/src/main/protobuf/spark/connect/<file>` |
| Taken | every `.proto` in that directory except `example_plugins.proto` (a test plugin no client sends). The only imports outside `spark/connect/` are protobuf's well-known types, which `protoc` provides |

The proxy forwards Spark Connect messages as bytes (D1994) and reads only
`session_id` (field 1) and `user_context` (field 2) of each request, so a
client newer than these protos (PySpark 4.2 against Sail) passes through
unchanged. The generated types serve the tests' fake backend and the
proxy's own `ReleaseSession` calls.

## Updating

1. Pick the Spark tag of the Apache Spark backend's image, at least 14 days
   old (D1274).
2. Download each file from the tag's commit and replace `proto/spark/connect/`.
   Change nothing inside the files.
3. Regenerate the list below with
   `(cd proto && find spark -type f | LC_ALL=C sort | xargs sha256sum)` and
   update this table, `SPARK_TAG` and `SPARK_COMMIT` in `src/proto.rs`, and the
   method list in `tests/proto.rs`.
4. Check that every request message still has `session_id = 1` and
   `user_context = 2` (`tests/proto.rs` does this from the descriptor set).

## Files

```
52f9fdcbbca7a71781de806d3853c6693e7311d4b9d0c9254feda24bb205e277  spark/connect/base.proto
f4e4211e4b1b0905c7b1e1b05bb008e2c0354b1dec971188cb2d76ab654699da  spark/connect/catalog.proto
1e87ce322d22957bd8e621b24fe36accccbee686324d6845094255a280454295  spark/connect/commands.proto
1c111676e247370259273863e2b37247b913f94221c6f1f4a609fe037169e783  spark/connect/common.proto
03a0995ca94aa8c87af64462e6926eade335732e6edf95e2a0625496c39d1eb5  spark/connect/expressions.proto
7d61c9098d25dd8b1c3484222d8298e6891a02366c59f454e24dd90993422a13  spark/connect/ml.proto
237628488246b193c2711d05b2657d640b5e044c4bcffd786dcd2ae56224e8ba  spark/connect/ml_common.proto
58b2e2e015aeac43f762309a0563ee07baa9617940ecfd88eb00948d7a6b69ab  spark/connect/pipelines.proto
84454ec683fb5ab786dd945e85cab2e3240f3b89418461d183ec3605c0407ef7  spark/connect/relations.proto
192e4c1af57e81247e946813f37c886af0ed201d9305e2350e9a228030ea0b8b  spark/connect/types.proto
```
