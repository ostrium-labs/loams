# SPDX-License-Identifier: Apache-2.0
from __future__ import annotations

import importlib
import pathlib

import yaml

from airflow.providers.loams.get_provider_info import get_provider_info

ROOT = pathlib.Path(__file__).resolve().parents[1]


def test_provider_yaml_matches() -> None:
    assert yaml.safe_load((ROOT / "provider.yaml").read_text()) == get_provider_info()


def test_every_named_module_and_class_imports() -> None:
    info = get_provider_info()
    modules = [
        m
        for section in ("hooks", "operators", "sensors", "triggers")
        for entry in info[section]
        for m in entry["python-modules"]
    ]
    modules += info["filesystems"]
    for name in modules:
        importlib.import_module(name)
    for entry in info["asset-uris"]:
        for key in ("handler", "factory"):
            module, _, attr = entry[key].rpartition(".")
            assert callable(getattr(importlib.import_module(module), attr))
    for entry in info["connection-types"]:
        module, _, attr = entry["hook-class-name"].rpartition(".")
        hook = getattr(importlib.import_module(module), attr)
        assert hook.conn_type == entry["connection-type"]


def test_entry_point_and_version() -> None:
    import tomllib

    project = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]
    assert project["entry-points"]["apache_airflow_provider"]["provider_info"] == (
        "airflow.providers.loams.get_provider_info:get_provider_info"
    )
    assert project["version"] in get_provider_info()["versions"]
    from airflow.providers.loams import __version__

    assert __version__ == project["version"]
