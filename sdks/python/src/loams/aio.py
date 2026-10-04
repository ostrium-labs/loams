"""The async twin of `loams.Loams` (design §44 §7.1).

    import asyncio, os
    from loams import AsyncLoams, async_api_key
    from loams.instance.v1.instance_pb2 import GetInstanceRequest

    async def main() -> None:
        async with AsyncLoams(os.environ["LOAMS_ENDPOINT"],
                              api_key=os.environ["LOAMS_API_KEY"]) as loams:
            info = await loams.instance.get_instance(GetInstanceRequest())
            async for transition in loams.live.watch(request):
                ...

Everything the two clients decide — the retry class from the generated bindings,
one idempotency key per logical call, one refresh on `token_expired`, the typed
error hierarchy, cursor resume — is written once in `loams.runtime` and shared.
What differs is only that the awaits are awaited. There is no thread pool and no
`run_in_executor` on the call path, so an async caller never blocks a worker
thread on an RPC; the one blocking call in the whole SDK, the RFC 8693 token
exchange in `async_oidc_exchange`, is moved off the loop explicitly and says so.
"""

from __future__ import annotations

from collections.abc import AsyncIterator, Mapping
from types import TracebackType
from typing import Any, Self, cast

from google.protobuf.message import Message

from loams._gen.facade import (
    CallBinding   ,
    InstanceModule,
    LiveModule    ,
    MODULES       ,
    ModuleBinding ,
    PROTO_PACKAGES,
    PROTO_REV     ,
    TablesModule  ,
)
from loams.runtime.call import AsyncCallInvoker
from loams.runtime.consistency import ConsistencySession
from loams.runtime.errors import LoamsError
from loams.runtime.options import CallOptions, ConsistencyTokenStore, PageRequestOptions
from loams.runtime.pagination import async_paginate
from loams.runtime.retry import DEFAULT_MAX_RETRIES
from loams.runtime.streams import ResumeOptions, async_watch
from loams.runtime.token_source import AsyncTokenSource
from loams.runtime.token_source import api_key as async_api_key_source
from loams.runtime.transports import TransportOptions, make_async_client, protocol_of
from loams.system import AsyncSystemApi

__all__ = ["AsyncLoams"]


class _AsyncModule:
    """One generated module for the async client, out of the same table."""

    def __init__(self, binding: ModuleBinding, invoker: AsyncCallInvoker) -> None:
        self.module = binding.name
        self.service = binding.service
        self.unstable = binding.unstable
        for call in binding.calls:
            setattr(self, call.name, self._make(call, invoker))

    @staticmethod
    def _make(call: CallBinding, invoker: AsyncCallInvoker) -> Any:
        if call.streaming == "server":
            def stream(request: Message, options: CallOptions | None = None) -> Any:
                return invoker.stream(call, request, options)
        else:
            def unary(request: Message, options: CallOptions | None = None) -> Any:
                return invoker.unary(call, request, options)

        invoke = stream if call.streaming == "server" else unary
        invoke.__name__ = call.name
        invoke.__qualname__ = f"loams.{call.module}.{call.name}"
        invoke.__doc__ = f"`{call.rpc}`, retried: {call.retry}."
        return invoke

    def __repr__(self) -> str:
        return f"<loams.{self.module} ({self.service})>"


def build_async_modules(invoker: AsyncCallInvoker) -> dict[str, Any]:
    """One object per generated module, for `AsyncLoams`."""
    return {binding.name: _AsyncModule(binding, invoker) for binding in MODULES}


class AsyncLoams:
    """One SDK, over one instance, awaited."""

    instance: InstanceModule
    live: LiveModule
    tables: TablesModule
    system: AsyncSystemApi

    def __init__(
        self,
        endpoint: str,
        *,
        api_key: str | None = None,
        auth: AsyncTokenSource | None = None,
        protocol: str = "connect",
        proto_json: bool = False,
        transport_options: TransportOptions | None = None,
        max_retries: int = DEFAULT_MAX_RETRIES,
        session_consistency: bool = False,
        timeout_ms: int | None = None,
    ) -> None:
        """Builds an async client for one instance. See `loams.Loams` for the arguments.

        The `api_key` here is the async token source: same key, no refresh.
        """
        if api_key is not None and auth is not None:
            raise ValueError("pass api_key or auth, not both: they answer the same question")
        if not endpoint:
            raise ValueError("endpoint is empty")

        options = transport_options or TransportOptions(
            proto_json=proto_json,
            protocol=protocol_of(protocol),
            timeout_ms=timeout_ms,
        )
        client = make_async_client(options.merge(endpoint))
        self._client = client
        source: AsyncTokenSource | None
        if auth is not None:
            source = auth
        elif api_key is not None:
            source = async_api_key_source(api_key)
        else:
            source = None
        self._consistency: ConsistencySession | None = (
            ConsistencySession() if session_consistency else None
        )
        self._invoker = AsyncCallInvoker(client, source, max_retries, self._consistency)
        self._endpoint = endpoint

        modules = build_async_modules(self._invoker)
        self.modules: Mapping[str, Any] = modules
        self.instance = cast(InstanceModule, modules["instance"])
        self.live = cast(LiveModule, modules["live"])
        self.tables = cast(TablesModule, modules["tables"])
        self.system = AsyncSystemApi(self.instance.get_instance)

    # -- lifecycle ---------------------------------------------------------

    async def close(self) -> None:
        """Releases the transport's connections."""
        await self._client.close()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        _exc_type: type[BaseException] | None,
        _exc_value: BaseException | None,
        _traceback: TracebackType | None,
    ) -> None:
        await self.close()

    def __repr__(self) -> str:
        return f"<AsyncLoams {self._endpoint}>"

    # -- the generated surface ---------------------------------------------

    @property
    def consistency(self) -> ConsistencyTokenStore | None:
        """The session consistency token store, when `session_consistency` is on."""
        return self._consistency

    @property
    def proto_rev(self) -> str:
        """The proto revision this SDK declares (§44 §10.3)."""
        return PROTO_REV

    @property
    def proto_packages(self) -> tuple[str, ...]:
        """The proto packages this SDK speaks."""
        return tuple(name for name in PROTO_PACKAGES if name.startswith("loams."))

    def binding(self, module: str, call: str) -> CallBinding:
        """The binding a module and call name identify, or a clear error."""
        return self._invoker.binding_for(module, call)

    def invalidate_catalogue(self) -> None:
        """Forgets the cached service catalogue, so the next check calls again."""
        self.system.invalidate()

    # -- pagination (R6) ---------------------------------------------------

    def paginate(
        self,
        module: str,
        call: str,
        request: Message,
        options: CallOptions | None = None,
        page_options: PageRequestOptions | None = None,
    ) -> AsyncIterator[Any]:
        """Every item of a paged call, awaited (R6).

        An async iterator rather than an async generator function's coroutine, so
        `async for x in loams.paginate(...)` reads the way the sync client does.
        """
        binding = self.binding(module, call)
        target = self.modules.get(module)
        if target is None:
            raise LoamsError(f"loams.{module} is not a generated module")
        method = getattr(target, call, None)
        if method is None:
            raise LoamsError(f"loams.{module} has no call {call}")
        return async_paginate(
            binding,
            lambda page, call_options: method(page, call_options),
            request,
            options,
            page_options,
        )

    # -- streaming (R7) ----------------------------------------------------

    def stream(
        self,
        module: str,
        call: str,
        request: Message,
        options: CallOptions | None = None,
        resume_options: ResumeOptions[Any, Any] | None = None,
    ) -> AsyncIterator[Message]:
        """A server stream that reconnects from its cursor, awaited (R7)."""
        binding = self.binding(module, call)
        target = self.modules.get(module)
        if target is None:
            raise LoamsError(f"loams.{module} is not a generated module")
        method = getattr(target, call, None)
        if method is None:
            raise LoamsError(f"loams.{module} has no call {call}")
        return async_watch(
            binding,
            lambda current, call_options: method(current, call_options),
            request,
            options,
            resume_options,
        )
