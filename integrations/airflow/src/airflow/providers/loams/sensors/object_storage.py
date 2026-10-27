# SPDX-License-Identifier: Apache-2.0
"""`LoamsObjectKeySensor`: waits for objects in Loams object storage."""

from __future__ import annotations

import fnmatch
from collections.abc import Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseSensorOperator, PokeReturnValue

from airflow.providers.loams.hooks.object_storage import LoamsObjectStorageHook

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsObjectKeySensor"]


class LoamsObjectKeySensor(BaseSensorOperator):
    """Succeeds when every key exists (or, with `wildcard`, each pattern matches one).

    :param keys: a key or keys (templated). With `wildcard`, shell patterns
        (`exports/2026-10-27/*.parquet`), listed under their fixed prefix.
    :param bucket: the bucket; the connection's `s3_bucket` by default.
    :return: (XCom) the matching keys.
    """

    template_fields: Sequence[str] = ("keys", "bucket")

    def __init__(
        self,
        *,
        keys: str | Sequence[str],
        bucket: str | None = None,
        wildcard: bool = False,
        loams_conn_id: str = LoamsObjectStorageHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.keys = keys
        self.bucket = bucket
        self.wildcard = wildcard
        self.loams_conn_id = loams_conn_id

    def poke(self, context: Context) -> PokeReturnValue:
        hook = LoamsObjectStorageHook(loams_conn_id=self.loams_conn_id, bucket=self.bucket)
        patterns = [self.keys] if isinstance(self.keys, str) else list(self.keys)
        found: list[str] = []
        for pattern in patterns:
            if not self.wildcard:
                if not hook.exists(pattern):
                    self.log.info("Waiting for %s", pattern)
                    return PokeReturnValue(is_done=False)
                found.append(pattern)
                continue
            prefix = _fixed_prefix(pattern)
            matched = [k for k in hook.list_keys(prefix) if fnmatch.fnmatchcase(k, pattern)]
            if not matched:
                self.log.info("Waiting for a key matching %s", pattern)
                return PokeReturnValue(is_done=False)
            found.extend(matched)
        return PokeReturnValue(is_done=True, xcom_value=found)


def _fixed_prefix(pattern: str) -> str:
    """The part of a shell pattern before its first wildcard."""
    cut = len(pattern)
    for char in "*?[":
        index = pattern.find(char)
        if index != -1:
            cut = min(cut, index)
    return pattern[:cut]
