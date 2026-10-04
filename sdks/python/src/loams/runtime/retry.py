"""Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).

The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
doubling, capped at 2 s, 3 retries, full jitter. Full jitter rather than
exponential backoff alone, because every client retrying at the same instant
after a node restart is how a recovering node gets knocked over again.

`RetryInfo` is *not* read: no proto in `proto/` carries it yet, so the jittered
backoff is all an SDK can do today (runtime contract R2).
"""

from __future__ import annotations

import random
import time
from typing import Literal

from connectrpc.code import Code

__all__ = [
    "BASE_DELAY_MS",
    "DEFAULT_MAX_RETRIES",
    "MAX_DELAY_MS",
    "MAX_SERVER_DELAY_MS",
    "RETRYABLE_CODES",
    "RetryClass",
    "backoff_seconds",
    "is_retryable_code",
    "should_retry",
    "sleep",
]

#: The first backoff, and the multiplier's base.
BASE_DELAY_MS = 100
#: The ceiling on one backoff.
MAX_DELAY_MS = 2_000
#: Retries after the first attempt, when nothing overrides it.
DEFAULT_MAX_RETRIES = 3
#: `RetryInfo.retry_delay` is honoured up to this, when the server sends one.
MAX_SERVER_DELAY_MS = 30_000

#: The codes a retry may answer (D610).
RETRYABLE_CODES: frozenset[Code] = frozenset(
    {Code.UNAVAILABLE, Code.DEADLINE_EXCEEDED, Code.RESOURCE_EXHAUSTED}
)

#: What the generated bindings say about a call: `safe` for reads and
#: idempotent RPCs, `manual` for a mutation.
RetryClass = Literal["safe", "manual"]


def is_retryable_code(code: Code) -> bool:
    """Whether the code is one a retry may answer."""
    return code in RETRYABLE_CODES


def backoff_seconds(attempt: int, server_delay_ms: float | None = None) -> float:
    """The backoff before retry number `attempt` (0 for the first retry).

    Full jitter: uniform over `[0, min(cap, base × 2^attempt)]`. A server-sent
    `RetryInfo.retry_delay` replaces it, up to `MAX_SERVER_DELAY_MS`.
    """
    if server_delay_ms is not None and server_delay_ms > 0:
        return min(server_delay_ms, float(MAX_SERVER_DELAY_MS)) / 1000.0
    ceiling_ms = min(MAX_DELAY_MS, BASE_DELAY_MS * 2**attempt)
    return random.uniform(0.0, ceiling_ms) / 1000.0  # noqa: S311


def sleep(seconds: float) -> None:
    """Sleeps, for the retry loop and for the tests that would otherwise wait."""
    if seconds > 0:
        time.sleep(seconds)


def should_retry(error_code: Code, retry_safe: bool, attempt: int, max_retries: int) -> bool:
    """Whether one more attempt is allowed.

    `retry_safe` is the call's class from the generated bindings, never a guess:
    a read or an idempotent RPC retries on its own, and a mutation retries once
    it carries an idempotency key, because the key is what makes the repeat safe
    (`loams.runtime.call` decides the key before the first attempt and reuses it
    on every retry).
    """
    if attempt >= max_retries:
        return False
    return retry_safe and is_retryable_code(error_code)
