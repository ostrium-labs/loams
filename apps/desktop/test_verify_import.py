"""Regression checks for verification without any sibling source repository."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import verify_import


class ImportVerificationTests(unittest.TestCase):
    def test_default_checks_only_read_desktop_files_without_git(self):
        original_open = Path.open

        def desktop_only_open(path, *args, **kwargs):
            self.assertTrue(
                path.resolve().is_relative_to(verify_import.ROOT),
                f"Default verification tried to read outside the desktop import: {path}",
            )
            return original_open(path, *args, **kwargs)

        output = io.StringIO()
        with tempfile.TemporaryDirectory() as cwd:
            with contextlib.chdir(cwd), mock.patch.object(sys, "argv", ["verify_import.py"]):
                with mock.patch.object(Path, "open", desktop_only_open):
                    with mock.patch.object(verify_import.subprocess, "check_output") as git:
                        with contextlib.redirect_stdout(output):
                            verify_import.main()
                        git.assert_not_called()
        self.assertIn("no sibling source checkout required", output.getvalue())

    def test_explicit_missing_source_fails_clearly_without_git(self):
        with tempfile.TemporaryDirectory() as cwd:
            missing = str(Path(cwd) / "missing-source")
            with mock.patch.object(sys, "argv", ["verify_import.py", "--source", missing]):
                with mock.patch.object(verify_import.subprocess, "check_output") as git:
                    with self.assertRaisesRegex(SystemExit, "Optional source checkout does not exist"):
                        verify_import.main()
                    git.assert_not_called()


if __name__ == "__main__":
    unittest.main()
