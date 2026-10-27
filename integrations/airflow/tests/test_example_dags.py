# SPDX-License-Identifier: Apache-2.0
"""Every example DAG imports and builds its tasks without touching a server."""

from __future__ import annotations

import importlib.util
import pathlib

import pytest

EXAMPLES = sorted((pathlib.Path(__file__).resolve().parents[1] / "example_dags").glob("example_*.py"))


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda p: p.stem)
def test_example_dag_imports(path: pathlib.Path) -> None:
    from airflow.sdk import DAG

    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    dags = [v for v in vars(module).values() if isinstance(v, DAG)]
    assert dags, f"{path.name} defines no DAG"
    for dag in dags:
        assert dag.dag_id.startswith("example_loams")
        assert dag.task_dict
        assert "loams" in dag.tags


def test_there_are_examples() -> None:
    assert len(EXAMPLES) >= 6
