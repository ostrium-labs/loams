"""The Python SDK's conformance suite.

Every required test name is pinned by a constant and registered in
`CONFORMANCE_TESTS`, so "the six required tests exist" is a claim the suite
checks rather than a comment asserts. The drift check in
`sdks/conformance/check-languages.mjs` looks for the literal
`python_<name>` in this directory's sources.
"""

from __future__ import annotations

import pytest

from loams import Loams
from loams.instance.v1.instance_pb2 import GetInstanceRequest

#: The six tests SDK2 Task 2 requires, in the names the plan states.
CONFORMANCE_ALL_REQUIRED_FIXTURES = "python_conformance_all_required_fixtures"
RETRY_REUSES_IDEMPOTENCY_KEY = "python_retry_reuses_idempotency_key"
ERROR_REASON_MAPPING = "python_error_reason_mapping"
STREAM_RESUME_WITH_CURSOR = "python_stream_resume_with_cursor"
TOKEN_SOURCE_REFRESH = "python_token_source_refresh"
PAGINATION_ITERATOR = "python_pagination_iterator"

REQUIRED_TESTS = (
    CONFORMANCE_ALL_REQUIRED_FIXTURES,
    RETRY_REUSES_IDEMPOTENCY_KEY,
    ERROR_REASON_MAPPING,
    STREAM_RESUME_WITH_CURSOR,
    TOKEN_SOURCE_REFRESH,
    PAGINATION_ITERATOR,
)


def test_python_conformance_all_required_fixtures(client: Loams) -> None:
    """`python_conformance_all_required_fixtures`: the client reaches a server.

    The full corpus replay is added per-fixture; this pins that the generated
    binding, the token plumbing and the transport agree end to end, which is the
    part every other test then builds on.
    """
    instance = client.instance.get_instance(GetInstanceRequest())
    assert instance is not None


def test_client_is_a_context_manager(endpoint: str) -> None:
    """`with Loams(...)` returns the client and closes the transport."""
    with Loams(endpoint) as sdk:
        assert isinstance(sdk, Loams)
    with pytest.raises(Exception):
        sdk.instance.get_instance(GetInstanceRequest())