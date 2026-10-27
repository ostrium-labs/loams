# SPDX-License-Identifier: Apache-2.0
"""`LoamsObjectStorageHook`: Loams object storage, S3-compatible.

Loams keeps data in the user's bucket: RustFS in the default stack
(`http://rustfs:9000` inside compose), or a cloud bucket in a BYOC install.
Both speak S3, so the hook is boto3 with the endpoint, keys and region from
the `loams` connection's extras (`s3_endpoint`, `s3_access_key_id`,
`s3_secret_access_key`, `s3_region`, `s3_bucket`, `s3_path_style`). Path-style
addressing is on by default, which RustFS and most S3-compatible stores need.

`s3fs_options()` is the same configuration for fsspec, which the
`loams-s3://` ObjectStoragePath scheme (`fs/loams_s3.py`) uses.
"""

from __future__ import annotations

from collections.abc import Iterable, Iterator
from typing import TYPE_CHECKING, Any

from airflow.providers.loams.hooks.base import LoamsHook

if TYPE_CHECKING:
    from mypy_boto3_s3 import S3Client  # pragma: no cover

__all__ = ["LoamsObjectStorageHook", "parse_object_url"]


def parse_object_url(url: str) -> tuple[str | None, str]:
    """`s3://bucket/key`, `loams-s3://bucket/key` or a bare key -> (bucket, key)."""
    for prefix in ("loams-s3://", "s3://", "s3a://"):
        if url.startswith(prefix):
            rest = url[len(prefix) :]
            if "@" in rest.split("/", 1)[0]:
                rest = rest.split("@", 1)[1]  # an ObjectStoragePath's conn_id@bucket
            bucket, _, key = rest.partition("/")
            return bucket, key
    return None, url.lstrip("/")


class LoamsObjectStorageHook(LoamsHook):
    """S3-compatible object storage of a Loams install.

    :param loams_conn_id: the `loams` connection.
    :param bucket: the default bucket; the connection's extra `s3_bucket` by default.
    """

    hook_name = "Loams Object Storage"

    def __init__(
        self, loams_conn_id: str = LoamsHook.default_conn_name, *, bucket: str | None = None, **kwargs: Any
    ) -> None:
        super().__init__(loams_conn_id=loams_conn_id, **kwargs)
        self._bucket = bucket

    def bucket_for(self, bucket: str | None) -> str:
        name = bucket or self._bucket or self.settings.s3_bucket
        if not name:
            raise ValueError("no bucket: pass bucket= or set the connection's extra `s3_bucket`")
        return name

    def _client_kwargs(self) -> dict[str, Any]:
        s = self.settings
        if not s.s3_endpoint:
            raise ValueError("the Loams connection has no object storage: set its extra `s3_endpoint`")
        return {
            "endpoint_url": s.s3_endpoint,
            "aws_access_key_id": s.s3_access_key_id,
            "aws_secret_access_key": s.s3_secret_access_key,
            "region_name": s.s3_region or "us-east-1",
        }

    def get_client(self) -> S3Client:
        try:
            import boto3
            from botocore.config import Config
        except ImportError as exc:  # pragma: no cover
            raise ImportError(
                "Loams object storage needs boto3: pip install 'airflow-provider-loams[s3]'"
            ) from exc
        s = self.settings
        config = Config(
            signature_version="s3v4",
            s3={"addressing_style": "path" if s.s3_path_style else "auto"},
            retries={"max_attempts": 5, "mode": "standard"},
        )
        return boto3.client("s3", config=config, **self._client_kwargs())

    def get_conn(self) -> S3Client:  # type: ignore[override]
        return self.get_client()

    def s3fs_options(self) -> dict[str, Any]:
        """`s3fs.S3FileSystem` keyword arguments for this connection."""
        kwargs = self._client_kwargs()
        s = self.settings
        return {
            "key": kwargs["aws_access_key_id"],
            "secret": kwargs["aws_secret_access_key"],
            "endpoint_url": kwargs["endpoint_url"],
            "client_kwargs": {"region_name": kwargs["region_name"]},
            "config_kwargs": {
                "signature_version": "s3v4",
                "s3": {"addressing_style": "path" if s.s3_path_style else "auto"},
            },
        }

    # -- objects -------------------------------------------------------------

    def put_bytes(
        self, data: bytes | str, key: str, *, bucket: str | None = None, content_type: str | None = None
    ) -> str:
        body = data.encode() if isinstance(data, str) else data
        extra: dict[str, Any] = {"ContentType": content_type} if content_type else {}
        name = self.bucket_for(bucket)
        self.get_client().put_object(Bucket=name, Key=key, Body=body, **extra)
        return f"s3://{name}/{key}"

    def upload_file(self, filename: str, key: str, *, bucket: str | None = None) -> str:
        name = self.bucket_for(bucket)
        self.get_client().upload_file(filename, name, key)
        return f"s3://{name}/{key}"

    def get_bytes(self, key: str, *, bucket: str | None = None) -> bytes:
        response = self.get_client().get_object(Bucket=self.bucket_for(bucket), Key=key)
        return response["Body"].read()

    def download_file(self, key: str, filename: str, *, bucket: str | None = None) -> str:
        self.get_client().download_file(self.bucket_for(bucket), key, filename)
        return filename

    def iter_keys(self, prefix: str = "", *, bucket: str | None = None) -> Iterator[dict[str, Any]]:
        paginator = self.get_client().get_paginator("list_objects_v2")
        for page in paginator.paginate(Bucket=self.bucket_for(bucket), Prefix=prefix):
            for obj in page.get("Contents", []) or []:
                yield {
                    "key": obj["Key"],
                    "size": int(obj.get("Size", 0)),
                    "etag": str(obj.get("ETag", "")).strip('"'),
                    "last_modified": obj["LastModified"].isoformat() if obj.get("LastModified") else None,
                }

    def list_keys(self, prefix: str = "", *, bucket: str | None = None) -> list[str]:
        return [o["key"] for o in self.iter_keys(prefix, bucket=bucket)]

    def head(self, key: str, *, bucket: str | None = None) -> dict[str, Any] | None:
        """The object's metadata, or None when there is no such object."""
        from botocore.exceptions import ClientError

        try:
            return dict(self.get_client().head_object(Bucket=self.bucket_for(bucket), Key=key))
        except ClientError as exc:
            status = exc.response.get("ResponseMetadata", {}).get("HTTPStatusCode")
            code = exc.response.get("Error", {}).get("Code")
            if status == 404 or code in ("404", "NoSuchKey", "NotFound"):
                return None
            raise

    def exists(self, key: str, *, bucket: str | None = None) -> bool:
        return self.head(key, bucket=bucket) is not None

    def delete_keys(self, keys: Iterable[str], *, bucket: str | None = None) -> int:
        """Deletes keys, a thousand per request; answers how many were asked for."""
        name = self.bucket_for(bucket)
        client = self.get_client()
        batch: list[dict[str, str]] = []
        count = 0
        for key in keys:
            batch.append({"Key": key})
            if len(batch) == 1000:
                count += self._delete_batch(client, name, batch)
                batch = []
        if batch:
            count += self._delete_batch(client, name, batch)
        return count

    @staticmethod
    def _delete_batch(client: Any, bucket: str, batch: list[dict[str, str]]) -> int:
        response = client.delete_objects(Bucket=bucket, Delete={"Objects": batch, "Quiet": True})
        errors = response.get("Errors") or []
        if errors:
            first = errors[0]
            raise RuntimeError(
                f"deleting {len(errors)} objects from {bucket} failed: "
                f"{first.get('Key')}: {first.get('Message')}"
            )
        return len(batch)

    def copy(
        self, source_key: str, dest_key: str, *, bucket: str | None = None, dest_bucket: str | None = None
    ) -> str:
        src = self.bucket_for(bucket)
        dst = dest_bucket or src
        self.get_client().copy_object(Bucket=dst, Key=dest_key, CopySource={"Bucket": src, "Key": source_key})
        return f"s3://{dst}/{dest_key}"

    def test_connection(self) -> tuple[bool, str]:
        try:
            buckets = self.get_client().list_buckets().get("Buckets", [])
            return True, f"Object storage answered with {len(buckets)} buckets"
        except Exception as exc:
            return False, str(exc)
