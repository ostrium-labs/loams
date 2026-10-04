"""Consistency tokens (design §44 §7.4, D609; runtime contract R4).

A write answers with a `consistency_token`; a read accepts one, so a caller that
just wrote can read its own write. Threading those by hand is the caller's job
today. The session store is the alternative §44 §7.4 asks for: **off by
default**, and when a call opts in, every response's token is folded into the
session and attached to later reads.

**The token's encoding is not in the protos yet.** §05 §5 defines the semantics
(offsets per stream and partition, `STRONG` as the default, `EVENTUAL`,
`AT_LEAST{token}`) and API1's write paths carry it as an opaque `v1:` string;
§44 §7.4 says it merges by "max offset per stream and partition", which needs the
encoding to be parsed. Until that lands this store keeps the token it was given,
reports two different tokens meeting as an error rather than merging them into a
wrong one, and counts it — a silently-wrong consistency token reads stale data,
which is worse than a failure.
"""

from __future__ import annotations

from typing import Final

from connectrpc.code import Code

from loams.runtime.errors import LoamsError

__all__ = ["TOKEN_PREFIX", "ConsistencySession", "is_consistency_token"]

#: The prefix every token carries (§44 §7.4).
TOKEN_PREFIX: Final[str] = "v1:"


def is_consistency_token(value: str) -> bool:
    """Whether a string looks like a consistency token."""
    return value.startswith(TOKEN_PREFIX) and len(value) > len(TOKEN_PREFIX)


class ConsistencySession:
    """A session's consistency token.

    Merging is deliberately conservative while the encoding is opaque: the store
    keeps what it has, and a second, different token is a conflict the caller has
    to resolve, not something to merge blindly.
    """

    def __init__(self) -> None:
        self._token: str | None = None
        self._conflicts = 0

    def current(self) -> str | None:
        """The token to attach to the next read."""
        return self._token

    def record(self, token: str | None) -> None:
        """Folds a token the server returned in.

        :raises LoamsError: when the token is not one, or when two different
            tokens met and the encoding cannot merge them yet. The call path
            counts the conflict rather than propagating it, because the RPC has
            already succeeded and a caller that retried on this error would write
            twice.
        """
        if token is None or token == "":
            return
        if not is_consistency_token(token):
            self._conflicts += 1
            raise LoamsError(f"not a consistency token: {token}", code=Code.INTERNAL)
        if self._token is None or self._token == token:
            self._token = token
            return
        self._conflicts += 1
        raise LoamsError(
            "two different consistency tokens met and the encoding cannot merge them yet; "
            "the session keeps the first. Merging by stream and partition offset arrives with "
            "the write paths that carry offsets (§44 §7.4, D609).",
            code=Code.FAILED_PRECONDITION,
        )

    @property
    def conflicts(self) -> int:
        """How many unmergeable pairs the session has seen."""
        return self._conflicts

    def clear(self) -> None:
        """Forgets the token, so the next read is not held to it."""
        self._token = None
        self._conflicts = 0
