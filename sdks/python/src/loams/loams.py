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
from typing import Any, TypeVar, cast

from connectrpc.code import Code
from google.protobuf.message import Message

from loams._gen.facade import (
    MODULES,
    PROTO_PACKAGES,
    PROTO_REV,
    CallBinding,
    GraphModuleSync,
    InstanceModuleSync,
    LiveModuleSync,
    ModuleBinding,
    TablesModuleSync,
    VectorModuleSync,
)
from loams.app import attach_app_calls
from loams.app_types import (
    CollectionsNamespace,
    DocumentsNamespace,
    NamespacesNamespace,
    SearchNamespace,
    SqlNamespace,
    StreamsNamespace,
)
from loams.handle import CollectionHandle
from loams.runtime.call import DEFAULT_TIMEOUT_MS, CallInvoker
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


def _from_env(
    environ: Mapping[str, str] | None, overrides: dict[str, Any]
) -> tuple[str, dict[str, Any]]:
    """The endpoint and constructor arguments `from_env` reads, for both clients."""
    import os

    env = os.environ if environ is None else environ
    endpoint = overrides.pop("endpoint", None) or env.get("LOAMS_ENDPOINT", "")
    if not endpoint:
        raise LoamsError(
            "Loams.from_env: LOAMS_ENDPOINT is not set",
            code=Code.INVALID_ARGUMENT,
            metadata={"field": "LOAMS_ENDPOINT"},
        )
    kwargs = dict(overrides)
    key = env.get("LOAMS_API_KEY") or env.get("LOAMS_TOKEN")
    if key and "api_key" not in kwargs and "auth" not in kwargs:
        kwargs["api_key"] = key
    namespace = env.get("LOAMS_NAMESPACE")
    if namespace and "namespace" not in kwargs:
        kwargs["namespace"] = namespace
    return endpoint, kwargs


def build_modules(invoker: CallInvoker) -> dict[str, Any]:
    """One object per generated module.

    A module the generator has not seen does not appear, so `loams.vector` is
    absent until `QueryService/Search` carries its facade options (API1 Task 2).
    """
    modules: dict[str, Any] = {}
    for binding in MODULES:
        modules[binding.name] = _Module(binding, invoker)
    return modules


# `typing.Self` is 3.11+, and this SDK supports 3.10. A bound TypeVar is
# the 3.10 spelling of the same idea: `with Loams(...) as c` keeps `c`
# typed as the concrete subclass rather than widening to the base.
_SelfT = TypeVar("_SelfT", bound="Loams")


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
        namespace: str | None = None,
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

            With it on, every consistency token a response carries -- the
            `consistency_token`, `token` and `read_token` fields and the
            `loams-consistency-token` header -- merges into `loams.consistency`
            (max offset per stream and partition, R4), and every later read whose
            request has a `consistency` field is sent `at_least` the session
            when the caller left that field unset.
        :param timeout_ms: the deadline of every unary call that does not set its
            own (R12). Default 30 000; `0` disables it. One budget covers a
            call's retries, and running out raises `DeadlineExceededError` with
            reason `client_deadline_exceeded`, which is never retried. Streams
            have no default deadline.
        :param namespace: the namespace of every request that has a `namespace`
            field and leaves it empty (R11). An explicit namespace wins; with
            neither, such a call raises `InvalidArgumentError` before any
            network call.
        """
        if api_key is not None and auth is not None:
            raise ValueError("pass api_key or auth, not both: they answer the same question")
        # Stripped for the check only, so a blank address from an environment
        # variable or a config file is refused here rather than becoming a request
        # to a URL that is nothing but whitespace.
        if not endpoint.strip():
            raise ValueError("endpoint is empty")

        from loams.runtime.transports import protocol_of

        # The deadline is the invoker's (R12), not the transport's, so one budget
        # covers a call's retries and `timeout_ms=0` can disable it.
        options = transport_options or TransportOptions(
            proto_json=proto_json,
            protocol=protocol_of(protocol),
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
        self._invoker = CallInvoker(
            client,
            source,
            max_retries,
            self._consistency,
            namespace=namespace,
            timeout_ms=DEFAULT_TIMEOUT_MS if timeout_ms is None else timeout_ms,
        )
        #: The default namespace (R11), if the client has one.
        self.namespace: str | None = namespace
        self._endpoint = endpoint

        modules = build_modules(self._invoker)
        attach_app_calls(modules, MODULES)
        #: Every generated module, by name: the catalogue a caller iterates. A
        #: module the generator has not seen yet is absent, so `modules` is how a
        #: caller asks what this build can do before naming one.
        self.modules: Mapping[str, Any] = modules
        self.instance = cast(InstanceModuleSync, modules["instance"])
        self.live = cast(LiveModuleSync, modules["live"])
        self.tables = cast(TablesModuleSync, modules["tables"])
        #: `loams.namespaces`: the containers every collection and stream live in.
        self.namespaces = cast(NamespacesNamespace, modules["namespaces"])
        #: `loams.collections`: collections, their schemas, aliases and versions.
        self.collections = cast(CollectionsNamespace, modules["collections"])
        #: `loams.documents`: write, read, scroll and count, with the conveniences.
        self.documents = cast(DocumentsNamespace, modules["documents"])
        #: `loams.search`: text, vector and hybrid search (`Search` builds one).
        self.search = cast(SearchNamespace, modules["search"])
        #: `loams.vector`: the kNN half of `search`, by its own name (Q613).
        self.vector = cast(VectorModuleSync, modules["vector"])
        #: `loams.graph`: Loams Graph, queried with GQL. Its package is unstable.
        self.graph = cast(GraphModuleSync, modules["graph"])
        #: `loams.sql`: one read-only SQL statement over a namespace's collections.
        self.sql = cast(SqlNamespace, modules["sql"])
        #: `loams.streams`: a namespace's logs, with `subscribe` (R14).
        self.streams = cast(StreamsNamespace, modules["streams"])
        self.system = SystemApi(self.instance.get_instance)

    @classmethod
    def from_env(
        cls,
        environ: Mapping[str, str] | None = None,
        **overrides: Any,
    ) -> Loams:
        """A client from the environment (R1): `LOAMS_ENDPOINT`, then
        `LOAMS_API_KEY` or `LOAMS_TOKEN`, then `LOAMS_NAMESPACE`. A keyword
        argument wins over the environment."""
        endpoint, kwargs = _from_env(environ, overrides)
        return cls(endpoint, **kwargs)

    def collection(self, name: str, *, namespace: str | None = None) -> CollectionHandle:
        """A handle on one collection: its document and search calls with
        `collection` (and `namespace`) filled in (SDKP §2.8). Holds no state."""
        return CollectionHandle(self.modules, name, namespace=namespace)

    # -- lifecycle ---------------------------------------------------------

    def close(self) -> None:
        """Releases the transport's connections."""
        self._client.close()

    def __enter__(self: _SelfT) -> _SelfT:
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
        # Checked before the lookup, because the lookup's own error for an empty
        # module reads `loams. has no generated call watch` -- naming the call and
        # an empty module, which sends a caller who simply forgot the module
        # looking for a typo in the call name instead. The guard used to be
        # `found.streaming != "server" and module == ""`, which made it
        # unreachable: `binding_for("")` always raised first, and the one case it
        # was meant to catch -- a streaming call -- is excluded by the condition.
        if module == "":
            raise LoamsError("a module name is required")
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
