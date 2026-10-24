#!/usr/bin/env python3
"""Licence gate over the compute harness's pins (D1271, D359; OC0 Task 1).

Every key of `conformance/compute/pins.env` needs an entry in
`conformance/compute/licences.toml`:

    [TEMPORAL_SDK_GO]
    value = "1.49.0"                 # the pin this entry was checked for
    licence = "MIT"                  # SPDX expression
    source = "https://..."           # where it was read
    resolve = "github:temporalio/sdk-go"   # how --online re-reads it

The gate fails when a pin has no entry, an entry's `value` is not the pin's
current value (a bump must re-check its licence), an entry names no pin, or
a licence is refused: AGPL, BUSL (the Business Source License; BSL-1.0, the
Boost licence, is fine), SSPL, Elastic-2.0, FSL, the Confluent Community
Licence, RSAL, Commons Clause, or none at all (NOASSERTION, unknown). An
entry may carry `exception = "D318"` only for a key in EXCEPTIONS below,
which records the decision that allows it.

`--online` also re-reads each entry's `resolve` (GitHub's licence API, PyPI's
or npm's metadata; GH_TOKEN is used when set) and fails when it disagrees.
The pull-request job runs offline; the nightly runs `--online`.

  licence_gate.py [--pins FILE] [--licences FILE] [--online]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REFUSED = [
    (re.compile(r"\bAGPL"), "AGPL"),
    (re.compile(r"\bBUSL"), "BUSL (Business Source License)"),
    (re.compile(r"\bSSPL"), "SSPL"),
    (re.compile(r"\bElastic-2\.0\b|\bELv2\b"), "Elastic License 2.0"),
    (re.compile(r"\bFSL-"), "Functional Source License"),
    (re.compile(r"Confluent|\bCCL\b"), "Confluent Community License"),
    (re.compile(r"\bRSAL|Redis-Source-Available"), "Redis Source Available License"),
    (re.compile(r"Commons-Clause"), "Commons Clause"),
]
UNLICENSED = {"", "NOASSERTION", "NONE", "UNKNOWN", "OTHER", "UNLICENSED"}
# Pins allowed despite a refused licence, and the decision that allows each.
EXCEPTIONS = {
    "PGDOG_IMAGE": "D318",  # PgDog: unmodified, separate process; scripts/ci/pgdog-licence.sh
}


def read_pins(path: Path) -> dict[str, str]:
    pins: dict[str, str] = {}
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        m = re.fullmatch(r"([A-Z][A-Z0-9_]*)=(\S+)", line)
        if not m:
            raise SystemExit(f"licence-gate: {path}:{n}: not KEY=value (literal, no spaces): {line}")
        if m.group(1) in pins:
            raise SystemExit(f"licence-gate: {path}:{n}: {m.group(1)} set twice")
        pins[m.group(1)] = m.group(2)
    return pins


def refused(licence: str) -> str | None:
    if licence.strip().upper() in UNLICENSED:
        return "no licence"
    for rx, name in REFUSED:
        if rx.search(licence):
            return name
    return None


def fetch_json(url: str) -> dict:
    req = urllib.request.Request(url, headers={"User-Agent": "loams-licence-gate"})
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=30) as r:  # noqa: S310 (fixed hosts)
        return json.load(r)


def resolve(spec: str, value: str) -> str:
    """The licence `spec` names today (`github:o/r`, `pypi:name`, `npm:name`)."""
    kind, _, target = spec.partition(":")
    if kind == "github":
        return fetch_json(f"https://api.github.com/repos/{target}").get("license", {}).get("spdx_id") or ""
    if kind == "pypi":
        info = fetch_json(f"https://pypi.org/pypi/{target}/{value}/json")["info"]
        return info.get("license_expression") or info.get("license") or ""
    if kind == "npm":
        return fetch_json(f"https://registry.npmjs.org/{target}/{value}").get("license") or ""
    raise ValueError(f"unknown resolve kind {kind!r}")


def gate(pins: dict[str, str], table: dict, online: bool) -> list[str]:
    problems: list[str] = []
    for key, value in pins.items():
        e = table.get(key)
        if not isinstance(e, dict):
            problems.append(f"{key}: no entry in licences.toml")
            continue
        if e.get("value") != value:
            problems.append(
                f"{key}: the entry was checked for {e.get('value')!r}, the pin is {value!r} (re-check its licence)"
            )
        lic = e.get("licence")
        if not isinstance(lic, str):
            problems.append(f"{key}: no `licence`")
            continue
        if not isinstance(e.get("source"), str) or not e["source"].startswith("https://"):
            problems.append(f"{key}: `source` must be the https URL the licence was read from")
        why = refused(lic)
        exc = e.get("exception")
        if exc is not None and EXCEPTIONS.get(key) != exc:
            problems.append(f"{key}: exception {exc!r} is not allowed for this pin")
        if why and not (exc is not None and EXCEPTIONS.get(key) == exc):
            problems.append(f"{key}: {lic} is refused ({why}; D1271)")
        if online and isinstance(e.get("resolve"), str):
            try:
                live = resolve(e["resolve"], value.removeprefix("v"))
            except Exception as err:  # noqa: BLE001 (report and fail)
                problems.append(f"{key}: cannot resolve {e['resolve']}: {err}")
            else:
                if live and live != lic and live != "NOASSERTION":
                    problems.append(f"{key}: {e['resolve']} now says {live!r}, the entry says {lic!r}")
    for key in table:
        if key not in pins:
            problems.append(f"{key}: entry for a pin that does not exist")
    return problems


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pins", type=Path, default=ROOT / "conformance/compute/pins.env")
    ap.add_argument("--licences", type=Path, default=ROOT / "conformance/compute/licences.toml")
    ap.add_argument("--online", action="store_true")
    args = ap.parse_args(argv)
    pins = read_pins(args.pins)
    table = tomllib.loads(args.licences.read_text(encoding="utf-8"))
    problems = gate(pins, table, args.online)
    for p in problems:
        print(f"licence-gate: {p}")
    print(f"licence-gate: {len(pins)} pins, {'FAIL' if problems else 'ok'}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
