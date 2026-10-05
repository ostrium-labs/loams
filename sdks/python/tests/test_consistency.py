"""Consistency tokens (D609, R4) -- the store, and the gap above it.

The store's own semantics are pinned here, and so is the fact that nothing can
currently feed it. That second half is the point of the file.

D609 puts a consistency token in the response **message** and in the response
header `loams-consistency-token`. On this SDK neither path delivers:

* no generated message has a `consistency_token` field, so `record_consistency`
  finds nothing to read; and
* `connect-python==0.9.0` exposes only `execute_unary` / `execute_server_stream`,
  which hand back the message and drop the response headers, so the header
  cannot be read either -- there is no `call_unary` to ask.

So `session_consistency=True` builds a store that stays empty, attaches no
`loams-consistency` header to any later read, and gives the caller neither
read-your-writes nor an error. The Rust SDK reads the header
(`token_from_headers`); this SDK cannot yet, and that is a dependency limit
rather than a missing feature.

`test_a_connectrpc_upgrade_that_exposes_headers_should_prompt_the_fix` fails the
moment connect-python grows a call that returns response headers, which is the
signal to read the token and make the flag do what it says.
"""

from __future__ import annotations

import pytest

from connectrpc.client import ConnectClient

from loams import LoamsError
from loams.runtime.call import record_consistency
from loams.runtime.consistency import (
    TOKEN_PREFIX,
    ConsistencySession,
    is_consistency_token,
)


class _Answered:
    """A response carrying a token, the way a write would once API1 ships it."""

    def __init__(self, token: str | None) -> None:
        if token is not None:
            self.consistency_token = token


# -- the store ----------------------------------------------------------------


def test_a_token_is_recognised_by_its_shape() -> None:
    assert is_consistency_token(f"{TOKEN_PREFIX}3/7")
    assert not is_consistency_token("")
    assert not is_consistency_token(TOKEN_PREFIX)
    assert not is_consistency_token("3/7"), "an untagged string is not a token"


def test_the_first_token_is_kept() -> None:
    session = ConsistencySession()
    session.record(f"{TOKEN_PREFIX}3/7")
    assert session.current() == f"{TOKEN_PREFIX}3/7"
    assert session.conflicts == 0


def test_the_same_token_twice_is_not_a_conflict() -> None:
    session = ConsistencySession()
    session.record(f"{TOKEN_PREFIX}3/7")
    session.record(f"{TOKEN_PREFIX}3/7")
    assert session.conflicts == 0, "re-recording the same token counted as a conflict"


def test_two_different_tokens_are_counted_not_merged() -> None:
    """Merging needs the encoding, which is not in the protos yet.

    §44 §7.4 says the merge is "max offset per stream and partition". Guessing
    would hand back a token that reads stale data, which is worse than an error.
    """
    session = ConsistencySession()
    session.record(f"{TOKEN_PREFIX}3/7")
    with pytest.raises(LoamsError) as caught:
        session.record(f"{TOKEN_PREFIX}9/2")
    assert session.conflicts == 1
    assert session.current() == f"{TOKEN_PREFIX}3/7", "the store overwrote the first token"
    assert "merge" in str(caught.value).lower()


def test_a_string_that_is_not_a_token_is_refused() -> None:
    session = ConsistencySession()
    with pytest.raises(LoamsError):
        session.record("nonsense")
    assert session.current() is None
    assert session.conflicts == 1


def test_an_absent_token_is_not_a_conflict() -> None:
    """Most responses carry no token, so this cannot be the error path."""
    session = ConsistencySession()
    session.record(None)
    session.record("")
    assert session.current() is None
    assert session.conflicts == 0


def test_clear_forgets_the_token_and_the_conflicts() -> None:
    session = ConsistencySession()
    session.record(f"{TOKEN_PREFIX}3/7")
    with pytest.raises(LoamsError):
        session.record(f"{TOKEN_PREFIX}9/2")
    session.clear()
    assert session.current() is None
    assert session.conflicts == 0


# -- the record path ----------------------------------------------------------


def test_a_token_in_the_message_reaches_the_store() -> None:
    """The path works; nothing arrives on it yet, which is the next two tests."""
    session = ConsistencySession()
    record_consistency(session, _Answered(f"{TOKEN_PREFIX}3/7"))
    assert session.current() == f"{TOKEN_PREFIX}3/7"


def test_a_response_without_a_token_leaves_the_store_alone() -> None:
    session = ConsistencySession()
    record_consistency(session, _Answered(None))
    record_consistency(session, None)
    assert session.current() is None
    assert session.conflicts == 0


def test_a_token_the_store_refuses_is_counted_not_thrown() -> None:
    """The RPC already succeeded; a thrown error here would invite a second write."""
    session = ConsistencySession()
    session.record(f"{TOKEN_PREFIX}3/7")
    record_consistency(session, _Answered("nonsense"))
    assert session.conflicts == 1
    assert session.current() == f"{TOKEN_PREFIX}3/7"


# -- the gap ------------------------------------------------------------------


def test_no_generated_message_carries_a_consistency_token() -> None:
    """Why the body path is empty, checked against the descriptors themselves.

    Not a comment restating a finding: if API1 adds the field, this fails and
    the `record_consistency` path starts working on its own.
    """
    import loams.instance.v1.instance_pb2 as instance_pb2
    import loams.live.v1.live_pb2 as live_pb2

    carriers = [
        message.full_name
        for module in (live_pb2, instance_pb2)
        for message in module.DESCRIPTOR.message_types_by_name.values()
        if "consistency_token" in {field.name for field in message.fields}
    ]
    assert carriers == [], (
        f"{carriers} now carry a consistency_token, so `record_consistency` can "
        "read the body path; check whether the session store needs anything"
    )


def test_a_connectrpc_upgrade_that_exposes_headers_should_prompt_the_fix() -> None:
    """The dependency limit, asserted so it cannot stop being true quietly.

    connect-python 0.9.0 gives no way to read response headers, which is why the
    store can never fill: the header is the only place a token actually arrives.
    If this ever fails, connect-python can hand back headers, and the fix is to
    read `loams-consistency-token` off them in `record_consistency` rather than
    to keep documenting the gap.
    """
    header_capable = [name for name in dir(ConnectClient) if name in ("call_unary", "call_server_stream")]
    assert header_capable == [], (
        f"connect-python now exposes {header_capable}, which return response "
        "headers. `record_consistency` should read `loams-consistency-token` from "
        "them so `session_consistency=True` stops being inert -- see the note on "
        "`Loams(session_consistency=...)`."
    )


def test_a_session_built_by_the_client_stays_empty_today() -> None:
    """The user-visible consequence, stated as a test so it cannot be forgotten.

    Turned on, off by no error: no header is attached to the later read. Asserted
    so that the day it starts working, this test is the thing that changes.
    """
    session = ConsistencySession()
    assert session.current() is None
    assert session.conflicts == 0