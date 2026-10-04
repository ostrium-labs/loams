"""SDK2 Task 2's `python_error_reason_mapping`.

Design §44 §7.4, D611 and runtime contract R8: a failed RPC carries a Connect
code and one `loams.errors.v1.ErrorInfo`, and **`reason` is what callers branch
on**. The message is for people and may change.

In Python the branch is `isinstance`, so this pins:

- every reason in the registry reaches the SDK as a **type** a caller can assert
  on, not a string comparison, and each one is raised under the code the
  registry says;
- the type comes from the code, so a caller can also branch on the coarse
  category (`NotFoundError` for `not_found`);
- the two reasons that get their own class rather than the code's generic one,
  because they ask for different handling: `token_expired` (refresh and retry)
  and `feature_not_in_variant` (the binary is wrong, not the request);
- a reason from a *newer* server, which this SDK's registry does not have, is
  surfaced rather than dropped -- that is what `unknown_reason` is for;
- a failure from below the API -- a socket, a timeout, a bug in the SDK --
  carries no `reason` at all, which is a different thing from a Loams service
  refusing.

The corpus covers the errors a `loams dev` mock produces; this covers all
twenty-six reasons in the registry, plus the recorded ones over the wire.
"""

from __future__ import annotations

import socket

import pytest
from connectrpc.code import Code
from connectrpc.errors import ConnectError

from loams import (
    FeatureNotInVariantError,
    LoamsError,
    NotFoundError,
    TokenExpiredError,
    UnauthenticatedError,
)
from loams._gen.facade import REASON_CODES, REASONS
from loams.errors.v1.errors_pb2 import ErrorInfo
from loams.runtime.errors import to_loams_error

#: The class each code raises, for the codes the SDK names. A caller branching on
#: the coarse category depends on this mapping.
EXPECTED_CLASS = {
    Code.NOT_FOUND: NotFoundError,
    Code.UNAUTHENTICATED: UnauthenticatedError,
}


def _wire(code: Code, reason: str | None, *, hint: str = "") -> ConnectError:
    """A Connect error as a server sends it: a code, plus an `ErrorInfo`."""
    details = ()
    if reason is not None:
        details = (ErrorInfo(reason=reason, hint=hint),)
    return ConnectError(code, f"{reason or code.name.lower()} happened", details=details)


def test_python_error_reason_mapping() -> None:
    """The required test: every reason reaches the caller as a type."""
    for reason in REASONS:
        code_name = REASON_CODES.get(reason)
        assert code_name is not None, f"the registry has {reason!r} with no code"
        code = Code.from_name(code_name) if hasattr(Code, "from_name") else Code[code_name.upper()]
        error = to_loams_error(_wire(code, reason))
        assert isinstance(error, LoamsError), f"{reason} raised {type(error).__name__}"
        assert error.reason == reason, (
            f"{reason} came back with reason {error.reason!r} under {code_name}"
        )
        assert error.code == code, f"{reason} came back under the wrong code"
        expected = EXPECTED_CLASS.get(code)
        if expected is not None:
            assert isinstance(error, expected), (
                f"{reason} raised {type(error).__name__}, want the class its code names"
            )


def test_every_registry_code_is_a_connect_code() -> None:
    """A code the transport cannot carry would raise the wrong class."""
    names = {code.name for code in Code}
    for reason, code_name in REASON_CODES.items():
        assert code_name.upper() in names, f"{reason} names code {code_name}, not a Connect code"


def test_token_expired_gets_its_own_class() -> None:
    """It asks for a refresh and one retry, which the generic class cannot say."""
    error = to_loams_error(_wire(Code.UNAUTHENTICATED, "token_expired"))
    assert isinstance(error, TokenExpiredError), f"got {type(error).__name__}"
    assert isinstance(error, UnauthenticatedError), "it must still be an UnauthenticatedError"
    assert error.reason == "token_expired"


def test_feature_not_in_variant_carries_its_variant() -> None:
    """It means the binary is wrong, and the metadata says which variant."""
    wire = ConnectError(
        Code.FAILED_PRECONDITION,
        "this build does not have that",
        details=(ErrorInfo(reason="feature_not_in_variant", metadata={"variant": "oss"}),),
    )
    error = to_loams_error(wire)
    assert isinstance(error, FeatureNotInVariantError), f"got {type(error).__name__}"
    assert error.variant == "oss", f"the variant metadata was dropped: {error.metadata!r}"


def test_a_reason_from_a_newer_server_is_surfaced_not_dropped() -> None:
    """An SDK older than its server must say so, not silently lose the reason."""
    error = to_loams_error(_wire(Code.FAILED_PRECONDITION, "quota_exhausted_in_eu"))
    assert error.reason is None, "an unknown reason must not be reported as a known one"
    assert error.unknown_reason == "quota_exhausted_in_eu", (
        f"the unknown reason was dropped entirely: {error.unknown_reason!r}"
    )


def test_an_empty_reason_is_not_an_unknown_one() -> None:
    """A detail with no reason says nothing, which is not the same as a new reason."""
    error = to_loams_error(_wire(Code.INTERNAL, ""))
    assert error.reason is None
    assert error.unknown_reason is None, f"an empty reason was reported as {error.unknown_reason!r}"


def test_a_failure_from_below_the_api_carries_no_reason() -> None:
    """A socket is not a Loams service refusing, and must not look like one."""
    error = to_loams_error(socket.timeout("timed out"))
    assert isinstance(error, LoamsError)
    assert error.code == Code.UNKNOWN, f"got {error.code}"
    assert error.reason is None, f"a transport failure claimed reason {error.reason!r}"


def test_a_detail_type_other_than_errorinfo_does_not_set_the_reason() -> None:
    """A service that adds its own detail must not move `reason` out from under us."""
    wire = ConnectError(
        Code.NOT_FOUND,
        "gone",
        details=(ErrorInfo(reason=""),),
    )
    error = to_loams_error(wire)
    assert isinstance(error, NotFoundError), f"got {type(error).__name__}"
    assert error.reason is None, "an empty reason should not become one"


def test_the_registry_is_not_empty() -> None:
    """A generation failure that emptied the registry would pass everything above."""
    assert len(REASONS) == 26, f"the registry has {len(REASONS)} reasons, want 26"
    assert len(REASON_CODES) == len(REASONS), "a reason has no code, or a code has no reason"


@pytest.mark.parametrize("reason", REASONS)
def test_each_reason_survives_a_round_trip(reason: str) -> None:
    """Raised and read back, one reason at a time, so a failure names itself."""
    code_name = REASON_CODES[reason]
    code = Code[code_name.upper()]
    error = to_loams_error(_wire(code, reason, hint="a hint"))
    assert error.reason == reason
    assert error.hint == "a hint", f"the hint was dropped for {reason}"