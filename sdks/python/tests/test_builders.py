"""SDKP's `python_builders_match_corpus`.

Every case in `sdks/fixtures/builders/` is driven through the public builders --
`Schema`, `Filter`, `Search` -- from its neutral spec, and the output is compared
with the case's `expect` after both are decoded into the named message, so key
order and number spelling do not matter and a field the message does not have
does. The notation is in that directory's README: a call is
`{"op": verb, "args": [...]}`, a nested object with an `op` is itself a call, and
a trailing plain object is the call's options (keyword arguments here).
"""

from __future__ import annotations

import json
import pathlib
from collections.abc import Callable
from typing import Any

import pytest
from google.protobuf import json_format

from loams.builders import Filter, Schema, Search, Sort, dbsf, rrf, weighted
from loams.collection.v1.query_pb2 import Query, SearchRequest
from loams.runtime.errors import InvalidArgumentError

DIR = pathlib.Path(__file__).resolve().parent.parent.parent / "fixtures" / "builders"
CASES: list[dict[str, Any]] = [
    json.loads(path.read_text(encoding="utf-8")) for path in sorted(DIR.glob("*.json"))
]

#: The corpus verbs that are Python keywords.
_KEYWORDS = {"and": "and_", "or": "or_", "not": "not_", "in": "in_"}


def _is_call(value: Any) -> bool:
    return isinstance(value, dict) and "op" in value


def _split(call: dict[str, Any]) -> tuple[list[Any], dict[str, Any]]:
    """Positional arguments (nested calls evaluated) and the trailing options."""
    raw = list(call.get("args", []))
    options: dict[str, Any] = {}
    if raw and isinstance(raw[-1], dict) and not _is_call(raw[-1]) and call["op"] != "ids":
        options = raw.pop()
    return [evaluate(a) if _is_call(a) else a for a in raw], options


def evaluate(call: dict[str, Any]) -> dict[str, Any]:
    """A filter, fusion or sort call."""
    args, options = _split(call)
    op = call["op"]
    others: dict[str, Callable[..., dict[str, Any]]] = {
        "rrf": rrf,
        "dbsf": dbsf,
        "weighted": weighted,
        "score": Sort.score,
        "pk": Sort.pk,
        "field": Sort.field,
    }
    fn = others.get(op) or getattr(Filter, _KEYWORDS.get(op, op))
    result: dict[str, Any] = fn(*args, **options)
    return result


def build_schema(calls: list[dict[str, Any]]) -> dict[str, Any]:
    schema = Schema()
    for call in calls:
        args, options = _split(call)
        schema = getattr(schema, call["op"])(*args, **options)
    return schema.to_json()


def build_search(spec: dict[str, Any]) -> Search:
    search = Search(spec["on"])
    for call in spec["calls"]:
        args, options = _split(call)
        search = getattr(search, call["op"])(*args, **options)
    return search


def test_python_builders_match_corpus_has_every_case() -> None:
    names = {case["name"] for case in CASES}
    assert {
        "schema_notes",
        "filter_term",
        "filter_bool_nested",
        "filter_range_open",
        "filter_ids",
        "search_text_filter_highlight_select",
        "search_vector_ann_params",
        "search_hybrid_rrf",
        "search_hybrid_dbsf",
        "search_hybrid_weighted",
        "search_after_page",
        "search_expand",
    } <= names


@pytest.mark.parametrize("case", CASES, ids=lambda c: c["name"])
def test_python_builders_match_corpus(case: dict[str, Any]) -> None:
    if case["builder"] == "schema":
        assert build_schema(case["spec"]) == case["expect"]
    elif case["builder"] == "filter":
        built = json_format.ParseDict(evaluate(case["spec"]), Query())
        assert built == json_format.ParseDict(case["expect"], Query())
    else:
        built = build_search(case["spec"]).build()
        assert built == json_format.ParseDict(case["expect"], SearchRequest())


def test_a_weighted_fusion_with_the_wrong_number_of_weights_is_refused_before_any_call() -> None:
    search = (
        Search("notes")
        .text("refund", fields=["body"])
        .vector("embedding", [1, 0, 0, 0])
        .fuse(weighted(1))
    )
    with pytest.raises(InvalidArgumentError) as caught:
        search.build()
    assert caught.value.metadata["field"] == "fusion.weighted_sum.weights"


def test_the_search_builder_is_pure() -> None:
    base = Search("notes").text("refund", fields=["body"])
    limited = base.limit(2)
    assert "limit" not in base.to_json()
    assert limited.to_json()["limit"] == 2


def test_after_takes_the_sort_values_a_hit_answers() -> None:
    """`Hit.sort_values` is a list of `google.protobuf.Value` messages."""
    from google.protobuf.struct_pb2 import Value

    hit_values = [Value(number_value=1.25), Value(string_value="k")]
    request = Search("notes").sort(Sort.score(), Sort.pk()).after(hit_values).build()
    assert json_format.MessageToDict(request)["searchAfter"] == [1.25, "k"]
