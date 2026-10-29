# SPDX-License-Identifier: Apache-2.0
"""The `loams://` asset URI scheme (design 68 §4.4).

An asset on Loams is `loams://<namespace>/<object>`. The object is a path
whose first segment says what it is, so two tasks that mean the same thing
write the same URI:

| URI | What |
|---|---|
| `loams://acme/streams/orders` | a stream (topic) |
| `loams://acme/tables/sales.orders` | an Iceberg table, `<namespace>.<table>` in the catalog |
| `loams://acme/house/default.events` | a House table, `<database>.<table>` |
| `loams://acme/postgres/prj-01J.../public.orders` | a Loams Postgres table, `<project>/<schema>.<table>` |
| `loams://acme/graphs/social` | a graph |
| `loams://acme/objects/bucket/path/to/key` | an object or prefix in the bucket |

Kinds other than these are allowed (the scheme is `loams://<namespace>/<object>`),
so a later Loams object needs no provider release; the handler only
normalises. It lower-cases the scheme and namespace, drops a trailing slash,
an empty query and the fragment, and refuses a URI without a namespace or an
object, or with a port, user or password.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any
from urllib.parse import SplitResult

if TYPE_CHECKING:
    from airflow.sdk import Asset

__all__ = ["KINDS", "create_asset", "loams_uri", "sanitize_uri"]

SCHEME = "loams"
KINDS = ("streams", "tables", "house", "postgres", "graphs", "objects")


def sanitize_uri(uri: SplitResult) -> SplitResult:
    if uri.scheme.lower() != SCHEME:
        raise ValueError(f"not a loams:// URI: {uri.geturl()}")
    if uri.username or uri.password:
        raise ValueError("a loams:// URI carries no credentials")
    if ":" in uri.netloc:
        raise ValueError("a loams:// URI names a namespace, not a host and port")
    namespace = uri.netloc.lower()
    if not namespace:
        raise ValueError("a loams:// URI must name a namespace: loams://<namespace>/<object>")
    path = "/" + "/".join(part for part in uri.path.split("/") if part)
    if path == "/":
        raise ValueError("a loams:// URI must name an object: loams://<namespace>/<object>")
    return SplitResult(scheme=SCHEME, netloc=namespace, path=path, query=uri.query, fragment="")


def loams_uri(namespace: str, kind: str, name: str) -> str:
    """`loams://<namespace>/<kind>/<name>`."""
    if not namespace or not kind or not name:
        raise ValueError("namespace, kind and name are all required")
    return f"{SCHEME}://{namespace.lower()}/{kind.strip('/')}/{name.strip('/')}"


def create_asset(*, namespace: str, kind: str, name: str, extra: dict[str, Any] | None = None) -> Asset:
    """The asset for one Loams object (the provider's asset factory)."""
    from airflow.sdk import Asset

    return Asset(uri=loams_uri(namespace, kind, name), extra=extra or {})
