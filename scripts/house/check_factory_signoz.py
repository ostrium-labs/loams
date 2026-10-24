#!/usr/bin/env python3
"""factory_signoz_house_command_is_the_release_one (HE1 R9.67).

The Factory's SigNoz overlay (deploy/factory/signoz/compose.yaml) restates the
release House's command to add `--native-tls-listen`. This keeps the two in
step: the overlay's command is the release's, plus that one flag, and the
overlay's DSN points at that listener over TLS. Stdlib only (the compose
files are read as text: each command is a block of `- ` lines).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]


def house_command(path):
    lines = path.read_text().splitlines()
    start = next(i for i, l in enumerate(lines) if l == "  house:")
    out, inside = [], False
    for line in lines[start + 1:]:
        if re.match(r"^  \S", line):
            break
        if line.strip() == "command:":
            inside = True
            continue
        if inside:
            m = re.match(r"^\s+- (.*)$", line)
            if m:
                out.append(m.group(1))
            elif line.strip() and not line.strip().startswith("#"):
                inside = False
    return out


def main():
    release = house_command(ROOT / "deploy/release/compose.yaml")
    overlay_path = ROOT / "deploy/factory/signoz/compose.yaml"
    overlay = house_command(overlay_path)
    want = release + ["--native-tls-listen=0.0.0.0:9440"]
    errors = []
    if not release:
        errors.append("no house command in deploy/release/compose.yaml")
    if overlay != want:
        errors.append(f"the overlay's house command is {overlay}, want {want}")
    text = overlay_path.read_text()
    if "?secure=true&skip_verify=true" not in text or "house:9440" not in text:
        errors.append("the overlay's DSN is not the House's native TLS listener")
    for e in errors:
        print(f"check_factory_signoz: {e}", file=sys.stderr)
    if not errors:
        print("check_factory_signoz: the overlay's House is the release's, plus native TLS")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
