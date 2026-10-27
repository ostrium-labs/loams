# SPDX-License-Identifier: Apache-2.0
"""`LoamsIcebergHook`: Iceberg tables through the Lakekeeper REST catalog.

Loams's one catalog is Lakekeeper, an Iceberg REST catalog (design 68 §9.1):
the House, the engines and Spark/Flink jobs all read and write the same
tables through it. The hook is PyIceberg's `RestCatalog` with the catalog URI,
warehouse and credentials from the `loams` connection:

- `iceberg_catalog_uri` (required) and `iceberg_warehouse`;
- `iceberg_credential` (`client_id:client_secret`, OAuth2 client credentials,
  with `iceberg_oauth2_server_uri` and `iceberg_scope`), or else
  `iceberg_token`, or else the Loams API key, as a bearer;
- the `s3_*` extras become PyIceberg's `s3.*` FileIO properties; without
  them the hook asks Lakekeeper for vended credentials;
- `iceberg_properties` passes anything else to PyIceberg as is.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.providers.loams.hooks.base import LoamsHook

if TYPE_CHECKING:
    from pyiceberg.catalog import Catalog
    from pyiceberg.table import Table

__all__ = ["LoamsIcebergHook"]


def _identifier(name: str | Sequence[str]) -> str:
    return name if isinstance(name, str) else ".".join(name)


class LoamsIcebergHook(LoamsHook):
    """Iceberg tables in the Loams catalog (Lakekeeper)."""

    hook_name = "Loams Iceberg"

    def catalog_properties(self) -> dict[str, str]:
        s = self.settings
        if not s.iceberg_catalog_uri:
            raise ValueError("the Loams connection has no catalog: set its extra `iceberg_catalog_uri`")
        props: dict[str, str] = {"uri": s.iceberg_catalog_uri}
        if s.iceberg_warehouse:
            props["warehouse"] = s.iceberg_warehouse
        if s.iceberg_credential:
            props["credential"] = s.iceberg_credential
            if s.iceberg_oauth2_server_uri:
                props["oauth2-server-uri"] = s.iceberg_oauth2_server_uri
            if s.iceberg_scope:
                props["scope"] = s.iceberg_scope
        elif s.iceberg_token or s.api_key:
            props["token"] = str(s.iceberg_token or s.api_key)
        if s.s3_endpoint:
            props["s3.endpoint"] = s.s3_endpoint
        if s.s3_access_key_id:
            props["s3.access-key-id"] = s.s3_access_key_id
            props["s3.secret-access-key"] = s.s3_secret_access_key or ""
        else:
            props["header.X-Iceberg-Access-Delegation"] = "vended-credentials"
        if s.s3_region:
            props["s3.region"] = s.s3_region
        props.update({str(k): str(v) for k, v in s.iceberg_properties.items()})
        return props

    def get_catalog(self) -> Catalog:
        try:
            from pyiceberg.catalog.rest import RestCatalog
        except ImportError as exc:  # pragma: no cover
            raise ImportError(
                "Loams Iceberg needs pyiceberg: pip install 'airflow-provider-loams[iceberg]'"
            ) from exc
        return RestCatalog("loams", **self.catalog_properties())

    def get_conn(self) -> Catalog:  # type: ignore[override]
        return self.get_catalog()

    # -- namespaces and tables ----------------------------------------------

    def list_namespaces(self) -> list[str]:
        return [".".join(ns) for ns in self.get_catalog().list_namespaces()]

    def create_namespace(
        self, namespace: str, properties: Mapping[str, str] | None = None, *, exist_ok: bool = True
    ) -> None:
        catalog = self.get_catalog()
        if exist_ok:
            catalog.create_namespace_if_not_exists(namespace, dict(properties or {}))
        else:
            catalog.create_namespace(namespace, dict(properties or {}))

    def list_tables(self, namespace: str) -> list[str]:
        return [".".join(t) for t in self.get_catalog().list_tables(namespace)]

    def table_exists(self, table: str | Sequence[str]) -> bool:
        return bool(self.get_catalog().table_exists(_identifier(table)))

    def load_table(self, table: str | Sequence[str]) -> Table:
        return self.get_catalog().load_table(_identifier(table))

    def current_snapshot(self, table: str | Sequence[str]) -> dict[str, Any] | None:
        """The table's current snapshot (id, time, operation summary), or None if it has none."""
        snapshot = self.load_table(table).current_snapshot()
        if snapshot is None:
            return None
        summary = snapshot.summary
        return {
            "snapshot_id": int(snapshot.snapshot_id),
            "parent_snapshot_id": snapshot.parent_snapshot_id,
            "sequence_number": snapshot.sequence_number,
            "timestamp_ms": int(snapshot.timestamp_ms),
            "operation": str(summary.operation.value) if summary is not None else None,
            "summary": {str(k): str(summary[k]) for k in summary} if summary is not None else {},
        }

    # -- data ----------------------------------------------------------------

    def append_rows(self, table: str | Sequence[str], rows: Sequence[Mapping[str, Any]]) -> int | None:
        """Appends rows (dicts keyed by column) as one commit; answers the new snapshot id."""
        import pyarrow as pa

        if not rows:
            return None
        tbl = self.load_table(table)
        data = pa.Table.from_pylist([dict(r) for r in rows], schema=tbl.schema().as_arrow())
        tbl.append(data)
        snapshot = tbl.refresh().current_snapshot()
        return int(snapshot.snapshot_id) if snapshot is not None else None

    def overwrite_rows(
        self, table: str | Sequence[str], rows: Sequence[Mapping[str, Any]], *, overwrite_filter: str = ""
    ) -> int | None:
        """Replaces the rows matching `overwrite_filter` (all rows when empty) with `rows`."""
        import pyarrow as pa

        tbl = self.load_table(table)
        data = pa.Table.from_pylist([dict(r) for r in rows], schema=tbl.schema().as_arrow())
        if overwrite_filter:
            tbl.overwrite(data, overwrite_filter=overwrite_filter)
        else:
            tbl.overwrite(data)
        snapshot = tbl.refresh().current_snapshot()
        return int(snapshot.snapshot_id) if snapshot is not None else None

    def scan_rows(
        self,
        table: str | Sequence[str],
        *,
        row_filter: str = "",
        selected_fields: Sequence[str] = ("*",),
        limit: int | None = None,
        snapshot_id: int | None = None,
    ) -> list[dict[str, Any]]:
        """Reads rows as dicts (keep `limit` small when the result goes to XCom)."""
        kwargs: dict[str, Any] = {"selected_fields": tuple(selected_fields)}
        if row_filter:
            kwargs["row_filter"] = row_filter
        if limit is not None:
            kwargs["limit"] = int(limit)
        if snapshot_id is not None:
            kwargs["snapshot_id"] = int(snapshot_id)
        return self.load_table(table).scan(**kwargs).to_arrow().to_pylist()

    def test_connection(self) -> tuple[bool, str]:
        try:
            namespaces = self.list_namespaces()
            return True, f"Catalog answered with {len(namespaces)} namespaces"
        except Exception as exc:
            return False, str(exc)
