"""Loams' own cases for the Spark parity harness (design 81 SP-b, D2009).

Run by run.sh beside PySpark's parity modules, against the same endpoint:
a table written through the `loams` Iceberg catalog (Lakekeeper, vended
credentials) reads back, and lands in the Iceberg namespace the House reads
(`ns_<id>.<database>`, design 49 §5.3). Needs `loams[spark]`,
LOAMS_SPARK_REMOTE, LOAMS_TOKEN, LOAMS_ICEBERG_URI, LOAMS_ICEBERG_WAREHOUSE
and LOAMS_SPARK_NAMESPACE_ID.
"""

from __future__ import annotations

import os
import uuid

import pytest

pytest.importorskip("pyspark")

from loams import spark as loams_spark  # noqa: E402


@pytest.fixture(scope="module")
def spark():  # type: ignore[no-untyped-def]
    session = loams_spark.session(app_name="loams-spark-conformance").getOrCreate()
    yield session
    session.stop()


def test_iceberg_round_trip_in_house_namespace(spark) -> None:  # type: ignore[no-untyped-def]
    ns_id = int(os.environ["LOAMS_SPARK_NAMESPACE_ID"])
    table = f"conformance_{uuid.uuid4().hex[:8]}"
    name = loams_spark.house_table(ns_id, "conformance", table)
    spark.sql(f"CREATE NAMESPACE IF NOT EXISTS loams.ns_{ns_id}.conformance")
    try:
        spark.range(100).withColumnRenamed("id", "n").writeTo(name).using("iceberg").create()
        assert spark.table(name).count() == 100
        spark.sql(f"INSERT INTO {name} VALUES (100)")
        assert spark.table(name).count() == 101
    finally:
        spark.sql(f"DROP TABLE IF EXISTS {name} PURGE")


def test_sql_file_statements_split_like_the_driver() -> None:
    from loams.spark.driver import split_sql

    assert split_sql("SELECT ';' AS a; -- c;\nSELECT 2;") == ["SELECT ';' AS a", "SELECT 2"]
