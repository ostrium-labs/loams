
"""Local, pinned protobuf tooling; never required by the baseline build."""
import argparse
import difflib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUF_VERSION = "1.73.0"
GENERATOR_VERSION = "2.16.0"
GENERATED = ROOT / "core/bi-rpc/src/gen"


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def require(command):
    executable = shutil.which(command)
    if not executable:
        raise SystemExit(f"Missing {command}. Install dependencies at the Loams root; "
                         f"install Buf {BUF_VERSION} separately. Baseline build/test do not need generation.")
    return executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("lint", "generate", "drift"))
    action = parser.parse_args().action
    buf = require("buf")
    version = subprocess.check_output([buf, "--version"], text=True).strip()
    if version != BUF_VERSION:
        raise SystemExit(f"Expected Buf {BUF_VERSION}, found {version}")
    if action == "lint":
        run(buf, "lint", "--config", "buf.yaml")
        return
    generator = require("protoc-gen-es")
    version = subprocess.check_output([generator, "--version"], text=True).strip()
    if version.split()[-1].lstrip("v") != GENERATOR_VERSION:
        raise SystemExit(f"Expected protoc-gen-es {GENERATOR_VERSION}, found {version}")
    pnpm = require("pnpm")
    with tempfile.TemporaryDirectory(prefix=".proto-generation-", dir=ROOT) as directory:
        output = Path(directory)
        template = {"version": "v2", "plugins": [{"local": generator, "out": str(output), "opt": ["target=ts"]}]}
        run(buf, "generate", "--config", "buf.yaml", "--template", json.dumps(template))
        # vp fmt discovers settings from cwd, not each input's directory. Select
        # a dedicated config so parent EditorConfig cannot reformat the bindings.
        format_config = output / "vite.config.mjs"
        format_config.write_text(
            'export default { fmt: { tabWidth: 2, useTabs: false, endOfLine: "lf" } };\n'
        )
        run(pnpm, "exec", "vp", "fmt", "--config", str(format_config), str(output))
        expected = {p.relative_to(output): p for p in output.rglob("*.ts")}
        actual = {p.relative_to(GENERATED): p for p in GENERATED.rglob("*.ts")}
        if action == "generate":
            for relative, path in expected.items():
                destination = GENERATED / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, destination)
            for relative in actual.keys() - expected.keys():
                actual[relative].unlink()
            print(f"Generated {len(expected)} bindings (bi.v1 unchanged)")
            return
        changed = False
        for relative in sorted(actual.keys() | expected.keys()):
            before = actual[relative].read_text() if relative in actual else ""
            after = expected[relative].read_text() if relative in expected else ""
            if before != after:
                changed = True
                print("".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                                  fromfile=str(GENERATED / relative), tofile=f"generated/{relative}")))
        if changed:
            raise SystemExit("Protobuf drift detected; run pnpm --filter @loams-plugins/root proto:gen")
        print(f"No protobuf drift in {len(expected)} bindings")


if __name__ == "__main__":
    main()
