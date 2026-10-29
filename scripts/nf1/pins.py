#!/usr/bin/env python3
"""The Loams Postgres pin file and every consumer generated from it (NF1 Task 2).

    pins.py [--root DIR] check
    pins.py [--root DIR] set [--from-release URL|PATH]
                             [--supported N,..] [--beta N,..] [--maintenance N,..] [--default N]
    pins.py [--root DIR] rust

`deploy/pins/loams-postgres-images.toml` names each image Loams runs by
digest. The consumers repeat those references and are generated from it:

- `x-neon-image` (images.storage) and `x-compute-image` (images.compute-v<default>)
  in deploy/loams-postgres-dev/compose.yaml and deploy/loams-pg-bench/compose.yaml,
  each still overridable by NEON_IMAGE / COMPUTE_IMAGE (PG2 R2.1; names kept, D823);
- the `image: ${NEON_IMAGE:-...}` default (images.storage) of the release stack's
  services outside the included stack, in deploy/release/compose.yaml (NF1 Task 14);
- `PG_HEADERS_IMAGE` (images.storage) in pg2.yml, pg2-e2e.yml and loams-pg-bench.yml;
- the image table between the `pins.py` markers in deploy/loams-postgres-dev/README.md;
- one `compute-v<N>` service per major in [majors] (supported, beta and maintenance),
  between the `pins.py` markers in deploy/loams-postgres-dev/compose.yaml (design 62
  §9.4; profiles `pg<N>`, and `beta` for a beta major), and pg-bootstrap's default
  major (majors.default); deploy/release/compose.yaml includes that stack, so the
  release stack has the same services;
- crates/loams-postgres/src/majors.rs (design 62 §9.1, NF2 Task 10): the majors, each
  one's pinned minor (images.storage.pg_minors) and upstream end of life.

Upstream's end-of-life dates are not release facts, so they are not in the pin file
(its schema stays as built, D1116): they are UPSTREAM_EOL below, from postgresql.org's
versions.json, and a major in [majors] without one is refused.

check  fails (exit 1) unless the pin file is valid and in pins.py's own layout,
       every consumer equals it, every `mirror:` source has its audit file
       (deploy/pins/audit/sha256-<hex>.json), and nothing outside the reference
       places names a ghcr.io/ostrium-labs/neon-archive image (those packages
       are private and never deployed; NF1 Task 1, controller ruling).
set    validates the release's pin document (a TOML file in the pin file's
       schema: a local path or an https URL, the asset Task 13 attaches to a
       release), then rewrites the pin file and every consumer. Without
       --from-release it re-renders them from the current pin file. The
       --supported, --beta, --maintenance and --default flags replace those
       [majors] entries first (a comma-separated list; "" for none), the only
       way to move a major between states. A refused document writes nothing.
rust   writes crates/loams-postgres/src/majors.rs alone (`set` writes it too).

Exit codes: 0 ok, 1 check or validation failure, 2 usage. Stdlib only.
"""

import argparse
import re
import subprocess
import sys
import tomllib
import urllib.request
from pathlib import Path

PIN_FILE = "deploy/pins/loams-postgres-images.toml"
AUDIT_DIR = "deploy/pins/audit"
README = "deploy/loams-postgres-dev/README.md"
MAJORS_RS = "crates/loams-postgres/src/majors.rs"
COMPOSE = ("deploy/loams-postgres-dev/compose.yaml", "deploy/loams-pg-bench/compose.yaml")
RELEASE_COMPOSE = "deploy/release/compose.yaml"
WORKFLOWS = (".github/workflows/pg2.yml", ".github/workflows/pg2-e2e.yml", ".github/workflows/loams-pg-bench.yml")

HEADER = "# Written by scripts/nf1/pins.py (set); checked by pins.py (check). Do not edit by hand."
README_BEGIN = "<!-- pins.py: begin images (generated from deploy/pins/loams-postgres-images.toml) -->"
README_END = "<!-- pins.py: end images -->"
COMPUTES_BEGIN = "  # pins.py: begin computes (generated from deploy/pins/loams-postgres-images.toml)"
COMPUTES_END = "  # pins.py: end computes"

# Upstream's end of life per major (postgresql.org versions.json, `eolDate`;
# checked 2026-10-18, design 62 §2). Not in the pin file: a release does not
# change them. A major enters [majors] only with its date here.
UPSTREAM_EOL = {
    14: "2026-11-12",
    15: "2027-11-11",
    16: "2028-11-09",
    17: "2029-11-08",
    18: "2030-11-14",
}

# The neon-archive packages hold Neon's binaries with CDDL `inferno` and Neon's
# private `subzero-core`: private for good, never deployed. Reference use only:
# the design and plan history, the Task 1 scripts, the audits, a pin's `mirror:`
# source (and the README's generated table, which shows it) and the README's
# Mirror section.
ARCHIVE = "ostrium-labs/neon-archive"
ARCHIVE_REFERENCE_DIRS = ("docs/", "scripts/nf1/", AUDIT_DIR + "/")

DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")
RELEASE = re.compile(r"lp-\d{4}\.\d{2}\.\d+|mirror-[0-9a-f]{8}")
REPOSITORY = re.compile(r"[a-z0-9.-]+(?:/[a-z0-9._-]+)+")
IMAGE_KEY = re.compile(r"[a-z][a-z0-9-]*")  # storage, compute-v<N>, later build-tools
FORK_SOURCE = re.compile(r"ostrium-labs/loams-postgres@[0-9a-f]{40}")
FORK_REPOSITORY_PREFIX = "ghcr.io/ostrium-labs/loams-postgres-"
MIRROR_SOURCE = re.compile(
    r"mirror:(?P<repo>(?:ghcr\.io/neondatabase|ghcr\.io/ostrium-labs/neon-archive)/[a-z0-9._/-]+)@(?P<digest>sha256:[0-9a-f]{64})")
MINOR = re.compile(r"(?P<major>\d+)\.\d+")
IMAGE_FIELDS = ("repository", "digest", "source", "pg_minors", "pg_minor")
MAJOR_LISTS = ("supported", "beta", "maintenance")


class Refused(Exception):
    pass


# ---------------------------------------------------------------- validation

def validate(pins, root):
    """Every reason the pin document is unusable, as messages."""
    errs = []
    if pins.get("schema") != 1:
        errs.append("schema must be 1")
    unknown = set(pins) - {"schema", "fork_release", "fork_commit", "majors", "images"}
    if unknown:
        errs.append(f"unknown top-level keys: {sorted(unknown)}")
    if not isinstance(pins.get("fork_release"), str) or not RELEASE.fullmatch(pins["fork_release"]):
        errs.append("fork_release must be lp-YYYY.MM.N or mirror-<8 hex>")
    if not isinstance(pins.get("fork_commit"), str) or not COMMIT.fullmatch(pins["fork_commit"]):
        errs.append("fork_commit must be 40 hex characters")

    majors = pins.get("majors")
    listed = []
    if not isinstance(majors, dict):
        errs.append("[majors] is missing")
        majors = {}
    else:
        unknown = set(majors) - set(MAJOR_LISTS) - {"default"}
        if unknown:
            errs.append(f"unknown [majors] keys: {sorted(unknown)}")
        for name in MAJOR_LISTS:
            v = majors.get(name)
            if not isinstance(v, list) or not all(type(m) is int for m in v):
                errs.append(f"majors.{name} must be a list of integers")
            else:
                listed += v
        if len(listed) != len(set(listed)):
            errs.append("a major is in more than one of majors.supported, beta and maintenance")
        if type(majors.get("default")) is not int or majors.get("default") not in (majors.get("supported") or []):
            errs.append("majors.default must be one of majors.supported")
        for major in listed:
            if major not in UPSTREAM_EOL:
                errs.append(f"major {major}: no upstream end of life in pins.py's UPSTREAM_EOL "
                            "(postgresql.org versions.json)")

    images = pins.get("images")
    if not isinstance(images, dict) or not images:
        errs.append("[images] is missing")
        return errs
    for key, img in images.items():
        where = f"images.{key}"
        if not IMAGE_KEY.fullmatch(key):
            errs.append(f"{where}: bad image key")
        if not isinstance(img, dict):
            errs.append(f"{where} must be a table")
            continue
        errs += [f"{where}: unknown key {k!r}" for k in img if k not in IMAGE_FIELDS]
        repo, digest, source = img.get("repository"), img.get("digest"), img.get("source")
        if not isinstance(repo, str) or not REPOSITORY.fullmatch(repo):
            errs.append(f"{where}.repository must be a repository without tag or digest")
            repo = ""
        elif ARCHIVE in repo:
            errs.append(f"{where}.repository {repo}: the neon-archive packages are never deployed")
        if not isinstance(digest, str) or not DIGEST.fullmatch(digest):
            errs.append(f"{where}.digest must be sha256: and 64 hex characters, got {digest!r}")
        if not isinstance(source, str):
            errs.append(f"{where}.source is missing")
        elif FORK_SOURCE.fullmatch(source):
            if not repo.startswith(FORK_REPOSITORY_PREFIX):
                errs.append(f"{where}: a fork source needs a {FORK_REPOSITORY_PREFIX}* repository")
        elif m := MIRROR_SOURCE.fullmatch(source):
            if m["digest"] != digest:
                errs.append(f"{where}.source: a mirror keeps the digest; {m['digest']} is not {digest}")
            audit = Path(root, AUDIT_DIR, m["digest"].replace(":", "-") + ".json")
            if not audit.is_file():
                errs.append(f"{where}.source: no audit file {AUDIT_DIR}/{audit.name}; audit before pinning a mirror")
        else:
            errs.append(f"{where}.source must be ostrium-labs/loams-postgres@<40 hex> or mirror:<ghcr.io/neondatabase|neon-archive repo>@<digest>")
        if "pg_minors" in img:
            minors = img["pg_minors"]
            if not isinstance(minors, dict) or not all(isinstance(v, str) and (m := MINOR.fullmatch(v)) and m["major"] == k
                                                       for k, v in minors.items()):
                errs.append(f"{where}.pg_minors must map \"<major>\" to \"<major>.<minor>\"")
        if "pg_minor" in img:
            m = MINOR.fullmatch(str(img["pg_minor"]))
            if not (isinstance(img["pg_minor"], str) and m and key == f"compute-v{m['major']}"):
                errs.append(f"{where}.pg_minor must be \"<N>.<minor>\" on images.compute-v<N>")

    storage = images.get("storage")
    if not isinstance(storage, dict):
        errs.append("images.storage is missing")
        storage = {}
    storage_minors = storage.get("pg_minors") if isinstance(storage.get("pg_minors"), dict) else {}
    for major in listed:
        if str(major) not in storage_minors:
            errs.append(f"major {major}: images.storage.pg_minors has no \"{major}\"")
        compute = images.get(f"compute-v{major}")
        if not isinstance(compute, dict):
            errs.append(f"major {major}: no images.compute-v{major}")
        elif compute.get("pg_minor") != storage_minors.get(str(major)):
            errs.append(f"major {major}: images.compute-v{major}.pg_minor differs from images.storage.pg_minors")
    return errs


# ----------------------------------------------------------------- rendering

def q(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def image_order(images):
    def key(k):
        if k == "storage":
            return (0, 0, k)
        if m := re.fullmatch(r"compute-v(\d+)", k):
            return (1, int(m[1]), k)
        return (2, 0, k)
    return sorted(images, key=key)


def render_pins(pins):
    mj = pins["majors"]
    out = [HEADER, "schema = 1", f"fork_release = {q(pins['fork_release'])}", f"fork_commit = {q(pins['fork_commit'])}",
           "", "[majors]"]
    out += [f"{name} = [{', '.join(str(m) for m in mj[name])}]" for name in MAJOR_LISTS]
    out.append(f"default = {mj['default']}")
    for key in image_order(pins["images"]):
        img = pins["images"][key]
        out += ["", f"[images.{key}]", f"repository = {q(img['repository'])}", f"digest = {q(img['digest'])}",
                f"source = {q(img['source'])}"]
        if "pg_minors" in img:
            minors = sorted(img["pg_minors"].items(), key=lambda kv: int(kv[0]))
            out.append("pg_minors = { " + ", ".join(f"{q(k)} = {q(v)}" for k, v in minors) + " }")
        if "pg_minor" in img:
            out.append(f"pg_minor = {q(img['pg_minor'])}")
    return "\n".join(out) + "\n"


def ref(img):
    return f"{img['repository']}@{img['digest']}"


def consumer_lines(pins):
    """(path, pattern of the generated line, the line) for each line consumer."""
    storage = ref(pins["images"]["storage"])
    compute = ref(pins["images"][f"compute-v{pins['majors']['default']}"])
    lines = []
    for path in COMPOSE:
        lines.append((path, r"^x-neon-image: &neon-image \$\{NEON_IMAGE:-[^}\n]*\}$",
                      "x-neon-image: &neon-image ${NEON_IMAGE:-" + storage + "}"))
        lines.append((path, r"^x-compute-image: &compute-image \$\{COMPUTE_IMAGE:-[^}\n]*\}$",
                      "x-compute-image: &compute-image ${COMPUTE_IMAGE:-" + compute + "}"))
    lines.append((RELEASE_COMPOSE, r"^(?P<indent>[ ]+)image: \$\{NEON_IMAGE:-[^}\n]*\}$",
                  "image: ${NEON_IMAGE:-" + storage + "}"))
    lines.append((COMPOSE[0], r"^(?P<indent>[ ]+)PG_VERSION: \$\{COMPUTE_PG_MAJOR:-[^}\n]*\}$",
                  "PG_VERSION: ${COMPUTE_PG_MAJOR:-" + str(pins["majors"]["default"]) + "}"))
    for path in WORKFLOWS:
        lines.append((path, r"^(?P<indent>[ ]+)PG_HEADERS_IMAGE: \S+$", "PG_HEADERS_IMAGE: " + storage))
    return lines


def render_readme_block(pins):
    out = [README_BEGIN,
           f"Pinned in [`{PIN_FILE}`](../pins/loams-postgres-images.toml): `fork_release` "
           f"`{pins['fork_release']}`, `fork_commit` `{pins['fork_commit']}`.",
           "",
           "| Pin | Image (multi-arch index) | Source | Postgres |",
           "|---|---|---|---|"]
    for key in image_order(pins["images"]):
        img = pins["images"][key]
        if "pg_minors" in img:
            pg = ", ".join(v for _, v in sorted(img["pg_minors"].items(), key=lambda kv: int(kv[0])))
        else:
            pg = img.get("pg_minor", "")
        out.append(f"| `{key}` | `{ref(img)}` | `{img['source']}` | {pg} |")
    mj = pins["majors"]
    out += ["", "Postgres majors: " + "; ".join(
        f"{name} {', '.join(str(m) for m in mj[name]) or 'none'}" for name in MAJOR_LISTS) + f"; default {mj['default']}.",
        README_END]
    return "\n".join(out)


def all_majors(pins):
    mj = pins["majors"]
    return sorted(m for name in MAJOR_LISTS for m in mj[name])


def major_state(pins, major):
    return next(name for name in MAJOR_LISTS if major in pins["majors"][name])


def render_rust(pins):
    mj = pins["majors"]
    minors = pins["images"]["storage"]["pg_minors"]

    def arr(v):
        return "&[" + ", ".join(str(m) for m in v) + "]"

    def pairs(f):
        return "&[" + ", ".join(f"({m}, {q(f(m))})" for m in all_majors(pins)) + "]"
    return "\n".join([
        "//! Postgres majors Loams Postgres serves, from deploy/pins/loams-postgres-images.toml",
        "//! (design 62 §9.1, D1114).",
        "//!",
        "//! Generated by `scripts/nf1/pins.py set` (or `rust`); checked by `pins.py check`.",
        "//! Do not edit by hand: move a major with `pins.py set --supported .. --beta ..",
        "//! --maintenance .. --default ..`.",
        "",
        "/// Majors new projects may use.",
        f"pub const SUPPORTED: &[u32] = {arr(mj['supported'])};",
        "/// Majors that need `allow_beta_major`.",
        f"pub const BETA: &[u32] = {arr(mj['beta'])};",
        "/// Majors kept for minors and exports only; no new projects.",
        f"pub const MAINTENANCE: &[u32] = {arr(mj['maintenance'])};",
        "/// The major for new projects and new desktop tenants.",
        f"pub const DEFAULT: u32 = {mj['default']};",
        "/// The pinned minor of each major above (`images.storage.pg_minors`).",
        f"pub const MINORS: &[(u32, &str)] = {pairs(lambda m: minors[str(m)])};",
        "/// Upstream's end of life of each major above (postgresql.org `versions.json`),",
        "/// as `YYYY-MM-DD`.",
        f"pub const EOL: &[(u32, &str)] = {pairs(lambda m: UPSTREAM_EOL[m])};",
        ""])


def render_computes(pins):
    """The compose services between the COMPUTES markers: one compute per major."""
    out = [COMPUTES_BEGIN,
           "  # One compute per Postgres major in the pin file (design 62 §9.4), off unless its profile",
           "  # `pg<N>` is selected (a beta major's also by `beta`). Each attaches only to the timeline",
           "  # PG<N>_TENANT_ID and PG<N>_TIMELINE_ID name, which must be a timeline of that major (a",
           "  # compute of another major is refused by loams-wal), and never falls back to compute1's.",
           "  # COMPUTE_V<N>_IMAGE overrides the pinned image; PG<N>_PORT the loopback port."]
    for major in all_majors(pins):
        state = major_state(pins, major)
        profiles = [f"pg{major}"] + (["beta"] if state == "beta" else [])
        img = ref(pins["images"][f"compute-v{major}"])
        out += [f"  compute-v{major}:",
                f"    # Postgres {major}: {state}.",
                "    profiles: [" + ", ".join(f'"{p}"' for p in profiles) + "]",
                f"    image: ${{COMPUTE_V{major}_IMAGE:-{img}}}",
                "    environment:",
                f"      TENANT_ID: ${{PG{major}_TENANT_ID:-}}",
                f"      TIMELINE_ID: ${{PG{major}_TIMELINE_ID:-}}",
                "    volumes:",
                "      - ./compute/config.json:/var/db/postgres/configs/config.json:ro",
                "      - ./compute/start.sh:/shell/start.sh:ro",
                "    ports:",
                f"      - 127.0.0.1:${{PG{major}_PORT:-{55400 + major}}}:55433",
                '    entrypoint: ["/bin/bash", "/shell/start.sh"]',
                "    depends_on: [pageserver, safekeeper1]"]
    out.append(COMPUTES_END)
    return "\n".join(out)


def replace_block(path, text, begin, end, block):
    b, e = text.find(begin), text.find(end)
    if b < 0 or e < b or text.count(begin) != 1 or text.count(end) != 1:
        raise Refused(f"{path}: expected one pins.py begin/end marker pair")
    return text[:b] + block + text[e + len(end):]


def generate(root, pins):
    """{path: wanted text} for the pin file and every consumer, or Refused."""
    wanted = {PIN_FILE: render_pins(pins)}
    for path, pattern, line in consumer_lines(pins):
        text = wanted.get(path)
        if text is None:
            text = read(root, path)
        matches = list(re.finditer(pattern, text, re.M))
        if len(matches) != 1:
            raise Refused(f"{path}: expected one line matching {pattern!r}, found {len(matches)}")
        m = matches[0]
        indent = m.groupdict().get("indent") or ""
        wanted[path] = text[:m.start()] + indent + line + text[m.end():]
    wanted[README] = replace_block(README, read(root, README), README_BEGIN, README_END, render_readme_block(pins))
    wanted[COMPOSE[0]] = replace_block(COMPOSE[0], wanted[COMPOSE[0]], COMPUTES_BEGIN, COMPUTES_END,
                                       render_computes(pins))
    wanted[MAJORS_RS] = render_rust(pins)
    return wanted


# --------------------------------------------------------------------- files

def read(root, path):
    p = Path(root, path)
    if not p.is_file():
        raise Refused(f"{path}: missing")
    return p.read_text()


def load_toml(text, origin):
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        raise Refused(f"{origin}: {e}") from None


def fetch(src):
    """The release's pin document: an https URL (checked in main) or a local path."""
    if src.startswith("https://"):
        with urllib.request.urlopen(src, timeout=60) as resp:  # noqa: S310 (https only)
            return resp.read().decode()
    return Path(src).read_text()


def archive_refs(root):
    """Files outside the reference places that name a neon-archive package."""
    root = Path(root)
    if (root / ".git").exists():
        r = subprocess.run(["git", "-C", str(root), "grep", "-l", "-z", "-F", ARCHIVE],
                           capture_output=True, check=False)
        if r.returncode not in (0, 1):
            raise Refused(f"git grep failed: {r.stderr.decode(errors='replace').strip()}")
        paths = [p for p in r.stdout.decode().split("\0") if p]
    else:
        paths = []
        for p in sorted(root.rglob("*")):
            if p.is_file():
                try:
                    if ARCHIVE in p.read_text():
                        paths.append(p.relative_to(root).as_posix())
                except UnicodeDecodeError:
                    pass
    bad = []
    for path in paths:
        if path.startswith(ARCHIVE_REFERENCE_DIRS) or path == PIN_FILE:
            continue  # the pin file's repositories are checked by validate()
        if path == README:
            # The Mirror section, and the generated table (a pin's `mirror:` source).
            outside = (root / path).read_text()
            for pattern in (r"^## Mirror\n.*?(?=^## |\Z)", re.escape(README_BEGIN) + ".*?" + re.escape(README_END)):
                outside = re.sub(pattern, "", outside, count=1, flags=re.M | re.S)
            if ARCHIVE not in outside:
                continue
        bad.append(path)
    return bad


# ------------------------------------------------------------------ commands

def cmd_check(root):
    errs = []
    try:
        text = read(root, PIN_FILE)
        pins = load_toml(text, PIN_FILE)
    except Refused as e:
        print(f"pins.py check: {e}", file=sys.stderr)
        return 1
    errs += [f"{PIN_FILE}: {e}" for e in validate(pins, root)]
    if not errs:
        try:
            for path, want in generate(root, pins).items():
                if not Path(root, path).is_file():
                    errs.append(f"{path}: missing (run pins.py set)")
                elif read(root, path) != want:
                    what = "is not in pins.py's layout (run pins.py set)" if path == PIN_FILE else \
                        "differs from the pin file (run pins.py set)"
                    errs.append(f"{path}: {what}")
        except Refused as e:
            errs.append(str(e))
    try:
        errs += [f"{p}: names {ARCHIVE}, whose images are never deployed (reference use only: "
                 f"{', '.join(ARCHIVE_REFERENCE_DIRS)}, mirror: sources, the README's Mirror section)"
                 for p in archive_refs(root)]
    except Refused as e:
        errs.append(str(e))
    for e in errs:
        print(f"pins.py check: {e}", file=sys.stderr)
    if not errs:
        print(f"pins.py check: ok ({PIN_FILE}, {len(COMPOSE) + len(WORKFLOWS) + 2} consumers)")
    return 1 if errs else 0


def cmd_set(root, src, majors=None):
    try:
        if src is None:
            pins = load_toml(read(root, PIN_FILE), PIN_FILE)
        else:
            try:
                text = fetch(src)
            except OSError as e:
                raise Refused(f"{src}: {e}") from None
            pins = load_toml(text, src)
        if majors:
            if not isinstance(pins.get("majors"), dict):
                pins["majors"] = {}
            pins["majors"].update(majors)
        errs = validate(pins, root)
        if errs:
            raise Refused("; ".join(errs))
        wanted = generate(root, pins)
    except Refused as e:
        print(f"pins.py set: refused, nothing written: {e}", file=sys.stderr)
        return 1
    for path, text in wanted.items():
        p = Path(root, path)
        if not p.is_file() or p.read_text() != text:
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
            print(f"pins.py set: wrote {path}")
    return 0


def cmd_rust(root):
    try:
        pins = load_toml(read(root, PIN_FILE), PIN_FILE)
        errs = validate(pins, root)
        if errs:
            raise Refused("; ".join(errs))
    except Refused as e:
        print(f"pins.py rust: {e}", file=sys.stderr)
        return 1
    p = Path(root, MAJORS_RS)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(render_rust(pins))
    print(f"pins.py rust: wrote {MAJORS_RS}")
    return 0


def major_list(text):
    try:
        return [int(m) for m in text.split(",") if m.strip()]
    except ValueError:
        raise argparse.ArgumentTypeError(f"{text!r}: expected majors like 16,17") from None


def main(argv):
    ap = argparse.ArgumentParser(prog="pins.py", description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", default=str(Path(__file__).resolve().parents[2]), help="repository root")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check")
    s = sub.add_parser("set")
    s.add_argument("--from-release", metavar="URL|PATH")
    for name in MAJOR_LISTS:
        s.add_argument(f"--{name}", metavar="N,..", type=major_list,
                       help=f"replace [majors] {name} (comma-separated; \"\" for none)")
    s.add_argument("--default", metavar="N", type=int, help="replace [majors] default")
    sub.add_parser("rust")
    args = ap.parse_args(argv)
    if args.cmd == "check":
        return cmd_check(args.root)
    if args.cmd == "set":
        if args.from_release and re.match(r"[a-z][a-z0-9+.-]*://", args.from_release) \
                and not args.from_release.startswith("https://"):
            ap.error("--from-release: only https URLs or local paths")
        majors = {name: getattr(args, name) for name in MAJOR_LISTS + ("default",)
                  if getattr(args, name) is not None}
        return cmd_set(args.root, args.from_release, majors)
    return cmd_rust(args.root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
