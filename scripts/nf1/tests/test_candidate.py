#!/usr/bin/env python3
"""NF1 Task 16: tests for scripts/nf1/candidate.py (G3's candidate pins).

Each test copies the pin file and its consumers into a scratch tree and
applies a candidate there; the repository is only read.

Run: python3 -I scripts/nf1/tests/test_candidate.py
"""

import contextlib
import importlib.util
import io
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
NF1 = HERE.parent
REPO = NF1.parent.parent
spec = importlib.util.spec_from_file_location("candidate", NF1 / "candidate.py")
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)
pins = candidate.pins

COMMIT = "c" * 40
NEW = "sha256:" + "d" * 64


def toml(doc):
    """A pin document in pins.py's own layout."""
    return pins.render_pins(doc)


class Candidate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / "repo"
        files = [pins.PIN_FILE, pins.README, pins.RELEASE_COMPOSE, *pins.COMPOSE, *pins.WORKFLOWS]
        for f in files:
            dst = self.root / f
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(REPO / f, dst)
        shutil.copytree(REPO / pins.AUDIT_DIR, self.root / pins.AUDIT_DIR)
        self.current = tomllib.loads((REPO / pins.PIN_FILE).read_text())

    def tearDown(self):
        self.tmp.cleanup()

    def built(self, keys=("storage", "compute-v16", "compute-v17")):
        """The current pins with `keys` rebuilt from COMMIT."""
        doc = tomllib.loads((REPO / pins.PIN_FILE).read_text())
        doc["fork_release"] = "lp-2026.11.0"
        doc["fork_commit"] = COMMIT
        for k in keys:
            doc["images"][k]["digest"] = NEW
            doc["images"][k]["source"] = f"ostrium-labs/loams-postgres@{COMMIT}"
        return doc

    def apply(self, doc, fork_commit=COMMIT):
        path = Path(self.tmp.name) / "doc.toml"
        path.write_text(toml(doc))
        env = Path(self.tmp.name) / "env"
        with contextlib.redirect_stdout(io.StringIO()):
            majors = candidate.apply(str(self.root), str(path), fork_commit, str(env))
        return majors, env.read_text()

    def test_candidate_rewrites_the_pins_and_the_headers_image(self):
        majors, env = self.apply(self.built())
        self.assertEqual(majors, [16, 17])
        storage = self.current["images"]["storage"]["repository"]
        self.assertEqual(env, f"PG_HEADERS_IMAGE={storage}@{NEW}\n")
        compose = (self.root / pins.COMPOSE[0]).read_text()
        self.assertIn(f"loams-postgres-compute-v17@{NEW}", compose)
        self.assertIn(f"loams-postgres-storage@{NEW}", compose)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(pins.cmd_check(str(self.root)), 0)

    def test_a_partial_release_keeps_todays_other_pins(self):
        self.apply(self.built(keys=("storage",)))

    def test_the_current_pins_are_a_candidate_of_any_commit(self):
        # g3_reports_status: the dry run sends today's document.
        self.apply(tomllib.loads((REPO / pins.PIN_FILE).read_text()))

    def test_an_image_from_another_commit_is_refused(self):
        doc = self.built()
        doc["images"]["compute-v17"]["source"] = "ostrium-labs/loams-postgres@" + "e" * 40
        with self.assertRaisesRegex(candidate.Refused, "compute-v17: neither built from"):
            self.apply(doc)

    def test_another_repository_is_refused(self):
        doc = self.built()
        doc["images"]["compute-v17"]["repository"] = "ghcr.io/example/compute"
        with self.assertRaisesRegex(candidate.Refused, "compute-v17"):
            self.apply(doc)

    def test_a_mirror_is_refused(self):
        doc = self.built()
        doc["images"]["storage"]["repository"] = "ghcr.io/neondatabase/neon"
        doc["images"]["storage"]["source"] = f"mirror:ghcr.io/neondatabase/neon@{NEW}"
        with self.assertRaisesRegex(candidate.Refused, "storage"):
            self.apply(doc)

    def test_the_neon_archive_is_refused(self):
        doc = self.built()
        doc["images"]["storage"]["repository"] = "ghcr.io/ostrium-labs/neon-archive/neon"
        with self.assertRaisesRegex(candidate.Refused, "never deployed"):
            self.apply(doc)

    def test_an_invalid_document_is_refused_and_nothing_is_written(self):
        doc = self.built()
        doc["images"]["storage"]["digest"] = "sha256:short"
        before = (self.root / pins.COMPOSE[0]).read_text()
        with self.assertRaisesRegex(candidate.Refused, "digest"):
            self.apply(doc)
        self.assertEqual((self.root / pins.COMPOSE[0]).read_text(), before)

    def test_a_bad_fork_commit_is_refused(self):
        with self.assertRaisesRegex(candidate.Refused, "40 hex"):
            self.apply(self.built(), fork_commit="main")

    def test_not_toml_is_refused(self):
        path = Path(self.tmp.name) / "doc.toml"
        path.write_text("schema = [")
        with self.assertRaises(candidate.Refused):
            candidate.apply(str(self.root), str(path), COMMIT)


class PerMajor(unittest.TestCase):
    """NF2 Task 7 (design §62 §8, D1118): G3 runs pg2-e2e.yml once per candidate major."""

    def doc(self, beta=(18,)):
        d = tomllib.loads((REPO / pins.PIN_FILE).read_text())
        d["majors"]["beta"] = list(beta)
        return d

    def test_g3_runs_e2e_per_major(self):
        # The legs come from the candidate's majors: beta advisory unless own.
        legs = candidate.legs(self.doc())
        self.assertEqual(legs, {"majors": [16, 17, 18], "required": [16, 17], "advisory": [18], "default": 17})
        own = candidate.legs(self.doc(), own=[18])
        self.assertEqual((own["required"], own["advisory"]), ([16, 17, 18], []))
        today = candidate.legs(tomllib.loads((REPO / pins.PIN_FILE).read_text()))
        self.assertEqual(today["majors"], sorted(today["required"] + today["advisory"]))
        with self.assertRaisesRegex(candidate.Refused, "own major 19"):
            candidate.legs(self.doc(), own=[19])

        # nf1-candidate.yml: one pg2-e2e.yml call per major, from prepare's
        # outputs (no hard-coded major), beta legs in their own job.
        wf = (REPO / ".github/workflows/nf1-candidate.yml").read_text()
        for job, outputs in (("e2e", "required"), ("e2e-beta", "advisory")):
            block = wf.split(f"\n  {job}:\n", 1)[1].split("\n\n  ", 1)[0]
            self.assertIn("uses: ./.github/workflows/pg2-e2e.yml", block, job)
            self.assertIn(f"major: ${{{{ fromJSON(needs.prepare.outputs.{outputs}) }}}}", block, job)
            self.assertIn("major: ${{ format('{0}', matrix.major) }}", block, job)
        report = wf.split("\n  report:\n", 1)[1]
        self.assertIn("e2e-beta", report)

        # pg2-e2e.yml reads compute-v<N> from its `major` input, never compute-v17.
        e2e = (REPO / ".github/workflows/pg2-e2e.yml").read_text()
        self.assertTrue("      major:\n" in e2e, "pg2-e2e.yml has no `major` input")
        kind = e2e.split("\n  pg-control-kind:\n", 1)[1].split("\n\n  ", 1)[0]
        self.assertNotIn("compute-v17", kind)
        self.assertIn("scripts/nf1/candidate.py image", kind)
        self.assertIn("IT_KIND_IMAGE", kind)
        ingest = e2e.split("\n  pageserver-loams-wal:\n", 1)[1].split("\n\n  ", 1)[0]
        self.assertIn("PG_MAJOR: ${{ inputs.major }}", ingest)

    def test_image_of_a_pinned_major(self):
        doc = self.doc(beta=())
        c = doc["images"]["compute-v16"]
        self.assertEqual(candidate.image(doc, 16), f"{c['repository']}@{c['digest']}")
        with self.assertRaisesRegex(candidate.Refused, "Postgres 15 is not in the pin file"):
            candidate.image(doc, 15)

    def test_it_pageserver_loams_wal_takes_any_pinned_major(self):
        script = REPO / "scripts/pg2/it-pageserver-loams-wal.sh"
        text = script.read_text()
        self.assertNotRegex(text, r"(?m)^PG_MAJOR=17$")
        env = {"PATH": "/usr/bin:/bin", "HOME": tempfile.gettempdir()}
        def run(*args, **extra):
            return subprocess.run(["bash", str(script), *args], env={**env, **extra},
                                  capture_output=True, text=True, timeout=60)
        r = run(PG_MAJOR="15")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("Postgres 15 is not in the pin file", r.stderr)
        r = run("16")
        self.assertIn("LOAMS_WAL", r.stderr)  # 16 passed the check; the next one stops it
        r = run(PG_MAJOR="16", COMPUTE_IMAGE="ghcr.io/ostrium-labs/loams-postgres-compute-v17@" + NEW)
        self.assertEqual(r.returncode, 1)
        self.assertIn("not a Postgres 16 compute image", r.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=1)
