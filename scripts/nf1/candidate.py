#!/usr/bin/env python3
"""NF1 Task 16: a Loams Postgres release candidate's pins, for G3.

    candidate.py [--root DIR] apply --doc FILE --fork-commit SHA [--env FILE] [--own-majors 18]
    candidate.py [--root DIR] image --major N

ostrium-labs/loams-postgres' gate (loams-gate.yml, G3) dispatches
nf1-candidate.yml here with the candidate's pin document: release-notes.py
pin-doc's output for the images it has just built and not yet tagged.
`apply` checks it and writes it over this checkout's pin file and every
consumer (`pins.py set --from-release FILE`), so the jobs that follow run on
the candidate's images. Nothing is committed: the change lives in the job's
checkout.

The document is refused unless:
- pins.py's own checks pass (validate());
- every image is either built from the candidate's commit (source
  `ostrium-labs/loams-postgres@<fork commit>`, a ghcr.io/ostrium-labs/loams-postgres-*
  repository), or exactly the entry Loams pins today (a partial release keeps
  the other images). So a candidate can never bring in a mirror, Neon's own
  images, or another repository.

With --env, the variables the jobs need are appended to FILE ($GITHUB_ENV):
PG_HEADERS_IMAGE (the storage image, which pg2.yml and pg2-e2e.yml read the
server headers from). The candidate's majors (supported, maintenance and beta)
are printed as `majors=<JSON list>` for $GITHUB_OUTPUT, split into
`required=` and `advisory=` (NF2 Task 7, design §62 §8, D1118: G3 runs
pg2-e2e.yml once per major; a beta major's legs are advisory for the other
majors' releases and required for its own, which --own-majors names), with
`default=`, the pin file's default major.

`image` prints the compute image (`repository@digest`) of Postgres N from this
checkout's pin file, refusing a major the pin file does not list (supported,
maintenance or beta); scripts/pg2/it-pageserver-loams-wal.sh and pg2-e2e.yml's
Kubernetes job use it.

Exit codes: 0 applied, 1 refused, 2 usage. Stdlib only.
"""

import argparse
import importlib.util
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("pins", HERE / "pins.py")
pins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pins)

COMMIT = re.compile(r"[0-9a-f]{40}")


class Refused(Exception):
    pass


def check(doc, current, fork_commit, root):
    """Every reason `doc` is not a candidate of `fork_commit`, as messages."""
    errs = pins.validate(doc, root)
    if errs:
        return errs
    ours = f"ostrium-labs/loams-postgres@{fork_commit}"
    pinned = (current or {}).get("images", {})
    for key, img in doc["images"].items():
        if img.get("source") == ours and img["repository"].startswith(pins.FORK_REPOSITORY_PREFIX):
            continue
        if pinned.get(key) == img:
            continue
        errs.append(f"images.{key}: neither built from {ours} nor the image Loams pins today")
    return errs


def majors(doc):
    m = doc["majors"]
    return sorted(set(m["supported"]) | set(m["maintenance"]) | set(m["beta"]))


def legs(doc, own=()):
    """The candidate's majors as G3's legs: required, advisory (beta, not own) and the default."""
    all_ = majors(doc)
    own = {int(o) for o in own}
    missing = sorted(own - set(all_))
    if missing:
        raise Refused(f"own major {', '.join(map(str, missing))} is not in the document's majors")
    beta = set(doc["majors"]["beta"]) - own
    return {"majors": all_, "required": [n for n in all_ if n not in beta],
            "advisory": [n for n in all_ if n in beta], "default": doc["majors"]["default"]}


def image(doc, major):
    """`repository@digest` of Postgres `major`'s compute image in pin document `doc`."""
    if major not in majors(doc):
        raise Refused(f"Postgres {major} is not in the pin file's majors {majors(doc)}")
    img = doc.get("images", {}).get(f"compute-v{major}")
    if not img:
        raise Refused(f"the pin file has no [images.compute-v{major}]")
    return f"{img['repository']}@{img['digest']}"


def apply(root, doc_path, fork_commit, env_path=None):
    if not COMMIT.fullmatch(fork_commit):
        raise Refused(f"--fork-commit must be 40 hex characters, not {fork_commit!r}")
    try:
        text = Path(doc_path).read_text(encoding="utf-8")
    except OSError as e:
        raise Refused(f"{doc_path}: {e}") from None
    try:
        doc = pins.load_toml(text, doc_path)
        current = pins.load_toml(pins.read(root, pins.PIN_FILE), pins.PIN_FILE)
    except pins.Refused as e:
        raise Refused(str(e)) from None
    errs = check(doc, current, fork_commit, root)
    if errs:
        raise Refused("; ".join(errs))
    if pins.cmd_set(root, str(Path(doc_path).resolve())) != 0:
        raise Refused("pins.py set refused the document")
    storage = doc["images"]["storage"]
    if env_path:
        with open(env_path, "a", encoding="utf-8") as f:
            f.write(f"PG_HEADERS_IMAGE={storage['repository']}@{storage['digest']}\n")
    return majors(doc)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", default=str(HERE.parents[1]), help="repository root")
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("apply")
    a.add_argument("--doc", required=True)
    a.add_argument("--fork-commit", required=True)
    a.add_argument("--env")
    a.add_argument("--own-majors", default="", help="comma-separated majors whose own images this candidate gates")
    i = sub.add_parser("image")
    i.add_argument("--major", required=True, type=int)
    args = ap.parse_args(argv)
    try:
        if args.cmd == "image":
            doc = pins.load_toml(pins.read(args.root, pins.PIN_FILE), pins.PIN_FILE)
            print(image(doc, args.major))
            return 0
        own = [o.strip() for o in args.own_majors.split(",") if o.strip()]
        if not all(o.isdigit() for o in own):
            print(f"candidate.py: --own-majors must be comma-separated numbers, not {args.own_majors!r}", file=sys.stderr)
            return 2
        apply(args.root, args.doc, args.fork_commit, args.env)
        p = legs(pins.load_toml(Path(args.doc).read_text(encoding="utf-8"), args.doc), own)
    except (Refused, pins.Refused) as e:
        print(f"candidate.py: refused: {e}", file=sys.stderr)
        return 1
    for k in ("majors", "required", "advisory", "default"):
        print(f"{k}={json.dumps(p[k])}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
