"""The Python SDK's conformance suite: the registry, and what it asserts.

Every required test name is pinned by a constant here, so "the six required
tests exist" is a claim the suite checks rather than a comment asserts. The
drift check in `sdks/conformance/check-languages.mjs` looks for the literal
`python_<name>` in this directory's sources, and `required.mjs` runs each one in
isolation with `-k`.

Each name is implemented in the module named beside it:

- `python_conformance_all_required_fixtures` -> `test_conformance_corpus.py`
- `python_retry_reuses_idempotency_key` -> `test_retry_idempotency.py`
- `python_error_reason_mapping` -> `test_error_reason.py`
- `python_stream_resume_with_cursor` -> `test_streams.py`
- `python_token_source_refresh` -> `test_token_source.py`
- `python_pagination_iterator` -> `test_pagination.py`

The test at the bottom checks that mapping still holds, which is what stops a
name from decaying into a constant that no longer names a real test -- which is
exactly how this file briefly lied about `python_retry_reuses_idempotency_key`.
"""

from __future__ import annotations

import importlib.util
import pathlib

from loams import Loams
from loams.instance.v1.instance_pb2 import GetInstanceRequest

#: The six tests SDK2 Task 2 requires, in the names the plan states, each with
#: the module that implements it.
CONFORMANCE_TESTS = {
    "python_conformance_all_required_fixtures": "test_conformance_corpus",
    "python_retry_reuses_idempotency_key": "test_retry_idempotency",
    "python_error_reason_mapping": "test_error_reason",
    "python_stream_resume_with_cursor": "test_streams",
    "python_token_source_refresh": "test_token_source",
    "python_pagination_iterator": "test_pagination",
}


def test_every_required_name_is_a_real_test() -> None:
    """A name that only exists as a constant has stopped meaning anything."""
    assert len(CONFORMANCE_TESTS) == 6, (
        f"the registry has {len(CONFORMANCE_TESTS)} entries, want 6"
    )
    for name, module_name in CONFORMANCE_TESTS.items():
        # Loaded by path: pytest imports test modules under names of its own
        # choosing, so importing by module name does not reliably find them.
        path = pathlib.Path(__file__).resolve().parent / f"{module_name}.py"
        assert path.is_file(), f"{module_name} does not exist at {path}"
        spec = importlib.util.spec_from_file_location(f"_registry_{module_name}", path)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        # `required.mjs` selects with `pytest -k <name>`, which is a substring
        # match, so the required name is the function name without `test_`.
        implemented = getattr(module, f"test_{name}", None)
        assert implemented is not None, (
            f"{module_name} does not define {name}, so the registry is claiming a "
            "test that does not exist"
        )
        assert callable(implemented), f"test_{name} is not callable in {module_name}"


def test_client_is_a_context_manager(endpoint: str) -> None:
    """`with Loams(...)` yields a usable client and does not raise on exit."""
    with Loams(endpoint) as sdk:
        assert isinstance(sdk, Loams)
        # Still usable inside the block: the fixture is served, so a bad close
        # path would surface here rather than as a later mysterious failure.
        assert sdk.instance.get_instance(GetInstanceRequest()) is not None


def test_the_fixture_server_is_actually_serving(endpoint: str) -> None:
    """The suite's other tests are only meaningful against a live fixture."""
    with Loams(endpoint) as sdk:
        assert sdk.instance.get_instance(GetInstanceRequest()).name != ""
