"""The types both sides of the call path need, in one leaf module.

`loams.gen.facade` is generated and imports from here so its typed module
protocols can name a call's options; `loams.runtime.call` imports the same types
so it can honour them. Nothing here imports either, which is what keeps the
direction of the dependency one-way.

Design §44 §7.4; runtime contract R1–R10 in `docs/sdk/runtime-contract.md`.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Literal, Protocol, runtime_checkable

__all__ = [
    "CallOptions",
    "ConsistencyOptions",
    "ConsistencyTokenStore",
    "PageRequestOptions",
    "Retryable",
    "SessionChoice",
]


@runtime_checkable
class ConsistencyTokenStore(Protocol):
    """A session's merged consistency token (D609).

    Off unless a client or a call opts in. `current` is what a later read
    attaches; `record` folds a token a write answered with in.
    """

    def current(self) -> str | None:
        """The token to attach to the next read, or `None` for none."""
        ...

    def record(self, token: str | None) -> None:
        """Folds a token the server returned into the session's.

        Raises `LoamsError` when the token cannot be merged, which the call path
        counts rather than propagates: the RPC already succeeded, and a caller
        that retried on a thrown error would write twice.
        """
        ...


#: Whether a call reads the client's session consistency token, brings its own,
#: or neither. `True` means "the client's", which is what `session=True` says.
SessionChoice = Literal[True, False] | ConsistencyTokenStore


@dataclass(frozen=True, slots=True)
class ConsistencyOptions:
    """How one call carries consistency tokens (D609)."""

    #: A token from a previous write, or `"strong"` (the default) / `"eventual"`
    #: to read without waiting. An explicit token wins over the session's.
    token: str | None = None
    #: Record the token every response carries, so later reads are
    #: read-your-writes without the caller threading tokens by hand.
    session: SessionChoice = False


@dataclass(frozen=True, slots=True)
class CallOptions:
    """Per-call overrides.

    Everything is optional: the client's defaults apply, and the *generated*
    binding supplies the retry class, so a caller never states it.

    The instance's URL is not here: in `connect-python` it belongs to the
    client, which is why a `Loams` hands its own address to the client it builds.
    """

    #: Retries after the first attempt. `0` disables retrying for this call.
    max_retries: int | None = None
    #: Extra request headers. `Authorization` is set by the SDK and wins.
    headers: Mapping[str, str] | None = None
    #: The idempotency key for a mutating call. Supply your own to make a retry
    #: yours rather than the SDK's; omit it and the SDK mints one UUIDv7 per
    #: logical call and reuses it on every retry.
    idempotency_key: str | None = None
    #: Overrides the call's retry class from the generated bindings, for this
    #: call only.
    retry_safe: bool | None = None
    #: The consistency token to read at, and the store to record writes in.
    consistency: ConsistencyOptions | None = None
    #: The deadline for this call in milliseconds, overriding the client's.
    timeout_ms: int | None = None


@dataclass(frozen=True, slots=True)
class PageRequestOptions:
    """Which request fields carry AIP-158's `page_size` and `page_token`.

    Both default to the proto3 JSON names, which is what the generated messages
    use: `page_size` in, `page_token` in, `next_page_token` out.
    """

    page_size_field: str = "page_size"
    page_token_field: str = "page_token"


@dataclass(frozen=True, slots=True)
class Retryable:
    """What a retried call looks like, so the loop is testable without a client."""

    #: The call's retry class from the generated binding.
    retry_safe: bool
    #: Retries after the first attempt.
    max_retries: int
