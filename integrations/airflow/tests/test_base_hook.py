# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import pytest

from airflow.providers.loams.hooks.base import LoamsHook, LoamsSettings, build_endpoint
from tests.conftest import make_connection


@pytest.mark.parametrize(
    ("host", "port", "scheme", "expected"),
    [
        ("loams.example.com", None, None, "https://loams.example.com"),
        ("loams.example.com", 8443, None, "https://loams.example.com:8443"),
        ("http://127.0.0.1", 8080, None, "http://127.0.0.1:8080"),
        ("http://127.0.0.1:8080/", 9999, None, "http://127.0.0.1:8080"),
        ("127.0.0.1", 8080, "http", "http://127.0.0.1:8080"),
        ("https://h/prefix", 1, None, "https://h:1/prefix"),
    ],
)
def test_build_endpoint(host: str, port: int | None, scheme: str | None, expected: str) -> None:
    assert build_endpoint(host, port, scheme) == expected


def test_build_endpoint_needs_a_host() -> None:
    with pytest.raises(ValueError, match="no host"):
        build_endpoint(None, None, None)


def test_settings_from_connection() -> None:
    s = LoamsSettings.from_connection(
        make_connection(
            {
                "kafka_config": '{"linger.ms": 5}',
                "pg_read_only": "true",
                "verify": "/etc/ca.pem",
                "pg_credential_ttl_seconds": "600",
            }
        )
    )
    assert s.endpoint == "http://loams:8080"
    assert s.api_key == "lk_test"
    assert s.namespace == "acme"
    assert s.kafka_config == {"linger.ms": 5}
    assert s.pg_read_only is True
    assert s.verify == "/etc/ca.pem"
    assert s.pg_credential_ttl_seconds == 600
    assert s.s3_path_style is True


def test_namespace_override_and_extra() -> None:
    s = LoamsSettings.from_connection(make_connection({"namespace": "x"}, schema=None), namespace=None)
    assert s.namespace == "x"
    s = LoamsSettings.from_connection(make_connection({}), namespace="y")
    assert s.namespace == "y"


def test_require_namespace() -> None:
    s = LoamsSettings.from_connection(make_connection({}, schema=None))
    with pytest.raises(ValueError, match="namespace"):
        s.require_namespace()


def test_verify_false() -> None:
    assert LoamsSettings.from_connection(make_connection({"verify": "false"})).verify is False
    assert LoamsSettings.from_connection(make_connection({"verify": False})).verify is False


def test_hook_clients_use_connection(loams_connection) -> None:
    hook = LoamsHook()
    client = hook.connect_client()
    assert client.base_url == "http://loams:8080"
    assert client.token == "lk_test"
    other = hook.connect_client("http://pg:7680", token="other")
    assert (other.base_url, other.token) == ("http://pg:7680", "other")
    assert hook.async_connect_client().base_url == "http://loams:8080"


def test_ui_field_behaviour() -> None:
    ui = LoamsHook.get_ui_field_behaviour()
    assert ui["relabeling"]["password"] == "API key"
    assert "login" in ui["hidden_fields"]
