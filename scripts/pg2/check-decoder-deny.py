#!/usr/bin/env python3
"""PG2 Task 31: keep the Neon fork's dependency tree out of the main workspace.

- The root Cargo.lock names no Neon-fork source (the fork is linked only by
  crates/loams-wal-decoder, a workspace of its own).
- crates/loams-wal-decoder/deny.toml extends the root deny.toml: the same
  licences, clarifications, bans and advisory settings (advisory ignores may
  be added), and the root's git sources plus only github.com/ostrium-labs
  ones.

Usage: scripts/pg2/check-decoder-deny.py [repo root]
"""
import sys
import tomllib
from pathlib import Path

NEON_SOURCES = ("ostrium-labs/neon", "neondatabase/neon", "rust-postgres", "azure-sdk-for-rust")


def main(root: Path) -> int:
    errors = []
    lock = (root / "Cargo.lock").read_text()
    for src in NEON_SOURCES:
        if src in lock:
            errors.append(f"Cargo.lock names {src}: the Neon fork belongs to crates/loams-wal-decoder only")

    base = tomllib.loads((root / "deny.toml").read_text())
    dec = tomllib.loads((root / "crates/loams-wal-decoder/deny.toml").read_text())
    for key in ("graph", "bans"):
        if base.get(key) != dec.get(key):
            errors.append(f"[{key}] differs from the root deny.toml")
    bl, dl = base.get("licenses", {}), dec.get("licenses", {})
    if set(dl.get("allow", [])) != set(bl.get("allow", [])):
        errors.append("[licenses] allow differs from the root deny.toml")
    for k in ("confidence-threshold", "clarify"):
        if bl.get(k) != dl.get(k):
            errors.append(f"[licenses] {k} differs from the root deny.toml")
    if dl.get("exceptions"):
        errors.append("[licenses] exceptions are not allowed in the decoder's policy")
    ba, da = base.get("advisories", {}), dec.get("advisories", {})
    for k in set(ba) | set(da):
        if k == "ignore":
            missing = [i for i in ba.get("ignore", []) if i not in da.get("ignore", [])]
            if missing:
                errors.append(f"[advisories] ignore drops root entries: {missing}")
        elif ba.get(k) != da.get(k):
            errors.append(f"[advisories] {k} differs from the root deny.toml")
    bs, ds = base.get("sources", {}), dec.get("sources", {})
    for k in ("unknown-registry", "unknown-git"):
        if bs.get(k) != ds.get(k):
            errors.append(f"[sources] {k} differs from the root deny.toml")
    root_git = set(bs.get("allow-git", []))
    extra = set(ds.get("allow-git", [])) - root_git
    if not root_git <= set(ds.get("allow-git", [])):
        errors.append("[sources] allow-git drops root entries")
    bad = [u for u in extra if not u.startswith("https://github.com/ostrium-labs/")]
    if bad:
        errors.append(f"[sources] decoder-only git sources must be ostrium-labs forks: {bad}")

    for e in errors:
        print(f"check-decoder-deny: {e}", file=sys.stderr)
    if not errors:
        print("check-decoder-deny: ok")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(Path(sys.argv[1] if len(sys.argv) > 1 else ".")))
