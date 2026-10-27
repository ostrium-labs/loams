"""Consistency tokens (design §44 §7.4, D609; runtime contract R4 as amended by
SDKP §2.1).

A write answers with a token; a read accepts one, so a caller that just wrote
can read its own write. The encoding is the server's
(`crates/loams-collection/src/token.rs`)::

    v1:s<stream>/p<partition>@<offset>,s<stream>/p<partition>@<offset>,…

Each item is "the next offset after the write" for one `(stream, partition)`.
Merging two tokens keeps, per pair, the larger offset: a read held to the merge
waits for both writes. Every number is canonical decimal (no sign, no leading
zeros), so a token has exactly one spelling, and a malformed token is
`invalid_argument` here, before any network call.

The session store is off by default. With `session_consistency=True` every token
a response carries merges into it, and every read whose request has a
`consistency` field is sent `at_least = session` when the caller left the field
unset.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Final

from connectrpc.code import Code

from loams.runtime.errors import InvalidArgumentError

__all__ = [
    "TOKEN_PREFIX",
    "ConsistencySession",
    "ConsistencyToken",
    "TokenPosition",
    "is_consistency_token",
]

#: The prefix every token carries (§44 §7.4).
TOKEN_PREFIX: Final[str] = "v1:"

_U32_MAX: Final[int] = 0xFFFF_FFFF
_U64_MAX: Final[int] = 0xFFFF_FFFF_FFFF_FFFF
_ITEM: Final[re.Pattern[str]] = re.compile(r"^s([^/]*)/p([^@]*)@(.*)$")
_DIGITS: Final[re.Pattern[str]] = re.compile(r"^[0-9]+$")


def is_consistency_token(value: str) -> bool:
    """Whether a string carries the token prefix (the shape, not the parse)."""
    return value.startswith(TOKEN_PREFIX)


@dataclass(frozen=True, slots=True, order=True)
class TokenPosition:
    """One item of a token: the next offset after a write, for one partition."""

    stream: int
    partition: int
    offset: int


def _canonical(text: str, maximum: int) -> int | None:
    """A decimal with no sign and no leading zeros (except `"0"`), in range."""
    if text == "" or _DIGITS.match(text) is None or (len(text) > 1 and text.startswith("0")):
        return None
    value = int(text)
    return value if value <= maximum else None


def _parse_item(item: str) -> TokenPosition | None:
    match = _ITEM.match(item)
    if match is None:
        return None
    stream = _canonical(match.group(1), _U64_MAX)
    partition = _canonical(match.group(2), _U32_MAX)
    offset = _canonical(match.group(3), _U64_MAX)
    if stream is None or partition is None or offset is None:
        return None
    return TokenPosition(stream, partition, offset)


class ConsistencyToken:
    """A parsed consistency token: `parse`, `str`, `merge`, equality.

    Immutable. `merge` returns a new token, normalised (sorted by stream and
    partition, one item per pair, the max offset kept), which is exactly what
    `ConsistencyToken::merge` on the server produces, so the two print the same.
    Equality compares the normalised forms.
    """

    __slots__ = ("_positions",)

    def __init__(self, positions: tuple[TokenPosition, ...] = ()) -> None:
        self._positions = tuple(positions)

    @classmethod
    def parse(cls, text: str) -> ConsistencyToken:
        """Parses the text form; the items keep the order given.

        :raises InvalidArgumentError: on anything that is not the `v1:` form.
        """
        if not text.startswith(TOKEN_PREFIX):
            raise _invalid(text)
        body = text[len(TOKEN_PREFIX):]
        if body == "":
            return cls()
        positions: list[TokenPosition] = []
        for item in body.split(","):
            position = _parse_item(item)
            if position is None:
                raise _invalid(text)
            positions.append(position)
        return cls(tuple(positions))

    @classmethod
    def is_valid(cls, text: str) -> bool:
        """Whether `text` parses."""
        try:
            cls.parse(text)
        except InvalidArgumentError:
            return False
        return True

    @property
    def positions(self) -> tuple[TokenPosition, ...]:
        """The items, in this token's order."""
        return self._positions

    @property
    def is_empty(self) -> bool:
        """Whether the token names nothing."""
        return not self._positions

    def normalized(self) -> ConsistencyToken:
        """Sorted by `(stream, partition)`, one item per pair, the max offset kept."""
        items: list[TokenPosition] = []
        for item in sorted(self._positions):
            if items and (items[-1].stream, items[-1].partition) == (item.stream, item.partition):
                items[-1] = item
            else:
                items.append(item)
        return ConsistencyToken(tuple(items))

    def merge(self, other: ConsistencyToken | str) -> ConsistencyToken:
        """Both tokens' offsets; a pair both name keeps the larger. Normalised."""
        right = ConsistencyToken.parse(other) if isinstance(other, str) else other
        return ConsistencyToken(self._positions + right._positions).normalized()

    def offset(self, stream: int, partition: int) -> int | None:
        """The offset for `(stream, partition)`, the max if listed twice."""
        found = [p.offset for p in self._positions if (p.stream, p.partition) == (stream, partition)]
        return max(found) if found else None

    def __str__(self) -> str:
        return TOKEN_PREFIX + ",".join(
            f"s{p.stream}/p{p.partition}@{p.offset}" for p in self._positions
        )

    def __repr__(self) -> str:
        return f"ConsistencyToken({str(self)!r})"

    def __eq__(self, other: object) -> bool:
        if isinstance(other, str):
            try:
                other = ConsistencyToken.parse(other)
            except InvalidArgumentError:
                return False
        if not isinstance(other, ConsistencyToken):
            return NotImplemented
        return self.normalized()._positions == other.normalized()._positions

    def __hash__(self) -> int:
        return hash(self.normalized()._positions)


def _invalid(text: str) -> InvalidArgumentError:
    return InvalidArgumentError(
        f"invalid consistency token {text!r}",
        code=Code.INVALID_ARGUMENT,
        reason="invalid_consistency_token",
        metadata={"field": "consistency_token"},
    )


class ConsistencySession:
    """A session's consistency tokens: every token a response carried, merged,
    kept **per proto package** (R4, SDKP-17).

    A Graph read refuses a token naming another package's stream, so a
    collection write's token must never become a graph read's `at_least`: the
    session holds one merged token per scope, and the call path passes the
    call's package as the scope. A caller using the store directly may leave the
    scope out, which is one shared scope.

    A token that does not parse is counted in `rejected` and raised; the call
    path swallows it, because the write that answered with it has already
    happened and a thrown error would invite a caller to retry it.
    """

    def __init__(self) -> None:
        self._tokens: dict[str, ConsistencyToken] = {}
        self._rejected = 0

    def current(self, scope: str = "") -> str | None:
        """The token to attach to the next read in `scope`, or `None`."""
        token = self._tokens.get(scope)
        return None if token is None else str(token)

    def token(self, scope: str = "") -> ConsistencyToken | None:
        """The merged token of `scope`, parsed."""
        return self._tokens.get(scope)

    def scopes(self) -> list[str]:
        """The scopes the session holds a token for."""
        return list(self._tokens)

    def record(self, token: str | None, scope: str = "") -> None:
        """Folds a token the server returned, in `scope`, in.

        :raises InvalidArgumentError: when the token does not parse, after
            counting it.
        """
        if token is None or token == "":
            return
        try:
            parsed = ConsistencyToken.parse(token)
        except InvalidArgumentError:
            self._rejected += 1
            raise
        held = self._tokens.get(scope)
        self._tokens[scope] = parsed.normalized() if held is None else held.merge(parsed)

    @property
    def rejected(self) -> int:
        """How many tokens the session could not parse."""
        return self._rejected

    @property
    def conflicts(self) -> int:
        """The old name of `rejected`, from before tokens merged."""
        return self._rejected

    def clear(self) -> None:
        """Forgets every token, so the next read is not held to one."""
        self._tokens.clear()
        self._rejected = 0
