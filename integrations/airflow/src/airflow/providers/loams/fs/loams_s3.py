# SPDX-License-Identifier: Apache-2.0
"""The `loams-s3://` ObjectStoragePath scheme.

    from airflow.sdk import ObjectStoragePath

    base = ObjectStoragePath("loams-s3://loams_default@lake/exports/")
    (base / "orders.csv").write_text(...)

The user part names the `loams` connection (`loams_default` when it is
left out); the host is the bucket. The filesystem is s3fs against the
connection's object storage (`s3_endpoint` and keys, path-style addressing),
with `loams-s3` added to the protocols it strips, so a full URL reaches it
intact.
"""

from __future__ import annotations

import functools
from typing import TYPE_CHECKING, Any

from airflow.providers.loams.hooks.object_storage import LoamsObjectStorageHook

if TYPE_CHECKING:
    from fsspec import AbstractFileSystem

__all__ = ["get_fs", "schemes"]

schemes = ["loams-s3"]


@functools.cache
def _filesystem_class() -> type:
    try:
        from s3fs import S3FileSystem
    except ImportError as exc:  # pragma: no cover
        raise ImportError(
            "The loams-s3:// scheme needs s3fs: pip install 'airflow-provider-loams[s3]'"
        ) from exc

    class LoamsS3FileSystem(S3FileSystem):
        protocol = ("loams-s3", "s3", "s3a")

    return LoamsS3FileSystem


def get_fs(conn_id: str | None, storage_options: dict[str, Any] | None = None) -> AbstractFileSystem:
    hook = LoamsObjectStorageHook(loams_conn_id=conn_id or LoamsObjectStorageHook.default_conn_name)
    options = hook.s3fs_options()
    for key, value in (storage_options or {}).items():
        if isinstance(value, dict) and isinstance(options.get(key), dict):
            options[key] = {**options[key], **value}
        else:
            options[key] = value
    return _filesystem_class()(**options)
