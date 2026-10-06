#!/usr/bin/env python3
"""Check canonical app bindings without rewriting the committed sources."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def tree(path: Path) -> dict[str, bytes]:
    return {str(p.relative_to(path)): p.read_bytes() for p in path.rglob("*.ts")}


def main() -> int:
    tool = ROOT / "web/packages/proto/node_modules/.bin/buf"
    if not tool.exists():
        print("Install workspace dependencies with pnpm install --frozen-lockfile first.")
        return 1
    with tempfile.TemporaryDirectory(prefix="loams-contracts-") as directory:
        subprocess.run(
            [str(tool), "generate", "--template", "buf.gen.apps.yaml", "--output", directory],
            cwd=ROOT,
            check=True,
        )
        generated = tree(Path(directory) / "web/packages/proto/src/gen")
        committed = tree(ROOT / "web/packages/proto/src/gen")
        changed = sorted(name for name in generated.keys() | committed.keys()
                         if generated.get(name) != committed.get(name))
        if not generated:
            print("Contract generation produced no TypeScript bindings.")
            return 1
        if changed:
            print("Canonical generated-code drift: " + ", ".join(changed))
            return 1
        print(f"Canonical app bindings match ({len(generated)} files).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
