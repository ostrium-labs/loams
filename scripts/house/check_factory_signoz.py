#!/usr/bin/env python3
"""factory_signoz_house_is_the_release_one (HE1 R9.67).

The Factory's SigNoz overlay (deploy/factory/signoz/compose.yaml) turns on the
release House's native TLS listener with LOAMS_HOUSE_NATIVE_TLS_LISTEN and does
not restate its command, so the overlay's House stays the release's. This
checks that, and that the overlay's DSN points at that listener over TLS.
Stdlib only (the compose file is read as text).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]


def house_block(path):
    lines = path.read_text().splitlines()
    start = next(i for i, l in enumerate(lines) if l == "  house:")
    out = []
    for line in lines[start + 1:]:
        if re.match(r"^  \S", line):
            break
        out.append(line)
    return out


def main():
    overlay_path = ROOT / "deploy/factory/signoz/compose.yaml"
    house = house_block(overlay_path)
    errors = []
    if any(l.strip() == "command:" or l.strip().startswith("command:") for l in house):
        errors.append("the overlay restates the House's command; use LOAMS_HOUSE_NATIVE_TLS_LISTEN")
    if not any(re.match(r"^\s+LOAMS_HOUSE_NATIVE_TLS_LISTEN:\s*0\.0\.0\.0:9440\s*$", l) for l in house):
        errors.append("the overlay's House does not set LOAMS_HOUSE_NATIVE_TLS_LISTEN: 0.0.0.0:9440")
    text = overlay_path.read_text()
    if "secure=true&skip_verify=true" not in text or "house:9440" not in text:
        errors.append("the overlay's DSN is not the House's native TLS listener")
    for e in errors:
        print(f"check_factory_signoz: {e}", file=sys.stderr)
    if not errors:
        print("check_factory_signoz: the overlay's House is the release's, with native TLS on")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
