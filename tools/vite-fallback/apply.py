#!/usr/bin/env python3
"""Turn a checkout's web/ toolchain from Vite+ back into plain Vite, Vitest and tsdown.

The weekly fallback lane (design 79 D1774, Q1309; plan VV1 item 5) runs this on a
throwaway CI checkout, then builds and tests the console and @loams/ui with the
plain tools. It proves the exit path stays a one-day job: if a Vite+-only feature
creeps into web/, this script or the build after it fails.

The plain versions are the ones the pinned vite-plus bundles (its vite-plus-core's
`bundledVersions`, and its own `vitest` dependency), so the lane compares like with
like and follows each pin bump without edits.

Never run it in a working tree you care about: it rewrites files in place.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WEB = ROOT / "web"
SOURCE_SUFFIXES = {".ts", ".tsx", ".mts", ".cts", ".js", ".mjs", ".cjs"}
SCRIPTS = [
    (re.compile(r"\bvp test run\b"), "vitest run"),
    (re.compile(r"\bvp test\b"), "vitest"),
    (re.compile(r"\bvp build\b"), "vite build"),
    (re.compile(r"\bvp preview\b"), "vite preview"),
    (re.compile(r"\bvp dev\b"), "vite"),
    (re.compile(r"\bvp pack\b"), "tsdown"),
    (re.compile(r"\bvp check\b"), "true"),
    (re.compile(r"\bvp fmt\b"), "true"),
]
# @loams/ui's `pack` section of vite.config.ts, as a plain tsdown config.
UI_TSDOWN_CONFIG = """export default {
  entry: ['src/index.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  fixedExtension: false,
};
"""


def npm_view(spec, field):
    out = json.loads(subprocess.check_output(["npm", "view", spec, field, "--json"], text=True))
    # Some npm versions wrap a single result in a list.
    return out[0] if isinstance(out, list) else out


def walk(base, predicate):
    for path in base.rglob("*"):
        if "node_modules" in path.parts or "dist" in path.parts:
            continue
        if path.is_file() and predicate(path):
            yield path


def main():
    workspace = ROOT / "pnpm-workspace.yaml"
    text = workspace.read_text(encoding="utf-8")
    pin = re.search(r"^  vite-plus: (\S+)$", text, re.M)
    if not pin:
        sys.exit("no `vite-plus:` catalog pin in pnpm-workspace.yaml")
    pin = pin.group(1)
    bundled = npm_view(f"@voidzero-dev/vite-plus-core@{pin}", "bundledVersions")
    vitest = npm_view(f"vite-plus@{pin}", "dependencies")["vitest"]
    vite, tsdown = bundled["vite"], bundled["tsdown"]
    print(f"vite-plus {pin} -> plain vite {vite}, vitest {vitest}, tsdown {tsdown}")

    # The catalog names plain tools; the vite-plus-core aliases and overrides go.
    text = re.sub(r"^  vite: npm:@voidzero-dev/vite-plus-core@\S+$", f"  vite: {vite}", text, count=1, flags=re.M)
    text = re.sub(r"^  vitest: \S+$", f"  vitest: {vitest}", text, count=1, flags=re.M)
    text = re.sub(r"^  ('[^']*>vite'|vite): npm:@voidzero-dev/vite-plus-core@\S+\n", "", text, flags=re.M)
    workspace.write_text(text, encoding="utf-8")

    # Manifests: no vite-plus, scripts on the plain binaries.
    for manifest in walk(WEB, lambda p: p.name == "package.json"):
        data = json.loads(manifest.read_text(encoding="utf-8"))
        changed = False
        for section in ("dependencies", "devDependencies"):
            if data.get(section, {}).pop("vite-plus", None) is not None:
                changed = True
        for name, script in data.get("scripts", {}).items():
            new = script
            for pattern, replacement in SCRIPTS:
                new = pattern.sub(replacement, new)
            if new != script:
                data["scripts"][name] = new
                changed = True
        if data.get("name") == "@loams/ui":
            data.setdefault("devDependencies", {})["tsdown"] = tsdown
            (manifest.parent / "tsdown.config.ts").write_text(UI_TSDOWN_CONFIG, encoding="utf-8")
            changed = True
        if changed:
            manifest.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    # Sources: the standard modules Vite+ re-exports.
    for source in walk(WEB, lambda p: p.suffix in SOURCE_SUFFIXES or p.name.endswith(".d.ts")):
        body = source.read_text(encoding="utf-8")
        new = re.sub(r"""(['"])vite-plus/test\1""", r"\1vitest\1", body)
        new = re.sub(r"""(['"])vite-plus/client\1""", r"\1vite/client\1", new)
        new = re.sub(r"""(from\s+)(['"])vite-plus\2""", r"\1\2vitest/config\2", new)
        if new != body:
            source.write_text(new, encoding="utf-8")

    # Anything left is a Vite+-only dependency: the lane fails here.
    left = [
        str(p.relative_to(ROOT))
        for p in walk(WEB, lambda p: p.suffix in SOURCE_SUFFIXES or p.name == "package.json")
        if re.search(r"""['"]vite-plus(/[^'"]*)?['"]""", p.read_text(encoding="utf-8"))
    ]
    if left:
        print("Vite+-only references remain:", *left, sep="\n  ")
        sys.exit(1)
    print("web/ is on plain Vite, Vitest and tsdown")


if __name__ == "__main__":
    main()
