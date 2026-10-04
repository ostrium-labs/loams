"""Server streams (design §44 §7.4, D610; runtime contract R7).

The API has server streams only (D420): no client streaming, no bidi, because a
browser cannot do it over fetch and half-duplex works through every proxy. In
Python a server stream is an **iterator** for `Loams` and an **async iterator**
for `AsyncLoams`, which is what `for ... in loams.live.watch(...)` and
`async for ... in` want respectively (design §44 §7.1).

A stream is the one call where "retry it" is not just "send it again": the server
hands out cursors, and a reconnect has to resume from the last one or the client
silently misses everything that changed in between. So `watch` tracks the cursor
of every message and, on a retryable failure, re-opens the stream from it. The
request that resumes is the stream's own business — `loams.live`'s
`WatchRequest.resume`, for instance — so the caller passes a `resume` function
and the runtime supplies the cursor.

**The only server stream (`LiveService/Watch`) is never served in any variant**,
so `python_stream_resume_with_cursor` drives a stub: a fixture for an RPC the
server refuses would test the refusal, not the resume.
"""

from __future__ import annotations

from collections.abc import AsyncIterator, Callable, Iterator
from dataclasses import dataclass
from typing import Generic, TypeVar

from google.protobuf.message import Message

from loams.gen.facade import CallBinding
from loams.runtime.errors import LoamsError, to_loams_error
from loams.runtime.options import CallOptions
from loams.runtime.retry import DEFAULT_MAX_RETRIES, backoff_seconds, should_retry, sleep

__all__ = ["CursorOf", "ResumeOptions", "async_watch", "cursor_field", "watch"]

MSG = TypeVar("MSG", bound=Message)
REQ = TypeVar("REQ", bound=Message)

#: The request-message field `loams.live.v1.WatchRequest` resumes from. Read from
#: the descriptor rather than hard-coded per call, so a second stream gets it too.
RESUME_FIELD = "resume"


def cursor_field(message: Message) -> str | None:
    """The cursor a stream message carries, when it carries one (R7).

    The default reader: a `cursor` field, which is what every stream in the API
    uses (§44 §7.4).
    """
    value = getattr(message, "cursor", None)
    return value if isinstance(value, str) and value != "" else None


@dataclass(frozen=True, slots=True)
class ResumeOptions(Generic[MSG, REQ]):
    """How a stream re-opens from a cursor."""

    #: The request to re-open with, given the last cursor seen. Returning the
    #: same request re-connects from the beginning, which is correct — and loses
    #: nothing but time — for a stream whose snapshot is complete.
    resume: Callable[[str | None, REQ], REQ]
    #: Reads the cursor off a message. Defaults to a `cursor` field.
    cursor: Callable[[MSG], str | None] | None = None
    #: Retries after the first failure. Defaults to the client's.
    max_retries: int | None = None
    #: Called after each message, with the cursor it carried.
    on_cursor: Callable[[str | None, MSG], None] | None = None
    #: Overrides the call's retry class for this stream only.
    retry_safe: bool | None = None


def _decide(
    error: LoamsError, retry_safe: bool, attempt: int, max_retries: int
) -> bool:
    return should_retry(error.code, retry_safe, attempt, max_retries)


def watch(
    binding: CallBinding,
    open_stream: Callable[[REQ, CallOptions | None], Iterator[MSG]],
    request: REQ,
    options: CallOptions | None = None,
    resume_options: ResumeOptions[MSG, REQ] | None = None,
) -> Iterator[MSG]:
    """A server stream that reconnects from its cursor.

    Yields until the stream ends; if it fails with a code a retry may answer and
    the call is retryable, it re-opens from the last cursor and carries on, so a
    node restart is a hiccup rather than a gap.

    :raises LoamsError: a failure the retry class does not cover — notably an
        `unimplemented` stream — which is reported rather than spun on.
    """
    resume = _resume_of(resume_options, request)
    cursor_of = resume_options.cursor if resume_options is not None else None
    cursor_of = cursor_of or cursor_field
    max_retries = _max_retries_of(resume_options)
    retry_safe = _retry_safe_of(resume_options)
    current = request
    cursor: str | None = None
    attempt = 0
    while True:
        try:
            for message in open_stream(current, options):
                # Progress earns a fresh budget: `max_retries` bounds the retries
                # in one *run* of disconnects, not for the life of the stream. A
                # watch that recovers from a node restart and then runs for days
                # must not spend its budget on the first failure of each of those
                # days.
                attempt = 0
                cursor = cursor_of(message) or cursor
                if resume_options is not None and resume_options.on_cursor is not None:
                    resume_options.on_cursor(cursor, message)
                yield message
            return
        except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
            error = to_loams_error(thrown, binding.rpc)
            if not _decide(error, retry_safe, attempt, max_retries):
                raise error from None
            current = resume(cursor, request)
            sleep(backoff_seconds(attempt))
            attempt += 1


async def async_watch(
    binding: CallBinding,
    open_stream: Callable[[REQ, CallOptions | None], AsyncIterator[MSG]],
    request: REQ,
    options: CallOptions | None = None,
    resume_options: ResumeOptions[MSG, REQ] | None = None,
) -> AsyncIterator[MSG]:
    """`watch` for `AsyncLoams`."""
    import asyncio

    resume = _resume_of(resume_options, request)
    cursor_of = resume_options.cursor if resume_options is not None else None
    cursor_of = cursor_of or cursor_field
    max_retries = _max_retries_of(resume_options)
    retry_safe = _retry_safe_of(resume_options)
    current = request
    cursor: str | None = None
    attempt = 0
    while True:
        try:
            async for message in open_stream(current, options):
                attempt = 0
                cursor = cursor_of(message) or cursor
                if resume_options is not None and resume_options.on_cursor is not None:
                    resume_options.on_cursor(cursor, message)
                yield message
            return
        except BaseException as thrown:  # noqa: BLE001 - re-raised as a LoamsError
            error = to_loams_error(thrown, binding.rpc)
            if not _decide(error, retry_safe, attempt, max_retries):
                raise error from None
            current = resume(cursor, request)
            if backoff_seconds(attempt) > 0:
                await asyncio.sleep(backoff_seconds(attempt))
            attempt += 1


def _resume_of(resume_options: ResumeOptions[MSG, REQ] | None, request: REQ) -> Callable[[str | None, REQ], REQ]:
    if resume_options is None:
        # Without a resume policy the stream re-opens from the beginning, which
        # is the honest default: it may repeat, and it does not silently skip.
        return lambda _cursor, original: original
    return resume_options.resume


def _max_retries_of(resume_options: ResumeOptions[MSG, REQ] | None) -> int:
    if resume_options is not None and resume_options.max_retries is not None:
        return resume_options.max_retries
    return DEFAULT_MAX_RETRIES


def _retry_safe_of(resume_options: ResumeOptions[MSG, REQ] | None) -> bool:
    if resume_options is not None and resume_options.retry_safe is not None:
        return resume_options.retry_safe
    # A server stream is a read: re-opening it is safe, and R7's whole point is
    # that a reconnect resumes rather than restarts.
    return True


def resume_field_of(request: Message) -> str | None:
    """The request field a stream resumes from, when its schema has one.

    Read from the descriptor rather than hard-coded per call, so a second stream
    that names its cursor differently is picked up rather than silently ignored.
    """
    if RESUME_FIELD in {field.name for field in request.DESCRIPTOR.fields}:
        return RESUME_FIELD
    return None


def apply_resume(request: Message, cursor: str | None) -> Message:
    """A copy of the request with the resume cursor set.

    A copy, because the caller's request is reused for every reconnect and
    mutating it would carry one cursor into the next request.
    """
    copy = type(request)()
    copy.CopyFrom(request)
    if cursor is not None and resume_field_of(copy) is not None:
        setattr(copy, RESUME_FIELD, cursor)
    return copy
