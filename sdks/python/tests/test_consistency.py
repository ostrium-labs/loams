"""Consistency tokens (D609, runtime contract R4 as amended by SDKP §2.1).

`python_consistency_token_merge` runs the token corpus,
`sdks/fixtures/tokens.json` -- the server's own
`crates/loams-collection/src/token.rs` cases, written down once for every SDK.
The rest pins the session store and the record path: every `v1:` token a
response carries merges into the session, a string that is not a token is not
the session's business, and a `v1:` token that does not parse is counted rather
than thrown, because the write that answered with it has already happened.

The header half of the record path (`loams-consistency-token`) and the request
half (`consistency.at_least`) go over a real server in
`tests/test_app_runtime.py`'s `python_session_read_your_writes`.
"""

from __future__ import annotations

import json
import pathlib
from typing import Any

import pytest

from loams import InvalidArgumentError
from loams.runtime.call import record_consistency
from loams.runtime.consistency import (
    TOKEN_PREFIX,
    ConsistencySession,
    ConsistencyToken,
    TokenPosition,
    is_consistency_token,
)

FIXTURES = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures"
CORPUS: dict[str, Any] = json.loads((FIXTURES / "tokens.json").read_text(encoding="utf-8"))


class _Answered:
    """A response carrying tokens in the fields a write or a read answers with."""

    def __init__(self, **fields: str) -> None:
        for name, value in fields.items():
            setattr(self, name, value)


# -- python_consistency_token_merge: the corpus ------------------------------


@pytest.mark.parametrize("case", CORPUS["parse"], ids=lambda c: c["name"])
def test_python_consistency_token_merge_parse(case: dict[str, Any]) -> None:
    token = ConsistencyToken.parse(case["text"])
    assert [(str(p.stream), p.partition, str(p.offset)) for p in token.positions] == [
        tuple(item) for item in case["items"]
    ]
    assert str(token) == case["text"]


@pytest.mark.parametrize("case", CORPUS["merge"], ids=lambda c: c["name"])
def test_python_consistency_token_merge(case: dict[str, Any]) -> None:
    a, b, merged = case["a"], case["b"], case["expect"]
    assert str(ConsistencyToken.parse(a).merge(b)) == merged
    # Commutative once normalised.
    assert str(ConsistencyToken.parse(b).merge(a)) == merged
    assert ConsistencyToken.parse(a).merge(b) == merged


@pytest.mark.parametrize("text", CORPUS["malformed"])
def test_python_consistency_token_merge_refuses_malformed(text: str) -> None:
    with pytest.raises(InvalidArgumentError) as caught:
        ConsistencyToken.parse(text)
    assert caught.value.reason == "invalid_consistency_token"
    assert not ConsistencyToken.is_valid(text)


def test_a_u64_offset_survives_without_rounding() -> None:
    big = ConsistencyToken.parse("v1:s18446744073709551615/p4294967295@18446744073709551615")
    assert big.positions == (TokenPosition(2**64 - 1, 2**32 - 1, 2**64 - 1),)
    assert big.offset(2**64 - 1, 2**32 - 1) == 2**64 - 1


# -- the store ----------------------------------------------------------------


def test_a_token_is_recognised_by_its_prefix() -> None:
    assert is_consistency_token(f"{TOKEN_PREFIX}s3/p0@7")
    assert not is_consistency_token("")
    assert not is_consistency_token("3/7"), "an untagged string is not a token"


def test_the_session_merges_every_corpus_pair() -> None:
    session = ConsistencySession()
    for case in CORPUS["merge"]:
        session.clear()
        session.record(case["a"])
        session.record(case["b"])
        assert session.current() == case["expect"]


def test_two_tokens_merge_by_partition() -> None:
    session = ConsistencySession()
    session.record("v1:s3/p0@7")
    session.record("v1:s3/p1@2")
    session.record("v1:s3/p0@5")
    assert session.current() == "v1:s3/p0@7,s3/p1@2"
    assert session.rejected == 0


def test_a_malformed_token_is_counted_and_refused() -> None:
    session = ConsistencySession()
    with pytest.raises(InvalidArgumentError):
        session.record("v1:nonsense")
    assert session.current() is None
    assert session.rejected == 1
    assert session.conflicts == 1, "the old name still reads the same count"


def test_an_absent_token_is_not_counted() -> None:
    session = ConsistencySession()
    session.record(None)
    session.record("")
    assert session.current() is None
    assert session.rejected == 0


def test_clear_forgets_the_token_and_the_count() -> None:
    session = ConsistencySession()
    session.record("v1:s3/p0@7")
    with pytest.raises(InvalidArgumentError):
        session.record("v1:bad")
    session.clear()
    assert session.current() is None
    assert session.rejected == 0


# -- the record path ----------------------------------------------------------


def test_every_token_field_reaches_the_store() -> None:
    session = ConsistencySession()
    record_consistency(session, _Answered(consistency_token="v1:s1/p0@1"))
    record_consistency(session, _Answered(token="v1:s1/p1@4"))
    record_consistency(session, _Answered(read_token="v1:s2/p0@9"))
    record_consistency(session, None, header_token="v1:s1/p0@3")
    assert session.current() == "v1:s1/p0@3,s1/p1@4,s2/p0@9"


def test_a_string_that_is_not_a_token_is_not_the_sessions_business() -> None:
    """`token` is a common field name: a page token or a bearer is skipped, not
    counted."""
    session = ConsistencySession()
    record_consistency(session, _Answered(token="page-2"))
    assert session.current() is None
    assert session.rejected == 0


def test_a_token_the_store_refuses_is_counted_not_thrown() -> None:
    """The RPC already succeeded; a thrown error here would invite a second write."""
    session = ConsistencySession()
    session.record("v1:s3/p0@7")
    record_consistency(session, _Answered(consistency_token="v1:garbage"))
    assert session.rejected == 1
    assert session.current() == "v1:s3/p0@7"


def test_the_app_messages_carry_tokens_where_the_record_path_reads_them() -> None:
    """The fields `record_consistency` reads exist on the app messages, checked
    against the descriptors rather than restated."""
    import loams.collection.v1.document_pb2 as document_pb2
    import loams.collection.v1.query_pb2 as query_pb2
    import loams.graph.v1.graph_pb2 as graph_pb2

    fields = {
        (message.full_name, field.name)
        for module in (document_pb2, query_pb2, graph_pb2)
        for message in module.DESCRIPTOR.message_types_by_name.values()
        for field in message.fields
        if field.name in ("consistency_token", "token", "read_token")
    }
    assert ("loams.collection.v1.WriteDocumentsResponse", "token") in fields
    assert ("loams.collection.v1.SearchResponse", "read_token") in fields
    assert ("loams.graph.v1.ExecuteResponse", "consistency_token") in fields
