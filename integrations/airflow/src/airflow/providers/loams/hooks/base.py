# SPDX-License-Identifier: Apache-2.0
"""`LoamsHook`: the `loams` connection type and what every Loams hook shares.

One Airflow connection describes one Loams instance:

| Connection field | Meaning |
|---|---|
| Host | The instance's base URL (`https://acme.loams.dev`, `http://127.0.0.1:8080`); |
| | without a scheme `https://` is assumed (extra `scheme` changes it) |
| Port | Optional; appended to the host |
| Password | The Loams API key (`lk_...`), sent as `Authorization: Bearer` |
| Schema | The Loams namespace (extra `namespace` also works) |
| Extra | Per-service endpoints and settings, below |

The per-service extras are optional. A service whose endpoint is unset uses
the instance's base URL, which is right for everything the engine's main port
serves or proxies (the SDK modules, `loams.house.v1`). Loams Postgres
discovery goes to `pg-control`, Kafka to the gateway, S3 to the bucket and
Iceberg to Lakekeeper, each of which is its own listener.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from functools import cached_property
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseHook

from airflow.providers.loams._connect import AsyncConnectClient, ConnectClient

if TYPE_CHECKING:
    from airflow.sdk import Connection

    from loams import Loams

__all__ = ["LoamsHook", "LoamsSettings"]

_TRUE = {"1", "true", "yes", "on"}


def _bool(value: Any, default: bool = False) -> bool:
    if value is None or value == "":
        return default
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in _TRUE


def _dict(value: Any) -> dict[str, Any]:
    if value is None or value == "":
        return {}
    if isinstance(value, Mapping):
        return dict(value)
    parsed = json.loads(str(value))
    if not isinstance(parsed, Mapping):
        raise ValueError(f"expected a JSON object, got {type(parsed).__name__}")
    return dict(parsed)


def _opt(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def build_endpoint(host: str | None, port: int | None, scheme: str | None) -> str:
    """The instance's base URL from the connection's host, port and scheme."""
    if not host:
        raise ValueError("the Loams connection has no host: set it to the instance's URL")
    host = host.strip().rstrip("/")
    if "://" not in host:
        host = f"{scheme or 'https'}://{host}"
    if port:
        scheme_part, _, rest = host.partition("://")
        authority, slash, path = rest.partition("/")
        if ":" not in authority.rsplit("]", 1)[-1]:
            authority = f"{authority}:{port}"
        host = f"{scheme_part}://{authority}{slash}{path}"
    return host


@dataclass(frozen=True)
class LoamsSettings:
    """Everything a `loams` connection says, parsed once."""

    endpoint: str
    api_key: str | None = None
    namespace: str | None = None
    verify: bool | str = True
    timeout: float = 30.0

    # Loams Postgres (pg-control: loams.postgres.v1.PostgresService).
    postgres_control_endpoint: str | None = None
    postgres_control_api_key: str | None = None
    pg_project: str | None = None
    pg_branch: str | None = None
    pg_endpoint: str | None = None
    pg_role: str | None = None
    pg_database: str | None = None
    pg_read_only: bool = False
    pg_credential_ttl_seconds: int = 900
    # A direct Postgres address skips discovery (a dev stack, a tunnel).
    pg_host: str | None = None
    pg_port: int | None = None
    pg_user: str | None = None
    pg_password: str | None = None
    pg_sslmode: str | None = None

    # Loams House (loams.house.v1.HouseService).
    house_endpoint: str | None = None
    house_database: str | None = None

    # Loams Graph (the SDK's `graph` module on the main port).
    graph: str | None = None

    # Streams through the Iggy Kafka gateway.
    kafka_bootstrap_servers: str | None = None
    kafka_security_protocol: str | None = None
    kafka_sasl_mechanism: str | None = None
    kafka_sasl_username: str | None = None
    kafka_sasl_password: str | None = None
    kafka_config: dict[str, Any] = field(default_factory=dict)

    # Object storage (S3-compatible: RustFS in the default stack, or a cloud bucket).
    s3_endpoint: str | None = None
    s3_region: str | None = None
    s3_access_key_id: str | None = None
    s3_secret_access_key: str | None = None
    s3_bucket: str | None = None
    s3_path_style: bool = True

    # Iceberg through the Lakekeeper REST catalog.
    iceberg_catalog_uri: str | None = None
    iceberg_warehouse: str | None = None
    iceberg_token: str | None = None
    iceberg_credential: str | None = None
    iceberg_oauth2_server_uri: str | None = None
    iceberg_scope: str | None = None
    iceberg_properties: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_connection(cls, conn: Connection, *, namespace: str | None = None) -> LoamsSettings:
        extra: dict[str, Any] = dict(conn.extra_dejson or {})
        endpoint = build_endpoint(conn.host, conn.port, _opt(extra.get("scheme")))
        verify_raw = extra.get("verify")
        verify: bool | str
        if verify_raw is None or isinstance(verify_raw, bool):
            verify = True if verify_raw is None else verify_raw
        elif str(verify_raw).lower() in _TRUE | {"false", "0", "no", "off"}:
            verify = _bool(verify_raw)
        else:
            verify = str(verify_raw)  # a CA bundle path
        ttl = extra.get("pg_credential_ttl_seconds")
        pg_port = extra.get("pg_port")
        return cls(
            endpoint=endpoint,
            api_key=_opt(conn.password) or _opt(extra.get("api_key")),
            namespace=_opt(namespace) or _opt(conn.schema) or _opt(extra.get("namespace")),
            verify=verify,
            timeout=float(extra.get("timeout") or 30.0),
            postgres_control_endpoint=_opt(extra.get("postgres_control_endpoint")),
            postgres_control_api_key=_opt(extra.get("postgres_control_api_key")),
            pg_project=_opt(extra.get("pg_project")),
            pg_branch=_opt(extra.get("pg_branch")),
            pg_endpoint=_opt(extra.get("pg_endpoint")),
            pg_role=_opt(extra.get("pg_role")),
            pg_database=_opt(extra.get("pg_database")),
            pg_read_only=_bool(extra.get("pg_read_only")),
            pg_credential_ttl_seconds=int(ttl) if ttl else 900,
            pg_host=_opt(extra.get("pg_host")),
            pg_port=int(pg_port) if pg_port else None,
            pg_user=_opt(extra.get("pg_user")),
            pg_password=_opt(extra.get("pg_password")),
            pg_sslmode=_opt(extra.get("pg_sslmode")),
            house_endpoint=_opt(extra.get("house_endpoint")),
            house_database=_opt(extra.get("house_database")),
            graph=_opt(extra.get("graph")),
            kafka_bootstrap_servers=_opt(extra.get("kafka_bootstrap_servers")),
            kafka_security_protocol=_opt(extra.get("kafka_security_protocol")),
            kafka_sasl_mechanism=_opt(extra.get("kafka_sasl_mechanism")),
            kafka_sasl_username=_opt(extra.get("kafka_sasl_username")),
            kafka_sasl_password=_opt(extra.get("kafka_sasl_password")),
            kafka_config=_dict(extra.get("kafka_config")),
            s3_endpoint=_opt(extra.get("s3_endpoint")),
            s3_region=_opt(extra.get("s3_region")),
            s3_access_key_id=_opt(extra.get("s3_access_key_id")),
            s3_secret_access_key=_opt(extra.get("s3_secret_access_key")),
            s3_bucket=_opt(extra.get("s3_bucket")),
            s3_path_style=_bool(extra.get("s3_path_style"), default=True),
            iceberg_catalog_uri=_opt(extra.get("iceberg_catalog_uri")),
            iceberg_warehouse=_opt(extra.get("iceberg_warehouse")),
            iceberg_token=_opt(extra.get("iceberg_token")),
            iceberg_credential=_opt(extra.get("iceberg_credential")),
            iceberg_oauth2_server_uri=_opt(extra.get("iceberg_oauth2_server_uri")),
            iceberg_scope=_opt(extra.get("iceberg_scope")),
            iceberg_properties=_dict(extra.get("iceberg_properties")),
        )

    def require_namespace(self) -> str:
        if not self.namespace:
            raise ValueError(
                "the Loams connection names no namespace: set its Schema field (or extra `namespace`)"
            )
        return self.namespace


class LoamsHook(BaseHook):
    """The `loams` connection: one Loams instance, an API key and a namespace.

    :param loams_conn_id: the Airflow connection id (type `loams`).
    :param namespace: overrides the connection's namespace for this hook.
    """

    conn_name_attr = "loams_conn_id"
    default_conn_name = "loams_default"
    conn_type = "loams"
    hook_name = "Loams"

    def __init__(
        self,
        loams_conn_id: str = default_conn_name,
        *,
        namespace: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.loams_conn_id = loams_conn_id
        self._namespace = namespace

    @classmethod
    def get_ui_field_behaviour(cls) -> dict[str, Any]:
        """The connection form (provider.yaml's `conn-fields` describes the extras)."""
        return {
            "hidden_fields": ["login"],
            "relabeling": {"host": "Endpoint", "password": "API key", "schema": "Namespace"},
            "placeholders": {
                "host": "https://acme.loams.dev",
                "password": "lk_...",
                "schema": "default",
                "extra": json.dumps(
                    {
                        "postgres_control_endpoint": "http://127.0.0.1:7680",
                        "pg_project": "prj-...",
                        "kafka_bootstrap_servers": "127.0.0.1:9093",
                        "s3_endpoint": "http://127.0.0.1:9000",
                        "iceberg_catalog_uri": "http://127.0.0.1:8181/catalog",
                    },
                    indent=1,
                ),
            },
        }

    @cached_property
    def connection(self) -> Connection:
        return self.get_connection(self.loams_conn_id)

    @cached_property
    def settings(self) -> LoamsSettings:
        return LoamsSettings.from_connection(self.connection, namespace=self._namespace)

    @property
    def namespace(self) -> str | None:
        return self.settings.namespace

    def connect_client(
        self, base_url: str | None = None, *, token: str | None = None, timeout: float | None = None
    ) -> ConnectClient:
        """A Connect (JSON) client for an RPC the SDK has no module for."""
        s = self.settings
        return ConnectClient(
            base_url=base_url or s.endpoint,
            token=token if token is not None else s.api_key,
            timeout=timeout if timeout is not None else s.timeout,
            verify=s.verify,
        )

    def async_connect_client(
        self, base_url: str | None = None, *, token: str | None = None, timeout: float | None = None
    ) -> AsyncConnectClient:
        s = self.settings
        return AsyncConnectClient(
            base_url=base_url or s.endpoint,
            token=token if token is not None else s.api_key,
            timeout=timeout if timeout is not None else s.timeout,
            verify=s.verify,
        )

    def get_conn(self) -> Loams:
        """A Loams SDK client for this connection (close it, or use `with`)."""
        from loams import Loams

        s = self.settings
        return Loams(
            s.endpoint,
            api_key=s.api_key,
            namespace=s.namespace,
            timeout_ms=int(s.timeout * 1000),
        )

    def test_connection(self) -> tuple[bool, str]:
        """Asks the instance what it is (`InstanceService/GetInstance`)."""
        try:
            from loams.instance.v1.instance_pb2 import GetInstanceRequest

            with self.get_conn() as client:
                info = client.instance.get_instance(GetInstanceRequest())
            name = getattr(info, "name", "") or getattr(info, "instance_id", "")
            version = getattr(info, "server_version", "")
            return True, f"Connected to Loams {name} {version}".strip()
        except Exception as exc:
            return False, str(exc)
