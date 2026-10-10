"""SDK2 Task 2's `python_pagination_iterator`.

Runtime contract R6: `page_size` and `page_token` in, `next_page_token` out, and
the caller gets **items, not pages**. D617's paging iterator is one function for
every paged RPC rather than one per list RPC, because the generated binding says
which two fields the page token is.

**The end-to-end half is a deliberate skip, not an omission.** No RPC is marked
paged yet -- `ListCollections` arrives with API1 Task 2, and the instance serves
no paged call that the fixture corpus records. A fixture for an RPC the server
does not serve would exercise the stub rather than the SDK, so the iterator is
driven against a stub `fetch` here and the wire half is left to the suite that
runs when a paged RPC exists. `pagination.py` says the same thing, and adding a
recorded fixture for an unserved method would make the fixture server 404 it.

The messages are `ListNotifications{Request,Response}`, which already carry
`page_token` and `next_page_token`, so the field names the iterator reads are
the real proto names rather than ones invented for the test.
"""

from __future__ import annotations

import asyncio

import pytest

from loams._gen.facade import CallBinding, Pagination
from loams.operations.v1.operations_pb2 import (
    ListOperationsRequest,
    ListOperationsResponse,
)
from loams.notifications.v1.notifications_pb2 import (
    ListNotificationsRequest,
    ListNotificationsResponse,
    Notification,
)
from loams.runtime.options import PageRequestOptions
from loams.runtime.pagination import async_paginate, page_fields, paginate

RPC = "loams.notifications.v1.NotificationService/ListNotifications"


def _paged_binding() -> CallBinding:
    """A binding marked paged, as the generator will emit once an RPC is."""
    return CallBinding(
        module="notifications",
        name="ListNotifications",
        proto_name="ListNotifications",
        method="ListNotifications",
        rpc=RPC,
        service="NotificationService",
        package="loams.notifications.v1",
        idempotency="no_side_effects",
        retry="safe",
        streaming="unary",
        pagination=Pagination(items="notifications", next_page_token="next_page_token"),
    )


def _unpaged_binding() -> CallBinding:
    return CallBinding(
        module="instance",
        name="GetInstance",
        proto_name="GetInstance",
        method="GetInstance",
        rpc="loams.instance.v1.InstanceService/GetInstance",
        service="InstanceService",
        package="loams.instance.v1",
        idempotency="no_side_effects",
        retry="safe",
        streaming="unary",
        pagination=None,
    )


def _pages(*groups: list[str]):
    """A stub `fetch` over fixed pages, recording the token it was asked for.

    Returns `(fetch, seen)` where `seen` is the page token of each request, so
    the test can assert the walk rather than only the totals.
    """
    seen: list[str] = []

    def fetch(request: ListNotificationsRequest, options: object = None):
        token = request.page_token
        seen.append(token)
        index = 0 if token == "" else int(token.removeprefix("t"))
        response = ListNotificationsResponse()
        for name in groups[index]:
            response.notifications.append(Notification(title=name))
        if index + 1 < len(groups):
            response.next_page_token = f"t{index + 1}"
        return response

    return fetch, seen


def test_python_pagination_iterator() -> None:
    """The required test: every item, in order, following the tokens."""
    fetch, seen = _pages(["a", "b"], ["c"], ["d", "e"])
    request = ListNotificationsRequest(unread_only=True, page_size=2)

    items = [item.title for item in paginate(_paged_binding(), fetch, request)]

    assert items == ["a", "b", "c", "d", "e"]
    # The first request is the caller's own, un-tokenized; each later one carries
    # the token the previous page handed back.
    assert seen == ["", "t1", "t2"], f"the page tokens walked were {seen}"


def test_the_callers_request_is_not_mutated() -> None:
    """The same request is reused for every page, so it must not be written to.

    Mutating it would carry one page's token into the next request, and after the
    walk the caller's object would still hold the last token -- so the same
    request reused for a second walk would start mid-list.
    """
    fetch, _ = _pages(["a"], ["b"])
    request = ListNotificationsRequest(page_size=1)

    first = [item.title for item in paginate(_paged_binding(), fetch, request)]
    assert first == ["a", "b"]
    assert request.page_token == "", f"the caller's request was left holding {request.page_token!r}"
    assert request.page_size == 1

    # And the second walk is identical, which is what a stale token would break.
    second = [item.title for item in paginate(_paged_binding(), fetch, request)]
    assert second == first


def test_an_unpaged_call_raises_rather_than_answering_once() -> None:
    """A binding with no pagination is a bug, not a one-page result."""
    fetch, seen = _pages(["a"])
    with pytest.raises(ValueError, match="not a paged call"):
        list(paginate(_unpaged_binding(), fetch, ListNotificationsRequest()))
    assert seen == [], "the stub was called for a call that is not paged"


def test_page_fields_default_to_the_proto_names_and_honour_overrides() -> None:
    """Both field names come from the binding, with proto3 defaults."""
    pagination = Pagination(items="notifications", next_page_token="next_page_token")
    assert page_fields(pagination) == ("page_size", "page_token")
    renamed = PageRequestOptions(page_size_field="limit", page_token_field="cursor")
    assert page_fields(pagination, renamed) == ("limit", "cursor")


def test_a_custom_page_token_field_is_the_one_walked() -> None:
    """A renamed token field is set and read, not just the default.

    `namespace` stands in for a future `cursor`: the point is that the field
    name comes from the options, so `page_token` stays empty throughout and a
    default-reading implementation would loop on the first page forever rather
    than finish.
    """
    seen: list[str] = []

    def fetch(request: ListOperationsRequest, options: object = None):
        seen.append(request.namespace)
        response = ListOperationsResponse()
        if request.namespace == "":
            response.operations.add(id="first")
            response.next_page_token = "next"
        else:
            response.operations.add(id="second")
        return response

    binding = CallBinding(
        module="operations",
        name="ListOperations",
        proto_name="ListOperations",
        method="ListOperations",
        rpc="loams.operations.v1.OperationsService/ListOperations",
        service="OperationsService",
        package="loams.operations.v1",
        idempotency="no_side_effects",
        retry="safe",
        streaming="unary",
        pagination=Pagination(items="operations", next_page_token="next_page_token"),
    )
    renamed = PageRequestOptions(page_size_field="page_size", page_token_field="namespace")
    items = [
        operation.id
        for operation in paginate(binding, fetch, ListOperationsRequest(), None, renamed)
    ]
    assert items == ["first", "second"]
    assert seen == ["", "next"]


def test_an_empty_first_page_still_walks() -> None:
    """An empty page with a token is not the end; only an empty token is."""
    fetch, seen = _pages([], ["a"], [])
    items = [item.title for item in paginate(_paged_binding(), fetch, ListNotificationsRequest())]
    assert items == ["a"]
    assert seen == ["", "t1", "t2"]


def test_the_async_iterator_walks_the_same_pages() -> None:
    """`async_paginate` is the same walk, and must not diverge."""

    async def fetch(request: ListNotificationsRequest, options: object = None):
        response = ListNotificationsResponse()
        response.notifications.append(Notification(title=request.page_token or "first"))
        if request.page_token == "":
            response.next_page_token = "second"
        return response

    async def collect() -> list[str]:
        return [
            item.title
            async for item in async_paginate(
                _paged_binding(), fetch, ListNotificationsRequest()
            )
        ]

    assert asyncio.run(collect()) == ["first", "second"]


def test_no_rpc_is_paged_yet_so_the_wire_half_is_skipped() -> None:
    """Records why the end-to-end half is absent, so it cannot be forgotten.

    If this ever starts failing, a paged RPC has been marked in the protos: give
    the suite a recorded fixture for it and drive the iterator over the wire
    instead of the stub.
    """
    from loams._gen.facade import METHODS

    paged = [
        name
        for name in METHODS.values()
        if name is not None and getattr(name, "paged", False)
    ]
    if paged:
        pytest.fail(f"a paged RPC exists now ({paged}); add the recorded fixture")
    pytest.skip(
        "no RPC is marked paged yet (ListCollections is API1 Task 2), so there is "
        "nothing on the wire to page; the iterator is driven against a stub"
    )