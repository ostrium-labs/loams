"""The typed builders of SDKP §2.4 and §2.6: `Schema`, `Filter` and `Search`.

    from loams.builders import Filter, Schema, Search, Sort, rrf

    schema = Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4)
    request = (
        Search("notes")
        .text("refund", fields=["body"])
        .vector("embedding", [0.1, 0.2, 0.3, 0.4], k=10)
        .fuse(rrf(60))
        .filter(Filter.eq("tags", "policy"))
        .highlight("body")
        .select(include=["title"])
        .limit(5)
    )
    answer = loams.search.query(request.build())

Every verb is pure: it returns a new builder (or, for `Filter`, a new dict) and
leaves its input as it was, so a base query can be shared and specialised.

The output is the request's **proto3 JSON** (`lowerCamelCase` field names, enums
by name, 64-bit integers as strings), which is how the builder corpus
(`sdks/fixtures/builders/*.json`) spells its expectations and what the
TypeScript reference builds, so one case is one comparison in every language.
`build()` decodes it into the generated message with `json_format.ParseDict`,
refusing first the requests the builder can tell are wrong (a weighted fusion
with the wrong number of weights) before any network call.

`Filter.in_`, `Filter.and_`, `Filter.or_` and `Filter.not_` carry a trailing
underscore because `in`, `and`, `or` and `not` are keywords; the corpus verbs
`in`, `and`, `or` and `not` map to them.
"""

from __future__ import annotations

import builtins
import copy
from collections.abc import Mapping, Sequence
from typing import Any, Literal, Union

from connectrpc.code import Code
from google.protobuf import json_format
from google.protobuf.message import Message

from loams.runtime.consistency import ConsistencyToken
from loams.runtime.errors import InvalidArgumentError

__all__ = [
    "DocId",
    "Filter",
    "FilterJson",
    "Fusion",
    "Schema",
    "Search",
    "Sort",
    "consistency_json",
    "dbsf",
    "id_value",
    "rrf",
    "to_query",
    "weighted",
]

#: JSON, as the builders produce it.
Json = Union[None, bool, int, float, str, list[Any], dict[str, Any]]
#: One filter: the proto3 JSON of a `loams.collection.v1.Query`.
FilterJson = dict[str, Any]
#: A document id: an int is a `uint` id, a str a `string` id, and
#: `{"uuid": "…"}` a `uuid` id.
DocId = Union[int, str, Mapping[str, Any]]
Order = Literal["asc", "desc"]
Fuzziness = Union[Literal["auto"], int]


def id_value(doc_id: DocId) -> Json:
    """A document id as `Query.ids` spells it: `{"uint": "1"}`, a bare string, or
    `{"uuid": "…"}`."""
    if isinstance(doc_id, bool):
        raise TypeError("a document id cannot be a bool")
    if isinstance(doc_id, int):
        if doc_id < 0:
            raise ValueError(f"a numeric document id must be non-negative, not {doc_id}")
        return {"uint": str(doc_id)}
    if isinstance(doc_id, str):
        return doc_id
    if "uuid" in doc_id:
        return {"uuid": str(doc_id["uuid"])}
    if "uint" in doc_id:
        return {"uint": str(doc_id["uint"])}
    if "string" in doc_id:
        return str(doc_id["string"])
    raise ValueError(f"not a document id: {doc_id!r}")


def _fuzziness(value: Fuzziness) -> dict[str, Any]:
    return {"auto": {}} if value == "auto" else {"edits": value}


def _bool_operator(operator: Literal["or", "and"]) -> str:
    return "BOOL_OPERATOR_AND" if operator == "and" else "BOOL_OPERATOR_OR"


def _multi_match_field(spec: str) -> dict[str, Any]:
    head, sep, tail = spec.rpartition("^")
    if sep and head:
        try:
            return {"field": head, "boost": float(tail)}
        except ValueError:
            pass
    return {"field": spec}


def _bounds(gt: Json, gte: Json, lt: Json, lte: Json) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in (("gt", gt), ("gte", gte), ("lt", lt), ("lte", lte)):
        if value is not None:
            out[key] = value
    return out


class Filter:
    """The filter verbs: one per arm of `loams.collection.v1.Query`.

    Each is a static method returning the arm's proto3 JSON.
    """

    @staticmethod
    def all() -> FilterJson:
        """Every document."""
        return {"matchAll": {}}

    @staticmethod
    def none() -> FilterJson:
        """No document."""
        return {"matchNone": {}}

    @staticmethod
    def eq(field: str, value: Json) -> FilterJson:
        """The field equals `value` (a term query)."""
        return {"term": {"field": field, "value": value}}

    @staticmethod
    def in_(field: str, values: Sequence[Json]) -> FilterJson:
        """The field equals any of `values` (a terms query)."""
        return {"terms": {"field": field, "values": list(values)}}

    @staticmethod
    def range(
        field: str,
        *,
        gt: Json = None,
        gte: Json = None,
        lt: Json = None,
        lte: Json = None,
    ) -> FilterJson:
        """The field is within the bounds; an omitted bound is open."""
        return {"range": {"field": field, **_bounds(gt, gte, lt, lte)}}

    @staticmethod
    def exists(field: str) -> FilterJson:
        """The field is present, whatever its value."""
        return {"exists": {"field": field}}

    @staticmethod
    def is_null(field: str) -> FilterJson:
        """The field is `null` or absent."""
        return {"isNull": {"field": field}}

    @staticmethod
    def is_empty(field: str) -> FilterJson:
        """The field is an empty string, list or object."""
        return {"isEmpty": {"field": field}}

    @staticmethod
    def prefix(field: str, value: str) -> FilterJson:
        """The field starts with `value`."""
        return {"prefix": {"field": field, "value": value}}

    @staticmethod
    def wildcard(field: str, pattern: str) -> FilterJson:
        """The field matches a `*`/`?` pattern."""
        return {"wildcard": {"field": field, "pattern": pattern}}

    @staticmethod
    def fuzzy(field: str, value: str, edits: Fuzziness | None = None) -> FilterJson:
        """The field is within `edits` edits of `value`."""
        query: dict[str, Any] = {"field": field, "value": value}
        if edits is not None:
            query["fuzziness"] = _fuzziness(edits)
        return {"fuzzy": query}

    @staticmethod
    def match(
        field: str,
        text: str,
        *,
        operator: Literal["or", "and"] | None = None,
        minimum_should_match: int | str | None = None,
        fuzziness: Fuzziness | None = None,
        analyzer: str | None = None,
    ) -> FilterJson:
        """A full-text match on one field."""
        query: dict[str, Any] = {"field": field, "text": text}
        if operator is not None:
            query["operator"] = operator
        if minimum_should_match is not None:
            query["minimumShouldMatch"] = str(minimum_should_match)
        if fuzziness is not None:
            query["fuzziness"] = _fuzziness(fuzziness)
        if analyzer is not None:
            query["analyzer"] = analyzer
        return {"match": query}

    @staticmethod
    def phrase(field: str, text: str, slop: int | None = None) -> FilterJson:
        """The words of `text`, in order, at most `slop` positions apart."""
        query: dict[str, Any] = {"field": field, "text": text}
        if slop is not None:
            query["slop"] = slop
        return {"matchPhrase": query}

    @staticmethod
    def multi_match(
        fields: Sequence[str],
        text: str,
        *,
        kind: Literal["best_fields", "most_fields", "cross_fields", "phrase", "phrase_prefix"]
        | None = None,
        operator: Literal["or", "and"] | None = None,
        tie_breaker: float | None = None,
    ) -> FilterJson:
        """A full-text match over several fields; `"title^2"` boosts one."""
        query: dict[str, Any] = {"fields": [_multi_match_field(f) for f in fields], "text": text}
        if kind is not None:
            query["kind"] = f"MULTI_MATCH_KIND_{kind.upper()}"
        if operator is not None:
            query["operator"] = _bool_operator(operator)
        if tie_breaker is not None:
            query["tieBreaker"] = tie_breaker
        return {"multiMatch": query}

    @staticmethod
    def query_string(
        query: str,
        *,
        fields: Sequence[str] | None = None,
        operator: Literal["or", "and"] | None = None,
    ) -> FilterJson:
        """The query-string syntax, over `fields` or the collection's defaults."""
        out: dict[str, Any] = {"query": query}
        if fields:
            out["defaultFields"] = list(fields)
        if operator is not None:
            out["defaultOperator"] = _bool_operator(operator)
        return {"queryString": out}

    @staticmethod
    def ids(*ids: DocId) -> FilterJson:
        """The documents with these ids, and no others."""
        return {"ids": [id_value(i) for i in ids]}

    @staticmethod
    def and_(*queries: FilterJson) -> FilterJson:
        """Every query matches (a bool `must`)."""
        return {"bool": {"must": list(queries)}}

    @staticmethod
    def or_(*queries: FilterJson) -> FilterJson:
        """At least one of the queries matches (a bool `should`)."""
        return {"bool": {"should": list(queries)}}

    @staticmethod
    def any_of(queries: Sequence[FilterJson], minimum_should_match: int | str) -> FilterJson:
        """At least `minimum_should_match` of the queries match."""
        return {
            "bool": {"should": list(queries), "minimumShouldMatch": str(minimum_should_match)}
        }

    @staticmethod
    def not_(*queries: FilterJson) -> FilterJson:
        """None of the queries match (a bool `must_not`)."""
        return {"bool": {"mustNot": list(queries)}}

    @staticmethod
    def filter(*queries: FilterJson) -> FilterJson:
        """Every query matches, without scoring (a bool `filter`)."""
        return {"bool": {"filter": list(queries)}}

    @staticmethod
    def boost(query: FilterJson, boost: float) -> FilterJson:
        """Multiplies the query's score by `boost`."""
        return {"boost": {"query": query, "boost": boost}}

    @staticmethod
    def constant_score(query: FilterJson, score: float) -> FilterJson:
        """Every match of `query` scores `score`."""
        return {"constantScore": {"query": query, "score": score}}


def to_query(filter_json: FilterJson) -> Message:
    """Decodes a filter into the generated `loams.collection.v1.Query`."""
    from loams.collection.v1.query_pb2 import Query

    return json_format.ParseDict(filter_json, Query())


# -- fusion and sort ---------------------------------------------------------


def rrf(k: int | None = None) -> dict[str, Any]:
    """Reciprocal-rank fusion; `k` absent is the engine's 60."""
    return {"rrf": {} if k is None else {"k": k}}


def dbsf() -> dict[str, Any]:
    """Distance-based score fusion, with the engine's parameters."""
    return {"dbsf": {}}


def weighted(*weights: float) -> dict[str, Any]:
    """A weighted sum, one weight per retriever in the order they were added."""
    return {"weightedSum": {"weights": list(weights)}}


class Fusion:
    """The fusions, as one namespace: `Fusion.rrf(60)`."""

    rrf = staticmethod(rrf)
    dbsf = staticmethod(dbsf)
    weighted = staticmethod(weighted)


def _order(direction: Order) -> str:
    return "SORT_ORDER_ASC" if direction == "asc" else "SORT_ORDER_DESC"


class Sort:
    """The sort keys. The primary key ascending always breaks ties on the
    server, but `query_all` needs it spelled out to page."""

    @staticmethod
    def score(direction: Order | None = None) -> dict[str, Any]:
        """By score; descending unless told otherwise."""
        return {"score": {} if direction is None else {"order": _order(direction)}}

    @staticmethod
    def pk(direction: Order | None = None) -> dict[str, Any]:
        """By primary key; ascending unless told otherwise."""
        return {"pk": {} if direction is None else {"order": _order(direction)}}

    @staticmethod
    def field(
        name: str,
        direction: Order | None = None,
        missing: Literal["first", "last"] | None = None,
    ) -> dict[str, Any]:
        """By a fast field. `missing` puts documents without it first or last."""
        key: dict[str, Any] = {"field": name}
        if direction is not None:
            key["order"] = _order(direction)
        if missing is not None:
            key["missing"] = "MISSING_ORDER_FIRST" if missing == "first" else "MISSING_ORDER_LAST"
        return {"field": key}


ConsistencyLevel = Union[Literal["strong", "eventual"], ConsistencyToken, str]


def consistency_json(level: ConsistencyLevel) -> dict[str, Any]:
    """The proto3 JSON of a `loams.collection.v1.Consistency`: `strong`,
    `eventual`, or a token (or its text) as `at_least`."""
    if level == "strong":
        return {"freshness": "FRESHNESS_STRONG"}
    if level == "eventual":
        return {"freshness": "FRESHNESS_EVENTUAL"}
    token = level if isinstance(level, ConsistencyToken) else ConsistencyToken.parse(level)
    return {"atLeast": str(token)}


def _sort_value(value: Any) -> Json:
    """A hit's sort value: a `google.protobuf.Value` message or plain JSON."""
    if isinstance(value, Message):
        result: Json = json_format.MessageToDict(value)
        return result
    return value  # type: ignore[no-any-return]


def _text_query(text: str | FilterJson, fields: Sequence[str] | None) -> FilterJson:
    if not isinstance(text, str):
        return text
    if not fields:
        return Filter.query_string(text)
    if len(fields) == 1:
        return Filter.match(fields[0], text)
    return Filter.multi_match(fields, text)


# -- the search builder ------------------------------------------------------


class Search:
    """The search builder: `Search("notes")`, or `Search.on("notes")`."""

    __slots__ = ("_json",)

    def __init__(self, collection: str, *, namespace: str | None = None) -> None:
        self._json: dict[str, Any] = {}
        if namespace is not None:
            self._json["namespace"] = namespace
        self._json["collection"] = collection

    @classmethod
    def on(cls, collection: str, *, namespace: str | None = None) -> Search:
        """A search of `collection` (a name or an alias)."""
        return cls(collection, namespace=namespace)

    @classmethod
    def from_json(cls, body: Mapping[str, Any]) -> Search:
        """A builder over an existing request's proto3 JSON."""
        out = cls.__new__(cls)
        out._json = copy.deepcopy(dict(body))
        return out

    def _with(self, **patch: Any) -> Search:
        out = Search.from_json(self._json)
        out._json.update(patch)
        return out

    def _retrievers(self) -> list[Any]:
        return list(self._json.get("retrievers", []))

    def namespace(self, namespace: str) -> Search:
        """The namespace, when it is not the client's default."""
        return self._with(namespace=namespace)

    def text(
        self,
        text: str | FilterJson,
        *,
        fields: Sequence[str] | None = None,
        k: int | None = None,
    ) -> Search:
        """A text retriever. A string is a `match` (one field), a `multi_match`
        (several) or a `query_string` (none); a `Filter` is used as is."""
        retriever: dict[str, Any] = {"query": _text_query(text, fields)}
        if k is not None:
            retriever["k"] = k
        return self._with(retrievers=[*self._retrievers(), {"text": retriever}])

    def vector(
        self,
        field: str,
        values: Sequence[float],
        *,
        k: int | None = None,
        exact: bool | None = None,
        nprobes: int | None = None,
        refine_factor: int | None = None,
        ef: int | None = None,
        oversampling: float | None = None,
        distance: str | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
    ) -> Search:
        """A dense-vector kNN retriever on `field`."""
        retriever: dict[str, Any] = {"field": field, "query": list(values)}
        if k is not None:
            retriever["k"] = k
        params: dict[str, Any] = {}
        for key, value in (
            ("exact", exact),
            ("nprobes", nprobes),
            ("refineFactor", refine_factor),
            ("ef", ef),
            ("oversampling", oversampling),
            ("distance", distance),
        ):
            if value is not None:
                params[key] = value
        if params:
            retriever["params"] = params
        if filter is not None:
            retriever["filter"] = filter
        return self._with(retrievers=[*self._retrievers(), {"vector": retriever}])

    def sparse(
        self,
        field: str,
        terms: Mapping[str, Sequence[float]] | Mapping[int, float],
        *,
        k: int | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
    ) -> Search:
        """A sparse-vector retriever: `{"indices": [...], "values": [...]}`, or an
        index-to-weight mapping."""
        raw: Mapping[Any, Any] = terms
        query: dict[str, Any]
        if "indices" in raw:
            query = {"indices": list(raw["indices"]), "values": list(raw["values"])}
        else:
            pairs = list(raw.items())
            query = {"indices": [int(i) for i, _ in pairs], "values": [v for _, v in pairs]}
        retriever: dict[str, Any] = {"field": field, "query": query}
        if k is not None:
            retriever["k"] = k
        if filter is not None:
            retriever["filter"] = filter
        return self._with(retrievers=[*self._retrievers(), {"sparse": retriever}])

    def fuse(self, fusion: Mapping[str, Any]) -> Search:
        """How the retrievers' lists combine: `rrf()`, `dbsf()` or `weighted(...)`."""
        return self._with(fusion=dict(fusion))

    def filter(self, filter_json: FilterJson) -> Search:
        """The filter every candidate must match."""
        return self._with(filter=filter_json)

    def sort(self, *keys: Mapping[str, Any]) -> Search:
        """The sort, replacing any earlier one."""
        return self._with(sort=[dict(key) for key in keys])

    def limit(self, limit: int) -> Search:
        """The window size; the engine's default is 10."""
        return self._with(limit=limit)

    def offset(self, offset: int) -> Search:
        """How many hits to skip before the window."""
        return self._with(offset=offset)

    def after(self, sort_values: Sequence[Any]) -> Search:
        """The page after the hit whose `sort_values` these are."""
        return self._with(searchAfter=[_sort_value(v) for v in sort_values])

    def min_score(self, score: float) -> Search:
        """Drops hits scoring below `score`."""
        return self._with(scoreThreshold=score)

    def select(
        self,
        *,
        include: Sequence[str] | None = None,
        exclude: Sequence[str] | None = None,
        no_source: bool = False,
        vectors: Sequence[str] | None = None,
        fields: Sequence[str] | None = None,
    ) -> Search:
        """What to return of each hit."""
        select: dict[str, Any] = {}
        if no_source:
            select["source"] = "none"
        elif include is not None or exclude is not None:
            source: dict[str, Any] = {}
            if include is not None:
                source["include"] = list(include)
            if exclude is not None:
                source["exclude"] = list(exclude)
            select["source"] = source
        if vectors is not None:
            select["vectors"] = list(vectors)
        if fields is not None:
            select["fields"] = list(fields)
        return self._with(select=select)

    def highlight(
        self,
        field: str,
        *,
        pre: str | None = None,
        post: str | None = None,
        fragment_size: int | None = None,
        fragments: int | None = None,
    ) -> Search:
        """Highlights `field` in each hit; call once per field."""
        entry: dict[str, Any] = {"field": field}
        for key, value in (
            ("preTag", pre),
            ("postTag", post),
            ("fragmentSize", fragment_size),
            ("numberOfFragments", fragments),
        ):
            if value is not None:
                entry[key] = value
        existing = list(self._json.get("highlight", {}).get("fields", []))
        return self._with(highlight={"fields": [*existing, entry]})

    def group_by(self, field: str, *, size: int | None = None, limit: int | None = None) -> Search:
        """Groups the hits by `field`: `size` hits per group, `limit` groups."""
        group: dict[str, Any] = {"field": field}
        if size is not None:
            group["groupSize"] = size
        if limit is not None:
            group["limit"] = limit
        return self._with(groupBy=group)

    def expand(
        self,
        graph: str,
        *,
        from_field: str | None = None,
        seed_label: str | None = None,
        seeds: int | None = None,
        hops: int | None = None,
        direction: Literal["out", "in", "both"] | None = None,
        edge_types: Sequence[str] | None = None,
        limit_per_seed: int | None = None,
        limit: int | None = None,
        output: Literal["nested", "flat"] | None = None,
    ) -> Search:
        """Walks `graph` from the top hits and returns their neighbours."""
        spec: dict[str, Any] = {"graph": graph}
        for key, value in (
            ("fromField", from_field),
            ("seedLabel", seed_label),
            ("seeds", seeds),
            ("hops", hops),
            ("limitPerSeed", limit_per_seed),
            ("limit", limit),
        ):
            if value is not None:
                spec[key] = value
        if direction is not None:
            spec["direction"] = f"EXPAND_DIRECTION_{direction.upper()}"
        if edge_types is not None:
            spec["edgeTypes"] = list(edge_types)
        if output is not None:
            spec["output"] = f"EXPAND_OUTPUT_{output.upper()}"
        return self._with(expandSpec=spec)

    def consistency(self, level: ConsistencyLevel) -> Search:
        """How fresh the read must be."""
        return self._with(consistency=consistency_json(level))

    @property
    def collection(self) -> str:
        """The collection searched."""
        return str(self._json["collection"])

    @property
    def has_tiebreak(self) -> bool:
        """Whether the sort ends in a primary-key tiebreaker, which `query_all`
        needs to page without skipping or repeating a hit."""
        keys = self._json.get("sort", [])
        return bool(keys) and "pk" in keys[-1]

    @property
    def page_size(self) -> int:
        """The window size `query_all` pages by: `limit`, or the engine's 10."""
        return int(self._json.get("limit", 10))

    def to_json(self) -> dict[str, Any]:
        """The request's proto3 JSON."""
        return copy.deepcopy(self._json)

    def build(self) -> Message:
        """The request as the generated `SearchRequest`, checked."""
        from loams.collection.v1.query_pb2 import SearchRequest

        weights = self._json.get("fusion", {}).get("weightedSum", {}).get("weights")
        retrievers = len(self._retrievers())
        if weights is not None and len(weights) != retrievers:
            raise InvalidArgumentError(
                "a weighted fusion needs one weight per retriever: "
                f"{len(weights)} weights for {retrievers} retrievers",
                code=Code.INVALID_ARGUMENT,
                metadata={"field": "fusion.weighted_sum.weights"},
            )
        return json_format.ParseDict(self._json, SearchRequest())


# -- the schema builder ------------------------------------------------------


class Schema:
    """`CreateCollectionRequest.schema`, a `Struct` in the REST route's schema JSON.

    The shape is `crates/loams/tests/connect_collections.rs` `kb_schema()`'s:
    every field spelled out (`name`, `source_path`, `kind`, `indexed`, `fast`),
    the vectors and sparse vectors as lists, and `dynamic` and `max_fields`
    always present, so nothing rides on a server default. Unlike `Search`, the
    verbs append to this builder and return it.
    """

    def __init__(self) -> None:
        self._fields: list[dict[str, Any]] = []
        self._vectors: list[dict[str, Any]] = []
        self._sparse: list[dict[str, Any]] = []
        self._dynamic = "ignore"
        self._max_fields = 1000

    def _field(
        self,
        name: str,
        kind: Json,
        fast_by_default: builtins.bool,
        indexed: builtins.bool | None,
        fast: builtins.bool | None,
        source_path: str | None,
    ) -> Schema:
        self._fields.append(
            {
                "name": name,
                "source_path": source_path if source_path is not None else name,
                "kind": kind,
                "indexed": True if indexed is None else indexed,
                "fast": fast_by_default if fast is None else fast,
            }
        )
        return self

    def text(
        self,
        name: str,
        *,
        analyzer: str = "standard",
        positions: builtins.bool = True,
        indexed: builtins.bool | None = None,
        fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """A full-text field."""
        kind = {"text": {"analyzer": analyzer, "positions": positions}}
        return self._field(name, kind, False, indexed, fast, source_path)

    def keyword(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """An exact-match string field."""
        return self._field(name, "keyword", True, indexed, fast, source_path)

    def i64(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """A signed 64-bit integer field."""
        return self._field(name, "i64", True, indexed, fast, source_path)

    def f64(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """A 64-bit float field."""
        return self._field(name, "f64", True, indexed, fast, source_path)

    def bool(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """A boolean field."""
        return self._field(name, "bool", True, indexed, fast, source_path)

    def timestamp(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """An RFC 3339 timestamp field (the schema's `date` kind)."""
        return self._field(name, "date", True, indexed, fast, source_path)

    def uuid(
        self, name: str, *, indexed: builtins.bool | None = None, fast: builtins.bool | None = None,
        source_path: str | None = None,
    ) -> Schema:
        """A UUID field."""
        return self._field(name, "uuid", True, indexed, fast, source_path)

    def vector(
        self,
        name: str,
        dim: int,
        distance: Literal["cosine", "dot", "euclid", "manhattan"] = "cosine",
    ) -> Schema:
        """A named dense vector of `dim` dimensions."""
        if dim <= 0:
            raise ValueError(f"a vector's dim must be a positive integer, not {dim}")
        self._vectors.append({"name": name, "dim": dim, "distance": distance})
        return self

    def sparse_vector(self, name: str, modifier: Literal["none", "idf"] | None = None) -> Schema:
        """A named sparse vector; `idf` reweights the query by IDF at search time."""
        entry: dict[str, Any] = {"name": name}
        if modifier is not None:
            entry["modifier"] = modifier
        self._sparse.append(entry)
        return self

    def dynamic(self, mode: Literal["ignore", "strict", "map"]) -> Schema:
        """What a write does with unmapped `_source` paths. Default `ignore`."""
        self._dynamic = mode
        return self

    def max_fields(self, n: int) -> Schema:
        """The most fields, vectors and sparse vectors together. Default 1000."""
        self._max_fields = n
        return self

    def to_json(self) -> dict[str, Any]:
        """The schema `Struct`, as `CreateCollectionRequest.schema` takes it."""
        return {
            "fields": copy.deepcopy(self._fields),
            "vectors": copy.deepcopy(self._vectors),
            "sparse_vectors": copy.deepcopy(self._sparse),
            "dynamic": self._dynamic,
            "max_fields": self._max_fields,
        }
