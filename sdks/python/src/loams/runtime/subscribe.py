"""`streams.subscribe` (runtime contract R14, SDKP §2.5 and SDKP-2).

    sub = loams.streams.subscribe("events", partition=0, from_offset=1)
    for record in sub:
        handle(record)
        if done:
            break                    # or sub.cancel()
    resume_later(sub.offset)         # the next offset to read

A client loop over unary `Fetch` rather than a server stream, so it works on
every transport (Connect-unary included) and through every proxy:

- The next offset is the last record's offset + 1.
- An empty fetch backs off 100 ms, doubling, up to 1 s, and the backoff resets
  when a record arrives. With `max_wait_ms` the server long-polls instead, and
  the backoff only applies when it answers empty at its deadline.
- `offset` is always the next offset to read, so a caller can stop and resume
  without losing or repeating a record.
- `cancel()`, or leaving a `for` loop and dropping the iterator, stops the loop;
  no further fetch is sent.

A fetch that fails is retried by the call path as any `safe` call is; a failure
that outlasts the retries ends the iteration with that error, and `offset`
still says where to resume.
"""

from __future__ import annotations

import asyncio
import threading
from collections import deque
from collections.abc import AsyncIterator, Awaitable, Callable, Iterator
from dataclasses import dataclass
from typing import Any, Final

__all__ = [
    "SUBSCRIBE_MAX_BACKOFF",
    "SUBSCRIBE_MIN_BACKOFF",
    "AsyncSubscription",
    "SubscribeRequest",
    "Subscription",
]

#: The first and the largest pause after an empty fetch, in seconds.
SUBSCRIBE_MIN_BACKOFF: Final[float] = 0.1
SUBSCRIBE_MAX_BACKOFF: Final[float] = 1.0


@dataclass(frozen=True, slots=True)
class SubscribeRequest:
    """Where a subscription starts and how much each fetch asks for."""

    stream: str
    partition: int = 0
    #: The first offset to read.
    from_offset: int = 0
    namespace: str | None = None
    max_records: int | None = None
    max_bytes: int | None = None
    #: A server long poll per fetch, when the server offers it (P0c).
    max_wait_ms: int | None = None

    def fetch_fields(self, offset: int) -> dict[str, Any]:
        """The `Fetch` request's fields at `offset`."""
        fields: dict[str, Any] = {
            "stream": self.stream,
            "partition": self.partition,
            "offset": offset,
        }
        for name in ("namespace", "max_records", "max_bytes", "max_wait_ms"):
            value = getattr(self, name)
            if value is not None:
                fields[name] = value
        return fields


def _records(answer: object) -> list[Any]:
    return list(getattr(answer, "records", None) or [])


def _offset_of(record: Any) -> int:
    return int(record.offset)


class Subscription(Iterator[Any]):
    """A running subscription: an iterator that knows where it is."""

    def __init__(
        self,
        fetch: Callable[[dict[str, Any]], object],
        request: SubscribeRequest,
        *,
        sleep: Callable[[float], None] | None = None,
    ) -> None:
        self._fetch = fetch
        self._request = request
        self._offset = request.from_offset
        self._buffer: deque[Any] = deque()
        self._backoff = SUBSCRIBE_MIN_BACKOFF
        self._cancelled = threading.Event()
        self._sleep = sleep

    @property
    def offset(self) -> int:
        """The next offset to read."""
        return self._offset

    def cancel(self) -> None:
        """Stops the loop; a fetch already in flight is the last one."""
        self._cancelled.set()

    def __iter__(self) -> Subscription:
        return self

    def __next__(self) -> Any:
        while not self._cancelled.is_set():
            if self._buffer:
                record = self._buffer.popleft()
                self._offset = _offset_of(record) + 1
                return record
            answer = self._fetch(self._request.fetch_fields(self._offset))
            if self._cancelled.is_set():
                break
            # A record below the next offset is one already yielded; skipping it
            # keeps resume exact.
            fresh = [r for r in _records(answer) if _offset_of(r) >= self._offset]
            if fresh:
                self._buffer.extend(fresh)
                self._backoff = SUBSCRIBE_MIN_BACKOFF
                continue
            if self._sleep is not None:
                self._sleep(self._backoff)
            else:
                self._cancelled.wait(self._backoff)
            self._backoff = min(self._backoff * 2, SUBSCRIBE_MAX_BACKOFF)
        raise StopIteration


class AsyncSubscription(AsyncIterator[Any]):
    """`Subscription` for `AsyncLoams`: `async for record in sub`."""

    def __init__(
        self,
        fetch: Callable[[dict[str, Any]], Awaitable[object]],
        request: SubscribeRequest,
    ) -> None:
        self._fetch = fetch
        self._request = request
        self._offset = request.from_offset
        self._buffer: deque[Any] = deque()
        self._backoff = SUBSCRIBE_MIN_BACKOFF
        self._cancelled = False
        self._wake: asyncio.Event | None = None

    @property
    def offset(self) -> int:
        """The next offset to read."""
        return self._offset

    def cancel(self) -> None:
        """Stops the loop, waking it if it is backing off."""
        self._cancelled = True
        if self._wake is not None:
            self._wake.set()

    def __aiter__(self) -> AsyncSubscription:
        return self

    async def __anext__(self) -> Any:
        while not self._cancelled:
            if self._buffer:
                record = self._buffer.popleft()
                self._offset = _offset_of(record) + 1
                return record
            answer = await self._fetch(self._request.fetch_fields(self._offset))
            if self._cancelled:
                break
            fresh = [r for r in _records(answer) if _offset_of(r) >= self._offset]
            if fresh:
                self._buffer.extend(fresh)
                self._backoff = SUBSCRIBE_MIN_BACKOFF
                continue
            self._wake = asyncio.Event()
            try:
                await asyncio.wait_for(self._wake.wait(), self._backoff)
            except asyncio.TimeoutError:
                pass
            self._backoff = min(self._backoff * 2, SUBSCRIBE_MAX_BACKOFF)
        raise StopAsyncIteration

    async def aclose(self) -> None:
        """Stops the loop, as leaving `async for` does."""
        self.cancel()
