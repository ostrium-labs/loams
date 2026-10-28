"""Stdlib-only checks of the Python worker's tables (no server, no SDK):
`python -m unittest discover -s tests` from workers/python."""

from __future__ import annotations

import ast
import json
import unittest
from pathlib import Path

from corpus_py.replay import workflow_id
from corpus_py.scenarios import NAMES

HERE = Path(__file__).resolve().parents[1]
CORPUS = HERE.parents[1]


class ScenarioTables(unittest.TestCase):
    def test_matches_scenarios_json_without_go_only(self) -> None:
        doc = json.loads((CORPUS / "scenarios.json").read_text())
        self.assertEqual(NAMES, [s["name"] for s in doc["scenarios"] if not s["go_only"]])

    def test_every_scenario_has_a_workflow(self) -> None:
        # Read the decorators from the source: importing workflows.py needs the SDK.
        tree = ast.parse((HERE / "corpus_py" / "workflows.py").read_text())
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                for d in node.decorator_list:
                    if isinstance(d, ast.Call) and getattr(d.func, "attr", "") == "defn":
                        for kw in d.keywords:
                            if kw.arg == "name" and isinstance(kw.value, ast.Constant):
                                names.add(kw.value.value)
        self.assertEqual(set(NAMES) - names, set())
        self.assertEqual(names - set(NAMES), {"child_greeter", "child_sleeper"})

    def test_replay_workflow_ids(self) -> None:
        self.assertEqual(workflow_id("hello.json"), "corpus-python-hello")
        self.assertEqual(workflow_id("continue_as_new_3.run2.json"), "corpus-python-continue_as_new_3")


if __name__ == "__main__":
    unittest.main()
