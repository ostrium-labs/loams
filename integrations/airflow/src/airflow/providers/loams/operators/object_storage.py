# SPDX-License-Identifier: Apache-2.0
"""Operators on Loams object storage (S3-compatible).

For reading and writing files from task code, `ObjectStoragePath` with the
`loams-s3://` scheme is usually simpler; these operators cover the common
one-step jobs.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseOperator

from airflow.providers.loams.hooks.object_storage import LoamsObjectStorageHook

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = [
    "LoamsObjectCopyOperator",
    "LoamsObjectDeleteOperator",
    "LoamsObjectListOperator",
    "LoamsObjectUploadOperator",
]


class LoamsObjectUploadOperator(BaseOperator):
    """Writes a local file or some data to an object.

    :param key: the object's key (templated).
    :param filename: a local file to upload, or
    :param data: text or bytes to write (templated).
    :param bucket: the bucket; the connection's `s3_bucket` by default.
    :param content_type: the object's content type, for `data`.
    :return: the object's `s3://bucket/key` URL.
    """

    template_fields: Sequence[str] = ("key", "filename", "data", "bucket")
    ui_color = "#e2c9a6"

    def __init__(
        self,
        *,
        key: str,
        filename: str | None = None,
        data: str | bytes | None = None,
        bucket: str | None = None,
        content_type: str | None = None,
        loams_conn_id: str = LoamsObjectStorageHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if (filename is None) == (data is None):
            raise ValueError("pass exactly one of filename and data")
        self.key = key
        self.filename = filename
        self.data = data
        self.bucket = bucket
        self.content_type = content_type
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> str:
        hook = LoamsObjectStorageHook(loams_conn_id=self.loams_conn_id, bucket=self.bucket)
        if self.filename is not None:
            url = hook.upload_file(self.filename, self.key)
        else:
            url = hook.put_bytes(self.data or b"", self.key, content_type=self.content_type)
        self.log.info("Wrote %s", url)
        return url


class LoamsObjectListOperator(BaseOperator):
    """Lists the keys under a prefix (returns them, so keep prefixes narrow)."""

    template_fields: Sequence[str] = ("prefix", "bucket")

    def __init__(
        self,
        *,
        prefix: str = "",
        bucket: str | None = None,
        loams_conn_id: str = LoamsObjectStorageHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.prefix = prefix
        self.bucket = bucket
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> list[str]:
        hook = LoamsObjectStorageHook(loams_conn_id=self.loams_conn_id, bucket=self.bucket)
        return hook.list_keys(self.prefix)


class LoamsObjectDeleteOperator(BaseOperator):
    """Deletes objects: the keys given, or everything under a prefix.

    :param keys: keys to delete (templated).
    :param prefix: delete every key under this prefix instead. An empty
        prefix is refused, so a typo cannot empty a bucket.
    """

    template_fields: Sequence[str] = ("keys", "prefix", "bucket")

    def __init__(
        self,
        *,
        keys: Sequence[str] | str | None = None,
        prefix: str | None = None,
        bucket: str | None = None,
        loams_conn_id: str = LoamsObjectStorageHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        if (keys is None) == (prefix is None):
            raise ValueError("pass exactly one of keys and prefix")
        if prefix is not None and not prefix.strip("/"):
            raise ValueError("an empty prefix would delete the whole bucket; name the keys instead")
        self.keys = keys
        self.prefix = prefix
        self.bucket = bucket
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> int:
        hook = LoamsObjectStorageHook(loams_conn_id=self.loams_conn_id, bucket=self.bucket)
        if self.prefix is not None:
            keys: list[str] = hook.list_keys(self.prefix)
        else:
            keys = [self.keys] if isinstance(self.keys, str) else list(self.keys or [])
        count = hook.delete_keys(keys) if keys else 0
        self.log.info("Deleted %d objects", count)
        return count


class LoamsObjectCopyOperator(BaseOperator):
    """Copies one object to another key (and optionally another bucket)."""

    template_fields: Sequence[str] = ("source_key", "dest_key", "bucket", "dest_bucket")

    def __init__(
        self,
        *,
        source_key: str,
        dest_key: str,
        bucket: str | None = None,
        dest_bucket: str | None = None,
        loams_conn_id: str = LoamsObjectStorageHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.source_key = source_key
        self.dest_key = dest_key
        self.bucket = bucket
        self.dest_bucket = dest_bucket
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> str:
        hook = LoamsObjectStorageHook(loams_conn_id=self.loams_conn_id, bucket=self.bucket)
        return hook.copy(self.source_key, self.dest_key, dest_bucket=self.dest_bucket)
