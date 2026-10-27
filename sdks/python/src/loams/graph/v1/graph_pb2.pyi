import datetime

from google.protobuf import field_mask_pb2 as _field_mask_pb2
from google.protobuf import struct_pb2 as _struct_pb2
from google.protobuf import timestamp_pb2 as _timestamp_pb2
from loams.errors.v1 import errors_pb2 as _errors_pb2
from loams.operations.v1 import operations_pb2 as _operations_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class QueryLanguage(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    QUERY_LANGUAGE_UNSPECIFIED: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_GQL: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_CYPHER: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_SQL_PGQ: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_GREMLIN: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_GRAPHQL: _ClassVar[QueryLanguage]
    QUERY_LANGUAGE_SPARQL: _ClassVar[QueryLanguage]

class GraphMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    GRAPH_MODE_UNSPECIFIED: _ClassVar[GraphMode]
    GRAPH_MODE_OWNED: _ClassVar[GraphMode]
    GRAPH_MODE_LINKED: _ClassVar[GraphMode]

class GraphState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    GRAPH_STATE_UNSPECIFIED: _ClassVar[GraphState]
    GRAPH_STATE_READY: _ClassVar[GraphState]
    GRAPH_STATE_OPENING: _ClassVar[GraphState]
    GRAPH_STATE_EVICTED: _ClassVar[GraphState]
    GRAPH_STATE_RELOADING: _ClassVar[GraphState]
    GRAPH_STATE_DELETING: _ClassVar[GraphState]
    GRAPH_STATE_FAILED: _ClassVar[GraphState]

class StreamOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STREAM_OPERATION_UNSPECIFIED: _ClassVar[StreamOperation]
    STREAM_OPERATION_UPSERT_NODE: _ClassVar[StreamOperation]
    STREAM_OPERATION_DELETE_NODE: _ClassVar[StreamOperation]
    STREAM_OPERATION_UPSERT_EDGE: _ClassVar[StreamOperation]
    STREAM_OPERATION_DELETE_EDGE: _ClassVar[StreamOperation]

class DataFormat(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DATA_FORMAT_UNSPECIFIED: _ClassVar[DataFormat]
    DATA_FORMAT_PARQUET: _ClassVar[DataFormat]
    DATA_FORMAT_CSV: _ClassVar[DataFormat]
    DATA_FORMAT_JSONL: _ClassVar[DataFormat]

class Isolation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ISOLATION_UNSPECIFIED: _ClassVar[Isolation]
    ISOLATION_SNAPSHOT: _ClassVar[Isolation]
    ISOLATION_SERIALIZABLE: _ClassVar[Isolation]
QUERY_LANGUAGE_UNSPECIFIED: QueryLanguage
QUERY_LANGUAGE_GQL: QueryLanguage
QUERY_LANGUAGE_CYPHER: QueryLanguage
QUERY_LANGUAGE_SQL_PGQ: QueryLanguage
QUERY_LANGUAGE_GREMLIN: QueryLanguage
QUERY_LANGUAGE_GRAPHQL: QueryLanguage
QUERY_LANGUAGE_SPARQL: QueryLanguage
GRAPH_MODE_UNSPECIFIED: GraphMode
GRAPH_MODE_OWNED: GraphMode
GRAPH_MODE_LINKED: GraphMode
GRAPH_STATE_UNSPECIFIED: GraphState
GRAPH_STATE_READY: GraphState
GRAPH_STATE_OPENING: GraphState
GRAPH_STATE_EVICTED: GraphState
GRAPH_STATE_RELOADING: GraphState
GRAPH_STATE_DELETING: GraphState
GRAPH_STATE_FAILED: GraphState
STREAM_OPERATION_UNSPECIFIED: StreamOperation
STREAM_OPERATION_UPSERT_NODE: StreamOperation
STREAM_OPERATION_DELETE_NODE: StreamOperation
STREAM_OPERATION_UPSERT_EDGE: StreamOperation
STREAM_OPERATION_DELETE_EDGE: StreamOperation
DATA_FORMAT_UNSPECIFIED: DataFormat
DATA_FORMAT_PARQUET: DataFormat
DATA_FORMAT_CSV: DataFormat
DATA_FORMAT_JSONL: DataFormat
ISOLATION_UNSPECIFIED: Isolation
ISOLATION_SNAPSHOT: Isolation
ISOLATION_SERIALIZABLE: Isolation

class Graph(_message.Message):
    __slots__ = ("namespace", "name", "id", "mode", "languages", "mapping", "limits", "replicas", "state", "stats", "create_time", "version")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    ID_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    LANGUAGES_FIELD_NUMBER: _ClassVar[int]
    MAPPING_FIELD_NUMBER: _ClassVar[int]
    LIMITS_FIELD_NUMBER: _ClassVar[int]
    REPLICAS_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    CREATE_TIME_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    id: str
    mode: GraphMode
    languages: _containers.RepeatedScalarFieldContainer[QueryLanguage]
    mapping: GraphMapping
    limits: GraphLimits
    replicas: int
    state: GraphState
    stats: GraphStats
    create_time: _timestamp_pb2.Timestamp
    version: int
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., id: _Optional[str] = ..., mode: _Optional[_Union[GraphMode, str]] = ..., languages: _Optional[_Iterable[_Union[QueryLanguage, str]]] = ..., mapping: _Optional[_Union[GraphMapping, _Mapping]] = ..., limits: _Optional[_Union[GraphLimits, _Mapping]] = ..., replicas: _Optional[int] = ..., state: _Optional[_Union[GraphState, str]] = ..., stats: _Optional[_Union[GraphStats, _Mapping]] = ..., create_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., version: _Optional[int] = ...) -> None: ...

class GraphStats(_message.Message):
    __slots__ = ("nodes", "edges", "stored_bytes", "durable_offset")
    NODES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    STORED_BYTES_FIELD_NUMBER: _ClassVar[int]
    DURABLE_OFFSET_FIELD_NUMBER: _ClassVar[int]
    nodes: int
    edges: int
    stored_bytes: int
    durable_offset: int
    def __init__(self, nodes: _Optional[int] = ..., edges: _Optional[int] = ..., stored_bytes: _Optional[int] = ..., durable_offset: _Optional[int] = ...) -> None: ...

class GraphLimits(_message.Message):
    __slots__ = ("timeout_ms", "max_rows", "max_result_bytes", "memory_bytes", "max_path_hops")
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    MAX_RESULT_BYTES_FIELD_NUMBER: _ClassVar[int]
    MEMORY_BYTES_FIELD_NUMBER: _ClassVar[int]
    MAX_PATH_HOPS_FIELD_NUMBER: _ClassVar[int]
    timeout_ms: int
    max_rows: int
    max_result_bytes: int
    memory_bytes: int
    max_path_hops: int
    def __init__(self, timeout_ms: _Optional[int] = ..., max_rows: _Optional[int] = ..., max_result_bytes: _Optional[int] = ..., memory_bytes: _Optional[int] = ..., max_path_hops: _Optional[int] = ...) -> None: ...

class GraphMapping(_message.Message):
    __slots__ = ("vertices", "edges", "streams")
    VERTICES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    STREAMS_FIELD_NUMBER: _ClassVar[int]
    vertices: _containers.RepeatedCompositeFieldContainer[VertexMapping]
    edges: _containers.RepeatedCompositeFieldContainer[EdgeMapping]
    streams: _containers.RepeatedCompositeFieldContainer[StreamMapping]
    def __init__(self, vertices: _Optional[_Iterable[_Union[VertexMapping, _Mapping]]] = ..., edges: _Optional[_Iterable[_Union[EdgeMapping, _Mapping]]] = ..., streams: _Optional[_Iterable[_Union[StreamMapping, _Mapping]]] = ...) -> None: ...

class StreamMapping(_message.Message):
    __slots__ = ("stream", "rules")
    STREAM_FIELD_NUMBER: _ClassVar[int]
    RULES_FIELD_NUMBER: _ClassVar[int]
    stream: str
    rules: _containers.RepeatedCompositeFieldContainer[StreamRule]
    def __init__(self, stream: _Optional[str] = ..., rules: _Optional[_Iterable[_Union[StreamRule, _Mapping]]] = ...) -> None: ...

class StreamRule(_message.Message):
    __slots__ = ("name", "event_type", "operation", "label", "key_field", "source_label", "source_key_field", "destination_label", "destination_key_field", "edge_type", "edge_type_field", "edge_key_field", "properties_field")
    NAME_FIELD_NUMBER: _ClassVar[int]
    EVENT_TYPE_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    LABEL_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_FIELD_NUMBER: _ClassVar[int]
    SOURCE_LABEL_FIELD_NUMBER: _ClassVar[int]
    SOURCE_KEY_FIELD_FIELD_NUMBER: _ClassVar[int]
    DESTINATION_LABEL_FIELD_NUMBER: _ClassVar[int]
    DESTINATION_KEY_FIELD_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_FIELD_FIELD_NUMBER: _ClassVar[int]
    EDGE_KEY_FIELD_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_FIELD_NUMBER: _ClassVar[int]
    name: str
    event_type: str
    operation: StreamOperation
    label: str
    key_field: str
    source_label: str
    source_key_field: str
    destination_label: str
    destination_key_field: str
    edge_type: str
    edge_type_field: str
    edge_key_field: str
    properties_field: str
    def __init__(self, name: _Optional[str] = ..., event_type: _Optional[str] = ..., operation: _Optional[_Union[StreamOperation, str]] = ..., label: _Optional[str] = ..., key_field: _Optional[str] = ..., source_label: _Optional[str] = ..., source_key_field: _Optional[str] = ..., destination_label: _Optional[str] = ..., destination_key_field: _Optional[str] = ..., edge_type: _Optional[str] = ..., edge_type_field: _Optional[str] = ..., edge_key_field: _Optional[str] = ..., properties_field: _Optional[str] = ...) -> None: ...

class VertexMapping(_message.Message):
    __slots__ = ("label", "collection", "key_field")
    LABEL_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_FIELD_NUMBER: _ClassVar[int]
    label: str
    collection: str
    key_field: str
    def __init__(self, label: _Optional[str] = ..., collection: _Optional[str] = ..., key_field: _Optional[str] = ...) -> None: ...

class EdgeMapping(_message.Message):
    __slots__ = ("type", "collection", "source_label", "source_field", "destination_label", "destination_field", "type_field")
    TYPE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    SOURCE_LABEL_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_FIELD_NUMBER: _ClassVar[int]
    DESTINATION_LABEL_FIELD_NUMBER: _ClassVar[int]
    DESTINATION_FIELD_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_FIELD_NUMBER: _ClassVar[int]
    type: str
    collection: str
    source_label: str
    source_field: str
    destination_label: str
    destination_field: str
    type_field: str
    def __init__(self, type: _Optional[str] = ..., collection: _Optional[str] = ..., source_label: _Optional[str] = ..., source_field: _Optional[str] = ..., destination_label: _Optional[str] = ..., destination_field: _Optional[str] = ..., type_field: _Optional[str] = ...) -> None: ...

class GetEngineInfoRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class EngineInfo(_message.Message):
    __slots__ = ("engine_version", "languages", "gql_profile", "max_limits")
    ENGINE_VERSION_FIELD_NUMBER: _ClassVar[int]
    LANGUAGES_FIELD_NUMBER: _ClassVar[int]
    GQL_PROFILE_FIELD_NUMBER: _ClassVar[int]
    MAX_LIMITS_FIELD_NUMBER: _ClassVar[int]
    engine_version: str
    languages: _containers.RepeatedScalarFieldContainer[QueryLanguage]
    gql_profile: str
    max_limits: GraphLimits
    def __init__(self, engine_version: _Optional[str] = ..., languages: _Optional[_Iterable[_Union[QueryLanguage, str]]] = ..., gql_profile: _Optional[str] = ..., max_limits: _Optional[_Union[GraphLimits, _Mapping]] = ...) -> None: ...

class CreateGraphRequest(_message.Message):
    __slots__ = ("namespace", "name", "mode", "languages", "mapping", "limits", "replicas", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    LANGUAGES_FIELD_NUMBER: _ClassVar[int]
    MAPPING_FIELD_NUMBER: _ClassVar[int]
    LIMITS_FIELD_NUMBER: _ClassVar[int]
    REPLICAS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    mode: GraphMode
    languages: _containers.RepeatedScalarFieldContainer[QueryLanguage]
    mapping: GraphMapping
    limits: GraphLimits
    replicas: int
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., mode: _Optional[_Union[GraphMode, str]] = ..., languages: _Optional[_Iterable[_Union[QueryLanguage, str]]] = ..., mapping: _Optional[_Union[GraphMapping, _Mapping]] = ..., limits: _Optional[_Union[GraphLimits, _Mapping]] = ..., replicas: _Optional[int] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class GetGraphRequest(_message.Message):
    __slots__ = ("namespace", "name")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ...) -> None: ...

class ListGraphsRequest(_message.Message):
    __slots__ = ("namespace", "page_size", "page_token")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    page_size: int
    page_token: str
    def __init__(self, namespace: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListGraphsResponse(_message.Message):
    __slots__ = ("graphs", "next_page_token")
    GRAPHS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    graphs: _containers.RepeatedCompositeFieldContainer[Graph]
    next_page_token: str
    def __init__(self, graphs: _Optional[_Iterable[_Union[Graph, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class UpdateGraphRequest(_message.Message):
    __slots__ = ("graph", "update_mask", "expected_version", "idempotency_key")
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    UPDATE_MASK_FIELD_NUMBER: _ClassVar[int]
    EXPECTED_VERSION_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    graph: Graph
    update_mask: _field_mask_pb2.FieldMask
    expected_version: int
    idempotency_key: str
    def __init__(self, graph: _Optional[_Union[Graph, _Mapping]] = ..., update_mask: _Optional[_Union[_field_mask_pb2.FieldMask, _Mapping]] = ..., expected_version: _Optional[int] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class DeleteGraphRequest(_message.Message):
    __slots__ = ("namespace", "name", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class GetSchemaRequest(_message.Message):
    __slots__ = ("namespace", "name")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ...) -> None: ...

class GraphSchema(_message.Message):
    __slots__ = ("labels", "edge_types", "property_keys", "indexes")
    LABELS_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    PROPERTY_KEYS_FIELD_NUMBER: _ClassVar[int]
    INDEXES_FIELD_NUMBER: _ClassVar[int]
    labels: _containers.RepeatedCompositeFieldContainer[LabelInfo]
    edge_types: _containers.RepeatedCompositeFieldContainer[EdgeTypeInfo]
    property_keys: _containers.RepeatedScalarFieldContainer[str]
    indexes: _containers.RepeatedCompositeFieldContainer[IndexInfo]
    def __init__(self, labels: _Optional[_Iterable[_Union[LabelInfo, _Mapping]]] = ..., edge_types: _Optional[_Iterable[_Union[EdgeTypeInfo, _Mapping]]] = ..., property_keys: _Optional[_Iterable[str]] = ..., indexes: _Optional[_Iterable[_Union[IndexInfo, _Mapping]]] = ...) -> None: ...

class LabelInfo(_message.Message):
    __slots__ = ("label", "count")
    LABEL_FIELD_NUMBER: _ClassVar[int]
    COUNT_FIELD_NUMBER: _ClassVar[int]
    label: str
    count: int
    def __init__(self, label: _Optional[str] = ..., count: _Optional[int] = ...) -> None: ...

class EdgeTypeInfo(_message.Message):
    __slots__ = ("type", "count")
    TYPE_FIELD_NUMBER: _ClassVar[int]
    COUNT_FIELD_NUMBER: _ClassVar[int]
    type: str
    count: int
    def __init__(self, type: _Optional[str] = ..., count: _Optional[int] = ...) -> None: ...

class IndexInfo(_message.Message):
    __slots__ = ("name", "target", "properties", "kind")
    NAME_FIELD_NUMBER: _ClassVar[int]
    TARGET_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    name: str
    target: str
    properties: _containers.RepeatedScalarFieldContainer[str]
    kind: str
    def __init__(self, name: _Optional[str] = ..., target: _Optional[str] = ..., properties: _Optional[_Iterable[str]] = ..., kind: _Optional[str] = ...) -> None: ...

class RestoreGraphRequest(_message.Message):
    __slots__ = ("namespace", "name", "target_name", "consistency_token", "time", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    TARGET_NAME_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    target_name: str
    consistency_token: str
    time: _timestamp_pb2.Timestamp
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., target_name: _Optional[str] = ..., consistency_token: _Optional[str] = ..., time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class ExportGraphRequest(_message.Message):
    __slots__ = ("namespace", "name", "object_prefix", "format", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    OBJECT_PREFIX_FIELD_NUMBER: _ClassVar[int]
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    object_prefix: str
    format: DataFormat
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., object_prefix: _Optional[str] = ..., format: _Optional[_Union[DataFormat, str]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class ImportGraphRequest(_message.Message):
    __slots__ = ("namespace", "name", "object_key", "format", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    OBJECT_KEY_FIELD_NUMBER: _ClassVar[int]
    FORMAT_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    object_key: str
    format: DataFormat
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., object_key: _Optional[str] = ..., format: _Optional[_Union[DataFormat, str]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class Consistency(_message.Message):
    __slots__ = ("strong", "eventual", "at_least")
    STRONG_FIELD_NUMBER: _ClassVar[int]
    EVENTUAL_FIELD_NUMBER: _ClassVar[int]
    AT_LEAST_FIELD_NUMBER: _ClassVar[int]
    strong: Strong
    eventual: Eventual
    at_least: str
    def __init__(self, strong: _Optional[_Union[Strong, _Mapping]] = ..., eventual: _Optional[_Union[Eventual, _Mapping]] = ..., at_least: _Optional[str] = ...) -> None: ...

class Strong(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class Eventual(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class ExecuteRequest(_message.Message):
    __slots__ = ("namespace", "graph", "statement", "language", "parameters", "read_only", "consistency", "timeout_ms", "max_rows", "idempotency_key")
    class ParametersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    STATEMENT_FIELD_NUMBER: _ClassVar[int]
    LANGUAGE_FIELD_NUMBER: _ClassVar[int]
    PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    READ_ONLY_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    graph: str
    statement: str
    language: QueryLanguage
    parameters: _containers.MessageMap[str, Value]
    read_only: bool
    consistency: Consistency
    timeout_ms: int
    max_rows: int
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., graph: _Optional[str] = ..., statement: _Optional[str] = ..., language: _Optional[_Union[QueryLanguage, str]] = ..., parameters: _Optional[_Mapping[str, Value]] = ..., read_only: _Optional[bool] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ..., timeout_ms: _Optional[int] = ..., max_rows: _Optional[int] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class ExecuteStreamRequest(_message.Message):
    __slots__ = ("request", "chunk_rows")
    REQUEST_FIELD_NUMBER: _ClassVar[int]
    CHUNK_ROWS_FIELD_NUMBER: _ClassVar[int]
    request: ExecuteRequest
    chunk_rows: int
    def __init__(self, request: _Optional[_Union[ExecuteRequest, _Mapping]] = ..., chunk_rows: _Optional[int] = ...) -> None: ...

class ExecuteResponse(_message.Message):
    __slots__ = ("rows", "truncated", "counters", "consistency_token", "commit_epoch", "elapsed_nanos", "notifications")
    ROWS_FIELD_NUMBER: _ClassVar[int]
    TRUNCATED_FIELD_NUMBER: _ClassVar[int]
    COUNTERS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    COMMIT_EPOCH_FIELD_NUMBER: _ClassVar[int]
    ELAPSED_NANOS_FIELD_NUMBER: _ClassVar[int]
    NOTIFICATIONS_FIELD_NUMBER: _ClassVar[int]
    rows: RowSet
    truncated: bool
    counters: Counters
    consistency_token: str
    commit_epoch: int
    elapsed_nanos: int
    notifications: _containers.RepeatedCompositeFieldContainer[Notification]
    def __init__(self, rows: _Optional[_Union[RowSet, _Mapping]] = ..., truncated: _Optional[bool] = ..., counters: _Optional[_Union[Counters, _Mapping]] = ..., consistency_token: _Optional[str] = ..., commit_epoch: _Optional[int] = ..., elapsed_nanos: _Optional[int] = ..., notifications: _Optional[_Iterable[_Union[Notification, _Mapping]]] = ...) -> None: ...

class Counters(_message.Message):
    __slots__ = ("nodes_created", "nodes_deleted", "relationships_created", "relationships_deleted", "properties_set", "labels_added", "labels_removed")
    NODES_CREATED_FIELD_NUMBER: _ClassVar[int]
    NODES_DELETED_FIELD_NUMBER: _ClassVar[int]
    RELATIONSHIPS_CREATED_FIELD_NUMBER: _ClassVar[int]
    RELATIONSHIPS_DELETED_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_SET_FIELD_NUMBER: _ClassVar[int]
    LABELS_ADDED_FIELD_NUMBER: _ClassVar[int]
    LABELS_REMOVED_FIELD_NUMBER: _ClassVar[int]
    nodes_created: int
    nodes_deleted: int
    relationships_created: int
    relationships_deleted: int
    properties_set: int
    labels_added: int
    labels_removed: int
    def __init__(self, nodes_created: _Optional[int] = ..., nodes_deleted: _Optional[int] = ..., relationships_created: _Optional[int] = ..., relationships_deleted: _Optional[int] = ..., properties_set: _Optional[int] = ..., labels_added: _Optional[int] = ..., labels_removed: _Optional[int] = ...) -> None: ...

class Notification(_message.Message):
    __slots__ = ("gqlstatus", "message")
    GQLSTATUS_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    gqlstatus: str
    message: str
    def __init__(self, gqlstatus: _Optional[str] = ..., message: _Optional[str] = ...) -> None: ...

class RowSet(_message.Message):
    __slots__ = ("columns", "column_types", "rows")
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    COLUMN_TYPES_FIELD_NUMBER: _ClassVar[int]
    ROWS_FIELD_NUMBER: _ClassVar[int]
    columns: _containers.RepeatedScalarFieldContainer[str]
    column_types: _containers.RepeatedScalarFieldContainer[str]
    rows: _containers.RepeatedCompositeFieldContainer[Row]
    def __init__(self, columns: _Optional[_Iterable[str]] = ..., column_types: _Optional[_Iterable[str]] = ..., rows: _Optional[_Iterable[_Union[Row, _Mapping]]] = ...) -> None: ...

class Row(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedCompositeFieldContainer[Value]
    def __init__(self, values: _Optional[_Iterable[_Union[Value, _Mapping]]] = ...) -> None: ...

class ExecuteBatchRequest(_message.Message):
    __slots__ = ("namespace", "graph", "statements", "atomic", "language", "isolation", "if_version", "timeout_ms", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    STATEMENTS_FIELD_NUMBER: _ClassVar[int]
    ATOMIC_FIELD_NUMBER: _ClassVar[int]
    LANGUAGE_FIELD_NUMBER: _ClassVar[int]
    ISOLATION_FIELD_NUMBER: _ClassVar[int]
    IF_VERSION_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    graph: str
    statements: _containers.RepeatedCompositeFieldContainer[Statement]
    atomic: bool
    language: QueryLanguage
    isolation: Isolation
    if_version: int
    timeout_ms: int
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., graph: _Optional[str] = ..., statements: _Optional[_Iterable[_Union[Statement, _Mapping]]] = ..., atomic: _Optional[bool] = ..., language: _Optional[_Union[QueryLanguage, str]] = ..., isolation: _Optional[_Union[Isolation, str]] = ..., if_version: _Optional[int] = ..., timeout_ms: _Optional[int] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class Statement(_message.Message):
    __slots__ = ("statement", "parameters", "language")
    class ParametersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    STATEMENT_FIELD_NUMBER: _ClassVar[int]
    PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    LANGUAGE_FIELD_NUMBER: _ClassVar[int]
    statement: str
    parameters: _containers.MessageMap[str, Value]
    language: QueryLanguage
    def __init__(self, statement: _Optional[str] = ..., parameters: _Optional[_Mapping[str, Value]] = ..., language: _Optional[_Union[QueryLanguage, str]] = ...) -> None: ...

class ExecuteBatchResponse(_message.Message):
    __slots__ = ("results", "committed", "committed_through", "consistency_token", "error", "commit_epoch")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    COMMITTED_FIELD_NUMBER: _ClassVar[int]
    COMMITTED_THROUGH_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    COMMIT_EPOCH_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[ExecuteResponse]
    committed: bool
    committed_through: int
    consistency_token: str
    error: StatementError
    commit_epoch: int
    def __init__(self, results: _Optional[_Iterable[_Union[ExecuteResponse, _Mapping]]] = ..., committed: _Optional[bool] = ..., committed_through: _Optional[int] = ..., consistency_token: _Optional[str] = ..., error: _Optional[_Union[StatementError, _Mapping]] = ..., commit_epoch: _Optional[int] = ...) -> None: ...

class StatementError(_message.Message):
    __slots__ = ("index", "code", "message", "info")
    INDEX_FIELD_NUMBER: _ClassVar[int]
    CODE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    INFO_FIELD_NUMBER: _ClassVar[int]
    index: int
    code: str
    message: str
    info: _errors_pb2.ErrorInfo
    def __init__(self, index: _Optional[int] = ..., code: _Optional[str] = ..., message: _Optional[str] = ..., info: _Optional[_Union[_errors_pb2.ErrorInfo, _Mapping]] = ...) -> None: ...

class ResultChunk(_message.Message):
    __slots__ = ("columns", "column_types", "rows", "last", "counters", "consistency_token", "elapsed_nanos", "truncated", "commit_epoch", "notifications")
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    COLUMN_TYPES_FIELD_NUMBER: _ClassVar[int]
    ROWS_FIELD_NUMBER: _ClassVar[int]
    LAST_FIELD_NUMBER: _ClassVar[int]
    COUNTERS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_TOKEN_FIELD_NUMBER: _ClassVar[int]
    ELAPSED_NANOS_FIELD_NUMBER: _ClassVar[int]
    TRUNCATED_FIELD_NUMBER: _ClassVar[int]
    COMMIT_EPOCH_FIELD_NUMBER: _ClassVar[int]
    NOTIFICATIONS_FIELD_NUMBER: _ClassVar[int]
    columns: _containers.RepeatedScalarFieldContainer[str]
    column_types: _containers.RepeatedScalarFieldContainer[str]
    rows: _containers.RepeatedCompositeFieldContainer[Row]
    last: bool
    counters: Counters
    consistency_token: str
    elapsed_nanos: int
    truncated: bool
    commit_epoch: int
    notifications: _containers.RepeatedCompositeFieldContainer[Notification]
    def __init__(self, columns: _Optional[_Iterable[str]] = ..., column_types: _Optional[_Iterable[str]] = ..., rows: _Optional[_Iterable[_Union[Row, _Mapping]]] = ..., last: _Optional[bool] = ..., counters: _Optional[_Union[Counters, _Mapping]] = ..., consistency_token: _Optional[str] = ..., elapsed_nanos: _Optional[int] = ..., truncated: _Optional[bool] = ..., commit_epoch: _Optional[int] = ..., notifications: _Optional[_Iterable[_Union[Notification, _Mapping]]] = ...) -> None: ...

class ExplainRequest(_message.Message):
    __slots__ = ("namespace", "graph", "statement", "language", "parameters", "profile", "timeout_ms", "consistency")
    class ParametersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    GRAPH_FIELD_NUMBER: _ClassVar[int]
    STATEMENT_FIELD_NUMBER: _ClassVar[int]
    LANGUAGE_FIELD_NUMBER: _ClassVar[int]
    PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    PROFILE_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    graph: str
    statement: str
    language: QueryLanguage
    parameters: _containers.MessageMap[str, Value]
    profile: bool
    timeout_ms: int
    consistency: Consistency
    def __init__(self, namespace: _Optional[str] = ..., graph: _Optional[str] = ..., statement: _Optional[str] = ..., language: _Optional[_Union[QueryLanguage, str]] = ..., parameters: _Optional[_Mapping[str, Value]] = ..., profile: _Optional[bool] = ..., timeout_ms: _Optional[int] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ...) -> None: ...

class Plan(_message.Message):
    __slots__ = ("root", "text")
    ROOT_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    root: PlanOperator
    text: str
    def __init__(self, root: _Optional[_Union[PlanOperator, _Mapping]] = ..., text: _Optional[str] = ...) -> None: ...

class PlanOperator(_message.Message):
    __slots__ = ("name", "details", "children", "rows", "elapsed_nanos")
    class DetailsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    NAME_FIELD_NUMBER: _ClassVar[int]
    DETAILS_FIELD_NUMBER: _ClassVar[int]
    CHILDREN_FIELD_NUMBER: _ClassVar[int]
    ROWS_FIELD_NUMBER: _ClassVar[int]
    ELAPSED_NANOS_FIELD_NUMBER: _ClassVar[int]
    name: str
    details: _containers.ScalarMap[str, str]
    children: _containers.RepeatedCompositeFieldContainer[PlanOperator]
    rows: int
    elapsed_nanos: int
    def __init__(self, name: _Optional[str] = ..., details: _Optional[_Mapping[str, str]] = ..., children: _Optional[_Iterable[_Union[PlanOperator, _Mapping]]] = ..., rows: _Optional[int] = ..., elapsed_nanos: _Optional[int] = ...) -> None: ...

class Value(_message.Message):
    __slots__ = ("null", "boolean", "int64", "uint64", "float64", "string", "bytes", "decimal", "date", "local_time", "zoned_time", "local_datetime", "zoned_datetime", "duration", "list", "map", "node", "relationship", "path", "vector", "counter")
    NULL_FIELD_NUMBER: _ClassVar[int]
    BOOLEAN_FIELD_NUMBER: _ClassVar[int]
    INT64_FIELD_NUMBER: _ClassVar[int]
    UINT64_FIELD_NUMBER: _ClassVar[int]
    FLOAT64_FIELD_NUMBER: _ClassVar[int]
    STRING_FIELD_NUMBER: _ClassVar[int]
    BYTES_FIELD_NUMBER: _ClassVar[int]
    DECIMAL_FIELD_NUMBER: _ClassVar[int]
    DATE_FIELD_NUMBER: _ClassVar[int]
    LOCAL_TIME_FIELD_NUMBER: _ClassVar[int]
    ZONED_TIME_FIELD_NUMBER: _ClassVar[int]
    LOCAL_DATETIME_FIELD_NUMBER: _ClassVar[int]
    ZONED_DATETIME_FIELD_NUMBER: _ClassVar[int]
    DURATION_FIELD_NUMBER: _ClassVar[int]
    LIST_FIELD_NUMBER: _ClassVar[int]
    MAP_FIELD_NUMBER: _ClassVar[int]
    NODE_FIELD_NUMBER: _ClassVar[int]
    RELATIONSHIP_FIELD_NUMBER: _ClassVar[int]
    PATH_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    COUNTER_FIELD_NUMBER: _ClassVar[int]
    null: _struct_pb2.NullValue
    boolean: bool
    int64: int
    uint64: int
    float64: float
    string: str
    bytes: bytes
    decimal: Decimal
    date: Date
    local_time: LocalTime
    zoned_time: ZonedTime
    local_datetime: LocalDateTime
    zoned_datetime: ZonedDateTime
    duration: Duration
    list: ListValue
    map: MapValue
    node: Node
    relationship: Relationship
    path: Path
    vector: Vector
    counter: Counter
    def __init__(self, null: _Optional[_Union[_struct_pb2.NullValue, str]] = ..., boolean: _Optional[bool] = ..., int64: _Optional[int] = ..., uint64: _Optional[int] = ..., float64: _Optional[float] = ..., string: _Optional[str] = ..., bytes: _Optional[bytes] = ..., decimal: _Optional[_Union[Decimal, _Mapping]] = ..., date: _Optional[_Union[Date, _Mapping]] = ..., local_time: _Optional[_Union[LocalTime, _Mapping]] = ..., zoned_time: _Optional[_Union[ZonedTime, _Mapping]] = ..., local_datetime: _Optional[_Union[LocalDateTime, _Mapping]] = ..., zoned_datetime: _Optional[_Union[ZonedDateTime, _Mapping]] = ..., duration: _Optional[_Union[Duration, _Mapping]] = ..., list: _Optional[_Union[ListValue, _Mapping]] = ..., map: _Optional[_Union[MapValue, _Mapping]] = ..., node: _Optional[_Union[Node, _Mapping]] = ..., relationship: _Optional[_Union[Relationship, _Mapping]] = ..., path: _Optional[_Union[Path, _Mapping]] = ..., vector: _Optional[_Union[Vector, _Mapping]] = ..., counter: _Optional[_Union[Counter, _Mapping]] = ...) -> None: ...

class Decimal(_message.Message):
    __slots__ = ("value",)
    VALUE_FIELD_NUMBER: _ClassVar[int]
    value: str
    def __init__(self, value: _Optional[str] = ...) -> None: ...

class Date(_message.Message):
    __slots__ = ("year", "month", "day")
    YEAR_FIELD_NUMBER: _ClassVar[int]
    MONTH_FIELD_NUMBER: _ClassVar[int]
    DAY_FIELD_NUMBER: _ClassVar[int]
    year: int
    month: int
    day: int
    def __init__(self, year: _Optional[int] = ..., month: _Optional[int] = ..., day: _Optional[int] = ...) -> None: ...

class LocalTime(_message.Message):
    __slots__ = ("hour", "minute", "second", "nanosecond")
    HOUR_FIELD_NUMBER: _ClassVar[int]
    MINUTE_FIELD_NUMBER: _ClassVar[int]
    SECOND_FIELD_NUMBER: _ClassVar[int]
    NANOSECOND_FIELD_NUMBER: _ClassVar[int]
    hour: int
    minute: int
    second: int
    nanosecond: int
    def __init__(self, hour: _Optional[int] = ..., minute: _Optional[int] = ..., second: _Optional[int] = ..., nanosecond: _Optional[int] = ...) -> None: ...

class ZonedTime(_message.Message):
    __slots__ = ("time", "offset_seconds")
    TIME_FIELD_NUMBER: _ClassVar[int]
    OFFSET_SECONDS_FIELD_NUMBER: _ClassVar[int]
    time: LocalTime
    offset_seconds: int
    def __init__(self, time: _Optional[_Union[LocalTime, _Mapping]] = ..., offset_seconds: _Optional[int] = ...) -> None: ...

class LocalDateTime(_message.Message):
    __slots__ = ("date", "time")
    DATE_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    date: Date
    time: LocalTime
    def __init__(self, date: _Optional[_Union[Date, _Mapping]] = ..., time: _Optional[_Union[LocalTime, _Mapping]] = ...) -> None: ...

class ZonedDateTime(_message.Message):
    __slots__ = ("local", "offset_seconds")
    LOCAL_FIELD_NUMBER: _ClassVar[int]
    OFFSET_SECONDS_FIELD_NUMBER: _ClassVar[int]
    local: LocalDateTime
    offset_seconds: int
    def __init__(self, local: _Optional[_Union[LocalDateTime, _Mapping]] = ..., offset_seconds: _Optional[int] = ...) -> None: ...

class Duration(_message.Message):
    __slots__ = ("months", "days", "nanos")
    MONTHS_FIELD_NUMBER: _ClassVar[int]
    DAYS_FIELD_NUMBER: _ClassVar[int]
    NANOS_FIELD_NUMBER: _ClassVar[int]
    months: int
    days: int
    nanos: int
    def __init__(self, months: _Optional[int] = ..., days: _Optional[int] = ..., nanos: _Optional[int] = ...) -> None: ...

class ListValue(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedCompositeFieldContainer[Value]
    def __init__(self, values: _Optional[_Iterable[_Union[Value, _Mapping]]] = ...) -> None: ...

class MapValue(_message.Message):
    __slots__ = ("entries",)
    class EntriesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    ENTRIES_FIELD_NUMBER: _ClassVar[int]
    entries: _containers.MessageMap[str, Value]
    def __init__(self, entries: _Optional[_Mapping[str, Value]] = ...) -> None: ...

class Node(_message.Message):
    __slots__ = ("id", "labels", "properties")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    LABELS_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    id: int
    labels: _containers.RepeatedScalarFieldContainer[str]
    properties: _containers.MessageMap[str, Value]
    def __init__(self, id: _Optional[int] = ..., labels: _Optional[_Iterable[str]] = ..., properties: _Optional[_Mapping[str, Value]] = ...) -> None: ...

class Relationship(_message.Message):
    __slots__ = ("id", "type", "src", "dst", "properties")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[Value, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    SRC_FIELD_NUMBER: _ClassVar[int]
    DST_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    id: int
    type: str
    src: int
    dst: int
    properties: _containers.MessageMap[str, Value]
    def __init__(self, id: _Optional[int] = ..., type: _Optional[str] = ..., src: _Optional[int] = ..., dst: _Optional[int] = ..., properties: _Optional[_Mapping[str, Value]] = ...) -> None: ...

class Path(_message.Message):
    __slots__ = ("nodes", "relationships")
    NODES_FIELD_NUMBER: _ClassVar[int]
    RELATIONSHIPS_FIELD_NUMBER: _ClassVar[int]
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    relationships: _containers.RepeatedCompositeFieldContainer[Relationship]
    def __init__(self, nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ..., relationships: _Optional[_Iterable[_Union[Relationship, _Mapping]]] = ...) -> None: ...

class Vector(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, values: _Optional[_Iterable[float]] = ...) -> None: ...

class Counter(_message.Message):
    __slots__ = ("positive", "negative", "grow_only")
    class PositiveEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: int
        def __init__(self, key: _Optional[str] = ..., value: _Optional[int] = ...) -> None: ...
    class NegativeEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: int
        def __init__(self, key: _Optional[str] = ..., value: _Optional[int] = ...) -> None: ...
    POSITIVE_FIELD_NUMBER: _ClassVar[int]
    NEGATIVE_FIELD_NUMBER: _ClassVar[int]
    GROW_ONLY_FIELD_NUMBER: _ClassVar[int]
    positive: _containers.ScalarMap[str, int]
    negative: _containers.ScalarMap[str, int]
    grow_only: bool
    def __init__(self, positive: _Optional[_Mapping[str, int]] = ..., negative: _Optional[_Mapping[str, int]] = ..., grow_only: _Optional[bool] = ...) -> None: ...
