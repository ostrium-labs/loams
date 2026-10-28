"""Managed Spark from Python (design 81, SP-b D2005-D2008). **Preview.**

Two halves:

- **Sessions.** `session()` returns a Spark Connect `SparkSession.Builder` aimed at
  the namespace's Spark Connect endpoint, with the Loams token sent as gRPC
  metadata and the `loams` Iceberg catalog configured::

      from loams import spark as loams_spark

      spark = loams_spark.session().getOrCreate()
      spark.sql("SELECT * FROM loams.ns_42.sales.orders").show()

  PySpark is not a dependency of `loams`: install the extra, `pip install
  'loams[spark]'`, which pulls `pyspark-client` (Spark Connect only, no JVM).

- **Batch jobs.** `submit_job()` sends a job spec to the Loams Spark job facade
  and returns a `Run` to poll, read logs from, wait on or cancel. `schedule()`
  registers a cron schedule for a spec. These need no PySpark at all.

Every facade route is per namespace and per Spark engine instance (design 81
rulings R2, R15, D2015)::

    {LOAMS_ENDPOINT}/v1/namespaces/{ns}/engines/{instance}/jobs
                                                         /jobs/{run}[/logs|/cancel]
                                                         /schedules[/{id}]
                                                         /sessions

The namespace is `namespace=` or `LOAMS_NAMESPACE`; the instance is `instance=`
or `LOAMS_SPARK_INSTANCE` (its id `eng_<26>` or its name).

The Spark Connect endpoint (D2008) is, in the cloud::

    sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true

and `sc://127.0.0.1:15002` on compose.

The token never goes in the `sc://` URI (design 26 §9.2): a URI carries into
logs, process listings and Spark's own UI, a header does not.
"""

from __future__ import annotations

import importlib
import json
import os
import re
import time
import urllib.parse
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Final, Literal

from connectrpc.code import Code

from loams.runtime.errors import DeadlineExceededError, InternalError, InvalidArgumentError
from loams.runtime.token_source import ENV_ENDPOINT
from loams.spark._http import (
    ENV_INSTANCE,
    ENV_NAMESPACE,
    NAMESPACE_HEADER,
    Http,
    JsonClient,
    resolve_token,
)

__all__ = [
    "ENV_ICEBERG_URI",
    "ENV_ICEBERG_WAREHOUSE",
    "ENV_INSTANCE",
    "ENV_NAMESPACE",
    "ENV_SPARK_REMOTE",
    "TERMINAL_STATES",
    "Backend",
    "CatalogOptions",
    "Http",
    "LogLink",
    "Run",
    "RunStatus",
    "catalog_conf",
    "delete_schedule",
    "house_table",
    "job_spec",
    "list_jobs",
    "logs",
    "run",
    "schedule",
    "session",
    "status",
    "submit_job",
]

#: A Spark Connect URI that wins over everything but the `remote` argument
#: (D2008: `sc://127.0.0.1:15002` on compose, SP-a's proxy port).
ENV_SPARK_REMOTE: Final[str] = "LOAMS_SPARK_REMOTE"
#: The Lakekeeper Iceberg REST URI.
ENV_ICEBERG_URI: Final[str] = "LOAMS_ICEBERG_URI"
#: The Lakekeeper warehouse name.
ENV_ICEBERG_WAREHOUSE: Final[str] = "LOAMS_ICEBERG_WAREHOUSE"

# Relative to `JsonClient.base`, `{endpoint}/v1/namespaces/{ns}/engines/{instance}`.
_JOBS: Final[str] = "/jobs"
_SCHEDULES: Final[str] = "/schedules"
_SESSIONS: Final[str] = "/sessions"

Backend = Literal["auto", "connect", "kubernetes", "local"]
_BACKENDS: Final[frozenset[str]] = frozenset({"auto", "connect", "kubernetes", "local"})

#: The states a run does not leave (D2003).
TERMINAL_STATES: Final[frozenset[str]] = frozenset({"succeeded", "failed", "canceled"})

#: The resources of a job that does not name them, and the keys a caller may set.
_DEFAULT_RESOURCES: Final[Mapping[str, int | str]] = {
    "driver_cores": 1,
    "driver_memory": "1g",
    "executor_cores": 1,
    "executor_memory": "1g",
    "executors": 1,
}

#: The cap on the exponential backoff between attempts, in seconds.
_BACKOFF_MAX_SECS: Final[int] = 600

_PLAIN_IDENT: Final[re.Pattern[str]] = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


# --------------------------------------------------------------------------
# Catalog
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class CatalogOptions:
    """Where the `loams` Iceberg catalog lives (D2005).

    :param uri: the Iceberg REST URI (Lakekeeper).
    :param warehouse: the Lakekeeper warehouse.
    :param s3_endpoint: the S3 endpoint, for an S3-compatible store such as
        RustFS on compose; it also turns on path-style access.
    :param default: make this catalog `spark.sql.defaultCatalog`.
    :param name: the Spark catalog name. Keep `loams` unless two catalogs meet
        in one session: `house_table` and the docs assume it.
    """

    uri: str
    warehouse: str
    s3_endpoint: str | None = None
    default: bool = True
    name: str = "loams"

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> CatalogOptions:
        """`LOAMS_ICEBERG_URI` and `LOAMS_ICEBERG_WAREHOUSE`; both are required."""
        env = os.environ if environ is None else environ
        uri = env.get(ENV_ICEBERG_URI, "").strip()
        warehouse = env.get(ENV_ICEBERG_WAREHOUSE, "").strip()
        if not uri or not warehouse:
            missing = ENV_ICEBERG_URI if not uri else ENV_ICEBERG_WAREHOUSE
            raise InvalidArgumentError(
                f"loams.spark: {missing} is not set",
                code=Code.INVALID_ARGUMENT,
                metadata={"field": missing},
            )
        return cls(uri=uri, warehouse=warehouse)


def catalog_conf(options: CatalogOptions, token: str | None) -> dict[str, str]:
    """The Spark conf for an Iceberg REST catalog at Lakekeeper (D2005).

    Credentials are vended by the catalog per table
    (`X-Iceberg-Access-Delegation: vended-credentials`), so no S3 key is set
    here. The Loams token, when given, is the catalog's bearer; it goes in conf,
    never in the catalog URI.
    """
    prefix = f"spark.sql.catalog.{options.name}"
    conf = {
        prefix: "org.apache.iceberg.spark.SparkCatalog",
        f"{prefix}.type": "rest",
        f"{prefix}.uri": options.uri,
        f"{prefix}.warehouse": options.warehouse,
        f"{prefix}.header.X-Iceberg-Access-Delegation": "vended-credentials",
    }
    if token:
        conf[f"{prefix}.token"] = token
    if options.s3_endpoint:
        conf[f"{prefix}.s3.endpoint"] = options.s3_endpoint
        conf[f"{prefix}.s3.path-style-access"] = "true"
    if options.default:
        conf["spark.sql.defaultCatalog"] = options.name
    return conf


def _quote_ident(name: str) -> str:
    if not name:
        raise ValueError("loams.spark: an identifier may not be empty")
    if _PLAIN_IDENT.fullmatch(name):
        return name
    return "`" + name.replace("`", "``") + "`"


def house_table(namespace_id: int, database: str, table: str, catalog: str = "loams") -> str:
    """The Spark name of a table the House sees as `<database>.<table>`.

    The House reads the Iceberg namespace `ns_<id>.<db>` as the ClickHouse
    database `<db>` (design 49 §5.3), so a table Spark writes there is a House
    external table with no registration step (D2005). Identifiers that are not
    `[A-Za-z_][A-Za-z0-9_]*` are quoted with backticks.
    """
    if isinstance(namespace_id, bool) or not isinstance(namespace_id, int) or namespace_id < 0:
        raise ValueError(
            f"loams.spark: namespace_id must be a non-negative int, got {namespace_id!r}"
        )
    # `default` is the root namespace itself (design 49 §5.3), as in the Rust
    # `loams_spark::catalog::house_table`.
    parts = [catalog, f"ns_{namespace_id}", table]
    if database != "default":
        parts.insert(2, database)
    return ".".join(_quote_ident(part) for part in parts)


# --------------------------------------------------------------------------
# Sessions
# --------------------------------------------------------------------------


def _uri_params(remote: str) -> dict[str, str]:
    """The `;key=value` parameters of an `sc://host:port/;k=v;...` URI."""
    _, sep, tail = remote.partition("/;")
    if not sep:
        return {}
    params: dict[str, str] = {}
    for item in tail.split(";"):
        key, _, value = item.partition("=")
        if key:
            params[urllib.parse.unquote(key).strip().lower()] = urllib.parse.unquote(value)
    return params


def _check_remote(remote: str) -> str:
    remote = remote.strip()
    if not remote.startswith("sc://"):
        raise ValueError(f"loams.spark: a Spark Connect URI starts with sc://, got {remote!r}")
    if "token" in _uri_params(remote):
        # Design 26 §9.2: the token is gRPC metadata, never part of the URI.
        raise ValueError(
            "loams.spark: refusing a Spark Connect URI with a token= parameter; "
            "pass token= (or set LOAMS_TOKEN) and it is sent as an Authorization header"
        )
    return remote


def _resolve_remote(
    remote: str | None,
    *,
    endpoint: str | None,
    token: str | None,
    namespace: str | None,
    instance: str | None,
    http: Http | None,
) -> str:
    if remote:
        return _check_remote(remote)
    from_env = os.environ.get(ENV_SPARK_REMOTE, "").strip()
    if from_env:
        return _check_remote(from_env)
    if endpoint or os.environ.get(ENV_ENDPOINT, "").strip():
        client = JsonClient(
            endpoint, token=token, namespace=namespace, instance=instance, http=http
        )
        answer = client.json("POST", _SESSIONS, json_body={"kind": "python", "conf": {}})
        found: object = None
        if isinstance(answer, dict):
            # The SparkSession record's `connect_url`; `remote` is the older key.
            found = answer.get("connect_url") or answer.get("remote")
        if not isinstance(found, str) or not found:
            raise InternalError(
                f"POST {_SESSIONS}: the answer has no connect_url",
                code=Code.INTERNAL,
                rpc=_SESSIONS,
            )
        return _check_remote(found)
    raise InvalidArgumentError(
        "loams.spark: no Spark Connect endpoint: pass remote=, or set LOAMS_SPARK_REMOTE "
        "or LOAMS_ENDPOINT",
        code=Code.INVALID_ARGUMENT,
        metadata={"field": ENV_SPARK_REMOTE},
    )


def _channel_builder(remote: str, metadata: Sequence[tuple[str, str]]) -> Any:
    """A Spark Connect `ChannelBuilder` that adds `metadata` to every call.

    PySpark 4.x lets a session take a custom channel builder
    (`SparkSession.builder.channelBuilder(cb)`), and the client sends whatever
    `cb.metadata()` returns with each RPC. Subclassing the URL-parsing builder
    and appending to `metadata()` is how the bearer reaches Sail or Spark
    without being in the URI (design 26 §9.2, D2007).
    """
    client = importlib.import_module("pyspark.sql.connect.client")
    base: Any = getattr(client, "DefaultChannelBuilder", None) or client.ChannelBuilder
    extra = list(metadata)

    class _LoamsChannelBuilder(base):  # type: ignore[misc]
        def metadata(self) -> list[tuple[str, str]]:
            return [*super().metadata(), *extra]

    return _LoamsChannelBuilder(remote)


def session(
    remote: str | None = None,
    *,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    catalog: CatalogOptions | None = None,
    app_name: str | None = None,
    conf: Mapping[str, str] | None = None,
    endpoint: str | None = None,
    http: Http | None = None,
) -> Any:
    """A Spark Connect `SparkSession.Builder` for this namespace (D2007).

    Call `.getOrCreate()` on it, or add `.config(...)` first.

    :param remote: the `sc://` URI. Else `LOAMS_SPARK_REMOTE`, else the
        `connect_url` of a session opened on the Loams endpoint
        (`POST .../engines/{instance}/sessions`), else an error. A URI with a
        `token=` parameter is refused with `ValueError`. Cloud:
        `sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true`;
        compose: `sc://127.0.0.1:15002`.
    :param token: the bearer; else `LOAMS_API_KEY`, then `LOAMS_TOKEN`. Sent as
        gRPC metadata `authorization: Bearer ...`.
    :param namespace: sent as `x-loams-namespace`; else `LOAMS_NAMESPACE`.
    :param instance: the Spark engine instance (id or name) for the session
        lookup; else `LOAMS_SPARK_INSTANCE`.
    :param catalog: the Iceberg catalog; else `CatalogOptions.from_env()` when
        `LOAMS_ICEBERG_URI` is set, else no catalog conf (the server's own).
    :param app_name: the Spark application name.
    :param conf: more Spark conf, applied after the catalog conf.
    :param endpoint: the Loams endpoint for the session lookup; else
        `LOAMS_ENDPOINT`.
    :param http: the HTTP client for the session lookup (tests).
    """
    bearer = resolve_token(token)
    ns = namespace or os.environ.get(ENV_NAMESPACE) or None
    url = _resolve_remote(
        remote, endpoint=endpoint, token=bearer, namespace=ns, instance=instance, http=http
    )

    metadata: list[tuple[str, str]] = []
    if bearer:
        metadata.append(("authorization", f"Bearer {bearer}"))
    if ns:
        metadata.append((NAMESPACE_HEADER, ns))

    module = importlib.import_module("pyspark.sql.connect.session")
    builder: Any = module.SparkSession.builder.channelBuilder(_channel_builder(url, metadata))
    if app_name:
        builder = builder.appName(app_name)
    if catalog is None and os.environ.get(ENV_ICEBERG_URI, "").strip():
        catalog = CatalogOptions.from_env()
    merged: dict[str, str] = catalog_conf(catalog, bearer) if catalog is not None else {}
    merged.update(conf or {})
    for key, value in merged.items():
        builder = builder.config(key, value)
    return builder


# --------------------------------------------------------------------------
# Jobs
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class LogLink:
    """Where one attempt's driver log is (D2003)."""

    attempt: int
    url: str


@dataclass(frozen=True)
class RunStatus:
    """One run of a Spark job, as the facade reports it (D2003).

    `state` is one of `pending`, `submitted`, `running`, `retrying`,
    `succeeded`, `failed`, `canceled`; it is kept as text so a state a newer
    server adds is carried rather than refused.
    """

    run_id: str
    name: str
    state: str
    attempt: int
    max_attempts: int
    backend: str
    message: str | None
    exit_code: int | None
    logs: tuple[LogLink, ...]
    created_at: str
    updated_at: str

    @property
    def terminal(self) -> bool:
        """Whether the run is over: succeeded, failed or canceled."""
        return self.state in TERMINAL_STATES

    @classmethod
    def from_json(cls, value: object) -> RunStatus:
        """Parses the facade's `RunStatus` JSON."""
        if not isinstance(value, dict):
            raise InternalError(
                "loams.spark: a run status is not a JSON object", code=Code.INTERNAL
            )
        try:
            raw_logs = value.get("logs") or []
            links = tuple(
                LogLink(attempt=int(item["attempt"]), url=str(item["url"])) for item in raw_logs
            )
            message = value.get("message")
            exit_code = value.get("exit_code")
            return cls(
                run_id=str(value["run_id"]),
                name=str(value.get("name", "")),
                state=str(value["state"]),
                attempt=int(value.get("attempt", 0)),
                max_attempts=int(value.get("max_attempts", 1)),
                backend=str(value.get("backend", "")),
                message=None if message is None else str(message),
                exit_code=None if exit_code is None else int(exit_code),
                logs=links,
                created_at=str(value.get("created_at", "")),
                updated_at=str(value.get("updated_at", "")),
            )
        except (KeyError, TypeError, ValueError) as err:
            raise InternalError(
                f"loams.spark: malformed run status: {err}", code=Code.INTERNAL
            ) from err


def _run_path(run_id: str) -> str:
    return f"{_JOBS}/{urllib.parse.quote(run_id, safe='')}"


class Run:
    """A handle on one submitted run."""

    def __init__(self, run_id: str, client: JsonClient, initial: RunStatus | None = None) -> None:
        """:param initial: the status the submit answered with, if any."""
        self.run_id = run_id
        self._client = client
        #: The last status this handle saw.
        self.last: RunStatus | None = initial

    def __repr__(self) -> str:
        state = self.last.state if self.last is not None else "?"
        return f"Run({self.run_id!r}, state={state!r})"

    def status(self) -> RunStatus:
        """`GET .../jobs/{run}`."""
        self.last = RunStatus.from_json(self._client.json("GET", _run_path(self.run_id)))
        return self.last

    def logs(self, attempt: int | None = None) -> str:
        """The driver log of one attempt (the latest when `attempt` is None)."""
        query = None if attempt is None else {"attempt": str(attempt)}
        body = self._client.call(
            "GET", _run_path(self.run_id) + "/logs", query=query, accept="text/plain"
        )
        return body.decode("utf-8", errors="replace")

    def cancel(self) -> None:
        """`POST .../jobs/{run}/cancel`. Canceling a finished run is a no-op."""
        self._client.call("POST", _run_path(self.run_id) + "/cancel")

    def wait(
        self,
        poll_secs: float = 5,
        timeout: float | None = None,
        *,
        sleep: Callable[[float], None] = time.sleep,
        clock: Callable[[], float] = time.monotonic,
    ) -> RunStatus:
        """Polls until the run is succeeded, failed or canceled, and returns that status.

        It does not raise for a failed run: check `state`. Past `timeout`
        seconds it raises `DeadlineExceededError` and leaves the run running.
        """
        if poll_secs <= 0:
            raise ValueError("loams.spark: poll_secs must be positive")
        deadline = None if timeout is None else clock() + timeout
        while True:
            current = self.status()
            if current.terminal:
                return current
            if deadline is not None:
                left = deadline - clock()
                if left <= 0:
                    raise DeadlineExceededError(
                        f"loams.spark: run {self.run_id} still {current.state} after {timeout}s",
                        code=Code.DEADLINE_EXCEEDED,
                        reason="client_deadline_exceeded",
                    )
                sleep(min(poll_secs, left))
            else:
                sleep(poll_secs)


def job_spec(
    name: str,
    *,
    pyspark: str | None = None,
    jar: str | None = None,
    main_class: str | None = None,
    sql: str | None = None,
    py_files: Sequence[str] = (),
    jars: Sequence[str] = (),
    args: Sequence[str] = (),
    conf: Mapping[str, str] | None = None,
    resources: Mapping[str, int | str] | None = None,
    max_attempts: int = 1,
    backoff_secs: int = 30,
    backend: Backend = "auto",
    image: str | None = None,
) -> dict[str, object]:
    """The job spec JSON of D2000, as `submit_job` and `schedule` send it.

    Exactly one of `pyspark`, `jar`, `sql`; `jar` needs `main_class`. Files are
    object-store URIs (`s3://...`), or local paths for the local runner only.
    """
    if not name:
        raise ValueError("loams.spark: a job needs a name")
    given = [kind for kind, value in (("pyspark", pyspark), ("jar", jar), ("sql", sql)) if value]
    if len(given) != 1:
        raise ValueError(
            "loams.spark: pass exactly one of pyspark=, jar=, sql=, got "
            + (", ".join(given) if given else "none")
        )
    if py_files and pyspark is None:
        raise ValueError("loams.spark: py_files= applies to pyspark= jobs only")
    if jars and jar is None:
        raise ValueError("loams.spark: jars= applies to jar= jobs only")
    if main_class and jar is None:
        raise ValueError("loams.spark: main_class= applies to jar= jobs only")

    app: dict[str, object]
    if pyspark:
        app = {"kind": "pyspark", "file": pyspark, "py_files": list(py_files)}
    elif jar:
        if not main_class:
            raise ValueError("loams.spark: a jar= job needs main_class=")
        app = {"kind": "jar", "file": jar, "main_class": main_class, "jars": list(jars)}
    else:
        app = {"kind": "sql", "file": sql}

    unknown = set(resources or {}) - set(_DEFAULT_RESOURCES)
    if unknown:
        raise ValueError(f"loams.spark: unknown resources key(s): {', '.join(sorted(unknown))}")
    if max_attempts < 1:
        raise ValueError("loams.spark: max_attempts must be at least 1")
    if backoff_secs < 0:
        raise ValueError("loams.spark: backoff_secs may not be negative")
    if backend not in _BACKENDS:
        raise ValueError(
            f"loams.spark: backend must be one of {sorted(_BACKENDS)}, got {backend!r}"
        )

    return {
        "name": name,
        "app": app,
        "args": [str(a) for a in args],
        "conf": dict(conf or {}),
        "resources": {**_DEFAULT_RESOURCES, **(resources or {})},
        "retry": {
            "max_attempts": max_attempts,
            "backoff": {
                "kind": "exponential",
                "base_secs": backoff_secs,
                "max_secs": _BACKOFF_MAX_SECS,
            },
        },
        "backend": backend,
        "image": image,
    }


def submit_job(
    name: str,
    *,
    pyspark: str | None = None,
    jar: str | None = None,
    main_class: str | None = None,
    sql: str | None = None,
    py_files: Sequence[str] = (),
    jars: Sequence[str] = (),
    args: Sequence[str] = (),
    conf: Mapping[str, str] | None = None,
    resources: Mapping[str, int | str] | None = None,
    max_attempts: int = 1,
    backoff_secs: int = 30,
    backend: Backend = "auto",
    image: str | None = None,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> Run:
    """Submits a Spark batch job (`POST .../engines/{instance}/jobs`) and returns its `Run`.

    `backend="auto"` lets the server pick: Spark Connect for `pyspark` and
    `sql`, Spark on Kubernetes for `jar` (D2001). The endpoint is `endpoint=`
    or `LOAMS_ENDPOINT`; the namespace `namespace=` or `LOAMS_NAMESPACE`; the
    instance `instance=` or `LOAMS_SPARK_INSTANCE`; the token `token=`, else
    `LOAMS_API_KEY`, then `LOAMS_TOKEN`.
    """
    spec = job_spec(
        name,
        pyspark=pyspark,
        jar=jar,
        main_class=main_class,
        sql=sql,
        py_files=py_files,
        jars=jars,
        args=args,
        conf=conf,
        resources=resources,
        max_attempts=max_attempts,
        backoff_secs=backoff_secs,
        backend=backend,
        image=image,
    )
    client = JsonClient(endpoint, token=token, namespace=namespace, instance=instance, http=http)
    first = RunStatus.from_json(client.json("POST", _JOBS, json_body=spec))
    return Run(first.run_id, client, first)


def run(
    run_id: str,
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> Run:
    """A handle on an existing run, without a request."""
    client = JsonClient(endpoint, token=token, namespace=namespace, instance=instance, http=http)
    return Run(run_id, client)


def status(
    run_id: str,
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> RunStatus:
    """`GET .../engines/{instance}/jobs/{run}`."""
    handle = run(
        run_id, endpoint=endpoint, token=token, namespace=namespace, instance=instance, http=http
    )
    return handle.status()


def logs(
    run_id: str,
    attempt: int | None = None,
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> str:
    """The driver log of one attempt of a run (the latest when `attempt` is None)."""
    handle = run(
        run_id, endpoint=endpoint, token=token, namespace=namespace, instance=instance, http=http
    )
    return handle.logs(attempt)


def list_jobs(
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> list[RunStatus]:
    """`GET .../engines/{instance}/jobs`: the instance's runs.

    The answer may be a bare list, `{"jobs": [...]}` or `{"runs": [...]}`.
    """
    client = JsonClient(endpoint, token=token, namespace=namespace, instance=instance, http=http)
    answer = client.json("GET", _JOBS)
    items: object = answer
    if isinstance(answer, dict):
        items = answer.get("jobs", answer.get("runs"))
    if answer is None:
        items = []
    if not isinstance(items, list):
        raise InternalError(
            f"GET {_JOBS}: the answer is not a list of runs", code=Code.INTERNAL, rpc=_JOBS
        )
    return [RunStatus.from_json(item) for item in items]


def schedule(
    name: str,
    cron: str,
    job: Mapping[str, object],
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> object:
    """Registers a cron schedule for a job spec (`POST .../engines/{instance}/schedules`).

    `job` is a spec as `job_spec(...)` builds it. Each tick starts one run
    through the embedded durable server (D2004). Returns the facade's answer.
    """
    if not name or not cron.strip():
        raise ValueError("loams.spark: a schedule needs a name and a cron expression")
    client = JsonClient(endpoint, token=token, namespace=namespace, instance=instance, http=http)
    body = {"id": name, "cron": cron, "job": dict(job)}
    # Round-trip through json here so a spec that cannot be sent fails before
    # the request, with the caller's traceback.
    json.dumps(body)
    return client.json("POST", _SCHEDULES, json_body=body)


def delete_schedule(
    name: str,
    *,
    endpoint: str | None = None,
    token: str | None = None,
    namespace: str | None = None,
    instance: str | None = None,
    http: Http | None = None,
) -> None:
    """`DELETE .../engines/{instance}/schedules/{id}`. Runs already started keep running."""
    if not name:
        raise ValueError("loams.spark: delete_schedule needs a schedule id")
    client = JsonClient(endpoint, token=token, namespace=namespace, instance=instance, http=http)
    client.call("DELETE", f"{_SCHEDULES}/{urllib.parse.quote(name, safe='')}")
