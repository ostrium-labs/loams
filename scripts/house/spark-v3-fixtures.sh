#!/usr/bin/env bash
# Writes the Spark-written Iceberg v3 tables `spark_v3_tables_read_as_spark`
# reads (plan HE1 Task 26, R26.10): unmodified Spark 4.0.4 with Iceberg
# 1.12.0 (Apache-2.0), a Hadoop catalog on a local directory, running
# fabric/crates/loams-house-iceberg/tests/fixtures/spark-v3/write.sql. Then
# runs the test against them.
#
#   scripts/house/spark-v3-fixtures.sh [<warehouse dir>]
#
# The warehouse defaults to fabric/crates/loams-house-iceberg/tests/fixtures/
# spark-v3/warehouse. Needs podman (or docker) and network access for the
# Iceberg runtime jar.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixtures="$root/fabric/crates/loams-house-iceberg/tests/fixtures/spark-v3"
warehouse="${1:-$fixtures/warehouse}"
rm -rf "$warehouse"
mkdir -p "$warehouse"
runtime="$(command -v podman || command -v docker)"
"$runtime" run --rm --user "$(id -u):$(id -g)" \
  -v "$warehouse:/warehouse:Z" -v "$fixtures/write.sql:/write.sql:ro,Z" \
  -e HOME=/tmp -e SPARK_LOCAL_HOSTNAME=localhost \
  docker.io/apache/spark:4.0.4 \
  /opt/spark/bin/spark-sql --master 'local[2]' \
    --packages org.apache.iceberg:iceberg-spark-runtime-4.0_2.13:1.12.0 \
    --conf spark.jars.ivy=/tmp/.ivy \
    --conf spark.sql.extensions=org.apache.iceberg.spark.extensions.IcebergSparkSessionExtensions \
    --conf spark.sql.catalog.spark=org.apache.iceberg.spark.SparkCatalog \
    --conf spark.sql.catalog.spark.type=hadoop \
    --conf spark.sql.catalog.spark.warehouse=file:///warehouse \
    -f /write.sql
cd "$root/fabric"
LOAMS_SPARK_V3_WAREHOUSE="$warehouse" ${BUILD_WRAPPER:-} cargo test --locked -j "${JOBS:-4}" \
  -p loams-house-iceberg --test spark_v3 -- --ignored
