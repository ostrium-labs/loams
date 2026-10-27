"""`documents.write_batch` (runtime contract R13, SDKP §2.3; design §44 §7.5).

A bulk load as a loop of ordinary `WriteDocuments` calls, chunked by rows and by
bytes:

- **One chunk in flight at a time.** Order is preserved and a failure stops the
  load at a known point, which concurrency would blur.
- **One idempotency key per chunk**, minted before the chunk's first attempt and
  reused on every retry of that chunk, so a retried chunk is the same write
  (D610) and two chunks are never mistaken for one.
- **The tokens merge.** The result carries the merge of every chunk's
  consistency token, so a read held to it sees the whole load.
- **The first failed chunk stops the load** with a `BatchError` saying how many
  chunks (and operations) were applied before it, and carrying their merged
  token. Those chunks stay applied; nothing is rolled back.
"""

from __future__ import annotations

import json
from collections.abc import AsyncIterable, Awaitable, Callable, Iterable
from dataclasses import dataclass
from typing import Any, Final, TypeVar

from connectrpc.code import Code

from loams.runtime.consistency import ConsistencyToken
from loams.runtime.errors import BatchError, InvalidArgumentError, to_loams_error
from loams.runtime.uuidv7 import uuidv7

__all__ = [
    "DEFAULT_CHUNK_BYTES",
    "DEFAULT_CHUNK_ROWS",
    "BatchResult",
    "async_write_batch",
    "write_batch",
]

#: The default chunk bounds (§44 §7.5).
DEFAULT_CHUNK_ROWS: Final[int] = 10_000
DEFAULT_CHUNK_BYTES: Final[int] = 4 * 1024 * 1024

OP = TypeVar("OP")


@dataclass(frozen=True, slots=True)
class BatchResult:
    """What a finished batch reports."""

    #: The merge of every chunk's consistency token.
    token: str | None
    #: How many chunks were written.
    chunks: int
    #: How many operations those chunks carried.
    rows: int


def _size_of(op: Any) -> int:
    return len(json.dumps(op, separators=(",", ":"), default=str).encode("utf-8"))


def _merge(current: ConsistencyToken | None, answer: object) -> ConsistencyToken | None:
    token = getattr(answer, "token", None)
    if not isinstance(token, str) or token == "":
        return current
    try:
        parsed = ConsistencyToken.parse(token)
    except InvalidArgumentError:
        return current
    return parsed.normalized() if current is None else current.merge(parsed)


def _check(chunk_rows: int, chunk_bytes: int) -> None:
    if chunk_rows <= 0 or chunk_bytes <= 0:
        raise ValueError("write_batch: chunk_rows and chunk_bytes must be positive")


class _Chunker:
    """Splits operations into chunks by rows and bytes."""

    def __init__(self, chunk_rows: int, chunk_bytes: int) -> None:
        self._rows = chunk_rows
        self._bytes = chunk_bytes
        self._pending: list[Any] = []
        self._pending_bytes = 0

    def add(self, op: Any) -> list[Any] | None:
        """Adds `op`; answers the chunk to send first, if `op` does not fit."""
        size = _size_of(op)
        ready: list[Any] | None = None
        if self._pending and (
            len(self._pending) >= self._rows or self._pending_bytes + size > self._bytes
        ):
            ready = self.take()
        self._pending.append(op)
        self._pending_bytes += size
        return ready

    def take(self) -> list[Any]:
        chunk, self._pending, self._pending_bytes = self._pending, [], 0
        return chunk


def _failed(cause: BaseException, chunks: int, rows: int, token: ConsistencyToken | None) -> BatchError:
    error = to_loams_error(cause)
    metadata = {"chunks_applied": str(chunks), "rows_applied": str(rows)}
    if error.reason is not None:
        metadata["cause_reason"] = error.reason
    batch = BatchError(
        f"write_batch stopped at chunk {chunks + 1}: {chunks} chunks ({rows} operations) "
        f"were applied before it; {error}",
        chunks_applied=chunks,
        rows_applied=rows,
        token=None if token is None else str(token),
        code=Code.ABORTED if error.code == Code.UNKNOWN else error.code,
        metadata=metadata,
        rpc=error.rpc,
    )
    batch.__cause__ = error
    return batch


def write_batch(
    write: Callable[[list[OP], str], object],
    ops: Iterable[OP],
    *,
    chunk_rows: int = DEFAULT_CHUNK_ROWS,
    chunk_bytes: int = DEFAULT_CHUNK_BYTES,
) -> BatchResult:
    """Splits `ops` into chunks and writes them one after another.

    :param write: sends one chunk with its own idempotency key (the second
        argument) and answers the response, whose `token` is merged.
    :raises BatchError: at the first chunk that fails.
    """
    _check(chunk_rows, chunk_bytes)
    chunker = _Chunker(chunk_rows, chunk_bytes)
    token: ConsistencyToken | None = None
    chunks = rows = 0

    def flush(chunk: list[OP]) -> None:
        nonlocal token, chunks, rows
        if not chunk:
            return
        try:
            # The key is minted here, once per chunk; the call path reuses it on
            # every retry of this chunk.
            answer = write(chunk, uuidv7())
        except Exception as cause:
            raise _failed(cause, chunks, rows, token) from cause
        token = _merge(token, answer)
        chunks += 1
        rows += len(chunk)

    for op in ops:
        ready = chunker.add(op)
        if ready is not None:
            flush(ready)
    flush(chunker.take())
    return BatchResult(None if token is None else str(token), chunks, rows)


async def async_write_batch(
    write: Callable[[list[OP], str], Awaitable[object]],
    ops: Iterable[OP] | AsyncIterable[OP],
    *,
    chunk_rows: int = DEFAULT_CHUNK_ROWS,
    chunk_bytes: int = DEFAULT_CHUNK_BYTES,
) -> BatchResult:
    """`write_batch` for `AsyncLoams`; `ops` may be an async iterable."""
    _check(chunk_rows, chunk_bytes)
    chunker = _Chunker(chunk_rows, chunk_bytes)
    token: ConsistencyToken | None = None
    chunks = rows = 0

    async def flush(chunk: list[OP]) -> None:
        nonlocal token, chunks, rows
        if not chunk:
            return
        try:
            answer = await write(chunk, uuidv7())
        except Exception as cause:
            raise _failed(cause, chunks, rows, token) from cause
        token = _merge(token, answer)
        chunks += 1
        rows += len(chunk)

    if isinstance(ops, AsyncIterable):
        async for op in ops:
            ready = chunker.add(op)
            if ready is not None:
                await flush(ready)
    else:
        for op in ops:
            ready = chunker.add(op)
            if ready is not None:
                await flush(ready)
    await flush(chunker.take())
    return BatchResult(None if token is None else str(token), chunks, rows)
