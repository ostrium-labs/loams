"""The `Loams` object: one client with namespaced modules (design §44 §7.1).

    import os
    from loams import Loams, api_key
    from loams.instance.v1.instance_pb2 import GetInstanceRequest

    loams = Loams(os.environ["LOAMS_ENDPOINT"], api_key=os.environ["LOAMS_API_KEY"])
    info = loams.instance.get_instance(GetInstanceRequest())
    for transition in loams.live.watch(request):
        ...

What is generated and what is hand-written, once more, because it decides where a
change goes. The **module surface** is generated: `loams._gen.facade` has one
typed protocol per annotated service, one method per `FacadeOptions` call, and
`MODULES` says which service and which retry class each call has. The **runtime**
behind those methods is hand-written, once, in `loams.runtime/`: transport,
credentials, retry, errors, tokens, pagination, streams. This module is the thin
join — it reads the generated table, builds one object per module out of it, and
hands every call to the same invoker. It contains no method names and no RPC
paths, which is why annotating a proto is enough to add an SDK method.

The async twin is `loams.aio.AsyncLoams`, over the same table and the same
runtime decisions.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from types import TracebackType
from typing import Any, Self, cast

from google.protobuf.message import Message

from loams._gen.facade import (
    MODULES,
    PROTO_PACKAGES,
    PROTO_REV,
    CallBinding,
    InstanceModuleSync,
    LiveModuleSync,
    ModuleBinding,
    TablesModuleSync,
)
from loams.runtime.call import CallInvoker
from loams.runtime.consistency import ConsistencySession
from loams.runtime.errors import LoamsError
from loams.runtime.options import CallOptions, ConsistencyTokenStore, PageRequestOptions
from loams.runtime.pagination import paginate
from loams.runtime.retry import DEFAULT_MAX_RETRIES
from loams.runtime.streams import ResumeOptions, watch
from loams.runtime.token_source import TokenSource, api_key as api_key_source
from loams.runtime.transports import TransportOptions, make_sync_client
from loams.system import SystemApi

__all__ = ["Loams"]

ITEM = Any


class _Module:
    """One generated module, built out of the generated table.

    One function per generated call, each delegating to the same invoker. A
    server-streaming call returns the iterator directly rather than a promise of
    one, so `for t in loams.live.watch(...)` reads the way design §44 §7.1 says
    it should. `loams.stream()` wraps the same call with cursor resume.
    """

    def __init__(self, binding: ModuleBinding, invoker: CallInvoker) -> None:
        self.module = binding.name
        self.service = binding.service
        self.unstable = binding.unstable
        for call in binding.calls:
            setattr(self, call.name, self._make(call, invoker))

    @staticmethod
    def _make(call: CallBinding, invoker: CallInvoker) -> Any:
        def invoke(request: Message, options: CallOptions | None = None) -> Any:
            if call.streaming == "server":
                return invoker.stream(call, request, options)
            return invoker.unary(call, request, options)

        invoke.__name__ = call.name
        invoke.__qualname__ = f"loams.{call.module}.{call.name}"
        invoke.__doc__ = f"`{call.rpc}`, retried: {call.retry}."
        return invoke

    def __repr__(self) -> str:
        return f"<loams.{self.module} ({self.service})>"


def build_modules(invoker: CallInvoker) -> dict[str, Any]:
    """One object per generated module.

    A module the generator has not seen does not appear, so `loams.vector` is
    absent until `QueryService/Search` carries its facade options (API1 Task 2).
    """
    modules: dict[str, Any] = {}
    for binding in MODULES:
        modules[binding.name] = _Module(binding, invoker)
    return modules


class Loams:
    """One SDK, over one instance."""

    #: `loams.instance` — what this instance is, and who the caller is.
    instance: InstanceModuleSync
    #: `loams.live` — the live sync session half. Its package is `unstable`.
    live: LiveModuleSync
    #: `loams.tables` — the table half of the same service (design §44 §7.2).
    tables: TablesModuleSync
    #: The module catalogue, feature detection and the version check.
    system: SystemApi

    def __init__(
        self,
        endpoint: str,
        *,
        api_key: str | None = None,
        auth: TokenSource | None = None,
        protocol: str = "connect",
        proto_json: bool = False,
        transport_options: TransportOptions | None = None,
        max_retries: int = DEFAULT_MAX_RETRIES,
        session_consistency: bool = False,
        timeout_ms: int | None = None,
    ) -> None:
        """Builds a client for one instance.

        :param endpoint: the instance's base URL, for example
            `https://acme.loams.dev`. A loopback stack is `http://127.0.0.1:8080`.
        :param api_key: the bearer, for a script or a CI job that has one. It does
            not expire, so there is nothing to refresh.
        :param auth: a token source, for everything else — `env_token()`,
            `oidc_exchange()`, or your own. `api_key` and `auth` are mutually
            exclusive because they answer the same question.
        :param protocol: `"connect"` (the default), `"grpc"` or `"grpc-web"`: which
            of the three the one port speaks for this client.
        :param proto_json: send and receive the proto3 JSON mapping rather than
            protobuf. JSON is what `curl` sends (design §44 §4); protobuf is
            smaller and is the default.
        :param session_consistency: hold a session consistency token across calls
            (D609). **Off by default**: every read is then `STRONG` on its own,
            which is correct but does not give read-your-writes across processes.
        """
        if api_key is not None and auth is not None:
            raise ValueError("pass api_key or auth, not both: they answer the same question")
        if not endpoint:
            raise ValueError("endpoint is empty")

        from loams.runtime.transports import protocol_of

        options = transport_options or TransportOptions(
            proto_json=proto_json,
            protocol=protocol_of(protocol),
            timeout_ms=timeout_ms,
        )
        client = make_sync_client(options.merge(endpoint))
        self._client = client
        source: TokenSource | None
        if auth is not None:
            source = auth
        elif api_key is not None:
            source = api_key_source(api_key)
        else:
            # An unauthenticated client, which is what `GetInstance` needs anyway
            # and what a caller feature-detecting before sign-in needs.
            source = None
        # The session store is built first so the invoker can hold it: a call that
        # asks for the session's consistency token reads and records through the
        # same store the client exposes.
        self._consistency: ConsistencySession | None = (
            ConsistencySession() if session_consistency else None
        )
        self._invoker = CallInvoker(client, source, max_retries, self._consistency)
        self._endpoint = endpoint

        modules = build_modules(self._invoker)
        #: Every generated module, by name: the catalogue a caller iterates. A
        #: module the generator has not seen yet is absent, so `modules` is how a
        #: caller asks what this build can do before naming one.
        self.modules: Mapping[str, Any] = modules
        self.instance = cast(InstanceModuleSync, modules["instance"])
        self.live = cast(LiveModuleSync, modules["live"])
        self.tables = cast(TablesModuleSync, modules["tables"])
        self.system = SystemApi(self.instance.get_instance)

    # -- lifecycle ---------------------------------------------------------

    def close(self) -> None:
        """Releases the transport's connections."""
        self._client.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        _exc_type: type[BaseException] | None,
        _exc_value: BaseException | None,
        _traceback: TracebackType | None,
    ) -> None:
        self.close()

    def __repr__(self) -> str:
        return f"<Loams {self._endpoint}>"

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
        found = self._invoker.binding_for(module, call)
        if found.streaming != "server" and module == "":
            raise LoamsError("a module name is required")
        return found

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
    ) -> Iterator[Any]:
        """Every item of a paged call (D617's iterator).

            for collection in loams.paginate("collections", "list_collections", request):

        The per-call alias §44 §7.4 sketches (`loams.collections.list_all`)
        arrives with the first paged RPC, when there is a generated signature to
        hang it on; until then this is the same iterator under its module and call
        names. **No RPC is paged yet** — `ListCollections` is API1 Task 2 — so this
        is exercised against a stub.
        """
        binding = self.binding(module, call)
        target = self.modules.get(module)
        if target is None:
            raise LoamsError(f"loams.{module} is not a generated module")
        method = getattr(target, call, None)
        if method is None:
            raise LoamsError(f"loams.{module} has no call {call}")
        return paginate(
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
    ) -> Iterator[Message]:
        """A server stream that reconnects from its cursor (runtime contract R7).

            for transition in loams.stream("live", "watch", request, resume_options=ResumeOptions(resume=set_resume)):

        `resume_options.resume` is how the stream re-opens: the runtime supplies
        the last cursor it applied and the caller says what to do with it.
        """
        binding = self.binding(module, call)
        target = self.modules.get(module)
        if target is None:
            raise LoamsError(f"loams.{module} is not a generated module")
        method = getattr(target, call, None)
        if method is None:
            raise LoamsError(f"loams.{module} has no call {call}")
        return watch(
            binding,
            lambda current, call_options: iter(method(current, call_options)),
            request,
            options,
            resume_options,
        )
