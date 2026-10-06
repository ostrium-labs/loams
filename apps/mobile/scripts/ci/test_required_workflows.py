import shutil
import tempfile
import unittest
from pathlib import Path

from required_workflows import matching_glob, matches_pull_request, selected_workflows, runs_complete


class RequiredWorkflowsTests(unittest.TestCase):
    def test_globs_follow_github_path_boundaries(self):
        self.assertTrue(matching_glob("android/**", "android/core/src/Foo.kt"))
        self.assertFalse(matching_glob("android/*", "android/core/src/Foo.kt"))
        self.assertTrue(matching_glob("**/*.md", "docs/setup/readme.md"))

    def test_mobile_paths_select_only_applicable_workflows(self):
        mobile = Path(__file__).resolve().parents[2]
        cache = mobile / ".cache"
        cache.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=cache) as directory:
            root = Path(directory)
            shutil.copytree(mobile / ".github/workflows", root / ".github/workflows")
            active = {str(p.relative_to(root)) for p in (root / ".github/workflows").glob("*.yml")}
            active.discard(".github/workflows/mobile-governance-ci.yml")
            self.assertEqual(
                selected_workflows(root, "pull_request", "dev", ["CONTRIBUTING.md"], active),
                {".github/workflows/mobile-dco.yml"},
            )
            changed = ["apps/mobile/android/core/src/Foo.kt"]
            selected = selected_workflows(root, "pull_request", "dev", changed, active)
            self.assertIn(".github/workflows/mobile-android.yml", selected)
            self.assertIn(".github/workflows/mobile-conformance.yml", selected)
            self.assertNotIn(".github/workflows/mobile-ios.yml", selected)
            push = selected_workflows(root, "push", "dev", changed, active)
            self.assertIn(".github/workflows/mobile-android.yml", push)
            self.assertIn(".github/workflows/mobile-conformance.yml", push)
            unrelated = selected_workflows(root, "pull_request", "dev", ["proto/core.proto"], active)
            self.assertEqual(unrelated, {".github/workflows/mobile-dco.yml"})

    def test_incomplete_or_failed_expected_run_cannot_pass(self):
        expected = {".github/workflows/android.yml", ".github/workflows/dco.yml"}
        android = {"path": ".github/workflows/android.yml@refs/heads/dev", "status": "completed", "conclusion": "success"}
        dco = {"path": ".github/workflows/dco.yml", "status": "in_progress", "conclusion": None}
        self.assertEqual(runs_complete(expected, [android]), (False, None))
        self.assertEqual(runs_complete(expected, [android, dco]), (False, None))
        dco["status"] = "completed"
        dco["conclusion"] = "failure"
        self.assertEqual(runs_complete(expected, [android, dco]), (False, ".github/workflows/dco.yml: failure"))
        dco["conclusion"] = "success"
        self.assertEqual(runs_complete(expected, [android, dco]), (True, None))

    def test_fork_runs_without_pull_request_entries_match_the_head_repository(self):
        payload = {"number": 7, "pull_request": {"base": {"sha": "base"}, "head": {"ref": "fix", "repo": {"full_name": "contributor/loams-mobile"}}}}
        run = {"pull_requests": [], "head_branch": "fix", "head_repository": {"full_name": "contributor/loams-mobile"}}
        self.assertTrue(matches_pull_request(run, payload))
        run["head_repository"]["full_name"] = "someone-else/loams-mobile"
        self.assertFalse(matches_pull_request(run, payload))
        run["pull_requests"] = [{"number": 7, "base": {"sha": "base"}}]
        self.assertTrue(matches_pull_request(run, payload))
        run["pull_requests"][0]["base"]["sha"] = "old-base"
        self.assertFalse(matches_pull_request(run, payload))


if __name__ == "__main__":
    unittest.main()
