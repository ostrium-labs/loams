#!/usr/bin/env python3
"""Regression checks for non-mutating contract drift and native platform guards."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import MagicMock, patch

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    if spec is None or spec.loader is None:
        raise ImportError(f"Cannot load migration helper {name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MigrationTests(unittest.TestCase):
    def test_mobile_graph_names_and_build_isolation(self):
        paths = {
            "mobile-contracts": "project.json",
            "mobile-fixtures": "conformance/project.json",
            "mobile-mock": "mock/project.json",
            "mobile-android": "android/project.json",
            "mobile-ios": "ios/project.json",
        }
        edges = {
            "mobile-contracts": {"mobile-fixtures"},
            "mobile-fixtures": set(),
            "mobile-mock": {"mobile-contracts", "mobile-fixtures"},
            "mobile-android": {"mobile-contracts", "mobile-fixtures", "mobile-mock"},
            "mobile-ios": {"mobile-contracts", "mobile-fixtures"},
        }
        for name, path in paths.items():
            project = json.loads((ROOT / path).read_text())
            self.assertEqual(project["name"], name)
            self.assertEqual(set(project.get("implicitDependencies", [])), edges[name])
            for target in project["targets"].values():
                self.assertTrue(target["options"]["cwd"].startswith("apps/mobile"))
            if "build" in project["targets"]:
                self.assertEqual(project["targets"]["build"]["dependsOn"], [])
        self.assertEqual(json.loads((ROOT / paths["mobile-fixtures"]).read_text())["targets"], {})

    def test_drift_is_non_mutating_and_detects_extra_files(self):
        drift = load("check-drift")
        (ROOT / ".cache").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / ".cache") as directory:
            local = Path(directory)
            (local / "proto").mkdir()
            (local / "proto/local.proto").write_text("placeholder")
            content_hash = hashlib.sha256(b"placeholder").hexdigest()
            tree_hash = hashlib.sha256(f"{content_hash}  proto/local.proto\n".encode()).hexdigest()
            (local / "conformance").mkdir()
            (local / "conformance/proto-ref.lock").write_text(f"tree_sha256={tree_hash}\n")
            (local / "scripts").mkdir()
            for name in ("generate.sh", "proto-hash.sh"):
                (local / "scripts" / name).write_text("unused")
            for path in drift.GENERATED:
                (local / path).mkdir(parents=True)
                (local / path / "client.txt").write_text("original")
            before = {path: drift.manifest(local / path) for path in drift.GENERATED}

            def generate(args, cwd, **kwargs):
                self.assertNotEqual(cwd, local)
                self.assertEqual((cwd / "proto/local.proto").read_text(), "placeholder")
                for path in drift.GENERATED:
                    shutil.copytree(local / path, cwd / path)

            with patch.object(drift, "ROOT", local), patch.object(drift.subprocess, "run", generate):
                with contextlib.redirect_stdout(io.StringIO()):
                    drift.main()
                self.assertEqual(before, {p: drift.manifest(local / p) for p in drift.GENERATED})
                def stale(args, cwd, **kwargs):
                    generate(args, cwd, **kwargs)
                    (cwd / drift.GENERATED[0] / "new.txt").write_text("new")
                with patch.object(drift.subprocess, "run", stale):
                    with self.assertRaisesRegex(SystemExit, "Generated code is stale"):
                        drift.main()
                self.assertEqual(before, {p: drift.manifest(local / p) for p in drift.GENERATED})

    def test_conformance_does_not_overwrite_cached_mock_binary(self):
        conformance = load("conformance")
        (ROOT / ".cache").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / ".cache") as directory:
            local = Path(directory)
            cached_binary = local / "mock/bin/loams-mock"
            cached_binary.parent.mkdir(parents=True)
            cached_binary.write_bytes(b"cached-build")
            reports = local / "android/conformance/build/test-results/test"
            reports.mkdir(parents=True)
            (reports / "TEST-suite.xml").write_text('<testsuite tests="1" skipped="0" failures="0" errors="0"/>')
            binaries = []

            def run(args, **kwargs):
                if args[0] == "go":
                    binary = Path(args[args.index("-o") + 1])
                    self.assertNotEqual(binary, cached_binary)
                    self.assertIn(local / ".cache", binary.parents)
                    self.assertIn("-buildvcs=false", args)
                    self.assertEqual(kwargs["env"]["CGO_ENABLED"], "0")
                    binary.write_bytes(b"isolated-mock")
                    binaries.append(binary)

            process = MagicMock()
            process.poll.return_value = None
            response = MagicMock()
            response.__enter__.return_value.status = 200
            with patch.object(conformance, "ROOT", local), \
                    patch.object(conformance.subprocess, "run", side_effect=run), \
                    patch.object(conformance.subprocess, "Popen", return_value=process), \
                    patch.object(conformance, "urlopen", return_value=response), \
                    patch.object(conformance.time, "sleep"):
                conformance.main()
            self.assertEqual(cached_binary.read_bytes(), b"cached-build")
            self.assertEqual(len(binaries), 1)
            self.assertFalse(binaries[0].exists())
            process.terminate.assert_called_once()
            process.wait.assert_called_once()

    def test_ios_requires_macos_before_running_tools(self):
        ios = load("ios")
        with patch.object(ios.sys, "platform", "linux"), patch.object(ios.subprocess, "run") as run:
            with self.assertRaisesRegex(SystemExit, "requires macOS"):
                ios.main()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
