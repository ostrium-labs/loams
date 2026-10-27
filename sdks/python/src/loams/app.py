"""The hand-written half of the app modules (SDKP §2.3–§2.5, §4).

The generated facade gives one function per proto method. A developer building
an app wants a few more, each a thin loop or shorthand over those:

- `documents.upsert`, `delete`, `patch` and `write_batch` over `write`;
- `documents.scroll_all` over `scroll` (its cursor is a `DocumentId`, not an
  AIP-158 string, so the generic `<call>_all` does not fit it);
- `search.text`, `search.hybrid` and `search.query_all` over `search.query`;
- `sql.records` over `sql.query`, and `sql_records` for an answer in hand;
- `streams.subscribe` over `streams.fetch`;
- `<call>_all` for every paged call (`namespaces.list_all`,
  `collections.list_all`), over the generic paging iterator.

`attach_app_calls` and `attach_async_app_calls` add them to the module objects
the facade built, and only to modules that exist: a build whose facade has no
`documents` module gets no `documents.upsert`. Nothing here names an RPC path or
a retry class; those stay the generated bindings'.
"""

from __future__ import annotations

from collections.abc import (
    AsyncIterable,
    AsyncIterator,
    Callable,
    Iterable,
    Iterator,
    Mapping,
    Sequence,
)
from dataclasses import dataclass, field
from typing import Any, cast

from connectrpc.code import Code
from google.protobuf import json_format
from google.protobuf.message import Message

from loams._gen.facade import METHODS, CallBinding, ModuleBinding
from loams.builders import (
    ConsistencyLevel,
    DocId,
    FilterJson,
    Search,
    consistency_json,
    id_value,
    rrf,
)
from loams.runtime.batch import (
    DEFAULT_CHUNK_BYTES,
    DEFAULT_CHUNK_ROWS,
    BatchResult,
    async_write_batch,
    write_batch,
)
from loams.runtime.errors import InvalidArgumentError, LoamsError
from loams.runtime.options import CallOptions
from loams.runtime.pagination import async_paginate, paginate
from loams.runtime.subscribe import AsyncSubscription, SubscribeRequest, Subscription

__all__ = [
    "AsyncDocumentCalls",
    "AsyncSearchCalls",
    "AsyncSqlCalls",
    "AsyncStreamCalls",
    "Document",
    "DocumentCalls",
    "DocumentInput",
    "SearchCalls",
    "SqlCalls",
    "StreamCalls",
    "WriteOps",
    "attach_app_calls",
    "attach_async_app_calls",
    "document_id_json",
    "sql_records",
]

Unary = Callable[[Message, CallOptions | None], Any]


@dataclass(frozen=True, slots=True)
class Document:
    """One document to write."""

    id: DocId
    #: The document's `_source`.
    source: Mapping[str, Any] | None = None
    #: Dense vectors by name.
    vectors: Mapping[str, Sequence[float]] | None = None
    #: Sparse vectors by name: `{"indices": [...], "values": [...]}`.
    sparse_vectors: Mapping[str, Mapping[str, Sequence[float]]] | None = field(default=None)


#: A document as a `Document` or a mapping with the same keys.
DocumentInput = Document | Mapping[str, Any]


def document_id_json(doc_id: DocId) -> dict[str, Any]:
    """A document id as `DocumentId`'s proto3 JSON: `{"uint": "1"}`,
    `{"string": "a"}` or `{"uuid": "…"}`."""
    value = id_value(doc_id)
    if isinstance(value, str):
        return {"string": value}
    assert isinstance(value, dict)
    return value


def _document_json(doc: DocumentInput) -> dict[str, Any]:
    if isinstance(doc, Document):
        doc_id, source, vectors, sparse = doc.id, doc.source, doc.vectors, doc.sparse_vectors
    else:
        doc_id = doc["id"]
        source = doc.get("source")
        vectors = doc.get("vectors")
        sparse = doc.get("sparse_vectors")
    out: dict[str, Any] = {"id": document_id_json(doc_id)}
    if source is not None:
        out["source"] = dict(source)
    if vectors is not None:
        out["vectors"] = {name: list(values) for name, values in vectors.items()}
    if sparse is not None:
        out["sparseVectors"] = {name: dict(value) for name, value in sparse.items()}
    return out


class WriteOps:
    """The `WriteOp`s of the document conveniences, as proto3 JSON."""

    @staticmethod
    def upsert(doc: DocumentInput) -> dict[str, Any]:
        return {"upsert": _document_json(doc)}

    @staticmethod
    def delete(doc_id: DocId) -> dict[str, Any]:
        return {"delete": {"id": document_id_json(doc_id)}}

    @staticmethod
    def patch(
        doc_id: DocId,
        *,
        set: Mapping[str, Any] | None = None,  # noqa: A002 - SDKP §2.3's name
        unset: Sequence[str] | None = None,
        mode: str | None = None,
        vectors: Mapping[str, Sequence[float] | None] | None = None,
        upsert: DocumentInput | None = None,
    ) -> dict[str, Any]:
        patch: dict[str, Any] = {"id": document_id_json(doc_id)}
        if mode is not None:
            patch["mode"] = mode
        if set is not None:
            patch["source"] = dict(set)
        if unset:
            patch["deleteKeys"] = list(unset)
        if vectors is not None:
            patch["vectors"] = {k: (None if v is None else list(v)) for k, v in vectors.items()}
        if upsert is not None:
            patch["upsert"] = _document_json(upsert)
        return {"patch": patch}


def _write_request(collection: str, namespace: str | None, ops: Sequence[Mapping[str, Any]]) -> Message:
    from loams.collection.v1.document_pb2 import WriteDocumentsRequest

    body: dict[str, Any] = {"collection": collection, "ops": list(ops)}
    if namespace is not None:
        body["namespace"] = namespace
    return json_format.ParseDict(body, WriteDocumentsRequest())


def _scroll_request(
    collection: str,
    namespace: str | None,
    where: FilterJson | None,
    limit: int | None,
    select: Mapping[str, Any] | None,
    read: ConsistencyLevel | None,
    after: Message | None,
) -> Message:
    from loams.collection.v1.document_pb2 import ScrollDocumentsRequest

    body: dict[str, Any] = {"collection": collection}
    if namespace is not None:
        body["namespace"] = namespace
    if where is not None:
        # `where` is the Filter's proto3 JSON, a `Struct` on the wire (SDKP-12).
        body["where"] = where
    if limit is not None:
        body["limit"] = limit
    if select is not None:
        body["select"] = dict(select)
    if read is not None:
        body["consistency"] = consistency_json(read)
    request = json_format.ParseDict(body, ScrollDocumentsRequest())
    if after is not None:
        request.after.CopyFrom(after)  # type: ignore[arg-type]
    return request


def _has_next(page: object) -> Message | None:
    if page is None or not isinstance(page, Message):
        return None
    if not page.DESCRIPTOR.fields_by_name.get("next") or not page.HasField("next"):
        return None
    cursor = getattr(page, "next")
    assert isinstance(cursor, Message)
    return cursor if cursor.WhichOneof("id") is not None else None


# -- documents ---------------------------------------------------------------


class DocumentCalls:
    """The sync document conveniences over `documents.write` and `documents.scroll`."""

    def __init__(self, write: Unary, scroll: Unary | None) -> None:
        self._write = write
        self._scroll = scroll

    def upsert(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Writes (creates or replaces) documents in one call. Answers the
        response, whose `token` is the write's consistency token."""
        ops = [WriteOps.upsert(d) for d in docs]
        return self._write(_write_request(collection, namespace, ops), options)

    def delete(
        self,
        collection: str,
        ids: Iterable[DocId],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Removes documents by id in one call."""
        ops = [WriteOps.delete(i) for i in ids]
        return self._write(_write_request(collection, namespace, ops), options)

    def patch(
        self,
        collection: str,
        doc_id: DocId,
        *,
        set: Mapping[str, Any] | None = None,  # noqa: A002 - SDKP §2.3's name
        unset: Sequence[str] | None = None,
        mode: str | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Changes part of one document."""
        op = WriteOps.patch(doc_id, set=set, unset=unset, mode=mode)
        return self._write(_write_request(collection, namespace, [op]), options)

    def write_batch(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        chunk_rows: int = DEFAULT_CHUNK_ROWS,
        chunk_bytes: int = DEFAULT_CHUNK_BYTES,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> BatchResult:
        """A bulk upsert, chunked, one chunk in flight (R13)."""
        base = options or CallOptions()

        def send(ops: list[dict[str, Any]], key: str) -> object:
            return self._write(_write_request(collection, namespace, ops), _with_key(base, key))

        return write_batch(
            send,
            (WriteOps.upsert(d) for d in docs),
            chunk_rows=chunk_rows,
            chunk_bytes=chunk_bytes,
        )

    def scroll_all(
        self,
        collection: str,
        *,
        where: FilterJson | None = None,
        limit: int | None = None,
        select: Mapping[str, Any] | None = None,
        read: ConsistencyLevel | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Iterator[Any]:
        """Every document, page by page, following `next` until it is unset."""
        scroll = _need(self._scroll, "documents.scroll")
        after: Message | None = None
        while True:
            page = scroll(
                _scroll_request(collection, namespace, where, limit, select, read, after), options
            )
            yield from getattr(page, "documents", [])
            after = _has_next(page)
            if after is None:
                return


class AsyncDocumentCalls:
    """`DocumentCalls` for `AsyncLoams`."""

    def __init__(self, write: Unary, scroll: Unary | None) -> None:
        self._write = write
        self._scroll = scroll

    async def upsert(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        ops = [WriteOps.upsert(d) for d in docs]
        return await self._write(_write_request(collection, namespace, ops), options)

    async def delete(
        self,
        collection: str,
        ids: Iterable[DocId],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        ops = [WriteOps.delete(i) for i in ids]
        return await self._write(_write_request(collection, namespace, ops), options)

    async def patch(
        self,
        collection: str,
        doc_id: DocId,
        *,
        set: Mapping[str, Any] | None = None,  # noqa: A002 - SDKP §2.3's name
        unset: Sequence[str] | None = None,
        mode: str | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        op = WriteOps.patch(doc_id, set=set, unset=unset, mode=mode)
        return await self._write(_write_request(collection, namespace, [op]), options)

    async def write_batch(
        self,
        collection: str,
        docs: Iterable[DocumentInput] | AsyncIterable[DocumentInput],
        *,
        chunk_rows: int = DEFAULT_CHUNK_ROWS,
        chunk_bytes: int = DEFAULT_CHUNK_BYTES,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> BatchResult:
        base = options or CallOptions()

        async def send(ops: list[dict[str, Any]], key: str) -> object:
            answer: object = await self._write(
                _write_request(collection, namespace, ops), _with_key(base, key)
            )
            return answer

        async def ops() -> AsyncIterator[dict[str, Any]]:
            if isinstance(docs, AsyncIterable):
                async for doc in docs:
                    yield WriteOps.upsert(doc)
            else:
                for doc in docs:
                    yield WriteOps.upsert(doc)

        return await async_write_batch(
            send, ops(), chunk_rows=chunk_rows, chunk_bytes=chunk_bytes
        )

    async def scroll_all(
        self,
        collection: str,
        *,
        where: FilterJson | None = None,
        limit: int | None = None,
        select: Mapping[str, Any] | None = None,
        read: ConsistencyLevel | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> AsyncIterator[Any]:
        scroll = _need(self._scroll, "documents.scroll")
        after: Message | None = None
        while True:
            page = await scroll(
                _scroll_request(collection, namespace, where, limit, select, read, after), options
            )
            for doc in getattr(page, "documents", []):
                yield doc
            after = _has_next(page)
            if after is None:
                return


def _with_key(options: CallOptions, key: str) -> CallOptions:
    from dataclasses import replace

    return replace(options, idempotency_key=key)


def _need(fn: Unary | None, name: str) -> Unary:
    if fn is None:
        raise LoamsError(f"this SDK's facade has no {name}", code=Code.UNIMPLEMENTED)
    return fn


# -- search ------------------------------------------------------------------


def _shorthand(
    collection: str,
    namespace: str | None,
    filter_json: FilterJson | None,
    limit: int | None,
    builder: Callable[[Search], Search],
) -> Search:
    search = builder(Search(collection, namespace=namespace))
    if filter_json is not None:
        search = search.filter(filter_json)
    if limit is not None:
        search = search.limit(limit)
    return search


def _require_tiebreak(search: Search) -> None:
    if not search.has_tiebreak:
        raise InvalidArgumentError(
            "search.query_all needs a sort that ends in the primary key, such as "
            ".sort(Sort.score(), Sort.pk())",
            code=Code.INVALID_ARGUMENT,
            reason="search_after_needs_tiebreak",
            metadata={"field": "sort"},
        )


class SearchCalls:
    """The sync search conveniences over `search.query`."""

    def __init__(self, query: Unary) -> None:
        self._query = query

    def text(
        self,
        collection: str,
        text: str,
        *,
        fields: Sequence[str] | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        limit: int | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """A text search: one field is a `match`, several a `multi_match`, none a
        `query_string`."""
        search = _shorthand(collection, namespace, filter, limit, lambda s: s.text(text, fields=fields))
        return self._query(search.build(), options)

    def hybrid(
        self,
        collection: str,
        text: str,
        vector: Sequence[float],
        *,
        field: str,
        fields: Sequence[str] | None = None,
        fusion: Mapping[str, Any] | None = None,
        k: int | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        limit: int | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Text plus vector, fused (RRF by default)."""
        search = _shorthand(
            collection,
            namespace,
            filter,
            limit,
            lambda s: s.text(text, fields=fields, k=k)
            .vector(field, vector, k=k)
            .fuse(fusion if fusion is not None else rrf()),
        )
        return self._query(search.build(), options)

    def query_all(self, search: Search, options: CallOptions | None = None) -> Iterator[Any]:
        """Every hit, page after page, following `search_after` from the last
        hit's `sort_values` until a page comes back short.

        :raises InvalidArgumentError: before any call, when the sort does not end
            in a primary-key tiebreaker: without one, two hits with equal sort
            values could straddle a page and be skipped or repeated.
        """
        _require_tiebreak(search)
        page = search
        while True:
            answer = self._query(page.build(), options)
            hits = list(getattr(answer, "hits", []))
            yield from hits
            if len(hits) < search.page_size or not hits:
                return
            page = search.after(list(hits[-1].sort_values))


class AsyncSearchCalls:
    """`SearchCalls` for `AsyncLoams`."""

    def __init__(self, query: Unary) -> None:
        self._query = query

    async def text(
        self,
        collection: str,
        text: str,
        *,
        fields: Sequence[str] | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        limit: int | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        search = _shorthand(collection, namespace, filter, limit, lambda s: s.text(text, fields=fields))
        return await self._query(search.build(), options)

    async def hybrid(
        self,
        collection: str,
        text: str,
        vector: Sequence[float],
        *,
        field: str,
        fields: Sequence[str] | None = None,
        fusion: Mapping[str, Any] | None = None,
        k: int | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        limit: int | None = None,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        search = _shorthand(
            collection,
            namespace,
            filter,
            limit,
            lambda s: s.text(text, fields=fields, k=k)
            .vector(field, vector, k=k)
            .fuse(fusion if fusion is not None else rrf()),
        )
        return await self._query(search.build(), options)

    async def query_all(self, search: Search, options: CallOptions | None = None) -> AsyncIterator[Any]:
        _require_tiebreak(search)
        page = search
        while True:
            answer = await self._query(page.build(), options)
            hits = list(getattr(answer, "hits", []))
            for hit in hits:
                yield hit
            if len(hits) < search.page_size or not hits:
                return
            page = search.after(list(hits[-1].sort_values))


# -- streams -----------------------------------------------------------------


def _fetch_request(binding: CallBinding, fields: dict[str, Any]) -> Message:
    method = METHODS.get(binding.rpc)
    if method is None:
        raise LoamsError(f"the generated table has no method for {binding.rpc}", code=Code.INTERNAL)
    message_type: Any = method.input
    known = message_type.DESCRIPTOR.fields_by_name
    request: Message = message_type(**{k: v for k, v in fields.items() if k in known})
    return request


# -- sql ---------------------------------------------------------------------

_SQL_QUERY = "loams.sql.v1.SqlService/Query"


def sql_records(answer: object) -> list[dict[str, Any]]:
    """A `sql.query` answer's rows as one dict per row, keyed by column name
    (SDKP §2.5). A value is its JSON: a 64-bit integer past 2^53 is the decimal
    string the server sent, so it stays exact. A column name that repeats keeps
    its last value."""
    names = [column.name for column in getattr(answer, "columns", [])]
    records: list[dict[str, Any]] = []
    for row in getattr(answer, "rows", []):
        # A `ListValue`'s JSON is a list, though the stubs type every message's
        # JSON as a dict.
        values = cast("list[Any]", json_format.MessageToDict(row))
        records.append({name: values[at] if at < len(values) else None for at, name in enumerate(names)})
    return records


def _sql_request(
    statement: str, params: Sequence[Any], namespace: str | None, max_rows: int | None
) -> Message:
    message_type: Any = METHODS[_SQL_QUERY].input
    fields: dict[str, Any] = {"sql": statement, "params": list(params)}
    if namespace is not None:
        fields["namespace"] = namespace
    if max_rows is not None:
        fields["maxRows"] = max_rows
    request: Message = json_format.ParseDict(fields, message_type())
    return request


class SqlCalls:
    """The sync SQL conveniences over `sql.query`."""

    def __init__(self, query: Unary) -> None:
        self._query = query

    def records(
        self,
        statement: str,
        params: Sequence[Any] = (),
        *,
        namespace: str | None = None,
        max_rows: int | None = None,
        options: CallOptions | None = None,
    ) -> list[dict[str, Any]]:
        """One read-only statement with its `$1…$n` values (JSON scalars, which
        the server casts to each placeholder's inferred type), answered as
        records (`sql_records`)."""
        return sql_records(self._query(_sql_request(statement, params, namespace, max_rows), options))


class AsyncSqlCalls:
    """`SqlCalls` for `AsyncLoams`."""

    def __init__(self, query: Unary) -> None:
        self._query = query

    async def records(
        self,
        statement: str,
        params: Sequence[Any] = (),
        *,
        namespace: str | None = None,
        max_rows: int | None = None,
        options: CallOptions | None = None,
    ) -> list[dict[str, Any]]:
        """`SqlCalls.records`, awaited."""
        answer = await self._query(_sql_request(statement, params, namespace, max_rows), options)
        return sql_records(answer)


class StreamCalls:
    """The sync stream conveniences over `streams.fetch`."""

    def __init__(self, fetch: Unary, binding: CallBinding) -> None:
        self._fetch = fetch
        self._binding = binding

    def subscribe(
        self,
        stream: str,
        *,
        partition: int = 0,
        from_offset: int = 0,
        namespace: str | None = None,
        max_records: int | None = None,
        max_wait_ms: int | None = None,
        options: CallOptions | None = None,
    ) -> Subscription:
        """Every record from `from_offset` on, as an iterator (R14)."""
        request = SubscribeRequest(
            stream, partition, from_offset, namespace, max_records, None, max_wait_ms
        )
        return Subscription(
            lambda fields: self._fetch(_fetch_request(self._binding, fields), options), request
        )


class AsyncStreamCalls:
    """`StreamCalls` for `AsyncLoams`."""

    def __init__(self, fetch: Unary, binding: CallBinding) -> None:
        self._fetch = fetch
        self._binding = binding

    def subscribe(
        self,
        stream: str,
        *,
        partition: int = 0,
        from_offset: int = 0,
        namespace: str | None = None,
        max_records: int | None = None,
        max_wait_ms: int | None = None,
        options: CallOptions | None = None,
    ) -> AsyncSubscription:
        """Every record from `from_offset` on, as an async iterator (R14)."""
        request = SubscribeRequest(
            stream, partition, from_offset, namespace, max_records, None, max_wait_ms
        )

        async def fetch(fields: dict[str, Any]) -> object:
            answer: object = await self._fetch(_fetch_request(self._binding, fields), options)
            return answer

        return AsyncSubscription(fetch, request)


# -- attaching ---------------------------------------------------------------


def _add(target: object, name: str, value: object) -> None:
    """Adds a convenience without overwriting a generated call of the same name."""
    if not hasattr(target, name):
        setattr(target, name, value)


def _attach_conveniences(target: object, conveniences: object) -> None:
    for name in dir(conveniences):
        if name.startswith("_"):
            continue
        _add(target, name, getattr(conveniences, name))


def _binding(bindings: Sequence[ModuleBinding], module: str, call: str) -> CallBinding | None:
    for entry in bindings:
        if entry.name == module:
            for candidate in entry.calls:
                if candidate.name == call:
                    return candidate
    return None


def _paged_all(binding: CallBinding, method: Unary, *, is_async: bool) -> Callable[..., Any]:
    def request_of(request: Message | None) -> Message:
        if request is not None:
            return request
        info = METHODS[binding.rpc]
        message_type: Any = info.input
        built: Message = message_type()
        return built

    if is_async:

        def list_all_async(
            request: Message | None = None, options: CallOptions | None = None
        ) -> AsyncIterator[Any]:
            return async_paginate(binding, method, request_of(request), options)

        return list_all_async

    def list_all(request: Message | None = None, options: CallOptions | None = None) -> Iterator[Any]:
        return paginate(binding, method, request_of(request), options)

    return list_all


def _attach(modules: Mapping[str, Any], bindings: Sequence[ModuleBinding], *, is_async: bool) -> None:
    for entry in bindings:
        target = modules.get(entry.name)
        if target is None:
            continue
        for call in entry.calls:
            if call.pagination is None or call.streaming != "unary":
                continue
            method = getattr(target, call.name, None)
            if method is not None:
                _add(target, f"{call.name}_all", _paged_all(call, method, is_async=is_async))
    documents = modules.get("documents")
    write = getattr(documents, "write", None)
    if documents is not None and write is not None:
        scroll = getattr(documents, "scroll", None)
        calls: object = (AsyncDocumentCalls if is_async else DocumentCalls)(write, scroll)
        _attach_conveniences(documents, calls)
    search = modules.get("search")
    query = getattr(search, "query", None)
    if search is not None and query is not None:
        _attach_conveniences(search, (AsyncSearchCalls if is_async else SearchCalls)(query))
    sql = modules.get("sql")
    sql_query = getattr(sql, "query", None)
    if sql is not None and sql_query is not None:
        _attach_conveniences(sql, (AsyncSqlCalls if is_async else SqlCalls)(sql_query))
    streams = modules.get("streams")
    fetch = getattr(streams, "fetch", None)
    fetch_binding = _binding(bindings, "streams", "fetch")
    if streams is not None and fetch is not None and fetch_binding is not None:
        stream_calls: object = (AsyncStreamCalls if is_async else StreamCalls)(fetch, fetch_binding)
        _attach_conveniences(streams, stream_calls)


def attach_app_calls(modules: Mapping[str, Any], bindings: Sequence[ModuleBinding]) -> None:
    """Adds the hand-written calls to the sync client's module objects, in place."""
    _attach(modules, bindings, is_async=False)


def attach_async_app_calls(modules: Mapping[str, Any], bindings: Sequence[ModuleBinding]) -> None:
    """Adds the hand-written calls to the async client's module objects, in place."""
    _attach(modules, bindings, is_async=True)


