from google.protobuf import empty_pb2 as _empty_pb2
from google.protobuf import struct_pb2 as _struct_pb2
from loams.collection.v1 import document_pb2 as _document_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class BoolOperator(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    BOOL_OPERATOR_UNSPECIFIED: _ClassVar[BoolOperator]
    BOOL_OPERATOR_OR: _ClassVar[BoolOperator]
    BOOL_OPERATOR_AND: _ClassVar[BoolOperator]

class MultiMatchKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    MULTI_MATCH_KIND_UNSPECIFIED: _ClassVar[MultiMatchKind]
    MULTI_MATCH_KIND_BEST_FIELDS: _ClassVar[MultiMatchKind]
    MULTI_MATCH_KIND_MOST_FIELDS: _ClassVar[MultiMatchKind]
    MULTI_MATCH_KIND_CROSS_FIELDS: _ClassVar[MultiMatchKind]
    MULTI_MATCH_KIND_PHRASE: _ClassVar[MultiMatchKind]
    MULTI_MATCH_KIND_PHRASE_PREFIX: _ClassVar[MultiMatchKind]

class SortOrder(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SORT_ORDER_UNSPECIFIED: _ClassVar[SortOrder]
    SORT_ORDER_ASC: _ClassVar[SortOrder]
    SORT_ORDER_DESC: _ClassVar[SortOrder]

class MissingOrder(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    MISSING_ORDER_UNSPECIFIED: _ClassVar[MissingOrder]
    MISSING_ORDER_FIRST: _ClassVar[MissingOrder]
    MISSING_ORDER_LAST: _ClassVar[MissingOrder]

class ExpandDirection(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EXPAND_DIRECTION_UNSPECIFIED: _ClassVar[ExpandDirection]
    EXPAND_DIRECTION_OUT: _ClassVar[ExpandDirection]
    EXPAND_DIRECTION_IN: _ClassVar[ExpandDirection]
    EXPAND_DIRECTION_BOTH: _ClassVar[ExpandDirection]

class ExpandOutput(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EXPAND_OUTPUT_UNSPECIFIED: _ClassVar[ExpandOutput]
    EXPAND_OUTPUT_NESTED: _ClassVar[ExpandOutput]
    EXPAND_OUTPUT_FLAT: _ClassVar[ExpandOutput]

class TotalRelation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    TOTAL_RELATION_UNSPECIFIED: _ClassVar[TotalRelation]
    TOTAL_RELATION_EQ: _ClassVar[TotalRelation]
    TOTAL_RELATION_GTE: _ClassVar[TotalRelation]
BOOL_OPERATOR_UNSPECIFIED: BoolOperator
BOOL_OPERATOR_OR: BoolOperator
BOOL_OPERATOR_AND: BoolOperator
MULTI_MATCH_KIND_UNSPECIFIED: MultiMatchKind
MULTI_MATCH_KIND_BEST_FIELDS: MultiMatchKind
MULTI_MATCH_KIND_MOST_FIELDS: MultiMatchKind
MULTI_MATCH_KIND_CROSS_FIELDS: MultiMatchKind
MULTI_MATCH_KIND_PHRASE: MultiMatchKind
MULTI_MATCH_KIND_PHRASE_PREFIX: MultiMatchKind
SORT_ORDER_UNSPECIFIED: SortOrder
SORT_ORDER_ASC: SortOrder
SORT_ORDER_DESC: SortOrder
MISSING_ORDER_UNSPECIFIED: MissingOrder
MISSING_ORDER_FIRST: MissingOrder
MISSING_ORDER_LAST: MissingOrder
EXPAND_DIRECTION_UNSPECIFIED: ExpandDirection
EXPAND_DIRECTION_OUT: ExpandDirection
EXPAND_DIRECTION_IN: ExpandDirection
EXPAND_DIRECTION_BOTH: ExpandDirection
EXPAND_OUTPUT_UNSPECIFIED: ExpandOutput
EXPAND_OUTPUT_NESTED: ExpandOutput
EXPAND_OUTPUT_FLAT: ExpandOutput
TOTAL_RELATION_UNSPECIFIED: TotalRelation
TOTAL_RELATION_EQ: TotalRelation
TOTAL_RELATION_GTE: TotalRelation

class SearchRequest(_message.Message):
    __slots__ = ("namespace", "collection", "consistency", "retrievers", "fusion", "filter", "sort", "offset", "limit", "search_after", "score_threshold", "select", "aggregations", "highlight", "group_by", "track_total_hits", "rerank", "expand", "retrieve", "fuse", "expand_spec", "rerank_spec")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    RETRIEVERS_FIELD_NUMBER: _ClassVar[int]
    FUSION_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    SORT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    SEARCH_AFTER_FIELD_NUMBER: _ClassVar[int]
    SCORE_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    SELECT_FIELD_NUMBER: _ClassVar[int]
    AGGREGATIONS_FIELD_NUMBER: _ClassVar[int]
    HIGHLIGHT_FIELD_NUMBER: _ClassVar[int]
    GROUP_BY_FIELD_NUMBER: _ClassVar[int]
    TRACK_TOTAL_HITS_FIELD_NUMBER: _ClassVar[int]
    RERANK_FIELD_NUMBER: _ClassVar[int]
    EXPAND_FIELD_NUMBER: _ClassVar[int]
    FROM_FIELD_NUMBER: _ClassVar[int]
    RETRIEVE_FIELD_NUMBER: _ClassVar[int]
    FUSE_FIELD_NUMBER: _ClassVar[int]
    EXPAND_SPEC_FIELD_NUMBER: _ClassVar[int]
    RERANK_SPEC_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    consistency: _document_pb2.Consistency
    retrievers: _containers.RepeatedCompositeFieldContainer[Retriever]
    fusion: Fusion
    filter: Query
    sort: _containers.RepeatedCompositeFieldContainer[SortKey]
    offset: int
    limit: int
    search_after: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    score_threshold: float
    select: _struct_pb2.Struct
    aggregations: _struct_pb2.Value
    highlight: Highlight
    group_by: GroupBy
    track_total_hits: _struct_pb2.Value
    rerank: _struct_pb2.Struct
    expand: _struct_pb2.Struct
    retrieve: _containers.RepeatedCompositeFieldContainer[Retriever]
    fuse: _struct_pb2.Struct
    expand_spec: ExpandSpec
    rerank_spec: RerankSpec
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., consistency: _Optional[_Union[_document_pb2.Consistency, _Mapping]] = ..., retrievers: _Optional[_Iterable[_Union[Retriever, _Mapping]]] = ..., fusion: _Optional[_Union[Fusion, _Mapping]] = ..., filter: _Optional[_Union[Query, _Mapping]] = ..., sort: _Optional[_Iterable[_Union[SortKey, _Mapping]]] = ..., offset: _Optional[int] = ..., limit: _Optional[int] = ..., search_after: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ..., score_threshold: _Optional[float] = ..., select: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., aggregations: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., highlight: _Optional[_Union[Highlight, _Mapping]] = ..., group_by: _Optional[_Union[GroupBy, _Mapping]] = ..., track_total_hits: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., rerank: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., expand: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., retrieve: _Optional[_Iterable[_Union[Retriever, _Mapping]]] = ..., fuse: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., expand_spec: _Optional[_Union[ExpandSpec, _Mapping]] = ..., rerank_spec: _Optional[_Union[RerankSpec, _Mapping]] = ..., **kwargs) -> None: ...

class Retriever(_message.Message):
    __slots__ = ("vector", "text", "fused", "rescore", "sparse")
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    FUSED_FIELD_NUMBER: _ClassVar[int]
    RESCORE_FIELD_NUMBER: _ClassVar[int]
    SPARSE_FIELD_NUMBER: _ClassVar[int]
    vector: VectorQuery
    text: TextQuery
    fused: FusedQuery
    rescore: RescoreQuery
    sparse: SparseQuery
    def __init__(self, vector: _Optional[_Union[VectorQuery, _Mapping]] = ..., text: _Optional[_Union[TextQuery, _Mapping]] = ..., fused: _Optional[_Union[FusedQuery, _Mapping]] = ..., rescore: _Optional[_Union[RescoreQuery, _Mapping]] = ..., sparse: _Optional[_Union[SparseQuery, _Mapping]] = ...) -> None: ...

class VectorQuery(_message.Message):
    __slots__ = ("field", "query", "k", "params", "filter")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    PARAMS_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    field: str
    query: _containers.RepeatedScalarFieldContainer[float]
    k: int
    params: AnnParams
    filter: Query
    def __init__(self, field: _Optional[str] = ..., query: _Optional[_Iterable[float]] = ..., k: _Optional[int] = ..., params: _Optional[_Union[AnnParams, _Mapping]] = ..., filter: _Optional[_Union[Query, _Mapping]] = ...) -> None: ...

class TextQuery(_message.Message):
    __slots__ = ("query", "k")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    query: Query
    k: int
    def __init__(self, query: _Optional[_Union[Query, _Mapping]] = ..., k: _Optional[int] = ...) -> None: ...

class FusedQuery(_message.Message):
    __slots__ = ("inputs", "fusion", "k")
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    FUSION_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    inputs: _containers.RepeatedCompositeFieldContainer[Retriever]
    fusion: Fusion
    k: int
    def __init__(self, inputs: _Optional[_Iterable[_Union[Retriever, _Mapping]]] = ..., fusion: _Optional[_Union[Fusion, _Mapping]] = ..., k: _Optional[int] = ...) -> None: ...

class RescoreQuery(_message.Message):
    __slots__ = ("input", "field", "query", "k")
    INPUT_FIELD_NUMBER: _ClassVar[int]
    FIELD_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    input: Retriever
    field: str
    query: _containers.RepeatedScalarFieldContainer[float]
    k: int
    def __init__(self, input: _Optional[_Union[Retriever, _Mapping]] = ..., field: _Optional[str] = ..., query: _Optional[_Iterable[float]] = ..., k: _Optional[int] = ...) -> None: ...

class SparseQuery(_message.Message):
    __slots__ = ("field", "query", "k", "filter", "params")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    PARAMS_FIELD_NUMBER: _ClassVar[int]
    field: str
    query: _struct_pb2.Struct
    k: int
    filter: Query
    params: SparseParams
    def __init__(self, field: _Optional[str] = ..., query: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., k: _Optional[int] = ..., filter: _Optional[_Union[Query, _Mapping]] = ..., params: _Optional[_Union[SparseParams, _Mapping]] = ...) -> None: ...

class AnnParams(_message.Message):
    __slots__ = ("exact", "nprobes", "refine_factor", "ef", "oversampling", "distance")
    EXACT_FIELD_NUMBER: _ClassVar[int]
    NPROBES_FIELD_NUMBER: _ClassVar[int]
    REFINE_FACTOR_FIELD_NUMBER: _ClassVar[int]
    EF_FIELD_NUMBER: _ClassVar[int]
    OVERSAMPLING_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_FIELD_NUMBER: _ClassVar[int]
    exact: bool
    nprobes: int
    refine_factor: int
    ef: int
    oversampling: float
    distance: str
    def __init__(self, exact: _Optional[bool] = ..., nprobes: _Optional[int] = ..., refine_factor: _Optional[int] = ..., ef: _Optional[int] = ..., oversampling: _Optional[float] = ..., distance: _Optional[str] = ...) -> None: ...

class SparseParams(_message.Message):
    __slots__ = ("idf_corpus",)
    IDF_CORPUS_FIELD_NUMBER: _ClassVar[int]
    idf_corpus: Query
    def __init__(self, idf_corpus: _Optional[_Union[Query, _Mapping]] = ...) -> None: ...

class Fusion(_message.Message):
    __slots__ = ("rrf", "dbsf", "weighted_sum")
    RRF_FIELD_NUMBER: _ClassVar[int]
    DBSF_FIELD_NUMBER: _ClassVar[int]
    WEIGHTED_SUM_FIELD_NUMBER: _ClassVar[int]
    rrf: Rrf
    dbsf: _empty_pb2.Empty
    weighted_sum: WeightedSum
    def __init__(self, rrf: _Optional[_Union[Rrf, _Mapping]] = ..., dbsf: _Optional[_Union[_empty_pb2.Empty, _Mapping]] = ..., weighted_sum: _Optional[_Union[WeightedSum, _Mapping]] = ...) -> None: ...

class Rrf(_message.Message):
    __slots__ = ("k",)
    K_FIELD_NUMBER: _ClassVar[int]
    k: int
    def __init__(self, k: _Optional[int] = ...) -> None: ...

class WeightedSum(_message.Message):
    __slots__ = ("weights",)
    WEIGHTS_FIELD_NUMBER: _ClassVar[int]
    weights: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, weights: _Optional[_Iterable[float]] = ...) -> None: ...

class Query(_message.Message):
    __slots__ = ("match_all", "match_none", "match", "match_phrase", "multi_match", "term", "terms", "range", "exists", "is_null", "is_empty", "values_count", "prefix", "wildcard", "fuzzy", "query_string", "bool", "boost", "constant_score", "ids")
    MATCH_ALL_FIELD_NUMBER: _ClassVar[int]
    MATCH_NONE_FIELD_NUMBER: _ClassVar[int]
    MATCH_FIELD_NUMBER: _ClassVar[int]
    MATCH_PHRASE_FIELD_NUMBER: _ClassVar[int]
    MULTI_MATCH_FIELD_NUMBER: _ClassVar[int]
    TERM_FIELD_NUMBER: _ClassVar[int]
    TERMS_FIELD_NUMBER: _ClassVar[int]
    RANGE_FIELD_NUMBER: _ClassVar[int]
    EXISTS_FIELD_NUMBER: _ClassVar[int]
    IS_NULL_FIELD_NUMBER: _ClassVar[int]
    IS_EMPTY_FIELD_NUMBER: _ClassVar[int]
    VALUES_COUNT_FIELD_NUMBER: _ClassVar[int]
    PREFIX_FIELD_NUMBER: _ClassVar[int]
    WILDCARD_FIELD_NUMBER: _ClassVar[int]
    FUZZY_FIELD_NUMBER: _ClassVar[int]
    QUERY_STRING_FIELD_NUMBER: _ClassVar[int]
    BOOL_FIELD_NUMBER: _ClassVar[int]
    BOOST_FIELD_NUMBER: _ClassVar[int]
    CONSTANT_SCORE_FIELD_NUMBER: _ClassVar[int]
    IDS_FIELD_NUMBER: _ClassVar[int]
    match_all: _empty_pb2.Empty
    match_none: _empty_pb2.Empty
    match: MatchQuery
    match_phrase: MatchPhraseQuery
    multi_match: MultiMatchQuery
    term: TermQuery
    terms: TermsQuery
    range: RangeQuery
    exists: FieldQuery
    is_null: FieldQuery
    is_empty: FieldQuery
    values_count: ValuesCountQuery
    prefix: PrefixQuery
    wildcard: WildcardQuery
    fuzzy: FuzzyQuery
    query_string: QueryStringQuery
    bool: BoolQuery
    boost: BoostQuery
    constant_score: ConstantScoreQuery
    ids: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    def __init__(self, match_all: _Optional[_Union[_empty_pb2.Empty, _Mapping]] = ..., match_none: _Optional[_Union[_empty_pb2.Empty, _Mapping]] = ..., match: _Optional[_Union[MatchQuery, _Mapping]] = ..., match_phrase: _Optional[_Union[MatchPhraseQuery, _Mapping]] = ..., multi_match: _Optional[_Union[MultiMatchQuery, _Mapping]] = ..., term: _Optional[_Union[TermQuery, _Mapping]] = ..., terms: _Optional[_Union[TermsQuery, _Mapping]] = ..., range: _Optional[_Union[RangeQuery, _Mapping]] = ..., exists: _Optional[_Union[FieldQuery, _Mapping]] = ..., is_null: _Optional[_Union[FieldQuery, _Mapping]] = ..., is_empty: _Optional[_Union[FieldQuery, _Mapping]] = ..., values_count: _Optional[_Union[ValuesCountQuery, _Mapping]] = ..., prefix: _Optional[_Union[PrefixQuery, _Mapping]] = ..., wildcard: _Optional[_Union[WildcardQuery, _Mapping]] = ..., fuzzy: _Optional[_Union[FuzzyQuery, _Mapping]] = ..., query_string: _Optional[_Union[QueryStringQuery, _Mapping]] = ..., bool: _Optional[_Union[BoolQuery, _Mapping]] = ..., boost: _Optional[_Union[BoostQuery, _Mapping]] = ..., constant_score: _Optional[_Union[ConstantScoreQuery, _Mapping]] = ..., ids: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ...) -> None: ...

class MatchQuery(_message.Message):
    __slots__ = ("field", "text", "operator", "minimum_should_match", "fuzziness", "analyzer")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    OPERATOR_FIELD_NUMBER: _ClassVar[int]
    MINIMUM_SHOULD_MATCH_FIELD_NUMBER: _ClassVar[int]
    FUZZINESS_FIELD_NUMBER: _ClassVar[int]
    ANALYZER_FIELD_NUMBER: _ClassVar[int]
    field: str
    text: str
    operator: str
    minimum_should_match: str
    fuzziness: Fuzziness
    analyzer: str
    def __init__(self, field: _Optional[str] = ..., text: _Optional[str] = ..., operator: _Optional[str] = ..., minimum_should_match: _Optional[str] = ..., fuzziness: _Optional[_Union[Fuzziness, _Mapping]] = ..., analyzer: _Optional[str] = ...) -> None: ...

class MatchPhraseQuery(_message.Message):
    __slots__ = ("field", "text", "slop")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    SLOP_FIELD_NUMBER: _ClassVar[int]
    field: str
    text: str
    slop: int
    def __init__(self, field: _Optional[str] = ..., text: _Optional[str] = ..., slop: _Optional[int] = ...) -> None: ...

class MultiMatchQuery(_message.Message):
    __slots__ = ("fields", "text", "kind", "operator", "tie_breaker")
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    OPERATOR_FIELD_NUMBER: _ClassVar[int]
    TIE_BREAKER_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.RepeatedCompositeFieldContainer[MultiMatchField]
    text: str
    kind: MultiMatchKind
    operator: BoolOperator
    tie_breaker: float
    def __init__(self, fields: _Optional[_Iterable[_Union[MultiMatchField, _Mapping]]] = ..., text: _Optional[str] = ..., kind: _Optional[_Union[MultiMatchKind, str]] = ..., operator: _Optional[_Union[BoolOperator, str]] = ..., tie_breaker: _Optional[float] = ...) -> None: ...

class MultiMatchField(_message.Message):
    __slots__ = ("field", "boost")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    BOOST_FIELD_NUMBER: _ClassVar[int]
    field: str
    boost: float
    def __init__(self, field: _Optional[str] = ..., boost: _Optional[float] = ...) -> None: ...

class TermQuery(_message.Message):
    __slots__ = ("field", "value")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    field: str
    value: _struct_pb2.Value
    def __init__(self, field: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...

class TermsQuery(_message.Message):
    __slots__ = ("field", "values")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    VALUES_FIELD_NUMBER: _ClassVar[int]
    field: str
    values: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    def __init__(self, field: _Optional[str] = ..., values: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ...) -> None: ...

class RangeQuery(_message.Message):
    __slots__ = ("field", "gt", "gte", "lt", "lte")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    GT_FIELD_NUMBER: _ClassVar[int]
    GTE_FIELD_NUMBER: _ClassVar[int]
    LT_FIELD_NUMBER: _ClassVar[int]
    LTE_FIELD_NUMBER: _ClassVar[int]
    field: str
    gt: _struct_pb2.Value
    gte: _struct_pb2.Value
    lt: _struct_pb2.Value
    lte: _struct_pb2.Value
    def __init__(self, field: _Optional[str] = ..., gt: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., gte: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., lt: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., lte: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...

class FieldQuery(_message.Message):
    __slots__ = ("field",)
    FIELD_FIELD_NUMBER: _ClassVar[int]
    field: str
    def __init__(self, field: _Optional[str] = ...) -> None: ...

class ValuesCountQuery(_message.Message):
    __slots__ = ("field", "gt", "gte", "lt", "lte")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    GT_FIELD_NUMBER: _ClassVar[int]
    GTE_FIELD_NUMBER: _ClassVar[int]
    LT_FIELD_NUMBER: _ClassVar[int]
    LTE_FIELD_NUMBER: _ClassVar[int]
    field: str
    gt: int
    gte: int
    lt: int
    lte: int
    def __init__(self, field: _Optional[str] = ..., gt: _Optional[int] = ..., gte: _Optional[int] = ..., lt: _Optional[int] = ..., lte: _Optional[int] = ...) -> None: ...

class PrefixQuery(_message.Message):
    __slots__ = ("field", "value")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    field: str
    value: str
    def __init__(self, field: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class WildcardQuery(_message.Message):
    __slots__ = ("field", "pattern")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    PATTERN_FIELD_NUMBER: _ClassVar[int]
    field: str
    pattern: str
    def __init__(self, field: _Optional[str] = ..., pattern: _Optional[str] = ...) -> None: ...

class FuzzyQuery(_message.Message):
    __slots__ = ("field", "value", "fuzziness")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    FUZZINESS_FIELD_NUMBER: _ClassVar[int]
    field: str
    value: str
    fuzziness: Fuzziness
    def __init__(self, field: _Optional[str] = ..., value: _Optional[str] = ..., fuzziness: _Optional[_Union[Fuzziness, _Mapping]] = ...) -> None: ...

class QueryStringQuery(_message.Message):
    __slots__ = ("query", "default_fields", "default_operator")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    DEFAULT_FIELDS_FIELD_NUMBER: _ClassVar[int]
    DEFAULT_OPERATOR_FIELD_NUMBER: _ClassVar[int]
    query: str
    default_fields: _containers.RepeatedScalarFieldContainer[str]
    default_operator: BoolOperator
    def __init__(self, query: _Optional[str] = ..., default_fields: _Optional[_Iterable[str]] = ..., default_operator: _Optional[_Union[BoolOperator, str]] = ...) -> None: ...

class BoolQuery(_message.Message):
    __slots__ = ("must", "should", "must_not", "filter", "minimum_should_match")
    MUST_FIELD_NUMBER: _ClassVar[int]
    SHOULD_FIELD_NUMBER: _ClassVar[int]
    MUST_NOT_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    MINIMUM_SHOULD_MATCH_FIELD_NUMBER: _ClassVar[int]
    must: _containers.RepeatedCompositeFieldContainer[Query]
    should: _containers.RepeatedCompositeFieldContainer[Query]
    must_not: _containers.RepeatedCompositeFieldContainer[Query]
    filter: _containers.RepeatedCompositeFieldContainer[Query]
    minimum_should_match: str
    def __init__(self, must: _Optional[_Iterable[_Union[Query, _Mapping]]] = ..., should: _Optional[_Iterable[_Union[Query, _Mapping]]] = ..., must_not: _Optional[_Iterable[_Union[Query, _Mapping]]] = ..., filter: _Optional[_Iterable[_Union[Query, _Mapping]]] = ..., minimum_should_match: _Optional[str] = ...) -> None: ...

class BoostQuery(_message.Message):
    __slots__ = ("query", "boost")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    BOOST_FIELD_NUMBER: _ClassVar[int]
    query: Query
    boost: float
    def __init__(self, query: _Optional[_Union[Query, _Mapping]] = ..., boost: _Optional[float] = ...) -> None: ...

class ConstantScoreQuery(_message.Message):
    __slots__ = ("query", "score")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    query: Query
    score: float
    def __init__(self, query: _Optional[_Union[Query, _Mapping]] = ..., score: _Optional[float] = ...) -> None: ...

class Fuzziness(_message.Message):
    __slots__ = ("auto", "edits")
    AUTO_FIELD_NUMBER: _ClassVar[int]
    EDITS_FIELD_NUMBER: _ClassVar[int]
    auto: _empty_pb2.Empty
    edits: int
    def __init__(self, auto: _Optional[_Union[_empty_pb2.Empty, _Mapping]] = ..., edits: _Optional[int] = ...) -> None: ...

class SortKey(_message.Message):
    __slots__ = ("score", "pk", "field")
    SCORE_FIELD_NUMBER: _ClassVar[int]
    PK_FIELD_NUMBER: _ClassVar[int]
    FIELD_FIELD_NUMBER: _ClassVar[int]
    score: ScoreSort
    pk: PrimaryKeySort
    field: FieldSort
    def __init__(self, score: _Optional[_Union[ScoreSort, _Mapping]] = ..., pk: _Optional[_Union[PrimaryKeySort, _Mapping]] = ..., field: _Optional[_Union[FieldSort, _Mapping]] = ...) -> None: ...

class ScoreSort(_message.Message):
    __slots__ = ("order",)
    ORDER_FIELD_NUMBER: _ClassVar[int]
    order: SortOrder
    def __init__(self, order: _Optional[_Union[SortOrder, str]] = ...) -> None: ...

class PrimaryKeySort(_message.Message):
    __slots__ = ("order",)
    ORDER_FIELD_NUMBER: _ClassVar[int]
    order: SortOrder
    def __init__(self, order: _Optional[_Union[SortOrder, str]] = ...) -> None: ...

class FieldSort(_message.Message):
    __slots__ = ("field", "order", "missing")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    ORDER_FIELD_NUMBER: _ClassVar[int]
    MISSING_FIELD_NUMBER: _ClassVar[int]
    field: str
    order: SortOrder
    missing: MissingOrder
    def __init__(self, field: _Optional[str] = ..., order: _Optional[_Union[SortOrder, str]] = ..., missing: _Optional[_Union[MissingOrder, str]] = ...) -> None: ...

class Highlight(_message.Message):
    __slots__ = ("fields",)
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.RepeatedCompositeFieldContainer[HighlightField]
    def __init__(self, fields: _Optional[_Iterable[_Union[HighlightField, _Mapping]]] = ...) -> None: ...

class HighlightField(_message.Message):
    __slots__ = ("field", "pre_tag", "post_tag", "fragment_size", "number_of_fragments")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    PRE_TAG_FIELD_NUMBER: _ClassVar[int]
    POST_TAG_FIELD_NUMBER: _ClassVar[int]
    FRAGMENT_SIZE_FIELD_NUMBER: _ClassVar[int]
    NUMBER_OF_FRAGMENTS_FIELD_NUMBER: _ClassVar[int]
    field: str
    pre_tag: str
    post_tag: str
    fragment_size: int
    number_of_fragments: int
    def __init__(self, field: _Optional[str] = ..., pre_tag: _Optional[str] = ..., post_tag: _Optional[str] = ..., fragment_size: _Optional[int] = ..., number_of_fragments: _Optional[int] = ...) -> None: ...

class GroupBy(_message.Message):
    __slots__ = ("field", "group_size", "limit")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    GROUP_SIZE_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    field: str
    group_size: int
    limit: int
    def __init__(self, field: _Optional[str] = ..., group_size: _Optional[int] = ..., limit: _Optional[int] = ...) -> None: ...

class SearchResponse(_message.Message):
    __slots__ = ("hits", "total", "aggregations", "groups", "read_token", "hot_used", "performance", "expand")
    HITS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FIELD_NUMBER: _ClassVar[int]
    AGGREGATIONS_FIELD_NUMBER: _ClassVar[int]
    GROUPS_FIELD_NUMBER: _ClassVar[int]
    READ_TOKEN_FIELD_NUMBER: _ClassVar[int]
    HOT_USED_FIELD_NUMBER: _ClassVar[int]
    PERFORMANCE_FIELD_NUMBER: _ClassVar[int]
    EXPAND_FIELD_NUMBER: _ClassVar[int]
    hits: _containers.RepeatedCompositeFieldContainer[Hit]
    total: TotalHits
    aggregations: _struct_pb2.Value
    groups: _containers.RepeatedCompositeFieldContainer[HitGroup]
    read_token: str
    hot_used: _containers.RepeatedScalarFieldContainer[str]
    performance: _struct_pb2.Struct
    expand: ExpandStats
    def __init__(self, hits: _Optional[_Iterable[_Union[Hit, _Mapping]]] = ..., total: _Optional[_Union[TotalHits, _Mapping]] = ..., aggregations: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., groups: _Optional[_Iterable[_Union[HitGroup, _Mapping]]] = ..., read_token: _Optional[str] = ..., hot_used: _Optional[_Iterable[str]] = ..., performance: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., expand: _Optional[_Union[ExpandStats, _Mapping]] = ...) -> None: ...

class Hit(_message.Message):
    __slots__ = ("pk", "score", "sort_values", "source", "vectors", "sparse_vectors", "highlight", "fields", "neighbors", "graph")
    class VectorsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    class SparseVectorsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Struct
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...
    class HighlightEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    PK_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    SORT_VALUES_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    SPARSE_VECTORS_FIELD_NUMBER: _ClassVar[int]
    HIGHLIGHT_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    NEIGHBORS_FIELD_NUMBER: _ClassVar[int]
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    pk: _document_pb2.DocumentId
    score: float
    sort_values: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Value]
    source: _struct_pb2.Struct
    vectors: _containers.MessageMap[str, _struct_pb2.Value]
    sparse_vectors: _containers.MessageMap[str, _struct_pb2.Struct]
    highlight: _containers.MessageMap[str, _struct_pb2.Value]
    fields: _containers.MessageMap[str, _struct_pb2.Value]
    neighbors: _containers.RepeatedCompositeFieldContainer[Neighbor]
    graph: GraphHit
    def __init__(self, pk: _Optional[_Union[_document_pb2.DocumentId, _Mapping]] = ..., score: _Optional[float] = ..., sort_values: _Optional[_Iterable[_Union[_struct_pb2.Value, _Mapping]]] = ..., source: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., vectors: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., sparse_vectors: _Optional[_Mapping[str, _struct_pb2.Struct]] = ..., highlight: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., fields: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., neighbors: _Optional[_Iterable[_Union[Neighbor, _Mapping]]] = ..., graph: _Optional[_Union[GraphHit, _Mapping]] = ...) -> None: ...

class ExpandSpec(_message.Message):
    __slots__ = ("graph", "from_field", "seed_label", "seeds", "hops", "direction", "edge_types", "edge_filter", "vertex_filter", "limit_per_seed", "limit", "select", "output", "consistency_token")
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    FROM_FIELD_FIELD_NUMBER: _ClassVar[int]
    SEED_LABEL_FIELD_NUMBER: _ClassVar[int]
    SEEDS_FIELD_NUMBER: _ClassVar[int]
    HOPS_FIELD_NUMBER: _ClassVar[int]
    DIRECTION_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    EDGE_FILTER_FIELD_NUMBER: _ClassVar[int]
    VERTEX_FILTER_FIELD_NUMBER: _ClassVar[int]
    LIMIT_PER_SEED_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    SELECT_FIELD_NUMBER: _ClassVar[int]
    OUTPUT_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    graph: str
    from_field: str
    seed_label: str
    seeds: int
    hops: int
    direction: ExpandDirection
    edge_types: _containers.RepeatedScalarFieldContainer[str]
    edge_filter: Query
    vertex_filter: Query
    limit_per_seed: int
    limit: int
    select: _struct_pb2.Struct
    output: ExpandOutput
    consistency_token: str
    def __init__(self, graph: _Optional[str] = ..., from_field: _Optional[str] = ..., seed_label: _Optional[str] = ..., seeds: _Optional[int] = ..., hops: _Optional[int] = ..., direction: _Optional[_Union[ExpandDirection, str]] = ..., edge_types: _Optional[_Iterable[str]] = ..., edge_filter: _Optional[_Union[Query, _Mapping]] = ..., vertex_filter: _Optional[_Union[Query, _Mapping]] = ..., limit_per_seed: _Optional[int] = ..., limit: _Optional[int] = ..., select: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., output: _Optional[_Union[ExpandOutput, str]] = ..., consistency_token: _Optional[str] = ...) -> None: ...

class RerankSpec(_message.Message):
    __slots__ = ("inherit", "vector", "model")
    INHERIT_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    MODEL_FIELD_NUMBER: _ClassVar[int]
    inherit: InheritRerank
    vector: VectorRerank
    model: ModelRerank
    def __init__(self, inherit: _Optional[_Union[InheritRerank, _Mapping]] = ..., vector: _Optional[_Union[VectorRerank, _Mapping]] = ..., model: _Optional[_Union[ModelRerank, _Mapping]] = ...) -> None: ...

class InheritRerank(_message.Message):
    __slots__ = ("decay",)
    DECAY_FIELD_NUMBER: _ClassVar[int]
    decay: float
    def __init__(self, decay: _Optional[float] = ...) -> None: ...

class VectorRerank(_message.Message):
    __slots__ = ("field", "query")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    field: str
    query: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, field: _Optional[str] = ..., query: _Optional[_Iterable[float]] = ...) -> None: ...

class ModelRerank(_message.Message):
    __slots__ = ("endpoint", "text_field", "query", "top_n")
    ENDPOINT_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    TOP_N_FIELD_NUMBER: _ClassVar[int]
    endpoint: str
    text_field: str
    query: str
    top_n: int
    def __init__(self, endpoint: _Optional[str] = ..., text_field: _Optional[str] = ..., query: _Optional[str] = ..., top_n: _Optional[int] = ...) -> None: ...

class Neighbor(_message.Message):
    __slots__ = ("label", "key", "hop", "via", "edge_type", "edge", "score", "collection", "source")
    LABEL_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    HOP_FIELD_NUMBER: _ClassVar[int]
    VIA_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    EDGE_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    label: str
    key: _document_pb2.DocumentId
    hop: int
    via: _document_pb2.DocumentId
    edge_type: str
    edge: _document_pb2.DocumentId
    score: float
    collection: str
    source: _struct_pb2.Struct
    def __init__(self, label: _Optional[str] = ..., key: _Optional[_Union[_document_pb2.DocumentId, _Mapping]] = ..., hop: _Optional[int] = ..., via: _Optional[_Union[_document_pb2.DocumentId, _Mapping]] = ..., edge_type: _Optional[str] = ..., edge: _Optional[_Union[_document_pb2.DocumentId, _Mapping]] = ..., score: _Optional[float] = ..., collection: _Optional[str] = ..., source: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class GraphHit(_message.Message):
    __slots__ = ("label", "hop", "seeds", "collection")
    LABEL_FIELD_NUMBER: _ClassVar[int]
    HOP_FIELD_NUMBER: _ClassVar[int]
    SEEDS_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    label: str
    hop: int
    seeds: _containers.RepeatedCompositeFieldContainer[_document_pb2.DocumentId]
    collection: str
    def __init__(self, label: _Optional[str] = ..., hop: _Optional[int] = ..., seeds: _Optional[_Iterable[_Union[_document_pb2.DocumentId, _Mapping]]] = ..., collection: _Optional[str] = ...) -> None: ...

class ExpandStats(_message.Message):
    __slots__ = ("seeds", "seeds_skipped", "neighbors", "graph_token")
    SEEDS_FIELD_NUMBER: _ClassVar[int]
    SEEDS_SKIPPED_FIELD_NUMBER: _ClassVar[int]
    NEIGHBORS_FIELD_NUMBER: _ClassVar[int]
    GRAPH_TOKEN_FIELD_NUMBER: _ClassVar[int]
    seeds: int
    seeds_skipped: int
    neighbors: int
    graph_token: str
    def __init__(self, seeds: _Optional[int] = ..., seeds_skipped: _Optional[int] = ..., neighbors: _Optional[int] = ..., graph_token: _Optional[str] = ...) -> None: ...

class HitGroup(_message.Message):
    __slots__ = ("key", "hits")
    KEY_FIELD_NUMBER: _ClassVar[int]
    HITS_FIELD_NUMBER: _ClassVar[int]
    key: _struct_pb2.Value
    hits: _containers.RepeatedCompositeFieldContainer[Hit]
    def __init__(self, key: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., hits: _Optional[_Iterable[_Union[Hit, _Mapping]]] = ...) -> None: ...

class TotalHits(_message.Message):
    __slots__ = ("value", "relation")
    VALUE_FIELD_NUMBER: _ClassVar[int]
    RELATION_FIELD_NUMBER: _ClassVar[int]
    value: int
    relation: TotalRelation
    def __init__(self, value: _Optional[int] = ..., relation: _Optional[_Union[TotalRelation, str]] = ...) -> None: ...
