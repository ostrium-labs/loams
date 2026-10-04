"""UUIDv7 (design §44 §7.4, D610).

An idempotency key has to be unique across every client that has ever talked to
an instance *and* sort by creation time, because a key that sorts is one an
operator can correlate in a log. UUIDv4 is unique but unordered; ULIDs would do,
but thirteen SDKs would each need their own implementation, and `secrets` is in
every runtime this SDK targets.

Layout: 48 bits of Unix milliseconds, 4 bits of version (7), 12 bits of counter
within the millisecond, 2 bits of variant, 62 random bits.
"""

from __future__ import annotations

import re
import secrets
import threading
import time
from typing import Final

__all__ = ["uuidv7", "uuidv7_time"]

_V7: Final[re.Pattern[str]] = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


#: `rand_a` is 12 bits, so 4096 keys fit inside one millisecond.
_COUNTER_BITS: Final[int] = 12
_COUNTER_MAX: Final[int] = (1 << _COUNTER_BITS) - 1

_tick_lock = threading.Lock()
_tick_ms = 0
_tick_counter = 0


def _next_tick() -> tuple[int, int]:
    """`(milliseconds, counter-within-the-millisecond)`, monotonic per process.

    RFC 9562 allows `rand_a` to be random or to be a counter, and this is the
    counter. The difference is observable: with random bits, two keys minted in
    the same millisecond sort in *random* order, which is the case that matters,
    because a burst of mutations or a retry storm is exactly what lands inside one
    millisecond -- and the sort is the feature. A docstring claiming a counter
    over random bits is a claim the code did not keep.

    The counter is seeded randomly per millisecond, so two processes minting in
    the same millisecond still differ; the 62 random bits after it carry the rest
    of the uniqueness. A wall clock that steps backwards keeps the last timestamp
    and carries the counter on, rather than issuing a key that sorts before one
    already handed out.
    """
    global _tick_ms, _tick_counter
    with _tick_lock:
        now = int(time.time() * 1000)
        if now > _tick_ms:
            _tick_ms = now
            _tick_counter = secrets.randbits(_COUNTER_BITS)
        else:
            _tick_counter += 1
            if _tick_counter > _COUNTER_MAX:
                # 4096 keys in one millisecond. Borrow from the next millisecond
                # rather than wrapping: wrapping would reuse a counter and hand
                # out a key that sorts before one already issued. 48 bits of
                # milliseconds lasts to the year 10889, so borrowing is free.
                _tick_ms += 1
                _tick_counter = 0
        return _tick_ms, _tick_counter


def uuidv7() -> str:
    """A fresh UUIDv7 as the canonical hyphenated string."""
    random = bytearray(secrets.token_bytes(16))
    # The 48-bit timestamp is big-endian, so it is written in with shifts rather
    # than by dividing and masking: dividing keeps the fractional bits of the
    # lower digits and truncates the carry, which puts the wrong byte in.
    millis, counter = _next_tick()
    for index in range(6):
        shift = (5 - index) * 8
        random[index] = (millis >> shift) & 0xFF
    # `rand_a`: the counter, which is what makes keys minted in one millisecond
    # sort in the order they were issued.
    random[6] = 0x70 | ((counter >> 8) & 0x0F)
    random[7] = counter & 0xFF
    random[8] = (random[8] & 0x3F) | 0x80
    value = random.hex()
    return (
        f"{value[0:8]}-{value[8:12]}-{value[12:16]}-{value[16:20]}-{value[20:32]}"
    )


def uuidv7_time(value: str) -> int | None:
    """The Unix milliseconds a UUIDv7 encodes, or `None` for anything else.

    The timestamp is the first **twelve** hex digits, not eight: 48 bits, and
    milliseconds since the epoch use 41 of them. Reading eight digits would
    return a number around 2**25, which is January 1970.
    """
    if _V7.match(value) is None:
        return None
    digits = value.replace("-", "")[:12]
    return int(digits, 16)
