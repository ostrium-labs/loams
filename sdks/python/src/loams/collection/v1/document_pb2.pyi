from google.protobuf import struct_pb2 as _struct_pb2
from loams.options.v1 import options_pb2 as _options_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class OpResult(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    OP_RESULT_UNSPECIFIED: _ClassVar[OpResult]
    OP_RESULT_CREATED: _ClassVar[OpResult]
    OP_RESULT_UPDATED: _ClassVar[OpResult]
    OP_RESULT_DELETED: _ClassVar[OpResult]
    OP_RESULT_NOT_FOUND: _ClassVar[OpResult]
    OP_RESULT_NOOP: _ClassVar[OpResult]
    OP_RESULT_ACCEPTED: _ClassVar[OpResult]

class Freshness(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FRESHNESS_UNSPECIFIED: _ClassVar[Freshness]
    FRESHNESS_STRONG: _ClassVar[Freshness]
    FRESHNESS_EVENTUAL: _ClassVar[Freshness]
OP_RESULT_UNSPECIFIED: OpResult
OP_RESULT_CREATED: OpResult
OP_RESULT_UPDATED: OpResult
OP_RESULT_DELETED: OpResult
OP_RESULT_NOT_FOUND: OpResult
OP_RESULT_NOOP: OpResult
OP_RESULT_ACCEPTED: OpResult
FRESHNESS_UNSPECIFIED: Freshness
FRESHNESS_STRONG: Freshness
FRESHNESS_EVENTUAL: Freshness

class WriteDocumentsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "ops", "report_existence", "idempotency_key")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    OPS_FIELD_NUMBER: _ClassVar[int]
    REPORT_EXISTENCE_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    ops: _containers.RepeatedCompositeFieldContainer[WriteOp]
    report_existence: bool
    idempotency_key: str
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., ops: _Optional[_Iterable[_Union[WriteOp, _Mapping]]] = ..., report_existence: _Optional[bool] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class WriteDocumentsResponse(_message.Message):
    __slots__ = ("token", "results", "positions", "unapplied_records", "unapplied_bytes")
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    POSITIONS_FIELD_NUMBER: _ClassVar[int]
    UNAPPLIED_RECORDS_FIELD_NUMBER: _ClassVar[int]
    UNAPPLIED_BYTES_FIELD_NUMBER: _ClassVar[int]
    token: str
    results: _containers.RepeatedScalarFieldContainer[OpResult]
    positions: _containers.RepeatedCompositeFieldContainer[OpPosition]
    unapplied_records: int
    unapplied_bytes: int
    def __init__(self, token: _Optional[str] = ..., results: _Optional[_Iterable[_Union[OpResult, str]]] = ..., positions: _Optional[_Iterable[_Union[OpPosition, _Mapping]]] = ..., unapplied_records: _Optional[int] = ..., unapplied_bytes: _Optional[int] = ...) -> None: ...

class WriteOp(_message.Message):
    __slots__ = ("upsert", "delete", "patch")
    UPSERT_FIELD_NUMBER: _ClassVar[int]
    DELETE_FIELD_NUMBER: _ClassVar[int]
    PATCH_FIELD_NUMBER: _ClassVar[int]
    upsert: WriteDocument
    delete: DeleteDocument
    patch: Patch
    def __init__(self, upsert: _Optional[_Union[WriteDocument, _Mapping]] = ..., delete: _Optional[_Union[DeleteDocument, _Mapping]] = ..., patch: _Optional[_Union[Patch, _Mapping]] = ...) -> None: ...

class WriteDocument(_message.Message):
    __slots__ = ("id", "source", "vectors", "sparse_vectors")
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
    ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    SPARSE_VECTORS_FIELD_NUMBER: _ClassVar[int]
    id: DocumentId
    source: _struct_pb2.Struct
    vectors: _containers.MessageMap[str, _struct_pb2.Value]
    sparse_vectors: _containers.MessageMap[str, _struct_pb2.Struct]
    def __init__(self, id: _Optional[_Union[DocumentId, _Mapping]] = ..., source: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., vectors: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., sparse_vectors: _Optional[_Mapping[str, _struct_pb2.Struct]] = ...) -> None: ...

class DeleteDocument(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: DocumentId
    def __init__(self, id: _Optional[_Union[DocumentId, _Mapping]] = ...) -> None: ...

class Patch(_message.Message):
    __slots__ = ("id", "mode", "source", "delete_keys", "vectors", "sparse_vectors", "upsert")
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
    ID_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    DELETE_KEYS_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    SPARSE_VECTORS_FIELD_NUMBER: _ClassVar[int]
    UPSERT_FIELD_NUMBER: _ClassVar[int]
    id: DocumentId
    mode: str
    source: _struct_pb2.Struct
    delete_keys: _containers.RepeatedScalarFieldContainer[str]
    vectors: _containers.MessageMap[str, _struct_pb2.Value]
    sparse_vectors: _containers.MessageMap[str, _struct_pb2.Struct]
    upsert: WriteDocument
    def __init__(self, id: _Optional[_Union[DocumentId, _Mapping]] = ..., mode: _Optional[str] = ..., source: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., delete_keys: _Optional[_Iterable[str]] = ..., vectors: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., sparse_vectors: _Optional[_Mapping[str, _struct_pb2.Struct]] = ..., upsert: _Optional[_Union[WriteDocument, _Mapping]] = ...) -> None: ...

class DocumentId(_message.Message):
    __slots__ = ("uint", "string", "uuid")
    UINT_FIELD_NUMBER: _ClassVar[int]
    STRING_FIELD_NUMBER: _ClassVar[int]
    UUID_FIELD_NUMBER: _ClassVar[int]
    uint: int
    string: str
    uuid: str
    def __init__(self, uint: _Optional[int] = ..., string: _Optional[str] = ..., uuid: _Optional[str] = ...) -> None: ...

class OpPosition(_message.Message):
    __slots__ = ("partition", "seq_no")
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    SEQ_NO_FIELD_NUMBER: _ClassVar[int]
    partition: int
    seq_no: int
    def __init__(self, partition: _Optional[int] = ..., seq_no: _Optional[int] = ...) -> None: ...

class GetDocumentsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "ids", "select", "consistency")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    IDS_FIELD_NUMBER: _ClassVar[int]
    SELECT_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    ids: _containers.RepeatedCompositeFieldContainer[DocumentId]
    select: _struct_pb2.Struct
    consistency: Consistency
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., ids: _Optional[_Iterable[_Union[DocumentId, _Mapping]]] = ..., select: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ...) -> None: ...

class GetDocumentsResponse(_message.Message):
    __slots__ = ("documents", "read_token")
    DOCUMENTS_FIELD_NUMBER: _ClassVar[int]
    READ_TOKEN_FIELD_NUMBER: _ClassVar[int]
    documents: _containers.RepeatedCompositeFieldContainer[Document]
    read_token: str
    def __init__(self, documents: _Optional[_Iterable[_Union[Document, _Mapping]]] = ..., read_token: _Optional[str] = ...) -> None: ...

class ScrollDocumentsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "filter", "after", "limit", "select", "consistency", "page_token", "where")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    AFTER_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    SELECT_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    WHERE_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    filter: _struct_pb2.Value
    after: DocumentId
    limit: int
    select: _struct_pb2.Struct
    consistency: Consistency
    page_token: DocumentId
    where: _struct_pb2.Struct
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., filter: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., after: _Optional[_Union[DocumentId, _Mapping]] = ..., limit: _Optional[int] = ..., select: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ..., page_token: _Optional[_Union[DocumentId, _Mapping]] = ..., where: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class ScrollDocumentsResponse(_message.Message):
    __slots__ = ("documents", "next", "read_token")
    DOCUMENTS_FIELD_NUMBER: _ClassVar[int]
    NEXT_FIELD_NUMBER: _ClassVar[int]
    READ_TOKEN_FIELD_NUMBER: _ClassVar[int]
    documents: _containers.RepeatedCompositeFieldContainer[Document]
    next: DocumentId
    read_token: str
    def __init__(self, documents: _Optional[_Iterable[_Union[Document, _Mapping]]] = ..., next: _Optional[_Union[DocumentId, _Mapping]] = ..., read_token: _Optional[str] = ...) -> None: ...

class CountDocumentsRequest(_message.Message):
    __slots__ = ("namespace", "collection", "filter", "consistency", "where")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    WHERE_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    filter: _struct_pb2.Value
    consistency: Consistency
    where: _struct_pb2.Struct
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., filter: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ..., where: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class CountDocumentsResponse(_message.Message):
    __slots__ = ("count", "read_token")
    COUNT_FIELD_NUMBER: _ClassVar[int]
    READ_TOKEN_FIELD_NUMBER: _ClassVar[int]
    count: int
    read_token: str
    def __init__(self, count: _Optional[int] = ..., read_token: _Optional[str] = ...) -> None: ...

class DeleteByFilterRequest(_message.Message):
    __slots__ = ("namespace", "collection", "filter", "max_rows", "allow_partial", "cursor", "consistency", "where")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    ALLOW_PARTIAL_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    WHERE_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    filter: _struct_pb2.Value
    max_rows: int
    allow_partial: bool
    cursor: FilterWriteCursor
    consistency: Consistency
    where: _struct_pb2.Struct
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., filter: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., max_rows: _Optional[int] = ..., allow_partial: _Optional[bool] = ..., cursor: _Optional[_Union[FilterWriteCursor, _Mapping]] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ..., where: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class PatchByFilterRequest(_message.Message):
    __slots__ = ("namespace", "collection", "filter", "patch", "max_rows", "allow_partial", "cursor", "consistency", "where")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    FILTER_FIELD_NUMBER: _ClassVar[int]
    PATCH_FIELD_NUMBER: _ClassVar[int]
    MAX_ROWS_FIELD_NUMBER: _ClassVar[int]
    ALLOW_PARTIAL_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    CONSISTENCY_FIELD_NUMBER: _ClassVar[int]
    WHERE_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    collection: str
    filter: _struct_pb2.Value
    patch: Patch
    max_rows: int
    allow_partial: bool
    cursor: FilterWriteCursor
    consistency: Consistency
    where: _struct_pb2.Struct
    def __init__(self, namespace: _Optional[str] = ..., collection: _Optional[str] = ..., filter: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ..., patch: _Optional[_Union[Patch, _Mapping]] = ..., max_rows: _Optional[int] = ..., allow_partial: _Optional[bool] = ..., cursor: _Optional[_Union[FilterWriteCursor, _Mapping]] = ..., consistency: _Optional[_Union[Consistency, _Mapping]] = ..., where: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ...) -> None: ...

class FilterWriteResponse(_message.Message):
    __slots__ = ("matched", "affected", "written", "batches", "rows_remaining", "cursor", "token", "pin", "retry_after_ms")
    MATCHED_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_FIELD_NUMBER: _ClassVar[int]
    WRITTEN_FIELD_NUMBER: _ClassVar[int]
    BATCHES_FIELD_NUMBER: _ClassVar[int]
    ROWS_REMAINING_FIELD_NUMBER: _ClassVar[int]
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    PIN_FIELD_NUMBER: _ClassVar[int]
    RETRY_AFTER_MS_FIELD_NUMBER: _ClassVar[int]
    matched: int
    affected: int
    written: int
    batches: int
    rows_remaining: bool
    cursor: FilterWriteCursor
    token: str
    pin: Pin
    retry_after_ms: int
    def __init__(self, matched: _Optional[int] = ..., affected: _Optional[int] = ..., written: _Optional[int] = ..., batches: _Optional[int] = ..., rows_remaining: _Optional[bool] = ..., cursor: _Optional[_Union[FilterWriteCursor, _Mapping]] = ..., token: _Optional[str] = ..., pin: _Optional[_Union[Pin, _Mapping]] = ..., retry_after_ms: _Optional[int] = ...) -> None: ...

class FilterWriteCursor(_message.Message):
    __slots__ = ("after", "manifest_version", "token")
    AFTER_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    after: DocumentId
    manifest_version: int
    token: str
    def __init__(self, after: _Optional[_Union[DocumentId, _Mapping]] = ..., manifest_version: _Optional[int] = ..., token: _Optional[str] = ...) -> None: ...

class Document(_message.Message):
    __slots__ = ("id", "source", "vectors", "sparse_vectors", "fields", "seq_no", "partition")
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
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    SPARSE_VECTORS_FIELD_NUMBER: _ClassVar[int]
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    SEQ_NO_FIELD_NUMBER: _ClassVar[int]
    PARTITION_FIELD_NUMBER: _ClassVar[int]
    id: DocumentId
    source: _struct_pb2.Struct
    vectors: _containers.MessageMap[str, _struct_pb2.Value]
    sparse_vectors: _containers.MessageMap[str, _struct_pb2.Struct]
    fields: _containers.MessageMap[str, _struct_pb2.Value]
    seq_no: int
    partition: int
    def __init__(self, id: _Optional[_Union[DocumentId, _Mapping]] = ..., source: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., vectors: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., sparse_vectors: _Optional[_Mapping[str, _struct_pb2.Struct]] = ..., fields: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., seq_no: _Optional[int] = ..., partition: _Optional[int] = ...) -> None: ...

class Consistency(_message.Message):
    __slots__ = ("freshness", "at_least", "pin")
    FRESHNESS_FIELD_NUMBER: _ClassVar[int]
    AT_LEAST_FIELD_NUMBER: _ClassVar[int]
    PIN_FIELD_NUMBER: _ClassVar[int]
    freshness: Freshness
    at_least: str
    pin: Pin
    def __init__(self, freshness: _Optional[_Union[Freshness, str]] = ..., at_least: _Optional[str] = ..., pin: _Optional[_Union[Pin, _Mapping]] = ...) -> None: ...

class Pin(_message.Message):
    __slots__ = ("manifest_version", "token")
    MANIFEST_VERSION_FIELD_NUMBER: _ClassVar[int]
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    manifest_version: int
    token: str
    def __init__(self, manifest_version: _Optional[int] = ..., token: _Optional[str] = ...) -> None: ...
