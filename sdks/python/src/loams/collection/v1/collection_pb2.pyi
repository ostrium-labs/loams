from google.protobuf import struct_pb2 as _struct_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class BackpressureState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    BACKPRESSURE_STATE_UNSPECIFIED: _ClassVar[BackpressureState]
    BACKPRESSURE_STATE_OPEN: _ClassVar[BackpressureState]
    BACKPRESSURE_STATE_THROTTLING: _ClassVar[BackpressureState]
    BACKPRESSURE_STATE_DISABLED: _ClassVar[BackpressureState]

class HotStateKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    HOT_STATE_KIND_UNSPECIFIED: _ClassVar[HotStateKind]
    HOT_STATE_KIND_OFF: _ClassVar[HotStateKind]
    HOT_STATE_KIND_BUILDING: _ClassVar[HotStateKind]
    HOT_STATE_KIND_READY: _ClassVar[HotStateKind]

class DeletionKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DELETION_KIND_UNSPECIFIED: _ClassVar[DeletionKind]
    DELETION_KIND_ARRAY: _ClassVar[DeletionKind]
    DELETION_KIND_BITMAP: _ClassVar[DeletionKind]
BACKPRESSURE_STATE_UNSPECIFIED: BackpressureState
BACKPRESSURE_STATE_OPEN: BackpressureState
BACKPRESSURE_STATE_THROTTLING: BackpressureState
BACKPRESSURE_STATE_DISABLED: BackpressureState
HOT_STATE_KIND_UNSPECIFIED: HotStateKind
HOT_STATE_KIND_OFF: HotStateKind
HOT_STATE_KIND_BUILDING: HotStateKind
HOT_STATE_KIND_READY: HotStateKind
DELETION_KIND_UNSPECIFIED: DeletionKind
DELETION_KIND_ARRAY: DeletionKind
DELETION_KIND_BITMAP: DeletionKind

class CreateNamespaceRequest(_message.Message):
    __slots__ = ("namespace",)
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    def __init__(self, namespace: _Optional[str] = ...) -> None: ...

class CreateNamespaceResponse(_message.Message):
    __slots__ = ("namespace", "namespace_id")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_ID_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    namespace_id: int
    def __init__(self, namespace: _Optional[str] = ..., namespace_id: _Optional[int] = ...) -> None: ...

class ListNamespacesRequest(_message.Message):
    __slots__ = ("page_size", "page_token")
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    page_size: int
    page_token: str
    def __init__(self, page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListNamespacesResponse(_message.Message):
    __slots__ = ("namespaces", "next_page_token")
    NAMESPACES_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespaces: _containers.RepeatedCompositeFieldContainer[NamespaceInfo]
    next_page_token: str
    def __init__(self, namespaces: _Optional[_Iterable[_Union[NamespaceInfo, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class NamespaceInfo(_message.Message):
    __slots__ = ("namespace", "namespace_id", "collection_count", "stream_count")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_ID_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_COUNT_FIELD_NUMBER: _ClassVar[int]
    STREAM_COUNT_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    namespace_id: int
    collection_count: int
    stream_count: int
    def __init__(self, namespace: _Optional[str] = ..., namespace_id: _Optional[int] = ..., collection_count: _Optional[int] = ..., stream_count: _Optional[int] = ...) -> None: ...

class CreateCollectionRequest(_message.Message):
    __slots__ = ("namespace", "name", "schema", "partitions")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    PARTITIONS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    name: str
    schema: _struct_pb2.Struct
    partitions: int
    def __init__(self, namespace: _Optional[str] = ..., name: _Optional[str] = ..., schema: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., partitions: _Optional[int] = ...) -> None: ...

class ListCollectionsRequest(_message.Message):
    __slots__ = ("namespace", "page_size", "page_token")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    page_size: int
    page_token: str
    def __init__(self, namespace: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListCollectionsResponse(_message.Message):
    __slots__ = ("collections", "next_page_token")
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    collections: _containers.RepeatedCompositeFieldContainer[CollectionInfo]
    next_page_token: str
    def __init__(self, collections: _Optional[_Iterable[_Union[CollectionInfo, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class GetCollectionRequest(_message.Message):
    __slots__ = ("namespace", "collection")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ...) -> None: ...

class DropCollectionRequest(_message.Message):
    __slots__ = ("namespace", "collection")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ...) -> None: ...

class DropCollectionResponse(_message.Message):
    __slots__ = ("dropped",)
    DROPPED_FIELD_NUMBER: _ClassVar[int]
    dropped: bool
    def __init__(self, dropped: _Optional[bool] = ...) -> None: ...

class AddFieldsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "fields", "vectors", "annotations")
    class AnnotationsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    ANNOTATIONS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    fields: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Struct]
    vectors: _containers.RepeatedCompositeFieldContainer[_struct_pb2.Struct]
    annotations: _containers.ScalarMap[str, str]
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., fields: _Optional[_Iterable[_Union[_struct_pb2.Struct, _Mapping]]] = ..., vectors: _Optional[_Iterable[_Union[_struct_pb2.Struct, _Mapping]]] = ..., annotations: _Optional[_Mapping[str, str]] = ...) -> None: ...

class AddFieldsResponse(_message.Message):
    __slots__ = ("schema",)
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    schema: _struct_pb2.Struct
    def __init__(self, schema: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class ListVersionsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "page_size", "page_token")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    page_size: int
    page_token: str
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., page_size: _Optional[int] = ..., page_token: _Optional[str] = ...) -> None: ...

class ListVersionsResponse(_message.Message):
    __slots__ = ("versions", "next_page_token")
    VERSIONS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    versions: _containers.RepeatedCompositeFieldContainer[ManifestVersion]
    next_page_token: str
    def __init__(self, versions: _Optional[_Iterable[_Union[ManifestVersion, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class ManifestVersion(_message.Message):
    __slots__ = ("version", "created_at_ms", "size_bytes", "live_doc_count", "lance_version")
    VERSION_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    LIVE_DOC_COUNT_FIELD_NUMBER: _ClassVar[int]
    LANCE_VERSION_FIELD_NUMBER: _ClassVar[int]
    version: int
    created_at_ms: int
    size_bytes: int
    live_doc_count: int
    lance_version: int
    def __init__(self, version: _Optional[int] = ..., created_at_ms: _Optional[int] = ..., size_bytes: _Optional[int] = ..., live_doc_count: _Optional[int] = ..., lance_version: _Optional[int] = ...) -> None: ...

class ScanRequest(_message.Message):
    __slots__ = ("namespace", "collection", "at")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    AT_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    at: ScanPoint
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., at: _Optional[_Union[ScanPoint, _Mapping]] = ...) -> None: ...

class ScanPoint(_message.Message):
    __slots__ = ("manifest_version", "token")
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    manifest_version: int
    token: str
    def __init__(self, manifest_version: _Optional[int] = ..., token: _Optional[str] = ...) -> None: ...

class UpdateAliasesRequest(_message.Message):
    __slots__ = ("namespace", "actions")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    ACTIONS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    actions: _containers.RepeatedCompositeFieldContainer[AliasAction]
    def __init__(self, namespace: _Optional[str] = ..., actions: _Optional[_Iterable[_Union[AliasAction, _Mapping]]] = ...) -> None: ...

class AliasAction(_message.Message):
    __slots__ = ("create", "delete")
    CREATE_FIELD_NUMBER: _ClassVar[int]
    DELETE_FIELD_NUMBER: _ClassVar[int]
    create: CreateAlias
    delete: DeleteAlias
    def __init__(self, create: _Optional[_Union[CreateAlias, _Mapping]] = ..., delete: _Optional[_Union[DeleteAlias, _Mapping]] = ...) -> None: ...

class CreateAlias(_message.Message):
    __slots__ = ("alias", "collection")
    ALIAS_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    alias: str
    collection: str
    def __init__(self, alias: _Optional[str] = ..., collection: _Optional[str] = ...) -> None: ...

class DeleteAlias(_message.Message):
    __slots__ = ("alias",)
    ALIAS_FIELD_NUMBER: _ClassVar[int]
    alias: str
    def __init__(self, alias: _Optional[str] = ...) -> None: ...

class UpdateAliasesResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class SetHotRequest(_message.Message):
    __slots__ = ("namespace", "collection", "hot")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    HOT_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    hot: HotConfig
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., hot: _Optional[_Union[HotConfig, _Mapping]] = ...) -> None: ...

class SetHotResponse(_message.Message):
    __slots__ = ("hot",)
    HOT_FIELD_NUMBER: _ClassVar[int]
    hot: HotStatus
    def __init__(self, hot: _Optional[_Union[HotStatus, _Mapping]] = ...) -> None: ...

class WarmCollectionRequest(_message.Message):
    __slots__ = ("namespace", "collection")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ...) -> None: ...

class WarmCollectionResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class HotConfig(_message.Message):
    __slots__ = ("vectors", "text", "fragments")
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    FRAGMENTS_FIELD_NUMBER: _ClassVar[int]
    vectors: bool
    text: bool
    fragments: bool
    def __init__(self, vectors: _Optional[bool] = ..., text: _Optional[bool] = ..., fragments: _Optional[bool] = ...) -> None: ...

class CollectionInfo(_message.Message):
    __slots__ = ("id", "name", "namespace", "schema", "partitions", "aliases", "stream", "manifest_version", "live_doc_count", "size_bytes", "created_at_ms", "link_lag_records", "hot", "unapplied_bytes", "backpressure")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    PARTITIONS_FIELD_NUMBER: _ClassVar[int]
    ALIASES_FIELD_NUMBER: _ClassVar[int]
    STREAM_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    LIVE_DOC_COUNT_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    LINK_LAG_RECORDS_FIELD_NUMBER: _ClassVar[int]
    HOT_FIELD_NUMBER: _ClassVar[int]
    UNAPPLIED_BYTES_FIELD_NUMBER: _ClassVar[int]
    BACKPRESSURE_FIELD_NUMBER: _ClassVar[int]
    id: int
    name: str
    namespace: str
    schema: _struct_pb2.Struct
    partitions: int
    aliases: _containers.RepeatedScalarFieldContainer[str]
    stream: int
    manifest_version: int
    live_doc_count: int
    size_bytes: int
    created_at_ms: int
    link_lag_records: int
    hot: HotStatus
    unapplied_bytes: int
    backpressure: BackpressureStatus
    def __init__(self, id: _Optional[int] = ..., name: _Optional[str] = ..., namespace: _Optional[str] = ..., schema: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., partitions: _Optional[int] = ..., aliases: _Optional[_Iterable[str]] = ..., stream: _Optional[int] = ..., manifest_version: _Optional[int] = ..., live_doc_count: _Optional[int] = ..., size_bytes: _Optional[int] = ..., created_at_ms: _Optional[int] = ..., link_lag_records: _Optional[int] = ..., hot: _Optional[_Union[HotStatus, _Mapping]] = ..., unapplied_bytes: _Optional[int] = ..., backpressure: _Optional[_Union[BackpressureStatus, _Mapping]] = ...) -> None: ...

class BackpressureStatus(_message.Message):
    __slots__ = ("state", "unapplied_records", "unapplied_bytes", "max_unapplied_records", "max_unapplied_bytes")
    STATE_FIELD_NUMBER: _ClassVar[int]
    UNAPPLIED_RECORDS_FIELD_NUMBER: _ClassVar[int]
    UNAPPLIED_BYTES_FIELD_NUMBER: _ClassVar[int]
    MAX_UNAPPLIED_RECORDS_FIELD_NUMBER: _ClassVar[int]
    MAX_UNAPPLIED_BYTES_FIELD_NUMBER: _ClassVar[int]
    state: BackpressureState
    unapplied_records: int
    unapplied_bytes: int
    max_unapplied_records: int
    max_unapplied_bytes: int
    def __init__(self, state: _Optional[_Union[BackpressureState, str]] = ..., unapplied_records: _Optional[int] = ..., unapplied_bytes: _Optional[int] = ..., max_unapplied_records: _Optional[int] = ..., max_unapplied_bytes: _Optional[int] = ...) -> None: ...

class HotStatus(_message.Message):
    __slots__ = ("enabled", "config", "pin_all", "promoted", "owner", "vectors", "text", "fragments")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    CONFIG_FIELD_NUMBER: _ClassVar[int]
    PIN_ALL_FIELD_NUMBER: _ClassVar[int]
    PROMOTED_FIELD_NUMBER: _ClassVar[int]
    OWNER_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    TEXT_FIELD_NUMBER: _ClassVar[int]
    FRAGMENTS_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    config: HotConfig
    pin_all: bool
    promoted: bool
    owner: HotOwner
    vectors: HotStructureStatus
    text: HotStructureStatus
    fragments: HotStructureStatus
    def __init__(self, enabled: _Optional[bool] = ..., config: _Optional[_Union[HotConfig, _Mapping]] = ..., pin_all: _Optional[bool] = ..., promoted: _Optional[bool] = ..., owner: _Optional[_Union[HotOwner, _Mapping]] = ..., vectors: _Optional[_Union[HotStructureStatus, _Mapping]] = ..., text: _Optional[_Union[HotStructureStatus, _Mapping]] = ..., fragments: _Optional[_Union[HotStructureStatus, _Mapping]] = ...) -> None: ...

class HotOwner(_message.Message):
    __slots__ = ("node_id", "local")
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    LOCAL_FIELD_NUMBER: _ClassVar[int]
    node_id: int
    local: bool
    def __init__(self, node_id: _Optional[int] = ..., local: _Optional[bool] = ...) -> None: ...

class HotStructureStatus(_message.Message):
    __slots__ = ("state", "source_version", "over_budget", "columns", "pinned_splits", "splits", "prefetched_bytes", "bytes")
    class ColumnsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: HotColumnStatus
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[HotColumnStatus, _Mapping]] = ...) -> None: ...
    STATE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_VERSION_FIELD_NUMBER: _ClassVar[int]
    OVER_BUDGET_FIELD_NUMBER: _ClassVar[int]
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    PINNED_SPLITS_FIELD_NUMBER: _ClassVar[int]
    SPLITS_FIELD_NUMBER: _ClassVar[int]
    PREFETCHED_BYTES_FIELD_NUMBER: _ClassVar[int]
    BYTES_FIELD_NUMBER: _ClassVar[int]
    state: HotStateKind
    source_version: int
    over_budget: bool
    columns: _containers.MessageMap[str, HotColumnStatus]
    pinned_splits: int
    splits: int
    prefetched_bytes: int
    bytes: int
    def __init__(self, state: _Optional[_Union[HotStateKind, str]] = ..., source_version: _Optional[int] = ..., over_budget: _Optional[bool] = ..., columns: _Optional[_Mapping[str, HotColumnStatus]] = ..., pinned_splits: _Optional[int] = ..., splits: _Optional[int] = ..., prefetched_bytes: _Optional[int] = ..., bytes: _Optional[int] = ...) -> None: ...

class HotColumnStatus(_message.Message):
    __slots__ = ("state", "source_version", "artifact_source_version", "delta_rows", "error")
    STATE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_VERSION_FIELD_NUMBER: _ClassVar[int]
    ARTIFACT_SOURCE_VERSION_FIELD_NUMBER: _ClassVar[int]
    DELTA_ROWS_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    state: HotStateKind
    source_version: int
    artifact_source_version: int
    delta_rows: int
    error: str
    def __init__(self, state: _Optional[_Union[HotStateKind, str]] = ..., source_version: _Optional[int] = ..., artifact_source_version: _Optional[int] = ..., delta_rows: _Optional[int] = ..., error: _Optional[str] = ...) -> None: ...

class ScanPlan(_message.Message):
    __slots__ = ("namespace", "collection", "collection_id", "manifest_version", "schema_version", "lance", "fragments", "live_rows", "columns", "pk_encoding", "tail", "tail_records", "offsets", "durable_token", "pin", "planned_at_ms", "expires_at_ms")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_VERSION_FIELD_NUMBER: _ClassVar[int]
    LANCE_FIELD_NUMBER: _ClassVar[int]
    FRAGMENTS_FIELD_NUMBER: _ClassVar[int]
    LIVE_ROWS_FIELD_NUMBER: _ClassVar[int]
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    PK_ENCODING_FIELD_NUMBER: _ClassVar[int]
    TAIL_FIELD_NUMBER: _ClassVar[int]
    TAIL_RECORDS_FIELD_NUMBER: _ClassVar[int]
    OFFSETS_FIELD_NUMBER: _ClassVar[int]
    DURABLE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    PIN_FIELD_NUMBER: _ClassVar[int]
    PLANNED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_MS_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    collection_id: int
    manifest_version: int
    schema_version: int
    lance: LanceVersionRef
    fragments: _containers.RepeatedCompositeFieldContainer[ScanFragment]
    live_rows: int
    columns: _containers.RepeatedCompositeFieldContainer[ScanColumn]
    pk_encoding: str
    tail: bool
    tail_records: int
    offsets: _containers.RepeatedCompositeFieldContainer[ScanOffsets]
    durable_token: str
    pin: ScanPin
    planned_at_ms: int
    expires_at_ms: int
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., collection_id: _Optional[int] = ..., manifest_version: _Optional[int] = ..., schema_version: _Optional[int] = ..., lance: _Optional[_Union[LanceVersionRef, _Mapping]] = ..., fragments: _Optional[_Iterable[_Union[ScanFragment, _Mapping]]] = ..., live_rows: _Optional[int] = ..., columns: _Optional[_Iterable[_Union[ScanColumn, _Mapping]]] = ..., pk_encoding: _Optional[str] = ..., tail: _Optional[bool] = ..., tail_records: _Optional[int] = ..., offsets: _Optional[_Iterable[_Union[ScanOffsets, _Mapping]]] = ..., durable_token: _Optional[str] = ..., pin: _Optional[_Union[ScanPin, _Mapping]] = ..., planned_at_ms: _Optional[int] = ..., expires_at_ms: _Optional[int] = ...) -> None: ...

class LanceVersionRef(_message.Message):
    __slots__ = ("uri", "version", "manifest_path", "storage_format", "stable_row_ids")
    URI_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_PATH_FIELD_NUMBER: _ClassVar[int]
    STORAGE_FORMAT_FIELD_NUMBER: _ClassVar[int]
    STABLE_ROW_IDS_FIELD_NUMBER: _ClassVar[int]
    uri: str
    version: int
    manifest_path: str
    storage_format: str
    stable_row_ids: bool
    def __init__(self, uri: _Optional[str] = ..., version: _Optional[int] = ..., manifest_path: _Optional[str] = ..., storage_format: _Optional[str] = ..., stable_row_ids: _Optional[bool] = ...) -> None: ...

class ScanFragment(_message.Message):
    __slots__ = ("id", "physical_rows", "deleted_rows", "live_rows", "files", "deletion_file", "lance")
    ID_FIELD_NUMBER: _ClassVar[int]
    PHYSICAL_ROWS_FIELD_NUMBER: _ClassVar[int]
    DELETED_ROWS_FIELD_NUMBER: _ClassVar[int]
    LIVE_ROWS_FIELD_NUMBER: _ClassVar[int]
    FILES_FIELD_NUMBER: _ClassVar[int]
    DELETION_FILE_FIELD_NUMBER: _ClassVar[int]
    LANCE_FIELD_NUMBER: _ClassVar[int]
    id: int
    physical_rows: int
    deleted_rows: int
    live_rows: int
    files: _containers.RepeatedCompositeFieldContainer[ScanFile]
    deletion_file: ScanDeletionFile
    lance: _struct_pb2.Struct
    def __init__(self, id: _Optional[int] = ..., physical_rows: _Optional[int] = ..., deleted_rows: _Optional[int] = ..., live_rows: _Optional[int] = ..., files: _Optional[_Iterable[_Union[ScanFile, _Mapping]]] = ..., deletion_file: _Optional[_Union[ScanDeletionFile, _Mapping]] = ..., lance: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class ScanFile(_message.Message):
    __slots__ = ("path", "size_bytes")
    PATH_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    path: str
    size_bytes: int
    def __init__(self, path: _Optional[str] = ..., size_bytes: _Optional[int] = ...) -> None: ...

class ScanDeletionFile(_message.Message):
    __slots__ = ("path", "kind", "deleted_rows")
    PATH_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    DELETED_ROWS_FIELD_NUMBER: _ClassVar[int]
    path: str
    kind: DeletionKind
    deleted_rows: int
    def __init__(self, path: _Optional[str] = ..., kind: _Optional[_Union[DeletionKind, str]] = ..., deleted_rows: _Optional[int] = ...) -> None: ...

class ScanColumn(_message.Message):
    __slots__ = ("name", "data_type", "role", "vector", "dim", "distance", "modifier")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DATA_TYPE_FIELD_NUMBER: _ClassVar[int]
    ROLE_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    DIM_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_FIELD_NUMBER: _ClassVar[int]
    MODIFIER_FIELD_NUMBER: _ClassVar[int]
    name: str
    data_type: str
    role: str
    vector: str
    dim: int
    distance: str
    modifier: str
    def __init__(self, name: _Optional[str] = ..., data_type: _Optional[str] = ..., role: _Optional[str] = ..., vector: _Optional[str] = ..., dim: _Optional[int] = ..., distance: _Optional[str] = ..., modifier: _Optional[str] = ...) -> None: ...

class ScanOffsets(_message.Message):
    __slots__ = ("partition", "applied", "target")
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    APPLIED_FIELD_NUMBER: _ClassVar[int]
    TARGET_FIELD_NUMBER: _ClassVar[int]
    partition: int
    applied: int
    target: int
    def __init__(self, partition: _Optional[int] = ..., applied: _Optional[int] = ..., target: _Optional[int] = ...) -> None: ...

class ScanPin(_message.Message):
    __slots__ = ("manifest_version", "token")
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    manifest_version: int
    token: str
    def __init__(self, manifest_version: _Optional[int] = ..., token: _Optional[str] = ...) -> None: ...
