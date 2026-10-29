#!/usr/bin/env python3
"""NF2 Task 12: tests for scripts/nf1/check_pgdog_majors.py.

`test_pgdog_major_has_no_error_rows_in_used_components` runs the checker on the
committed inventories (conformance/router/pgdog-pg*.tsv) and the pin file. The
others run it on scratch trees. No podman: scripts/nf1/pgdog-majors.sh, which
writes the inventories, runs locally.

Run: python3 -I scripts/nf1/tests/test_pgdog_majors.py
"""

import importlib.util
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
spec = importlib.util.spec_from_file_location("check_pgdog_majors", HERE.parent / "check_pgdog_majors.py")
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)

HEAD = "component\tcheck\tresult\tnotes\n"


def tree(root: Path, pins: str, files: dict[str, str]) -> Path:
    (root / "deploy/pins").mkdir(parents=True)
    (root / "conformance/router").mkdir(parents=True)
    (root / "deploy/pins/loams-postgres-images.toml").write_text(pins)
    for name, body in files.items():
        (root / "conformance/router" / name).write_text(body)
    return root


PINS = "[majors]\nsupported = [17]\nbeta = [18]\nmaintenance = [16]\ndefault = 17\n"
PINS_18 = "[majors]\nsupported = [17, 18]\nbeta = []\nmaintenance = [16]\ndefault = 17\n"
OK = HEAD + "auth\tSCRAM\tok\tfine\nquery\tsimple\tok\t\n"
BAD = HEAD + "protocol\t3.2 startup\tdiffers\tfalls back to 3.0\n"


class PgdogMajors(unittest.TestCase):
    def run_on(self, pins, files):
        with tempfile.TemporaryDirectory() as d:
            return chk.check(tree(Path(d), pins, files))

    def test_pgdog_major_has_no_error_rows_in_used_components(self):
        failures, notices = chk.check(REPO)
        self.assertEqual(failures, [])
        for n in notices:
            print("notice:", n)
        # Every major's inventory is committed (16 since it became supported).
        for major in (16, 17, 18):
            self.assertTrue((REPO / f"conformance/router/pgdog-pg{major}.tsv").is_file())

    def test_supported_major_with_differs_row_fails(self):
        failures, _ = self.run_on(PINS_18, {"pgdog-pg17.tsv": OK, "pgdog-pg18.tsv": BAD})
        self.assertEqual(len(failures), 1)
        self.assertIn("major 18 is supported", failures[0])

    def test_beta_major_with_differs_row_is_a_notice(self):
        failures, notices = self.run_on(PINS, {"pgdog-pg17.tsv": OK, "pgdog-pg18.tsv": BAD})
        self.assertEqual(failures, [])
        self.assertEqual(len(notices), 1)
        self.assertIn("blocks its promotion", notices[0])

    def test_error_row_in_supported_major_fails(self):
        bad = HEAD + "cancel\tCancelRequest\terror\tdropped\n"
        failures, _ = self.run_on(PINS, {"pgdog-pg17.tsv": bad})
        self.assertTrue(any("error in cancel" in f for f in failures))

    def test_supported_major_without_inventory_fails(self):
        failures, _ = self.run_on(PINS, {"pgdog-pg18.tsv": OK})
        self.assertTrue(any("major 17 has no" in f for f in failures))

    def test_malformed_rows_fail(self):
        cases = {
            "bad header": "a\tb\tc\td\n",
            "field count": HEAD + "auth\tSCRAM\tok\n",
            "result": HEAD + "auth\tSCRAM\tpass\t\n",
            "component": HEAD + "sharding\tx\tok\t\n",
            "repeat": HEAD + "auth\tSCRAM\tok\t\nauth\tSCRAM\tok\t\n",
            "empty": HEAD,
        }
        for what, body in cases.items():
            with self.subTest(what):
                failures, _ = self.run_on(PINS, {"pgdog-pg17.tsv": body})
                self.assertTrue(failures, what)

    def test_clean_inventories_pass(self):
        failures, notices = self.run_on(PINS_18, {"pgdog-pg17.tsv": OK, "pgdog-pg18.tsv": OK})
        self.assertEqual((failures, notices), ([], []))


if __name__ == "__main__":
    unittest.main(verbosity=2)
