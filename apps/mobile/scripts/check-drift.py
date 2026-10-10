#!/usr/bin/env python3
"""Compare local vendored contracts and generated trees without rewriting them."""
import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
GENERATED = (
    "android/proto/src/generated",
    "ios/Packages/LoamsProto/Sources/LoamsProto/Generated",
    "mock/gen",
)


def manifest(root):
    return {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file()}


def main():
    lines = "".join(
        f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(ROOT).as_posix()}\n"
        for p in sorted((ROOT / "proto").rglob("*.proto"))
    )
    lock = dict(line.split("=", 1) for line in
                (ROOT / "conformance/proto-ref.lock").read_text().splitlines()
                if line and not line.startswith("#"))
    if hashlib.sha256(lines.encode()).hexdigest() != lock["tree_sha256"]:
        raise SystemExit("Local proto tree differs from conformance/proto-ref.lock")
    cache = ROOT / ".cache"
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="drift-", dir=cache) as directory:
        temp = Path(directory)
        shutil.copytree(ROOT / "proto", temp / "proto")
        (temp / "scripts").mkdir()
        for name in ("generate.sh", "proto-hash.sh"):
            shutil.copy2(ROOT / "scripts" / name, temp / "scripts" / name)
        for config in ROOT.glob("buf*.yaml"):
            shutil.copy2(config, temp / config.name)
        subprocess.run(["bash", "scripts/generate.sh"], cwd=temp, check=True, timeout=540)
        stale = []
        for path in GENERATED:
            before, after = manifest(ROOT / path), manifest(temp / path)
            stale.extend(f"{path}/{name}" for name in sorted(before.keys() | after.keys())
                         if before.get(name) != after.get(name))
        if stale:
            raise SystemExit("Generated code is stale:\n" + "\n".join(stale))
    print("Local proto lock and generated code are in sync (source trees unchanged)")


if __name__ == "__main__":
    main()
