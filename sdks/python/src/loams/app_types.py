"""The types of the app modules (SDKP Part B): each generated module Protocol
plus the hand-written calls `loams.app.attach_app_calls` adds to it.

`loams.documents` is `DocumentsModuleSync` (the generated `write`, `get`,
`scroll`, `count`, …) **and** `upsert`, `delete`, `patch`, `write_batch` and
`scroll_all`; these Protocols say so, so a type checker sees both halves. The
signatures are `loams.app.DocumentCalls`'s (and its siblings'), restated
because a Protocol can only extend other Protocols.
"""

from __future__ import annotations

from collections.abc import AsyncIterable, AsyncIterator, Iterable, Iterator, Mapping, Sequence
from typing import TYPE_CHECKING, Any, Protocol

from google.protobuf.message import Message

from loams._gen.facade import (
    CollectionsModule,
    CollectionsModuleSync,
    DocumentsModule,
    DocumentsModuleSync,
    NamespacesModule,
    NamespacesModuleSync,
    SearchModule,
    SearchModuleSync,
    SqlModule,
    SqlModuleSync,
    StreamsModule,
    StreamsModuleSync,
)

if TYPE_CHECKING:
    from loams.app import DocumentInput
    from loams.builders import ConsistencyLevel, DocId, FilterJson, Search
    from loams.runtime.batch import BatchResult
    from loams.runtime.options import CallOptions
    from loams.runtime.subscribe import AsyncSubscription, Subscription

__all__ = [
    "AsyncCollectionsNamespace",
    "AsyncDocumentsNamespace",
    "AsyncNamespacesNamespace",
    "AsyncSearchNamespace",
    "AsyncSqlNamespace",
    "AsyncStreamsNamespace",
    "CollectionsNamespace",
    "DocumentsNamespace",
    "NamespacesNamespace",
    "SearchNamespace",
    "SqlNamespace",
    "StreamsNamespace",
]


class NamespacesNamespace(NamespacesModuleSync, Protocol):
    """`loams.namespaces`, with `list_all`."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> Iterator[Any]: ...


class AsyncNamespacesNamespace(NamespacesModule, Protocol):
    """`AsyncLoams.namespaces`, with `list_all`."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> AsyncIterator[Any]: ...


class CollectionsNamespace(CollectionsModuleSync, Protocol):
    """`loams.collections`, with `list_all` and `list_versions_all`."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> Iterator[Any]: ...

    def list_versions_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> Iterator[Any]: ...


class AsyncCollectionsNamespace(CollectionsModule, Protocol):
    """`AsyncLoams.collections`, with `list_all` and `list_versions_all`."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> AsyncIterator[Any]: ...

    def list_versions_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> AsyncIterator[Any]: ...


class DocumentsNamespace(DocumentsModuleSync, Protocol):
    """`loams.documents`, with the document conveniences (SDKP §2.3)."""

    def upsert(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any: ...

    def delete(
        self,
        collection: str,
        ids: Iterable[DocId],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any: ...

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
    ) -> Any: ...

    def write_batch(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        chunk_rows: int = ...,
        chunk_bytes: int = ...,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> BatchResult: ...

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
    ) -> Iterator[Any]: ...


class AsyncDocumentsNamespace(DocumentsModule, Protocol):
    """`AsyncLoams.documents`, with the document conveniences (SDKP §2.3)."""

    async def upsert(
        self,
        collection: str,
        docs: Iterable[DocumentInput],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any: ...

    async def delete(
        self,
        collection: str,
        ids: Iterable[DocId],
        *,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> Any: ...

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
    ) -> Any: ...

    async def write_batch(
        self,
        collection: str,
        docs: Iterable[DocumentInput] | AsyncIterable[DocumentInput],
        *,
        chunk_rows: int = ...,
        chunk_bytes: int = ...,
        namespace: str | None = None,
        options: CallOptions | None = None,
    ) -> BatchResult: ...

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
    ) -> AsyncIterator[Any]: ...


class SearchNamespace(SearchModuleSync, Protocol):
    """`loams.search`, with `text`, `hybrid` and `query_all` (SDKP §2.4)."""

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
    ) -> Any: ...

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
    ) -> Any: ...

    def query_all(self, search: Search, options: CallOptions | None = None) -> Iterator[Any]: ...


class AsyncSearchNamespace(SearchModule, Protocol):
    """`AsyncLoams.search`, with `text`, `hybrid` and `query_all`."""

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
    ) -> Any: ...

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
    ) -> Any: ...

    def query_all(self, search: Search, options: CallOptions | None = None) -> AsyncIterator[Any]: ...


class SqlNamespace(SqlModuleSync, Protocol):
    """`loams.sql`, with `records` (SDKP §2.5)."""

    def records(
        self,
        statement: str,
        params: Sequence[Any] = (),
        *,
        namespace: str | None = None,
        max_rows: int | None = None,
        options: CallOptions | None = None,
    ) -> list[dict[str, Any]]: ...


class AsyncSqlNamespace(SqlModule, Protocol):
    """`AsyncLoams.sql`, with `records`."""

    async def records(
        self,
        statement: str,
        params: Sequence[Any] = (),
        *,
        namespace: str | None = None,
        max_rows: int | None = None,
        options: CallOptions | None = None,
    ) -> list[dict[str, Any]]: ...


class StreamsNamespace(StreamsModuleSync, Protocol):
    """`loams.streams`, with `list_all` and `subscribe` (SDKP §2.5, R14)."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> Iterator[Any]: ...

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
    ) -> Subscription: ...


class AsyncStreamsNamespace(StreamsModule, Protocol):
    """`AsyncLoams.streams`, with `list_all` and `subscribe`."""

    def list_all(
        self, request: Message | None = None, options: CallOptions | None = None
    ) -> AsyncIterator[Any]: ...

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
    ) -> AsyncSubscription: ...
