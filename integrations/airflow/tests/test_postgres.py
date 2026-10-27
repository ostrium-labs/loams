# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import os
from unittest import mock

import pytest

from airflow.providers.loams.hooks.postgres import LoamsPostgresHook
from airflow.providers.loams.operators.postgres import LoamsPostgresOperator

ISSUED = {
    "connection": {
        "host": "pgdog.acme",
        "port": 6432,
        "database": "prj-01TEST",
        "user": "issued_login",
        "sslmode": "verify-full",
        "caPem": "-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n",
    },
    "password": "s3cret",
    "expireTime": "2026-10-27T01:00:00Z",
}


def test_discovery_issues_a_credential(loams_connection) -> None:
    with mock.patch("airflow.providers.loams._connect.ConnectClient.call", return_value=ISSUED) as call:
        hook = LoamsPostgresHook(credential_ttl_seconds=7200)
        conn = hook.connection
    service, method, body = call.call_args.args
    assert (service, method) == ("loams.postgres.v1.PostgresService", "IssueConnectCredential")
    assert body["namespace"] == "acme"
    assert body["projectId"] == "prj-01TEST"
    assert body["ttl"] == "3600s"  # capped at an hour
    assert body["idempotencyKey"]
    assert (conn.host, conn.port, conn.login, conn.password, conn.schema) == (
        "pgdog.acme",
        6432,
        "issued_login",
        "s3cret",
        "prj-01TEST",
    )
    extra = conn.extra_dejson
    assert extra["sslmode"] == "verify-full"
    with open(extra["sslrootcert"]) as fh:
        assert "BEGIN CERTIFICATE" in fh.read()
    assert oct(os.stat(extra["sslrootcert"]).st_mode & 0o777) == "0o600"
    assert hook.credential_expire_time == "2026-10-27T01:00:00Z"


def test_discovery_goes_to_pg_control(loams_connection) -> None:
    with mock.patch("airflow.providers.loams.hooks.base.ConnectClient") as client_cls:
        client_cls.return_value.call.return_value = ISSUED
        LoamsPostgresHook().connection  # noqa: B018
    kwargs = client_cls.call_args.kwargs
    assert kwargs["base_url"] == "http://pg-control:7680"
    assert kwargs["token"] == "lk_test"


def test_database_uses_connection_info(loams_connection) -> None:
    located = {
        "connection": {
            "host": "pgdog-ro.acme",
            "port": 6432,
            "database": "prj-01TEST__br.app",
            "user": "owner",
        }
    }
    with mock.patch(
        "airflow.providers.loams._connect.ConnectClient.call", side_effect=[ISSUED, located]
    ) as call:
        conn = LoamsPostgresHook(pg_database="app", read_only=True).connection
    method, body = call.call_args_list[1].args[1:]
    assert method == "GetConnectionInfo"
    assert body["database"] == "app"
    assert body["readOnly"] is True
    assert conn.host == "pgdog-ro.acme"
    assert conn.schema == "prj-01TEST__br.app"
    assert conn.login == "issued_login"  # the issued login, not the owner


def test_project_is_required(loams_connection) -> None:
    loams_connection({})
    with pytest.raises(ValueError, match="needs a project"):
        LoamsPostgresHook().connection  # noqa: B018


def test_direct_mode_skips_discovery(loams_connection) -> None:
    loams_connection(
        {"pg_host": "127.0.0.1", "pg_port": "55432", "pg_user": "postgres", "pg_database": "app"}
    )
    with mock.patch("airflow.providers.loams._connect.ConnectClient.call") as call:
        conn = LoamsPostgresHook().connection
    call.assert_not_called()
    assert (conn.host, conn.port, conn.login, conn.password, conn.schema) == (
        "127.0.0.1",
        55432,
        "postgres",
        "lk_test",
        "app",
    )


def test_get_conn_passes_discovered_args(loams_connection) -> None:
    with (
        mock.patch("airflow.providers.loams._connect.ConnectClient.call", return_value=ISSUED),
        mock.patch.object(LoamsPostgresHook, "_create_connection", create=True) as create,
    ):
        LoamsPostgresHook().get_conn()
    args = create.call_args.args[0]
    assert args["host"] == "pgdog.acme"
    assert args["user"] == "issued_login"
    assert args["password"] == "s3cret"
    assert args["dbname"] == "prj-01TEST"
    assert args["sslmode"] == "verify-full"


def test_operator_builds_a_loams_hook() -> None:
    op = LoamsPostgresOperator(task_id="t", sql="SELECT 1", project="prj-X", branch="br-Y", pg_database="db")
    hook = op.get_db_hook()
    assert isinstance(hook, LoamsPostgresHook)
    assert (hook.project, hook.branch, hook.pg_database) == ("prj-X", "br-Y", "db")
    assert hook.get_conn_id() == "loams_default"
    assert "project" in op.template_fields and "sql" in op.template_fields
