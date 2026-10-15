#!/usr/bin/env python3
"""gate_in_ci_required (UP1 Task 20, design §54 §14, Q826): the job
`upgrade-gate` ("Upgrade: previous release → candidate") exists in ci.yml, is
in `CI required`'s needs, runs on the full tier, the nightly and the
`ci:upgrade` label, and runs scripts/release/upgrade-gate.sh. Text checks
only, so the repository-checks job needs no YAML library."""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]


def job(text, name):
    """The lines of top-level job `name` (two-space indented key)."""
    m = re.search(rf"^  {re.escape(name)}:\n((?:(?:    .*)?\n)*)", text, re.M)
    return m.group(1) if m else None


def problems(text):
    gate = job(text, "upgrade-gate")
    if gate is None:
        return ["ci.yml has no upgrade-gate job"]
    out = []
    if 'name: "Upgrade: previous release → candidate"' not in gate:
        out.append("upgrade-gate is not named 'Upgrade: previous release → candidate'")
    required = job(text, "required") or ""
    if not re.search(r"^      - upgrade-gate$", required, re.M):
        out.append("upgrade-gate is not in CI required's needs")
    cond = next((l for l in gate.splitlines() if l.strip().startswith("if:")), "")
    for needle in ("needs.changes.result == 'skipped'", "outputs.full == 'true'", "'ci:upgrade'"):
        if needle not in cond:
            out.append(f"upgrade-gate's if lacks {needle}")
    if "scripts/release/upgrade-gate.sh" not in gate:
        out.append("upgrade-gate does not run scripts/release/upgrade-gate.sh")
    return out


def main():
    text = (ROOT / ".github/workflows/ci.yml").read_text()
    if sys.argv[1:] == ["--self-test"]:
        assert problems(text) == [], problems(text)
        planted = re.sub(r"^      - upgrade-gate\n", "", text, flags=re.M)
        assert problems(planted) == ["upgrade-gate is not in CI required's needs"], problems(planted)
        print("check-upgrade-gate: self-test passed")
        return 0
    found = problems(text)
    for p in found:
        print(f"check-upgrade-gate: {p}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
