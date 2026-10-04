"""Pagination (design §44 §7.4; runtime contract R6).

AIP-158: `page_size` and `page_token` in, `next_page_token` out. A generated
binding says which two fields those are (`FacadeOptions.pagination` is
`"<items>:<next page token>"`), so the iterator is one function for every paged
RPC rather than one per list RPC.

**No RPC is paged yet.** `ListCollections` arrives with API1 Task 2, so the
end-to-end half of `python_pagination_iterator` is a deliberate skip against a
stub, not an omission: a fixture for an RPC the server does not serve would test
the stub rather than the SDK. For the same reason there is no per-call
`list_all` alias — it needs a generated signature to hang on. `loams.paginate(...)`
is the same iterator under its module and call names.
"""

from __future__ import annotations

from collections.abc import AsyncIterator, Callable, Iterator
from typing import TypeVar

from google.protobuf.message import Message

from loams._gen.facade import CallBinding, Pagination
from loams.runtime.options import CallOptions, PageRequestOptions

__all__ = ["async_paginate", "paginate", "page_fields"]

REQ = TypeVar("REQ", bound=Message)
RES = TypeVar("RES", bound=Message)
ITEM = TypeVar("ITEM")


def page_fields(pagination: Pagination, options: PageRequestOptions | None = None) -> tuple[str, str]:
    """The request field names a paged call uses.

    Both default to the proto3 names, which is what the generated messages use.
    """
    settings = options or PageRequestOptions()
    return settings.page_size_field, settings.page_token_field


def _set_page_token(request: Message, field: str, token: str) -> Message:
    """A copy of the request with the page token set.

    Copied rather than mutated: the caller's request is reused for every page, so
    mutating it would carry one page's token into the next request.
    """
    copy = type(request)()
    copy.CopyFrom(request)
    setattr(copy, field, token)
    return copy


def paginate(
    binding: CallBinding,
    fetch: Callable[[Message, CallOptions | None], RES],
    request: REQ,
    options: CallOptions | None = None,
    page_options: PageRequestOptions | None = None,
) -> Iterator[ITEM]:
    """Every item of a paged call, following the tokens (D617's paging iterator).

    The caller gets **items, not pages**: `for c in loams.paginate(...)`. The raw
    page call is still available on the module, so a caller that wants pages, or
    wants to stop after one, does not have to use this.

    :raises LoamsError: when the binding is not a paged call, rather than looping
        once and pretending that was the whole answer.
    """
    pagination = binding.pagination
    if pagination is None:
        raise ValueError(
            f"loams.{binding.module}.{binding.name} is not a paged call: "
            "the proto's facade options name no pagination"
        )
    _, token_field = page_fields(pagination, page_options)
    page_token: str | None = None
    while True:
        attempt = request if page_token is None else _set_page_token(request, token_field, page_token)
        page = fetch(attempt, options)
        for item in getattr(page, pagination.items):
            yield item  # type: ignore[misc]
        next_token = getattr(page, pagination.next_page_token)
        if not isinstance(next_token, str) or next_token == "":
            return
        page_token = next_token


async def async_paginate(
    binding: CallBinding,
    fetch: Callable[[Message, CallOptions | None], "object"],
    request: REQ,
    options: CallOptions | None = None,
    page_options: PageRequestOptions | None = None,
) -> AsyncIterator[ITEM]:
    """`paginate` for `AsyncLoams`."""
    pagination = binding.pagination
    if pagination is None:
        raise ValueError(
            f"loams.{binding.module}.{binding.name} is not a paged call: "
            "the proto's facade options name no pagination"
        )
    _, token_field = page_fields(pagination, page_options)
    page_token: str | None = None
    while True:
        attempt = request if page_token is None else _set_page_token(request, token_field, page_token)
        page = await fetch(attempt, options)  # type: ignore[misc]
        for item in getattr(page, pagination.items):
            yield item  # type: ignore[misc]
        next_token = getattr(page, pagination.next_page_token)
        if not isinstance(next_token, str) or next_token == "":
            return
        page_token = next_token
