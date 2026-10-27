# SPDX-License-Identifier: Apache-2.0
"""`LoamsPostgresHook`: Loams Postgres through Airflow's Postgres hook.

The hook is `PostgresHook` with one change: where the connection comes from.
Instead of a host and password stored in Airflow, it asks Loams for them, from
the `loams` connection's API key (design 74 §9.4, `LoamsPostgresHook`):

1. `loams.postgres.v1.PostgresService/IssueConnectCredential` (on `pg-control`)
   answers a short-lived login (15 minutes by default, 1 hour at most) and
   where to connect: PgDog's host and port, the routed database name
   (`project[__branch[.database|__ro]]`), `sslmode` and the CA bundle.
2. With a `database` or `read_only`, `GetConnectionInfo` answers the routed
   name for that database or the branch's read-only endpoints, and the issued
   login is used against it.

Nothing is stored: the password lives in the hook's connection object for the
life of the hook, and the CA bundle in a private temporary file. Everything
`PostgresHook` and `DbApiHook` do (`run`, `get_records`, `get_df`,
`insert_rows`, `bulk_load`, SQLAlchemy engines, OpenLineage) works unchanged.

With extra `pg_host` set the hook skips discovery and connects to that
address with `pg_user`/`pg_password` (or the API key as the password): a dev
stack, a tunnel, or the gateway's `pg` SNI listener.
"""

from __future__ import annotations

import atexit
import contextlib
import json
import os
import tempfile
import uuid
from typing import TYPE_CHECKING, Any

from airflow.providers.postgres.hooks.postgres import PostgresHook
from airflow.sdk import Connection

from airflow.providers.loams._connect import field
from airflow.providers.loams.hooks.base import LoamsHook

if TYPE_CHECKING:
    from airflow.providers.loams.hooks.base import LoamsSettings

__all__ = ["LoamsPostgresHook"]

POSTGRES_SERVICE = "loams.postgres.v1.PostgresService"

_ca_files: list[str] = []


def _cleanup_ca_files() -> None:
    while _ca_files:
        with contextlib.suppress(OSError):
            os.unlink(_ca_files.pop())


atexit.register(_cleanup_ca_files)


def write_ca_bundle(pem: str) -> str:
    """Writes a CA bundle to a private file (mode 0600) and answers its path."""
    fd, path = tempfile.mkstemp(prefix="loams-pg-ca-", suffix=".pem")
    with os.fdopen(fd, "w") as fh:
        fh.write(pem)
    _ca_files.append(path)
    return path


class LoamsPostgresHook(PostgresHook):
    """Loams Postgres, with the address and a short-lived login from Loams.

    :param loams_conn_id: the `loams` connection.
    :param project: the Loams Postgres project id (`prj-...`); the connection's
        extra `pg_project` by default.
    :param branch: the branch id (`br-...`); empty takes the project's default.
    :param endpoint: the endpoint id (`ep-...`); empty takes the branch's
        read-write endpoint.
    :param role: the role the issued login belongs to; empty takes the
        caller's default.
    :param pg_database: the database on the branch; empty takes its first.
    :param read_only: connect to the branch's read-only endpoints.
    :param credential_ttl_seconds: how long the issued login lives (at most
        3600; the server cuts anything longer).
    :param kwargs: anything `PostgresHook` takes (`options`,
        `enable_log_db_messages`, `log_sql`, ...).
    """

    conn_name_attr = "loams_conn_id"
    default_conn_name = "loams_default"
    conn_type = "loams"
    hook_name = "Loams Postgres"

    def __init__(
        self,
        *args: Any,
        project: str | None = None,
        branch: str | None = None,
        endpoint: str | None = None,
        role: str | None = None,
        pg_database: str | None = None,
        read_only: bool | None = None,
        credential_ttl_seconds: int | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        self.project = project
        self.branch = branch
        self.endpoint = endpoint
        self.role = role
        self.pg_database = pg_database
        self.read_only = read_only
        self.credential_ttl_seconds = credential_ttl_seconds
        self.credential_expire_time: str | None = None

    @property
    def connection(self) -> Connection:  # type: ignore[override]
        if self._connection is None:
            self._connection = self._resolve_connection()
        return self._connection

    @connection.setter
    def connection(self, value: Any) -> None:
        self._connection = value

    def _loams_hook(self) -> LoamsHook:
        return LoamsHook(loams_conn_id=self.get_conn_id())

    def _resolve_connection(self) -> Connection:
        settings = self._loams_hook().settings
        if settings.pg_host:
            return self._direct_connection(settings)
        return self._discovered_connection(settings)

    def _direct_connection(self, s: LoamsSettings) -> Connection:
        extra: dict[str, Any] = {}
        if s.pg_sslmode:
            extra["sslmode"] = s.pg_sslmode
        return Connection(
            conn_id=self.get_conn_id(),
            conn_type="postgres",
            host=s.pg_host,
            port=s.pg_port or 5432,
            login=s.pg_user,
            password=s.pg_password or s.api_key,
            schema=self.pg_database or s.pg_database,
            extra=json.dumps(extra),
        )

    def _discovered_connection(self, s: LoamsSettings) -> Connection:
        project = self.project or s.pg_project
        if not project:
            raise ValueError(
                "LoamsPostgresHook needs a project: pass project= or set the connection's extra `pg_project`"
            )
        namespace = s.require_namespace()
        branch = self.branch or s.pg_branch or ""
        endpoint = self.endpoint or s.pg_endpoint or ""
        role = self.role or s.pg_role or ""
        database = self.pg_database or s.pg_database or ""
        read_only = s.pg_read_only if self.read_only is None else self.read_only
        ttl = min(int(self.credential_ttl_seconds or s.pg_credential_ttl_seconds), 3600)

        client = LoamsHook(loams_conn_id=self.get_conn_id()).connect_client(
            s.postgres_control_endpoint or s.endpoint,
            token=s.postgres_control_api_key or s.api_key,
        )
        issued = client.call(
            POSTGRES_SERVICE,
            "IssueConnectCredential",
            {
                "namespace": namespace,
                "projectId": project,
                "branchId": branch,
                "endpointId": endpoint,
                "role": role,
                "ttl": f"{ttl}s",
                "idempotencyKey": str(uuid.uuid4()),
            },
        )
        info = field(issued, "connection") or {}
        password = field(issued, "password")
        if not password:
            raise ValueError("IssueConnectCredential answered no password")
        self.credential_expire_time = field(issued, "expire_time")

        if database or read_only:
            located = client.call(
                POSTGRES_SERVICE,
                "GetConnectionInfo",
                {
                    "namespace": namespace,
                    "projectId": project,
                    "branchId": branch,
                    "endpointId": endpoint,
                    "role": role,
                    "database": database,
                    "readOnly": bool(read_only),
                },
            )
            where = field(located, "connection") or {}
            # The address and routed name come from GetConnectionInfo; the
            # login is still the issued one.
            info = {**info, **{k: v for k, v in where.items() if k not in ("user",)}}

        host = field(info, "host")
        port = field(info, "port")
        if not host:
            raise ValueError("Loams answered no Postgres host for the project")
        extra: dict[str, Any] = {"sslmode": field(info, "sslmode") or s.pg_sslmode or "verify-full"}
        ca_pem = field(info, "ca_pem")
        if ca_pem:
            extra["sslrootcert"] = write_ca_bundle(ca_pem)
        self.log.info(
            "Loams Postgres: %s:%s database %s as %s (login expires %s)",
            host,
            port,
            field(info, "database"),
            field(info, "user"),
            self.credential_expire_time or "unknown",
        )
        return Connection(
            conn_id=self.get_conn_id(),
            conn_type="postgres",
            host=str(host),
            port=int(port) if port else 5432,
            login=field(info, "user"),
            password=str(password),
            schema=field(info, "database"),
            extra=json.dumps(extra),
        )
