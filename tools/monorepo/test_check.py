import tempfile
import unittest
from pathlib import Path

from check import project_errors


class ProjectChecks(unittest.TestCase):
    def test_duplicate_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            errors = project_errors(Path(directory), {"a": {"name": "same"}, "b": {"name": "same"}})
        self.assertTrue(any("Duplicate" in error for error in errors))

    def test_commands_cannot_depend_on_sibling_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            projects = {"app": {"name": "app", "targets": {"build": {"options": {"cwd": "../source"}}}}}
            errors = project_errors(Path(directory), projects)
        self.assertTrue(any("invalid cwd" in error for error in errors))

    def test_unknown_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            errors = project_errors(Path(directory), {"a": {"name": "app", "implicitDependencies": ["missing"]}})
        self.assertTrue(any("unknown project" in error for error in errors))

    def test_valid_local_commands_and_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "app").mkdir()
            projects = {"app": {"name": "app", "implicitDependencies": ["contracts"],
                                "targets": {"test": {"options": {"cwd": "app"}}}},
                        "contracts": {"name": "contracts"}}
            self.assertEqual([], project_errors(root, projects))


if __name__ == "__main__":
    unittest.main()
