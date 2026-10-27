# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

from types import SimpleNamespace
from unittest import mock

import pytest

from airflow.providers.loams.hooks.iceberg import LoamsIcebergHook
from airflow.providers.loams.operators.iceberg import LoamsIcebergAppendOperator
from airflow.providers.loams.sensors.iceberg import LoamsIcebergSnapshotSensor


def test_catalog_properties_with_keys(loams_connection) -> None:
    assert LoamsIcebergHook().catalog_properties() == {
        "uri": "http://lakekeeper:8181/catalog",
        "warehouse": "wh",
        "token": "lk_test",
        "s3.endpoint": "http://rustfs:9000",
        "s3.access-key-id": "ak",
        "s3.secret-access-key": "sk",
    }


def test_catalog_properties_oauth_and_vended(loams_connection) -> None:
    loams_connection(
        {
            "iceberg_catalog_uri": "http://lk/catalog",
            "iceberg_credential": "id:secret",
            "iceberg_oauth2_server_uri": "http://idp/token",
            "iceberg_scope": "lakekeeper",
            "iceberg_properties": {"py-io-impl": "pyiceberg.io.fsspec.FsspecFileIO"},
        }
    )
    props = LoamsIcebergHook().catalog_properties()
    assert props["credential"] == "id:secret"
    assert props["oauth2-server-uri"] == "http://idp/token"
    assert props["scope"] == "lakekeeper"
    assert "token" not in props
    assert props["header.X-Iceberg-Access-Delegation"] == "vended-credentials"
    assert props["py-io-impl"] == "pyiceberg.io.fsspec.FsspecFileIO"


def test_needs_catalog(loams_connection) -> None:
    loams_connection({})
    with pytest.raises(ValueError, match="iceberg_catalog_uri"):
        LoamsIcebergHook().catalog_properties()


def test_get_catalog(loams_connection) -> None:
    with mock.patch("pyiceberg.catalog.rest.RestCatalog") as rest:
        LoamsIcebergHook().get_catalog()
    assert rest.call_args.args == ("loams",)
    assert rest.call_args.kwargs["uri"] == "http://lakekeeper:8181/catalog"


def test_namespaces_and_tables(loams_connection) -> None:
    catalog = mock.Mock()
    catalog.list_namespaces.return_value = [("sales",), ("a", "b")]
    catalog.list_tables.return_value = [("sales", "orders")]
    hook = LoamsIcebergHook()
    with mock.patch.object(hook, "get_catalog", return_value=catalog):
        assert hook.list_namespaces() == ["sales", "a.b"]
        assert hook.list_tables("sales") == ["sales.orders"]
        hook.create_namespace("sales")
        hook.create_namespace("x", exist_ok=False)
    catalog.create_namespace_if_not_exists.assert_called_once_with("sales", {})
    catalog.create_namespace.assert_called_once_with("x", {})


def test_current_snapshot(loams_connection) -> None:
    summary = mock.MagicMock()
    summary.operation.value = "append"
    summary.__iter__.return_value = iter(["added-records"])
    summary.__getitem__.side_effect = lambda k: "2"
    snapshot = SimpleNamespace(
        snapshot_id=7, parent_snapshot_id=6, sequence_number=3, timestamp_ms=1793059200000, summary=summary
    )
    table = mock.Mock()
    table.current_snapshot.return_value = snapshot
    hook = LoamsIcebergHook()
    with mock.patch.object(hook, "load_table", return_value=table):
        assert hook.current_snapshot("sales.orders") == {
            "snapshot_id": 7,
            "parent_snapshot_id": 6,
            "sequence_number": 3,
            "timestamp_ms": 1793059200000,
            "operation": "append",
            "summary": {"added-records": "2"},
        }
        table.current_snapshot.return_value = None
        assert hook.current_snapshot("sales.orders") is None


def test_append_rows_uses_table_schema(loams_connection) -> None:
    import pyarrow as pa

    table = mock.Mock()
    table.schema.return_value.as_arrow.return_value = pa.schema([("id", pa.int64()), ("name", pa.string())])
    table.refresh.return_value.current_snapshot.return_value = SimpleNamespace(snapshot_id=9)
    hook = LoamsIcebergHook()
    with mock.patch.object(hook, "load_table", return_value=table):
        assert hook.append_rows("sales.orders", [{"id": 1, "name": "a"}]) == 9
        assert hook.append_rows("sales.orders", []) is None
    data = table.append.call_args.args[0]
    assert data.to_pylist() == [{"id": 1, "name": "a"}]


def test_scan_rows_passes_options(loams_connection) -> None:
    table = mock.Mock()
    table.scan.return_value.to_arrow.return_value.to_pylist.return_value = [{"id": 1}]
    hook = LoamsIcebergHook()
    with mock.patch.object(hook, "load_table", return_value=table):
        assert hook.scan_rows("t", row_filter="id = 1", limit=5, snapshot_id=3) == [{"id": 1}]
    assert table.scan.call_args.kwargs == {
        "selected_fields": ("*",),
        "row_filter": "id = 1",
        "limit": 5,
        "snapshot_id": 3,
    }


def test_append_operator_refuses_text_rows(context) -> None:
    op = LoamsIcebergAppendOperator(task_id="a", table="t", rows="[{'id': 1}]")
    with pytest.raises(TypeError, match="native"):
        op.execute(context)


def test_snapshot_sensor(loams_connection, context) -> None:
    sensor = LoamsIcebergSnapshotSensor(task_id="s", table="t", after_snapshot_id="7")
    with (
        mock.patch.object(LoamsIcebergHook, "table_exists", return_value=True),
        mock.patch.object(
            LoamsIcebergHook, "current_snapshot", side_effect=[{"snapshot_id": 7}, {"snapshot_id": 8}]
        ),
    ):
        assert not sensor.poke(context).is_done
        result = sensor.poke(context)
    assert result.is_done and result.xcom_value == {"snapshot_id": 8}
    with mock.patch.object(LoamsIcebergHook, "table_exists", return_value=False):
        assert not sensor.poke(context).is_done
