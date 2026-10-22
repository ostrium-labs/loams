"""The typed error hierarchy of design §44 §7.4, decision D611.

A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
details. The `reason` is what callers branch on: it is a stable `snake_case`
string, registered in `docs/api/reasons.md` and generated into
`loams._gen.facade`'s `Reason`, so `error.reason == "feature_not_in_variant"`
means one thing and a reason the registry has lost stops type-checking. The
`message` is for people and may change; nothing in an SDK branches on it.

Three cases stay distinct (R8):

- a reason from a **newer** server, which this SDK's registry does not have: it
  is surfaced as text on `unknown_reason` and flagged, not dropped, because
  losing it would leave a caller unable to tell "not supported here" from "not
  supported at all";
- a failure from **below the API** — a socket, a timeout, an abort — which
  carries no `reason` at all;
- a `LoamsError`, which is a mapped one and is returned unchanged if mapped
  twice.
"""

from __future__ import annotations

from typing import Final, Literal, cast

from connectrpc.code import Code
from connectrpc.errors import ConnectError, ErrorDetail
from google.protobuf.any_pb2 import Any
from protobuf.wkt import Any as WireAny
from google.protobuf import descriptor
from google.protobuf.message import Message

from loams.errors.v1.errors_pb2 import ErrorInfo
from loams._gen.facade import FEATURE_NOT_IN_VARIANT, REASONS, TOKEN_EXPIRED, Reason

__all__ = [
    "CLIENT_REASONS",
    "AbortedError",
    "AlreadyExistsError",
    "BatchError",
    "ClientReason",
    "DeadlineExceededError",
    "FailedPreconditionError",
    "FeatureNotInVariantError",
    "InternalError",
    "InvalidArgumentError",
    "LoamsError",
    "NotFoundError",
    "PermissionDeniedError",
    "ResourceExhaustedError",
    "TokenExpiredError",
    "UnauthenticatedError",
    "UnavailableError",
    "UnimplementedError",
    "error_detail",
    "error_info",
    "is_loams_error",
    "to_loams_error",
]

#: The registry, as a runtime guard: a reason off the wire that the generated
#: `Reason` does not have is a server newer than this SDK.
_KNOWN_REASONS: Final[frozenset[str]] = frozenset(REASONS)

#: A reason the SDK raises itself, without a server answer (SDKP §2.1, R12 and
#: R13). Registered in `docs/api/reasons.md` as client-side, and kept here as
#: well so the SDK does not depend on when that registry lands.
ClientReason = Literal["client_deadline_exceeded", "batch_partially_applied"]

#: The client-side reasons, as a set.
CLIENT_REASONS: Final[frozenset[str]] = frozenset(
    {"client_deadline_exceeded", "batch_partially_applied"}
)

#: The `type.googleapis.com/loams.errors.v1.ErrorInfo` prefix Connect puts on a
#: packed detail. Matched rather than assumed, so a service that adds a detail
#: of its own does not move `reason` out from under a caller.
_ERROR_INFO_URL: Final[str] = "type.googleapis.com/loams.errors.v1.ErrorInfo"
_TYPE_URL_PREFIX: Final[str] = "type.googleapis.com/"


class LoamsError(Exception):
    """What every Loams failure carries."""

    def __init__(
        self,
        message: str,
        *,
        code: Code = Code.UNKNOWN,
        reason: Reason | ClientReason | None = None,
        unknown_reason: str | None = None,
        metadata: dict[str, str] | None = None,
        hint: str | None = None,
        rpc: str | None = None,
    ) -> None:
        """Builds one error.

        `reason` is a `Reason` when the registry knows it; `unknown_reason` is
        the text when it does not. Exactly one of the two is set for a failure
        that carried an `ErrorInfo`, and neither for a failure from below the
        API.
        """
        super().__init__(message)
        self.name = type(self).__name__
        #: The Connect code, the canonical classification.
        self.code: Code = code
        #: The stable cause, when the server sent an `ErrorInfo`. `None` means
        #: the failure came from below the API (a socket, a timeout), not from a
        #: Loams service.
        self.reason: Reason | ClientReason | None = reason
        #: Structured context the server sent. Never secrets.
        self.metadata: dict[str, str] = dict(metadata) if metadata else {}
        #: A short next step in the caller's locale, when the server sent one.
        self.hint: str | None = hint
        #: The RPC that failed, as `package.Service/Method`.
        self.rpc: str | None = rpc
        #: The reason text when it is not in this SDK's registry: the server is
        #: newer than the SDK, so the reason is carried as text, not as the
        #: generated type.
        self.unknown_reason: str | None = unknown_reason


class InvalidArgumentError(LoamsError):
    """`invalid_argument`: a malformed field or an unparseable value."""


class NotFoundError(LoamsError):
    """`not_found`: the named resource does not exist."""


class AlreadyExistsError(LoamsError):
    """`already_exists`: an RPC that creates a named resource found it there."""


class PermissionDeniedError(LoamsError):
    """`permission_denied`: the caller's role may not make this call."""


class UnauthenticatedError(LoamsError):
    """`unauthenticated`: no credential, or an unusable one."""


class FailedPreconditionError(LoamsError):
    """`failed_precondition`: the system is not in the required state."""


class ResourceExhaustedError(LoamsError):
    """`resource_exhausted`: a backpressure or quota refusal."""


class UnavailableError(LoamsError):
    """`unavailable`: a dependency is down, or this node cannot serve the read."""


class DeadlineExceededError(LoamsError):
    """`deadline_exceeded`: the caller's deadline passed."""


class AbortedError(LoamsError):
    """`aborted`: a concurrent write won; the caller retries."""


class InternalError(LoamsError):
    """`internal`: a bug. The message and `request_id` go to the log."""


class UnimplementedError(LoamsError):
    """`unimplemented`: the operation is not implemented, supported or enabled."""


class FeatureNotInVariantError(UnimplementedError):
    """A package this build variant does not carry (design §44 §4, D600).

    The server answers `unimplemented` with `ErrorInfo.reason =
    feature_not_in_variant` and names the variant in `metadata.variant`, which is
    what this class reads. A caller usually never gets here: `loams.system`
    feature-detects from `GetInstance.services[]` before calling, so an
    unavailable module raises `FeatureNotInVariantError` from
    `loams.system.guard()` with no request spent. This class is the path for a
    caller who skipped the guard, or whose instance changed variant.
    """

    def __init__(self, message: str, *, variant: str | None = None, **kwargs: object) -> None:
        """:param variant: the build variant that was asked for, from `metadata.variant`."""
        super().__init__(message, **kwargs)  # type: ignore[arg-type]
        #: The build variant that was asked for.
        self.variant: str | None = variant


class BatchError(LoamsError):
    """A `documents.write_batch` that stopped at a failed chunk (runtime contract
    R13).

    The chunks before it are applied and stay applied; `token` is their merged
    consistency token, so a caller can read what did land, and `__cause__` is
    the failed chunk's own error. `reason` is `batch_partially_applied`.
    """

    def __init__(
        self,
        message: str,
        *,
        chunks_applied: int,
        rows_applied: int,
        token: str | None = None,
        code: Code = Code.ABORTED,
        metadata: dict[str, str] | None = None,
        rpc: str | None = None,
    ) -> None:
        super().__init__(
            message,
            code=code,
            reason="batch_partially_applied",
            metadata=metadata,
            rpc=rpc,
        )
        #: How many chunks were applied before the failure.
        self.chunks_applied: int = chunks_applied
        #: How many operations those chunks carried.
        self.rows_applied: int = rows_applied
        #: The merged token of the applied chunks, if any carried one.
        self.token: str | None = token


class TokenExpiredError(UnauthenticatedError):
    """A token the server rejected as expired: `unauthenticated`, reason
    `token_expired`.

    The call path refreshes once and retries once (D608, R1).
    """


#: The code-to-class mapping of D611. Codes D611 does not name (`canceled`,
#: `out_of_range`, `data_loss`) fall through to the base `LoamsError`.
_BY_CODE: Final[dict[Code, type[LoamsError]]] = {
    Code.CANCELED: LoamsError,
    Code.UNKNOWN: LoamsError,
    Code.INVALID_ARGUMENT: InvalidArgumentError,
    Code.DEADLINE_EXCEEDED: DeadlineExceededError,
    Code.NOT_FOUND: NotFoundError,
    Code.ALREADY_EXISTS: AlreadyExistsError,
    Code.PERMISSION_DENIED: PermissionDeniedError,
    Code.RESOURCE_EXHAUSTED: ResourceExhaustedError,
    Code.FAILED_PRECONDITION: FailedPreconditionError,
    Code.ABORTED: AbortedError,
    Code.OUT_OF_RANGE: LoamsError,
    Code.UNIMPLEMENTED: UnimplementedError,
    Code.INTERNAL: InternalError,
    Code.UNAVAILABLE: UnavailableError,
    Code.DATA_LOSS: LoamsError,
    Code.UNAUTHENTICATED: UnauthenticatedError,
}


def is_loams_error(value: object) -> bool:
    """Whether a value is a Loams error rather than a raw `ConnectError`."""
    return isinstance(value, LoamsError)


def _unpack(detail: Any, message: Message) -> bool:
    """Unpacks a packed detail into `message`, whether or not the pool knows it."""
    try:
        # The stubs type DESCRIPTOR as descriptor.Descriptor, but the _upb
        # runtime hands back its own Descriptor type. They are the same object;
        # the stubs just do not say so.
        descriptor = cast("descriptor.Descriptor", message.DESCRIPTOR)
        if detail.Is(descriptor):
            detail.Unpack(message)
            return True
    except (TypeError, ValueError, KeyError):
        return False
    # A descriptor pool that has not registered `loams.errors.v1` cannot answer
    # `Is`, so the bytes are parsed directly: the wire bytes of an `Any` value
    # are the serialised message, and that does not need the pool.
    message.ParseFromString(detail.value)
    return True


#: The bare message name, which is what Connect's JSON encoding puts in a
#: detail's `type` and what both sides of the lookup normalize to.
_ERROR_INFO_MESSAGE: Final[str] = "loams.errors.v1.ErrorInfo"


def _message_name(type_url: str) -> str:
    """Strip every `type.googleapis.com/` prefix, so a doubled URL still names
    the message. Any prefix, not just our own, since the input is untrusted."""
    name = type_url
    while name.startswith(_TYPE_URL_PREFIX):
        name = name[len(_TYPE_URL_PREFIX):]
    return name


def error_info(error: ConnectError) -> ErrorInfo | None:
    """The `ErrorInfo` a Connect error carries, if any.

    Looked up by type rather than by position, so a service that adds a detail
    of its own does not move `reason` out from under a caller.

    The type is normalized before it is compared. Connect's JSON encoding puts
    the bare message name in `type` and the client prefixes it with
    `type.googleapis.com/`, but a recorder or a Go peer that already sends the
    full type URL gets that prefix applied *again*, arriving here as
    `type.googleapis.com/type.googleapis.com/loams.errors.v1.ErrorInfo`. An
    exact match only ever finds the first of those two shapes, and the symptom
    is a `token_expired` refusal arriving as a bare `UnauthenticatedError` with
    the reason dropped -- which silently disables the refresh-and-retry path.
    """
    # `connectrpc` 0.10+ hands back `ErrorDetail`s: `type_name` is the type URL
    # with one `type.googleapis.com/` removed, which `_message_name` finishes, and
    # `message_bytes` is the `Any`'s value. Neither needs a descriptor pool.
    for detail in error.details:
        if _message_name(detail.type_name) == _ERROR_INFO_MESSAGE:
            return ErrorInfo.FromString(detail.message_bytes)
    return None


def error_detail(message: Message) -> ErrorDetail:
    """A `google.protobuf` message as a `ConnectError` detail.

    `connectrpc` 0.10+ packs details with its own `protobuf-py` runtime, so a
    `google.protobuf` message (every message this SDK generates) has to arrive
    already packed. The `Any` carries the same type URL and bytes that
    `connect-python` 0.9.0's `pack_any` produced, so the wire is unchanged.
    """
    return ErrorDetail(
        WireAny(
            type_url=_TYPE_URL_PREFIX + message.DESCRIPTOR.full_name,
            value=message.SerializeToString(),
        )
    )


def to_loams_error(value: object, rpc: str | None = None) -> LoamsError:
    """Turns any thrown value into the typed hierarchy.

    A `ConnectError` becomes the class its code names, with `reason` and
    `metadata` lifted out of the `ErrorInfo` detail. Anything else — a socket, a
    timeout, a bug in the SDK — becomes a `LoamsError` with `Code.UNKNOWN` and
    no reason, because a failure from below the API carries none.
    """
    if isinstance(value, LoamsError):
        return value
    if not isinstance(value, ConnectError):
        return LoamsError(str(value), code=Code.UNKNOWN, rpc=rpc)

    info = error_info(value)
    raw = info.reason if info is not None else ""
    known = raw in _KNOWN_REASONS
    fields: dict[str, object] = {
        "code": value.code,
        "reason": (raw if known else None),
        "unknown_reason": (None if raw == "" or known else raw),
        "metadata": dict(info.metadata) if info is not None else {},
        "hint": (info.hint if info is not None and info.hint != "" else None),
        "rpc": rpc,
    }
    if known and raw == FEATURE_NOT_IN_VARIANT:
        metadata = dict(info.metadata) if info is not None else {}
        return FeatureNotInVariantError(
            value.message, variant=metadata.get("variant"), **fields
        )
    if value.code == Code.UNAUTHENTICATED and known and raw == TOKEN_EXPIRED:
        return TokenExpiredError(value.message, **fields)  # type: ignore[arg-type]
    constructor = _BY_CODE.get(value.code, LoamsError)
    return constructor(value.message, **fields)  # type: ignore[arg-type]
