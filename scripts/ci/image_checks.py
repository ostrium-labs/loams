#!/usr/bin/env python3
"""`cargo check` every release image's Rust build, with its exact feature set.

The release images are built only at release time (release-images.yml), so a
compile break in a feature set that no other CI job turns on (the 0.1.3
`loams-pg-control` break: `loams --features postgres,tikv`) reached a tag.
This script derives each image's `cargo build` invocations from the files the
release itself uses, so the CI check cannot drift from them:

- the image list is the `image:` matrix of .github/workflows/release-images.yml
  (both matrices there, and build.sh's default list, must agree);
- each image's builds are the `cargo build` commands in the `RUN` steps of
  deploy/release/Containerfile.<image>, with the `ARG` defaults substituted
  (`FEATURES`, `GATEWAY_FEATURES`, ...) and the `cd` before them honoured
  (loams-fabric builds in the `fabric/` workspace).

Each `cargo build --release ...` becomes `cargo check --locked ...` with the
same packages, binaries and features, run from the repository root or the
directory the Containerfile changes into.

Usage:
  python3 scripts/ci/image_checks.py --list         print the commands
  python3 scripts/ci/image_checks.py --json         the same, as JSON
  python3 scripts/ci/image_checks.py --run [IMAGE]  run them (all, or one image)
  python3 scripts/ci/image_checks.py --self-test    test the parser

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "release-images.yml")
BUILD_SH = os.path.join(ROOT, "deploy", "release", "build.sh")
CONTAINERFILES = os.path.join(ROOT, "deploy", "release")

# cargo build flags kept as they are; everything else is refused, so a new
# flag in a Containerfile is a deliberate change here too.
TAKES_VALUE = {"-p", "--package", "--bin", "--features", "-F", "--target", "--profile"}
FLAGS = {"--locked", "--release", "--all-features", "--no-default-features", "--frozen", "--offline"}


class ParseError(Exception):
    pass


def images_from_workflow(text: str) -> list[str]:
    """The `image: [..]` matrix lists; all of them must be the same list."""
    lists = re.findall(r"^\s*image:\s*\[([^\]]*)\]", text, re.M)
    if not lists:
        raise ParseError("no `image: [..]` matrix in release-images.yml")
    parsed = [[i.strip() for i in lst.split(",") if i.strip()] for lst in lists]
    if any(p != parsed[0] for p in parsed):
        raise ParseError(f"release-images.yml's image matrices differ: {parsed}")
    return parsed[0]


def images_from_build_sh(text: str) -> list[str]:
    m = re.search(r"\|\|\s*images=\(([^)]*)\)", text)
    if not m:
        raise ParseError("no default image list in build.sh")
    return m.group(1).split()


def logical_lines(text: str) -> list[str]:
    """Containerfile instructions with `\\` continuations joined."""
    out, cur = [], ""
    for raw in text.splitlines():
        line = raw.rstrip()
        if not cur and (not line.strip() or line.lstrip().startswith("#")):
            continue
        if cur and line.lstrip().startswith("#"):
            continue  # a comment inside a continued instruction
        if line.endswith("\\"):
            cur += line[:-1] + " "
            continue
        cur += line
        out.append(cur.strip())
        cur = ""
    if cur:
        out.append(cur.strip())
    return out


_VAR = re.compile(r"\$\{(\w+)(?::\+([^}]*))?\}|\$(\w+)")


def expand(s: str, args: dict[str, str]) -> str:
    """`$X`, `${X}` and `${X:+word}` from the ARG defaults. Unknown variables
    (runtime ones such as $CARGO_BUILD_JOBS) are left as they are."""

    def sub(m: re.Match) -> str:
        name = m.group(1) or m.group(3)
        if name not in args:
            return m.group(0)
        if m.group(2) is not None:
            return expand(m.group(2), args) if args[name] else ""
        return args[name]

    return _VAR.sub(sub, s)


def cargo_builds(containerfile: str) -> list[dict]:
    """Every `cargo build` in the build stage's RUN steps."""
    # ARGs before the first FROM are global; each stage starts from them.
    glob: dict[str, str] = {}
    args: dict[str, str] = glob
    builds = []
    for line in logical_lines(containerfile):
        word, _, rest = line.partition(" ")
        word = word.upper()
        if word == "FROM":
            args = dict(glob)
            continue
        if word == "ARG":
            name, eq, default = rest.strip().partition("=")
            args[name.strip()] = default.strip().strip('"') if eq else args.get(name.strip(), "")
            continue
        if word != "RUN":
            continue
        # Drop `--mount=...` options, then walk the `&&` chain.
        body = re.sub(r"^(\s*--\S+\s+)+", "", rest)
        cwd = "."
        for step in re.split(r"\s*&&\s*", body):
            step = step.strip()
            m = re.match(r"cd\s+(\S+)$", step)
            if m:
                cwd = os.path.normpath(os.path.join(cwd, m.group(1)))
                continue
            if not re.match(r"cargo\s+build\b", step):
                continue
            words = shlex.split(expand(step, args))[2:]
            builds.append({"dir": cwd, "args": check_args(words)})
    return builds


def check_args(words: list[str]) -> list[str]:
    out = ["--locked"]
    i = 0
    while i < len(words):
        w = words[i]
        if "=" in w and w.split("=", 1)[0] in TAKES_VALUE:
            w, v = w.split("=", 1)
            words[i : i + 1] = [w, v]
        if w in TAKES_VALUE:
            if i + 1 >= len(words):
                raise ParseError(f"{w} without a value")
            if words[i + 1]:
                out += [w, words[i + 1]]
            i += 2
            continue
        if w in FLAGS:
            if w not in ("--locked", "--release"):
                out.append(w)
            i += 1
            continue
        raise ParseError(f"unexpected cargo build argument {w!r}; teach scripts/ci/image_checks.py about it")
    if not any(a in ("-p", "--package") for a in out):
        raise ParseError(f"cargo build without -p: {words}")
    return out


def plan() -> list[dict]:
    with open(WORKFLOW) as fh:
        images = images_from_workflow(fh.read())
    with open(BUILD_SH) as fh:
        default = images_from_build_sh(fh.read())
    if sorted(default) != sorted(images):
        raise ParseError(f"build.sh's default images {default} differ from release-images.yml's {images}")
    out = []
    for image in images:
        path = os.path.join(CONTAINERFILES, f"Containerfile.{image}")
        with open(path) as fh:
            builds = cargo_builds(fh.read())
        if not builds:
            raise ParseError(f"{os.path.relpath(path, ROOT)} has no cargo build")
        for b in builds:
            out.append({"image": image, **b})
    return out


def command(c: dict) -> str:
    cd = "" if c["dir"] == "." else f"(cd {c['dir']} && "
    end = "" if c["dir"] == "." else ")"
    return f"{cd}cargo check {shlex.join(c['args'])}{end}"


def run(checks: list[dict]) -> int:
    failed = []
    for c in checks:
        print(f"::group::{c['image']}: {command(c)}", flush=True)
        t0 = time.monotonic()
        rc = subprocess.call(["cargo", "check", *c["args"]], cwd=os.path.join(ROOT, c["dir"]))
        print("::endgroup::", flush=True)
        print(f"{c['image']}: {command(c)} -> rc {rc} in {time.monotonic() - t0:.0f} s", flush=True)
        if rc:
            failed.append(c)
    for c in failed:
        print(f"::error::release image {c['image']} does not compile: {command(c)}", flush=True)
    return 1 if failed else 0


def self_test() -> int:
    cf = """\
# a comment
FROM rust AS build
ARG FEATURES=a,b
ARG EMPTY=
ARG GW=spark,temporal
RUN --mount=type=cache,target=/x \\
    --mount=type=cache,target=/y \\
    if [ -n "$CARGO_BUILD_JOBS" ]; then export CARGO_BUILD_JOBS; fi \\
 && cargo build --locked --release -p loams --bin loams --features "$FEATURES" \\
 # a comment line inside
 && cd fabric \\
 && cargo build --locked --release -p f --bin f ${GW:+--features "$GW"} \\
 && cargo build --locked --release -p g ${EMPTY:+--features "$EMPTY"} \\
 && install -D target/release/f /out/f
FROM debian
ARG FEATURES
RUN apt-get install -y cargo-is-not-built-here
"""
    got = cargo_builds(cf)
    want = [
        {"dir": ".", "args": ["--locked", "-p", "loams", "--bin", "loams", "--features", "a,b"]},
        {"dir": "fabric", "args": ["--locked", "-p", "f", "--bin", "f", "--features", "spark,temporal"]},
        {"dir": "fabric", "args": ["--locked", "-p", "g"]},
    ]
    assert got == want, got
    assert images_from_workflow("    image: [a, b]\n  x:\n    image: [a, b]\n") == ["a", "b"]
    for bad in ("    image: [a, b]\n    image: [a]\n",):
        try:
            images_from_workflow(bad)
        except ParseError:
            pass
        else:
            raise AssertionError("differing matrices accepted")
    try:
        cargo_builds("FROM x\nRUN cargo build --locked -p a --example e\n")
    except ParseError:
        pass
    else:
        raise AssertionError("unknown flag accepted")
    assert images_from_build_sh('((${#images[@]})) || images=(a b c)\n') == ["a", "b", "c"]
    # The real files parse, and the 0.1.3 regression case is in the plan.
    checks = plan()
    cmds = [command(c) for c in checks]
    assert any("-p loams --bin loams --features postgres,tikv" in c for c in cmds), cmds
    print(f"self-test ok ({len(checks)} checks from the release files)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--list", action="store_true")
    g.add_argument("--json", action="store_true")
    g.add_argument("--run", nargs="*", metavar="IMAGE")
    g.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    try:
        if a.self_test:
            return self_test()
        checks = plan()
    except ParseError as e:
        print(f"image_checks: {e}", file=sys.stderr)
        return 2
    if a.list:
        for c in checks:
            print(f"{c['image']}: {command(c)}")
        return 0
    if a.json:
        print(json.dumps(checks))
        return 0
    if a.run:
        unknown = set(a.run) - {c["image"] for c in checks}
        if unknown:
            print(f"image_checks: unknown image(s) {sorted(unknown)}", file=sys.stderr)
            return 2
        checks = [c for c in checks if c["image"] in a.run]
    return run(checks)


if __name__ == "__main__":
    sys.exit(main())
