# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

from urllib.parse import urlsplit

import pytest

from airflow.providers.loams.assets.loams import create_asset, loams_uri, sanitize_uri


def test_sanitize() -> None:
    assert sanitize_uri(urlsplit("LOAMS://Acme/streams//orders/#x")).geturl() == "loams://acme/streams/orders"
    assert (
        sanitize_uri(urlsplit("loams://acme/tables/sales.orders")).geturl()
        == "loams://acme/tables/sales.orders"
    )


@pytest.mark.parametrize(
    "uri",
    [
        "loams:///streams/x",
        "loams://acme",
        "loams://acme/",
        "loams://u:p@acme/x",
        "loams://acme:80/x",
        "s3://b/k",
    ],
)
def test_sanitize_refuses(uri: str) -> None:
    with pytest.raises(ValueError):
        sanitize_uri(urlsplit(uri))


def test_factory() -> None:
    asset = create_asset(namespace="Acme", kind="streams", name="orders", extra={"a": 1})
    assert asset.uri == "loams://acme/streams/orders"
    assert loams_uri("acme", "/objects/", "/lake/k") == "loams://acme/objects/lake/k"
    with pytest.raises(ValueError):
        loams_uri("", "streams", "x")
