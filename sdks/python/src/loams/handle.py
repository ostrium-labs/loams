"""The collection handle (SDKP §2.8).

    notes = loams.collection("notes")
    notes.upsert([{"id": 1, "source": {"title": "Refunds"}}])
    hits = notes.search(notes.query().text("refund", fields=["body"]))

Every call delegates to the module call of the same meaning with `collection`
(and `namespace`, when the handle was given one) filled in. The handle holds no
state of its own: two handles on the same collection are interchangeable.
"""

from __future__ import annotations

from collections.abc import AsyncIterable, Iterable, Mapping, Sequence
from typing import Any

from connectrpc.code import Code
from google.protobuf import json_format
from google.protobuf.message import Message

from loams.app import DocumentInput, document_id_json
from loams.builders import DocId, FilterJson, Search
from loams.runtime.errors import LoamsError
from loams.runtime.options import CallOptions

__all__ = ["AsyncCollectionHandle", "CollectionHandle"]


def _call(modules: Mapping[str, Any], module: str, name: str) -> Any:
    target = modules.get(module)
    fn = getattr(target, name, None) if target is not None else None
    if fn is None:
        raise LoamsError(f"this SDK's facade has no {module}.{name}", code=Code.UNIMPLEMENTED)
    return fn


class _Base:
    def __init__(self, modules: Mapping[str, Any], name: str, namespace: str | None) -> None:
        self._modules = modules
        #: The collection's name (or an alias).
        self.name = name
        #: The namespace, when it is not the client's default.
        self.namespace = namespace

    def _fields(self) -> dict[str, Any]:
        fields: dict[str, Any] = {"collection": self.name}
        if self.namespace is not None:
            fields["namespace"] = self.namespace
        return fields

    def _message(self, type_name: str, **extra: Any) -> Message:
        from loams.collection.v1 import collection_pb2, document_pb2

        for module in (document_pb2, collection_pb2):
            message_type = getattr(module, type_name, None)
            if message_type is not None:
                built: Message = json_format.ParseDict({**self._fields(), **extra}, message_type())
                return built
        raise LoamsError(f"no message {type_name}", code=Code.INTERNAL)

    def _get_request(self, ids: Sequence[DocId], select: Mapping[str, Any] | None) -> Message:
        extra: dict[str, Any] = {"ids": [document_id_json(i) for i in ids]}
        if select is not None:
            extra["select"] = dict(select)
        return self._message("GetDocumentsRequest", **extra)

    def _count_request(self, where: FilterJson | None) -> Message:
        # `where` is the Filter's proto3 JSON, a `Struct` on the wire (SDKP-12).
        return self._message("CountDocumentsRequest", **({} if where is None else {"where": where}))

    def query(self) -> Search:
        """A search builder already pointed at this collection."""
        return Search(self.name, namespace=self.namespace)

    def _search_request(self, builder: Search) -> Message:
        body = builder.to_json()
        body["collection"] = self.name
        if self.namespace is not None:
            body["namespace"] = self.namespace
        return Search.from_json(body).build()

    def _vector_builder(
        self,
        field: str,
        vector: Sequence[float],
        limit: int | None,
        filter_json: FilterJson | None,
        knobs: Mapping[str, Any],
    ) -> Search:
        builder = self.query().vector(field, vector, **knobs)
        if filter_json is not None:
            builder = builder.filter(filter_json)
        if limit is not None:
            builder = builder.limit(limit)
        return builder


class CollectionHandle(_Base):
    """A collection, by name, with the document and search calls bound to it."""

    def __init__(self, modules: Mapping[str, Any], name: str, *, namespace: str | None = None) -> None:
        super().__init__(modules, name, namespace)

    def upsert(self, docs: Iterable[DocumentInput], options: CallOptions | None = None) -> Any:
        """Writes (creates or replaces) documents."""
        return _call(self._modules, "documents", "upsert")(
            self.name, docs, namespace=self.namespace, options=options
        )

    def get(
        self,
        ids: Sequence[DocId],
        *,
        select: Mapping[str, Any] | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Reads documents by id."""
        return _call(self._modules, "documents", "get")(self._get_request(ids, select), options)

    def delete(self, ids: Iterable[DocId], options: CallOptions | None = None) -> Any:
        """Removes documents by id."""
        return _call(self._modules, "documents", "delete")(
            self.name, ids, namespace=self.namespace, options=options
        )

    def patch(
        self,
        doc_id: DocId,
        *,
        set: Mapping[str, Any] | None = None,  # noqa: A002 - SDKP §2.3's name
        unset: Sequence[str] | None = None,
        mode: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        """Changes part of one document."""
        return _call(self._modules, "documents", "patch")(
            self.name, doc_id, set=set, unset=unset, mode=mode, namespace=self.namespace,
            options=options,
        )

    def write_batch(self, docs: Iterable[DocumentInput], **kwargs: Any) -> Any:
        """A chunked bulk upsert (R13)."""
        return _call(self._modules, "documents", "write_batch")(
            self.name, docs, namespace=self.namespace, **kwargs
        )

    def count(self, *, where: FilterJson | None = None, options: CallOptions | None = None) -> Any:
        """How many documents match `where` (every document without one)."""
        return _call(self._modules, "documents", "count")(self._count_request(where), options)

    def scroll_all(self, **kwargs: Any) -> Any:
        """Every document, page by page."""
        return _call(self._modules, "documents", "scroll_all")(
            self.name, namespace=self.namespace, **kwargs
        )

    def search(self, builder: Search, options: CallOptions | None = None) -> Any:
        """Runs a search; the builder's collection is replaced by this one."""
        return _call(self._modules, "search", "query")(self._search_request(builder), options)

    def vector_search(
        self,
        field: str,
        vector: Sequence[float],
        *,
        limit: int | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        options: CallOptions | None = None,
        **knobs: Any,
    ) -> Any:
        """A kNN search on one vector field."""
        return self.search(self._vector_builder(field, vector, limit, filter, knobs), options)

    def info(self, options: CallOptions | None = None) -> Any:
        """The collection's schema and stats."""
        return _call(self._modules, "collections", "get")(
            self._message("GetCollectionRequest"), options
        )

    def drop(self, options: CallOptions | None = None) -> Any:
        """Drops the collection."""
        return _call(self._modules, "collections", "drop")(
            self._message("DropCollectionRequest"), options
        )


class AsyncCollectionHandle(_Base):
    """`CollectionHandle` for `AsyncLoams`: the same calls, awaited."""

    def __init__(self, modules: Mapping[str, Any], name: str, *, namespace: str | None = None) -> None:
        super().__init__(modules, name, namespace)

    async def upsert(self, docs: Iterable[DocumentInput], options: CallOptions | None = None) -> Any:
        return await _call(self._modules, "documents", "upsert")(
            self.name, docs, namespace=self.namespace, options=options
        )

    async def get(
        self,
        ids: Sequence[DocId],
        *,
        select: Mapping[str, Any] | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        return await _call(self._modules, "documents", "get")(
            self._get_request(ids, select), options
        )

    async def delete(self, ids: Iterable[DocId], options: CallOptions | None = None) -> Any:
        return await _call(self._modules, "documents", "delete")(
            self.name, ids, namespace=self.namespace, options=options
        )

    async def patch(
        self,
        doc_id: DocId,
        *,
        set: Mapping[str, Any] | None = None,  # noqa: A002 - SDKP §2.3's name
        unset: Sequence[str] | None = None,
        mode: str | None = None,
        options: CallOptions | None = None,
    ) -> Any:
        return await _call(self._modules, "documents", "patch")(
            self.name, doc_id, set=set, unset=unset, mode=mode, namespace=self.namespace,
            options=options,
        )

    async def write_batch(
        self, docs: Iterable[DocumentInput] | AsyncIterable[DocumentInput], **kwargs: Any
    ) -> Any:
        return await _call(self._modules, "documents", "write_batch")(
            self.name, docs, namespace=self.namespace, **kwargs
        )

    async def count(
        self, *, where: FilterJson | None = None, options: CallOptions | None = None
    ) -> Any:
        return await _call(self._modules, "documents", "count")(self._count_request(where), options)

    def scroll_all(self, **kwargs: Any) -> Any:
        """Every document, page by page, as an async iterator."""
        return _call(self._modules, "documents", "scroll_all")(
            self.name, namespace=self.namespace, **kwargs
        )

    async def search(self, builder: Search, options: CallOptions | None = None) -> Any:
        return await _call(self._modules, "search", "query")(self._search_request(builder), options)

    async def vector_search(
        self,
        field: str,
        vector: Sequence[float],
        *,
        limit: int | None = None,
        filter: FilterJson | None = None,  # noqa: A002 - the IR's name
        options: CallOptions | None = None,
        **knobs: Any,
    ) -> Any:
        return await self.search(self._vector_builder(field, vector, limit, filter, knobs), options)

    async def info(self, options: CallOptions | None = None) -> Any:
        return await _call(self._modules, "collections", "get")(
            self._message("GetCollectionRequest"), options
        )

    async def drop(self, options: CallOptions | None = None) -> Any:
        return await _call(self._modules, "collections", "drop")(
            self._message("DropCollectionRequest"), options
        )
