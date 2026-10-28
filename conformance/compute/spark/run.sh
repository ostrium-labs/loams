#!/usr/bin/env bash
# The Spark Connect parity harness (design 81 SP-b, D2009). SKELETON: written,
# not yet run in CI or locally. It runs a pinned subset of PySpark's own Spark
# Connect parity suites (modules.txt) plus Loams' cases
# (loams_iceberg_test.py) against a Loams Spark Connect endpoint, and judges
# the result with the strict expected-failure list (xfail.toml), the same
# gate as Temporal and DBOS here.
#
#   LOAMS_SPARK_REMOTE=sc://127.0.0.1:15002 conformance/compute/spark/run.sh
#
# PySpark's connect tests read the endpoint from SPARK_CONNECT_TESTING_REMOTE
# (pyspark.testing.connectutils); this sets it from LOAMS_SPARK_REMOTE. The
# token goes as gRPC metadata in Loams' cases; PySpark's own suites need an
# endpoint that accepts them without one (the compose proxy on loopback, or a
# test-only token the proxy maps, an SP-a decision).
#
# Writes $OC_OUT/spark.junit.xml. OC_SKIP=parity,loams skips a part.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

: "${LOAMS_SPARK_REMOTE:?set LOAMS_SPARK_REMOTE to the sc:// endpoint under test}"
case $LOAMS_SPARK_REMOTE in
  *token=*) echo "run.sh: the endpoint URL must not carry a token" >&2; exit 2 ;;
esac

# versions.env: literal KEY=value lines.
while IFS='=' read -r key value; do
  case $key in ''|\#*) continue ;; esac
  export "$key=$value"
done <"$here/versions.env"

skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
work="$OC_OUT/spark"
rm -rf "$work" "$OC_OUT/spark.junit.xml"
mkdir -p "$work"

# 1. A virtualenv with the pinned client and the SDK from this checkout.
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install -q "pyspark-client==$PYSPARK_CLIENT" pytest "$OC_ROOT/sdks/python[spark]"

# 2. PySpark's parity tests at the tag, from the Apache Spark source (a sparse
# checkout of python/ only; the client wheel does not ship the tests).
src="$work/spark-src"
git init -q "$src"
git -C "$src" remote add origin https://github.com/apache/spark.git
git -C "$src" sparse-checkout set python/pyspark/sql/tests python/pyspark/testing
git -C "$src" fetch -q --depth 1 origin "refs/tags/$SPARK_TAG"
git -C "$src" checkout -q FETCH_HEAD

mapfile -t modules < <(grep -v -e '^#' -e '^[[:space:]]*$' "$here/modules.txt")
reports=()

if [[ $skip != *,parity,* ]]; then
  # The tests import pyspark.testing and pyspark.sql.tests from the source
  # tree; the client package comes from the venv.
  SPARK_CONNECT_TESTING_REMOTE=$LOAMS_SPARK_REMOTE \
  PYTHONPATH="$src/python" \
    "$work/venv/bin/python" -m pytest -q -p no:cacheprovider \
      --junitxml="$work/parity.junit.xml" --pyargs "${modules[@]}" || true
  reports+=("$work/parity.junit.xml")
fi

if [[ $skip != *,loams,* ]]; then
  "$work/venv/bin/python" -m pytest -q -p no:cacheprovider \
    --junitxml="$work/loams.junit.xml" "$here/loams_iceberg_test.py" || true
  reports+=("$work/loams.junit.xml")
fi

if [ "${#reports[@]}" -eq 0 ]; then
  echo "run.sh: every part skipped" >&2
  exit 1
fi
cp "${reports[0]}" "$OC_OUT/spark.junit.xml"
oc_xfail_check spark "${reports[@]}"
