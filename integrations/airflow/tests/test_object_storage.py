# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import datetime as dt
from unittest import mock

import pytest
from botocore.exceptions import ClientError

from airflow.providers.loams.fs import loams_s3
from airflow.providers.loams.hooks.object_storage import LoamsObjectStorageHook, parse_object_url
from airflow.providers.loams.operators.object_storage import (
    LoamsObjectDeleteOperator,
    LoamsObjectUploadOperator,
)
from airflow.providers.loams.sensors.object_storage import LoamsObjectKeySensor, _fixed_prefix


def test_parse_object_url() -> None:
    assert parse_object_url("s3://b/k/x") == ("b", "k/x")
    assert parse_object_url("loams-s3://loams_default@b/k") == ("b", "k")
    assert parse_object_url("/k") == (None, "k")


def test_client_settings(loams_connection) -> None:
    hook = LoamsObjectStorageHook()
    with mock.patch("boto3.client") as client:
        hook.get_client()
    kwargs = client.call_args.kwargs
    assert client.call_args.args == ("s3",)
    assert kwargs["endpoint_url"] == "http://rustfs:9000"
    assert kwargs["aws_access_key_id"] == "ak"
    assert kwargs["region_name"] == "us-east-1"
    assert kwargs["config"].s3 == {"addressing_style": "path"}


def test_needs_endpoint(loams_connection) -> None:
    loams_connection({})
    with pytest.raises(ValueError, match="s3_endpoint"):
        LoamsObjectStorageHook().s3fs_options()


def test_bucket_resolution(loams_connection) -> None:
    assert LoamsObjectStorageHook().bucket_for(None) == "lake"
    assert LoamsObjectStorageHook(bucket="b2").bucket_for(None) == "b2"
    assert LoamsObjectStorageHook().bucket_for("b3") == "b3"
    loams_connection({"s3_endpoint": "http://x"})
    with pytest.raises(ValueError, match="bucket"):
        LoamsObjectStorageHook().bucket_for(None)


def test_s3fs_options(loams_connection) -> None:
    assert LoamsObjectStorageHook().s3fs_options() == {
        "key": "ak",
        "secret": "sk",
        "endpoint_url": "http://rustfs:9000",
        "client_kwargs": {"region_name": "us-east-1"},
        "config_kwargs": {"signature_version": "s3v4", "s3": {"addressing_style": "path"}},
    }


def test_get_fs_merges_options(loams_connection) -> None:
    fs = loams_s3.get_fs("loams_default", {"client_kwargs": {"verify": False}, "anon": False})
    assert "loams-s3" in type(fs).protocol
    assert fs.client_kwargs == {"region_name": "us-east-1", "verify": False}
    assert fs._strip_protocol("loams-s3://lake/a/b") == "lake/a/b"
    assert loams_s3.schemes == ["loams-s3"]


def test_head_and_exists(loams_connection) -> None:
    hook = LoamsObjectStorageHook()
    client = mock.Mock()
    client.head_object.side_effect = [
        {"ContentLength": 3},
        ClientError({"Error": {"Code": "404"}, "ResponseMetadata": {"HTTPStatusCode": 404}}, "HeadObject"),
        ClientError({"Error": {"Code": "403"}, "ResponseMetadata": {"HTTPStatusCode": 403}}, "HeadObject"),
    ]
    with mock.patch.object(hook, "get_client", return_value=client):
        assert hook.exists("a")
        assert not hook.exists("b")
        with pytest.raises(ClientError):
            hook.exists("c")


def test_iter_keys(loams_connection) -> None:
    hook = LoamsObjectStorageHook()
    client = mock.Mock()
    client.get_paginator.return_value.paginate.return_value = [
        {"Contents": [{"Key": "p/a", "Size": 1, "ETag": '"e"', "LastModified": dt.datetime(2026, 10, 27)}]},
        {},
    ]
    with mock.patch.object(hook, "get_client", return_value=client):
        assert list(hook.iter_keys("p/")) == [
            {"key": "p/a", "size": 1, "etag": "e", "last_modified": "2026-10-27T00:00:00"}
        ]


def test_delete_keys_batches(loams_connection) -> None:
    hook = LoamsObjectStorageHook()
    client = mock.Mock()
    client.delete_objects.return_value = {}
    with mock.patch.object(hook, "get_client", return_value=client):
        assert hook.delete_keys(f"k{i}" for i in range(1500)) == 1500
    assert client.delete_objects.call_count == 2


def test_delete_reports_errors(loams_connection) -> None:
    hook = LoamsObjectStorageHook()
    client = mock.Mock()
    client.delete_objects.return_value = {"Errors": [{"Key": "k", "Message": "denied"}]}
    with (
        mock.patch.object(hook, "get_client", return_value=client),
        pytest.raises(RuntimeError, match="denied"),
    ):
        hook.delete_keys(["k"])


def test_upload_operator(loams_connection, context) -> None:
    op = LoamsObjectUploadOperator(task_id="u", key="a/b.txt", data="hi", content_type="text/plain")
    with mock.patch.object(LoamsObjectStorageHook, "put_bytes", return_value="s3://lake/a/b.txt") as put:
        assert op.execute(context) == "s3://lake/a/b.txt"
    put.assert_called_once_with("hi", "a/b.txt", content_type="text/plain")
    with pytest.raises(ValueError):
        LoamsObjectUploadOperator(task_id="u2", key="k")


def test_delete_operator_refuses_empty_prefix() -> None:
    with pytest.raises(ValueError, match="whole bucket"):
        LoamsObjectDeleteOperator(task_id="d", prefix="/")


def test_key_sensor(loams_connection, context) -> None:
    sensor = LoamsObjectKeySensor(task_id="s", keys="day/*.csv", wildcard=True)
    with mock.patch.object(
        LoamsObjectStorageHook, "list_keys", return_value=["day/a.csv", "day/b.json"]
    ) as keys:
        result = sensor.poke(context)
    keys.assert_called_once_with("day/")
    assert result.is_done and result.xcom_value == ["day/a.csv"]
    with mock.patch.object(LoamsObjectStorageHook, "exists", return_value=False):
        assert not LoamsObjectKeySensor(task_id="s2", keys=["a", "b"]).poke(context).is_done


def test_fixed_prefix() -> None:
    assert _fixed_prefix("a/b/*.csv") == "a/b/"
    assert _fixed_prefix("a/[0-9]?") == "a/"
    assert _fixed_prefix("plain") == "plain"
