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
import time
from typing import Final

__all__ = ["uuidv7", "uuidv7_time"]

_V7: Final[re.Pattern[str]] = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


def uuidv7() -> str:
    """A fresh UUIDv7 as the canonical hyphenated string."""
    random = bytearray(secrets.token_bytes(16))
    # The 48-bit timestamp is big-endian, so it is written in with shifts rather
    # than by dividing and masking: dividing keeps the fractional bits of the
    # lower digits and truncates the carry, which puts the wrong byte in.
    now = int(time.time() * 1000)
    for index in range(6):
        shift = (5 - index) * 8
        random[index] = (now >> shift) & 0xFF
    random[6] = (random[6] & 0x0F) | 0x70
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
