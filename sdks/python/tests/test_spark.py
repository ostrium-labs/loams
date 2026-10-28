"""Unit tests for `loams.spark` (design 81, SP-b D2005-D2007).

PySpark is not installed in CI, so a fake `pyspark` module tree is put in
`sys.modules` for the session tests; it records what the SDK asks of the
builder. The job facade is driven through a fake `Http`.
"""

from __future__ import annotations

import json
import pathlib
import sys
import types
from collections.abc import Iterator, Mapping
from typing import Any

import pytest

from loams import spark as loams_spark
from loams.runtime.errors import (
    DeadlineExceededError,
    InvalidArgumentError,
    LoamsError,
    NotFoundError,
    UnauthenticatedError,
    UnavailableError,
)
from loams.spark import CatalogOptions, RunStatus, catalog_conf, house_table, job_spec
from loams.spark.driver import main as driver_main
from loams.spark.driver import add_py_files, env_conf, split_sql

ENV_VARS = (
    "LOAMS_ENDPOINT",
    "LOAMS_TOKEN",
    "LOAMS_API_KEY",
    "LOAMS_NAMESPACE",
    "LOAMS_SPARK_REMOTE",
    "LOAMS_SPARK_INSTANCE",
    "LOAMS_ICEBERG_URI",
    "LOAMS_ICEBERG_WAREHOUSE",
)


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in ENV_VARS:
        monkeypatch.delenv(name, raising=False)


#: The facade base for namespace `acme`, instance `spark1` (rulings R2, R15, D2015).
BASE = "http://x/v1/namespaces/acme/engines/spark1"


@pytest.fixture
def scoped(monkeypatch: pytest.MonkeyPatch) -> None:
    """The namespace and instance every facade call needs, from the environment."""
    monkeypatch.setenv("LOAMS_NAMESPACE", "acme")
    monkeypatch.setenv("LOAMS_SPARK_INSTANCE", "spark1")


# --------------------------------------------------------------------------
# Fakes
# --------------------------------------------------------------------------


class FakeChannelBuilder:
    """Stands in for `pyspark.sql.connect.client.ChannelBuilder`."""

    def __init__(self, url: str, channelOptions: Any = None) -> None:
        self.url = url

    def metadata(self) -> list[tuple[str, str]]:
        return [("x-base", "1")]


class FakeBuilder:
    """A chainable `SparkSession.Builder` that records every call."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, tuple[Any, ...]]] = []

    def remote(self, url: str) -> FakeBuilder:
        self.calls.append(("remote", (url,)))
        return self

    def config(self, key: str, value: str) -> FakeBuilder:
        self.calls.append(("config", (key, value)))
        return self

    def channelBuilder(self, cb: Any) -> FakeBuilder:
        self.calls.append(("channelBuilder", (cb,)))
        return self

    def appName(self, name: str) -> FakeBuilder:
        self.calls.append(("appName", (name,)))
        return self

    def conf(self) -> dict[str, str]:
        return {args[0]: args[1] for name, args in self.calls if name == "config"}

    def channel(self) -> Any:
        found = [args[0] for name, args in self.calls if name == "channelBuilder"]
        assert len(found) == 1
        return found[0]


@pytest.fixture
def fake_pyspark(monkeypatch: pytest.MonkeyPatch) -> Iterator[FakeBuilder]:
    recorder = FakeBuilder()

    class FakeSparkSession:
        builder = recorder

    modules: dict[str, types.ModuleType] = {}
    for name in (
        "pyspark",
        "pyspark.sql",
        "pyspark.sql.connect",
        "pyspark.sql.connect.client",
        "pyspark.sql.connect.session",
    ):
        modules[name] = types.ModuleType(name)
    modules["pyspark.sql.connect.client"].__dict__["ChannelBuilder"] = FakeChannelBuilder
    modules["pyspark.sql.connect.session"].__dict__["SparkSession"] = FakeSparkSession
    for name, module in modules.items():
        monkeypatch.setitem(sys.modules, name, module)
    yield recorder


Response = tuple[int, dict[str, str], bytes]


class FakeHttp:
    """Answers from a queue and records each request."""

    def __init__(self, *responses: Response) -> None:
        self.responses = list(responses)
        self.requests: list[tuple[str, str, dict[str, str], bytes | None]] = []

    def request(
        self, method: str, url: str, headers: Mapping[str, str], body: bytes | None
    ) -> Response:
        self.requests.append((method, url, dict(headers), body))
        assert self.responses, f"unexpected request {method} {url}"
        return self.responses.pop(0)

    def body(self, index: int = 0) -> Any:
        raw = self.requests[index][3]
        assert raw is not None
        return json.loads(raw)


def ok(value: object, status: int = 200) -> Response:
    return status, {"content-type": "application/json"}, json.dumps(value).encode()


def run_json(state: str = "pending", **extra: object) -> dict[str, object]:
    value: dict[str, object] = {
        "run_id": "run_1",
        "name": "etl",
        "state": state,
        "attempt": 1,
        "max_attempts": 3,
        "backend": "connect",
        "message": None,
        "exit_code": None,
        "logs": [{"attempt": 1, "url": "loams://spark/runs/run_1/logs?attempt=1"}],
        "created_at": "2026-10-27T10:00:00Z",
        "updated_at": "2026-10-27T10:00:01Z",
    }
    value.update(extra)
    return value


# --------------------------------------------------------------------------
# Sessions
# --------------------------------------------------------------------------


def test_session_sends_token_as_metadata_not_in_uri(fake_pyspark: FakeBuilder) -> None:
    loams_spark.session("sc://spark.example:443/;use_ssl=true", token="secret")
    cb = fake_pyspark.channel()
    assert "secret" not in cb.url
    assert cb.url == "sc://spark.example:443/;use_ssl=true"
    assert cb.metadata() == [("x-base", "1"), ("authorization", "Bearer secret")]
    assert not any(name == "remote" for name, _ in fake_pyspark.calls)


def test_session_adds_namespace_metadata(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_NAMESPACE", "acme")
    monkeypatch.setenv("LOAMS_TOKEN", "envtok")
    loams_spark.session("sc://h:15002")
    assert fake_pyspark.channel().metadata()[1:] == [
        ("authorization", "Bearer envtok"),
        ("x-loams-namespace", "acme"),
    ]


@pytest.mark.parametrize(
    "uri",
    ["sc://h:443/;token=abc", "sc://h:443/;use_ssl=true;token=abc", "sc://h/;TOKEN=abc"],
)
def test_session_refuses_token_in_uri(fake_pyspark: FakeBuilder, uri: str) -> None:
    with pytest.raises(ValueError, match="token"):
        loams_spark.session(uri)
    assert fake_pyspark.calls == []


def test_session_refuses_token_in_env_remote(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_SPARK_REMOTE", "sc://h:15002/;token=abc")
    with pytest.raises(ValueError, match="token"):
        loams_spark.session()


def test_remote_argument_wins_over_env(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_SPARK_REMOTE", "sc://env:15002")
    monkeypatch.setenv("LOAMS_ENDPOINT", "https://loams.example")
    http = FakeHttp()
    loams_spark.session("sc://arg:15002", http=http)
    assert fake_pyspark.channel().url == "sc://arg:15002"
    assert http.requests == []


def test_remote_env_wins_over_endpoint(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_SPARK_REMOTE", "sc://env:15002")
    monkeypatch.setenv("LOAMS_ENDPOINT", "https://loams.example")
    http = FakeHttp()
    loams_spark.session(http=http)
    assert fake_pyspark.channel().url == "sc://env:15002"
    assert http.requests == []


CLOUD_REMOTE = "sc://spark1.acme.eu-west-1.spark.loams.example:443/;use_ssl=true"


def test_remote_from_endpoint_session(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_ENDPOINT", "https://loams.example/")
    http = FakeHttp(ok({"id": "ses_1", "connect_url": CLOUD_REMOTE}))
    loams_spark.session(token="tok", namespace="acme", instance="spark1", http=http)
    method, url, headers, _ = http.requests[0]
    assert (method, url) == (
        "POST",
        "https://loams.example/v1/namespaces/acme/engines/spark1/sessions",
    )
    assert http.body() == {"kind": "python", "conf": {}}
    assert headers["authorization"] == "Bearer tok"
    assert headers["x-loams-namespace"] == "acme"
    assert fake_pyspark.channel().url == CLOUD_REMOTE


def test_remote_session_accepts_remote_key(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_NAMESPACE", "acme")
    monkeypatch.setenv("LOAMS_SPARK_INSTANCE", "eng_0123456789abcdefghijklmnop")
    http = FakeHttp(ok({"remote": "sc://127.0.0.1:15002"}))
    loams_spark.session(endpoint="http://x", http=http)
    assert http.requests[0][1] == (
        "http://x/v1/namespaces/acme/engines/eng_0123456789abcdefghijklmnop/sessions"
    )
    assert fake_pyspark.channel().url == "sc://127.0.0.1:15002"


def test_remote_session_needs_instance(fake_pyspark: FakeBuilder) -> None:
    with pytest.raises(InvalidArgumentError, match="LOAMS_SPARK_INSTANCE"):
        loams_spark.session(endpoint="http://x", namespace="acme", http=FakeHttp())


def test_remote_lookup_answer_with_token_is_refused(fake_pyspark: FakeBuilder) -> None:
    http = FakeHttp(ok({"connect_url": "sc://h:443/;token=leak"}))
    with pytest.raises(ValueError, match="token"):
        loams_spark.session(
            endpoint="https://loams.example", namespace="acme", instance="spark1", http=http
        )


def test_no_remote_is_an_error(fake_pyspark: FakeBuilder) -> None:
    with pytest.raises(InvalidArgumentError, match="LOAMS_SPARK_REMOTE"):
        loams_spark.session()


def test_session_applies_catalog_app_name_and_conf(fake_pyspark: FakeBuilder) -> None:
    catalog = CatalogOptions(uri="http://lakekeeper:8181/catalog", warehouse="wh")
    loams_spark.session(
        "sc://h:15002",
        token="tok",
        catalog=catalog,
        app_name="etl",
        conf={"spark.sql.shuffle.partitions": "4"},
    )
    assert ("appName", ("etl",)) in fake_pyspark.calls
    conf = fake_pyspark.conf()
    assert conf["spark.sql.catalog.loams.token"] == "tok"
    assert conf["spark.sql.shuffle.partitions"] == "4"
    assert conf["spark.sql.defaultCatalog"] == "loams"


def test_session_reads_catalog_from_env(
    fake_pyspark: FakeBuilder, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LOAMS_ICEBERG_URI", "http://lk:8181/catalog")
    monkeypatch.setenv("LOAMS_ICEBERG_WAREHOUSE", "wh")
    loams_spark.session("sc://h:15002")
    assert fake_pyspark.conf()["spark.sql.catalog.loams.uri"] == "http://lk:8181/catalog"


# --------------------------------------------------------------------------
# Catalog
# --------------------------------------------------------------------------


def test_catalog_conf_exact_keys() -> None:
    options = CatalogOptions(uri="http://lk:8181/catalog", warehouse="wh")
    assert catalog_conf(options, "tok") == {
        "spark.sql.catalog.loams": "org.apache.iceberg.spark.SparkCatalog",
        "spark.sql.catalog.loams.type": "rest",
        "spark.sql.catalog.loams.uri": "http://lk:8181/catalog",
        "spark.sql.catalog.loams.warehouse": "wh",
        "spark.sql.catalog.loams.header.X-Iceberg-Access-Delegation": "vended-credentials",
        "spark.sql.catalog.loams.token": "tok",
        "spark.sql.defaultCatalog": "loams",
    }


def test_catalog_conf_s3_and_no_token_no_default() -> None:
    options = CatalogOptions(
        uri="http://lk:8181/catalog",
        warehouse="wh",
        s3_endpoint="http://rustfs:9000",
        default=False,
        name="lake",
    )
    assert catalog_conf(options, None) == {
        "spark.sql.catalog.lake": "org.apache.iceberg.spark.SparkCatalog",
        "spark.sql.catalog.lake.type": "rest",
        "spark.sql.catalog.lake.uri": "http://lk:8181/catalog",
        "spark.sql.catalog.lake.warehouse": "wh",
        "spark.sql.catalog.lake.header.X-Iceberg-Access-Delegation": "vended-credentials",
        "spark.sql.catalog.lake.s3.endpoint": "http://rustfs:9000",
        "spark.sql.catalog.lake.s3.path-style-access": "true",
    }


def test_catalog_options_from_env() -> None:
    env = {"LOAMS_ICEBERG_URI": "http://lk", "LOAMS_ICEBERG_WAREHOUSE": "wh"}
    assert CatalogOptions.from_env(env) == CatalogOptions(uri="http://lk", warehouse="wh")
    with pytest.raises(InvalidArgumentError, match="LOAMS_ICEBERG_WAREHOUSE"):
        CatalogOptions.from_env({"LOAMS_ICEBERG_URI": "http://lk"})


def test_house_table_plain_and_quoted() -> None:
    assert house_table(42, "sales", "orders") == "loams.ns_42.sales.orders"
    assert house_table(42, "default", "orders") == "loams.ns_42.orders"
    assert house_table(7, "my-db", "2024 events") == "loams.ns_7.`my-db`.`2024 events`"
    assert house_table(1, "db", "we`ird", catalog="lake") == "lake.ns_1.db.`we``ird`"
    with pytest.raises(ValueError):
        house_table(-1, "db", "t")
    with pytest.raises(ValueError):
        house_table(1, "", "t")


# --------------------------------------------------------------------------
# Jobs
# --------------------------------------------------------------------------


def test_submit_pyspark_body_shape(scoped: None) -> None:
    http = FakeHttp(ok(run_json()))
    run = loams_spark.submit_job(
        "etl",
        pyspark="s3://b/etl.py",
        py_files=["s3://b/lib.zip"],
        args=["--day", "2026-10-27"],
        conf={"spark.x": "1"},
        resources={"executors": 4},
        max_attempts=3,
        backoff_secs=10,
        endpoint="https://loams.example",
        token="tok",
        http=http,
    )
    method, url, headers, _ = http.requests[0]
    assert (method, url) == (
        "POST",
        "https://loams.example/v1/namespaces/acme/engines/spark1/jobs",
    )
    assert headers["authorization"] == "Bearer tok"
    assert headers["content-type"] == "application/json"
    assert http.body() == {
        "name": "etl",
        "app": {"kind": "pyspark", "file": "s3://b/etl.py", "py_files": ["s3://b/lib.zip"]},
        "args": ["--day", "2026-10-27"],
        "conf": {"spark.x": "1"},
        "resources": {
            "driver_cores": 1,
            "driver_memory": "1g",
            "executor_cores": 1,
            "executor_memory": "1g",
            "executors": 4,
        },
        "retry": {
            "max_attempts": 3,
            "backoff": {"kind": "exponential", "base_secs": 10, "max_secs": 600},
        },
        "backend": "auto",
        "image": None,
    }
    assert run.run_id == "run_1"
    assert run.last is not None and run.last.state == "pending"


def test_submit_jar_and_sql_app_shapes() -> None:
    jar = job_spec("j", jar="s3://b/app.jar", main_class="com.acme.Main", backend="kubernetes")
    assert jar["app"] == {
        "kind": "jar",
        "file": "s3://b/app.jar",
        "main_class": "com.acme.Main",
        "jars": [],
    }
    assert jar["backend"] == "kubernetes"
    sql = job_spec("s", sql="s3://b/report.sql", image="ghcr.io/x/spark:4")
    assert sql["app"] == {"kind": "sql", "file": "s3://b/report.sql"}
    assert sql["image"] == "ghcr.io/x/spark:4"
    assert sql["args"] == [] and sql["conf"] == {}


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({}, "exactly one"),
        ({"pyspark": "a.py", "sql": "b.sql"}, "exactly one"),
        ({"jar": "a.jar"}, "main_class"),
        ({"sql": "a.sql", "py_files": ["x.zip"]}, "py_files"),
        ({"sql": "a.sql", "main_class": "M"}, "main_class"),
        ({"sql": "a.sql", "max_attempts": 0}, "max_attempts"),
        ({"sql": "a.sql", "resources": {"gpus": 1}}, "gpus"),
        ({"sql": "a.sql", "backend": "yarn"}, "backend"),
    ],
)
def test_job_spec_validation(scoped: None, kwargs: dict[str, Any], match: str) -> None:
    http = FakeHttp()
    with pytest.raises(ValueError, match=match):
        loams_spark.submit_job("j", endpoint="http://x", http=http, **kwargs)
    assert http.requests == []


def test_run_status_parsing() -> None:
    parsed = RunStatus.from_json(run_json("failed", exit_code=2, message="boom"))
    assert parsed.state == "failed" and parsed.terminal
    assert parsed.exit_code == 2 and parsed.message == "boom"
    assert parsed.logs[0].attempt == 1
    assert parsed.logs[0].url.startswith("loams://spark/runs/run_1/logs")
    assert not RunStatus.from_json(run_json("retrying")).terminal
    with pytest.raises(LoamsError):
        RunStatus.from_json({"state": "running"})


def test_facade_paths(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LOAMS_ENDPOINT", "https://loams.example")
    monkeypatch.setenv("LOAMS_NAMESPACE", "acme")
    monkeypatch.setenv("LOAMS_SPARK_INSTANCE", "spark1")
    http = FakeHttp(
        ok(run_json("running")),
        (200, {"content-type": "text/plain"}, b"line 1\nline 2\n"),
        (204, {}, b""),
        ok({"jobs": [run_json("running"), run_json("succeeded", run_id="run_2")]}),
        ok({"id": "nightly"}),
        (204, {}, b""),
    )
    assert loams_spark.status("run/1", http=http).state == "running"
    assert loams_spark.logs("run/1", attempt=2, http=http) == "line 1\nline 2\n"
    loams_spark.run("run/1", http=http).cancel()
    listed = loams_spark.list_jobs(http=http)
    assert [r.run_id for r in listed] == ["run_1", "run_2"]
    spec = job_spec("etl", pyspark="s3://b/etl.py")
    assert loams_spark.schedule("nightly", "0 2 * * *", spec, http=http) == {"id": "nightly"}
    loams_spark.delete_schedule("night ly", http=http)
    base = "https://loams.example/v1/namespaces/acme/engines/spark1"
    assert [(m, u) for m, u, _, _ in http.requests] == [
        ("GET", f"{base}/jobs/run%2F1"),
        ("GET", f"{base}/jobs/run%2F1/logs?attempt=2"),
        ("POST", f"{base}/jobs/run%2F1/cancel"),
        ("GET", f"{base}/jobs"),
        ("POST", f"{base}/schedules"),
        ("DELETE", f"{base}/schedules/night%20ly"),
    ]
    assert all(h["x-loams-namespace"] == "acme" for _, _, h, _ in http.requests)
    assert http.requests[1][2]["accept"] == "text/plain"
    assert http.body(4) == {"id": "nightly", "cron": "0 2 * * *", "job": spec}


def test_path_segments_are_quoted() -> None:
    http = FakeHttp(ok(run_json()))
    loams_spark.status("r", endpoint="http://x", namespace="a/b", instance="my spark", http=http)
    assert http.requests[0][1] == "http://x/v1/namespaces/a%2Fb/engines/my%20spark/jobs/r"


@pytest.mark.parametrize(
    "answer",
    [
        [run_json()],
        {"jobs": [run_json()]},
        {"runs": [run_json()]},
    ],
)
def test_list_jobs_answer_shapes(scoped: None, answer: object) -> None:
    http = FakeHttp(ok(answer))
    assert [r.run_id for r in loams_spark.list_jobs(endpoint="http://x", http=http)] == ["run_1"]
    assert http.requests[0][:2] == ("GET", f"{BASE}/jobs")


@pytest.mark.parametrize(
    ("env", "missing"),
    [
        ({"LOAMS_SPARK_INSTANCE": "spark1"}, "LOAMS_NAMESPACE"),
        ({"LOAMS_NAMESPACE": "acme"}, "LOAMS_SPARK_INSTANCE"),
    ],
)
def test_namespace_and_instance_are_required(
    monkeypatch: pytest.MonkeyPatch, env: dict[str, str], missing: str
) -> None:
    for key, value in env.items():
        monkeypatch.setenv(key, value)
    http = FakeHttp()
    with pytest.raises(InvalidArgumentError, match=missing) as caught:
        loams_spark.submit_job("j", sql="a.sql", endpoint="http://x", http=http)
    assert caught.value.metadata == {"field": missing}
    assert http.requests == []


def test_run_wait_polls_to_terminal_state(scoped: None) -> None:
    http = FakeHttp(
        ok(run_json("pending")),
        ok(run_json("submitted")),
        ok(run_json("running")),
        ok(run_json("succeeded", exit_code=0)),
    )
    run = loams_spark.submit_job("etl", sql="s3://b/x.sql", endpoint="http://x", http=http)
    sleeps: list[float] = []
    final = run.wait(poll_secs=2, sleep=sleeps.append, clock=lambda: 0.0)
    assert final.state == "succeeded"
    assert sleeps == [2, 2]


def test_run_wait_times_out(scoped: None) -> None:
    now = [0.0]

    def sleep(secs: float) -> None:
        now[0] += secs

    http = FakeHttp(*(ok(run_json("running")) for _ in range(10)))
    run = loams_spark.run("run_1", endpoint="http://x", http=http)
    with pytest.raises(DeadlineExceededError):
        run.wait(poll_secs=5, timeout=12, sleep=sleep, clock=lambda: now[0])
    assert now[0] == 12


@pytest.mark.parametrize(
    ("status", "cls"),
    [(401, UnauthenticatedError), (404, NotFoundError), (503, UnavailableError)],
)
def test_error_mapping(scoped: None, status: int, cls: type[LoamsError]) -> None:
    body = json.dumps({"message": "nope", "reason": "spark_run_missing"}).encode()
    http = FakeHttp((status, {}, body))
    with pytest.raises(cls) as caught:
        loams_spark.status("run_1", endpoint="http://x", http=http)
    assert "nope" in str(caught.value)
    assert caught.value.metadata == {"http_status": str(status), "reason": "spark_run_missing"}


def test_unmapped_status_is_base_error(scoped: None) -> None:
    http = FakeHttp((418, {}, b"teapot"))
    with pytest.raises(LoamsError) as caught:
        loams_spark.status("run_1", endpoint="http://x", http=http)
    assert type(caught.value) is LoamsError
    assert "teapot" in str(caught.value)


def test_missing_endpoint_is_an_error(scoped: None) -> None:
    with pytest.raises(InvalidArgumentError, match="LOAMS_ENDPOINT"):
        loams_spark.status("run_1", http=FakeHttp())


# --------------------------------------------------------------------------
# Driver
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("SELECT 1; SELECT 2;", ["SELECT 1", "SELECT 2"]),
        ("SELECT 'a;b'; SELECT \"c;d\"", ["SELECT 'a;b'", 'SELECT "c;d"']),
        ("SELECT `we;ird` FROM t", ["SELECT `we;ird` FROM t"]),
        ("SELECT 'it''s; fine'", ["SELECT 'it''s; fine'"]),
        ("SELECT 'esc\\'; still'", ["SELECT 'esc\\'; still'"]),
        ("SELECT 1 -- note; not a split\n; SELECT 2", ["SELECT 1", "SELECT 2"]),
        ("-- only a comment;\n", []),
        ("SELECT /* a; b */ 1", ["SELECT /* a; b */ 1"]),
        (";;  ;\n\t; SELECT 3  \n\n", ["SELECT 3"]),
        ("  CREATE TABLE t (x INT)  ;  \n  ", ["CREATE TABLE t (x INT)"]),
        ("SELECT 1 -- trailing comment", ["SELECT 1"]),
    ],
)
def test_split_sql(text: str, expected: list[str]) -> None:
    assert split_sql(text) == expected


class FakeFrame:
    def __init__(self, rows: int) -> None:
        self.rows = rows

    def take(self, n: int) -> list[tuple[int]]:
        return [(i,) for i in range(min(n, self.rows))]


class FakeSpark:
    def __init__(self) -> None:
        self.statements: list[str] = []

    def sql(self, statement: str) -> FakeFrame:
        self.statements.append(statement)
        return FakeFrame(3)


def test_driver_runs_sql_file(tmp_path: pathlib.Path, capsys: pytest.CaptureFixture[str]) -> None:
    script = tmp_path / "job.sql"
    script.write_text("CREATE TABLE t (x INT);\nSELECT * FROM t;\n", encoding="utf-8")
    spark = FakeSpark()
    code = driver_main(["--app", str(script), "--sql"], session_factory=lambda: spark)
    assert code == 0
    assert spark.statements == ["CREATE TABLE t (x INT)", "SELECT * FROM t"]
    out = capsys.readouterr().out.splitlines()
    assert out == [
        "[1/2] ok, 3 row(s): CREATE TABLE t (x INT)",
        "[2/2] ok, 3 row(s): SELECT * FROM t",
    ]


def test_driver_runs_script_as_main(tmp_path: pathlib.Path) -> None:
    out = tmp_path / "out.txt"
    script = tmp_path / "job.py"
    script.write_text(
        "import sys\n"
        "if __name__ == '__main__':\n"
        f"    with open({str(out)!r}, 'w') as f:\n"
        "        f.write(' '.join(sys.argv[1:]) + '|' + __file__)\n",
        encoding="utf-8",
    )
    saved = list(sys.argv)
    code = driver_main(
        ["--app", str(script), "--", "--day", "x"], session_factory=lambda: FakeSpark()
    )
    assert code == 0
    assert out.read_text(encoding="utf-8") == f"--day x|{script}"
    assert sys.argv == saved


def test_driver_exit_codes(tmp_path: pathlib.Path, capsys: pytest.CaptureFixture[str]) -> None:
    failing = tmp_path / "fail.py"
    failing.write_text("raise RuntimeError('boom')\n", encoding="utf-8")
    assert driver_main(["--app", str(failing)], session_factory=lambda: FakeSpark()) == 1
    assert "RuntimeError: boom" in capsys.readouterr().err
    exiting = tmp_path / "exit.py"
    exiting.write_text("import sys\nsys.exit(3)\n", encoding="utf-8")
    assert driver_main(["--app", str(exiting)], session_factory=lambda: FakeSpark()) == 3


def test_env_conf_reads_the_runner_conf() -> None:
    assert env_conf({}) == {}
    conf = env_conf({"LOAMS_SPARK_CONF": '{"spark.app.name": "etl", "spark.x": 8}'})
    assert conf == {"spark.app.name": "etl", "spark.x": "8"}
    with pytest.raises(ValueError, match="JSON object"):
        env_conf({"LOAMS_SPARK_CONF": "[1]"})


class _BinaryReader:
    def __init__(self, content: bytes) -> None:
        self.content = content
        self.loaded: list[str] = []

    @property
    def read(self) -> _BinaryReader:
        return self

    def format(self, name: str) -> _BinaryReader:
        assert name == "binaryFile"
        return self

    def load(self, uri: str) -> _BinaryReader:
        self.loaded.append(uri)
        return self

    def select(self, column: str) -> _BinaryReader:
        assert column == "content"
        return self

    def first(self) -> tuple[bytes]:
        return (self.content,)


class _ArtifactSpark(_BinaryReader):
    def __init__(self, content: bytes) -> None:
        super().__init__(content)
        self.artifacts: list[tuple[str, bool]] = []

    def addArtifacts(self, path: str, pyfile: bool = False) -> None:  # noqa: N802
        self.artifacts.append((path, pyfile))


def test_py_files_are_shipped_as_artifacts(tmp_path: pathlib.Path) -> None:
    local = tmp_path / "helpers.py"
    local.write_text("X = 1\n")
    spark = _ArtifactSpark(b"PK\x03\x04zip")
    added = add_py_files(spark, [str(local), "s3://jobs/lib/deps.zip"])
    assert added[0] == str(local)
    assert added[1].endswith("deps.zip")
    assert pathlib.Path(added[1]).read_bytes() == b"PK\x03\x04zip"
    assert spark.loaded == ["s3://jobs/lib/deps.zip"]
    assert spark.artifacts == [(str(local), True), (added[1], True)]
