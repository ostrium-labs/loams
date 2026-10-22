#!/usr/bin/env python3
"""The supported SDKs are additive-only (design §54 §13.2, D939; UP1 Task 17).

A public symbol that an SDK had at the previous release and does not have now
fails the check, unless it belongs to a package that is still `unstable`
(listed under `breaking.ignore` in buf.yaml). `buf breaking` covers the
protos; this covers what the SDKs add on top of them (clients, facades,
helpers).

    python3 sdks/fixtures/removed_symbols.py              # against the previous release
    python3 sdks/fixtures/removed_symbols.py --against v0.1.0-preview.2
    python3 sdks/fixtures/removed_symbols.py --self-test  # planted removals

The previous release is the `from` of the newest release in
release/upgrade-path.toml, as the tag `v<from>`. Public symbols are found
per language with the patterns below: a declaration's kind and name, so a
symbol that moves between files is not a removal.
"""

import io
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]

# The supported SDKs (design §44, D614: wave 1 and the grandfathered three).
SUPPORTED = ["typescript", "python", "go", "rust", "cpp", "csharp", "swift"]

# Where each SDK's public sources are, and which files are not public.
SOURCES = {
    "typescript": (["packages"], (".ts",), r"(test|spec|\.d\.ts$|/gen/)"),
    "python": (["src"], (".py", ".pyi"), r"(test|_pb2)"),
    "go": (["."], (".go",), r"(_test\.go$|/internal/|/examples?/)"),
    "rust": (["src"], (".rs",), r"(tests?/)"),
    "cpp": (["include"], (".hpp", ".h"), r"(detail/)"),
    "csharp": (["src"], (".cs",), r"(Test|/obj/|/bin/)"),
    "swift": (["Sources"], (".swift",), r"(Tests/)"),
}

PATTERNS = {
    "typescript": [
        r"^export\s+(?:declare\s+)?(?:default\s+)?(?:async\s+)?"
        r"(function|class|const|let|interface|type|enum)\s+([A-Za-z_$][\w$]*)",
    ],
    "python": [r"^(def|class)\s+([A-Za-z][\w]*)"],
    "go": [
        r"^(func)\s+(?:\([^)]*\)\s+)?([A-Z]\w*)",
        r"^(type)\s+([A-Z]\w*)",
        r"^\s*(var|const)\s+([A-Z]\w*)",
    ],
    "rust": [r"^\s*pub\s+(?:async\s+)?(fn|struct|enum|trait|type|const|mod|static)\s+(\w+)"],
    "cpp": [r"^\s*(class|struct|enum class|enum|using|namespace)\s+([A-Za-z_]\w*)"],
    "csharp": [
        r"^\s*public\s+(?:static\s+|sealed\s+|abstract\s+|partial\s+|readonly\s+|async\s+)*"
        r"(record\s+struct|record\s+class|class|struct|record|interface|enum)\s+(\w+)",
        r"^\s*public\s+(?:static\s+|virtual\s+|override\s+|async\s+)*[\w<>\[\],.? ]+\s+(\w+)\s*\(",
    ],
    "swift": [
        r"^\s*(?:@\w+\s+)*public\s+(?:final\s+|static\s+|mutating\s+)*"
        r"(class|struct|enum|protocol|func|var|let|typealias|actor)\s+(\w+)",
    ],
}


def unstable_packages(root):
    """The proto packages buf.yaml's `breaking.ignore` lists (`live`, `graph`, …)."""
    text = (root / "buf.yaml").read_text()
    section = text.split("\nbreaking:", 1)[-1]
    return sorted(set(re.findall(r"^\s*-\s*proto/loams/([a-z]+)", section, re.M)))


def is_unstable(path, unstable):
    """Whether `path` (relative to the SDK) is one of an unstable package's files."""
    parts = re.split(r"[/._-]", path.lower())
    return any(pkg in parts for pkg in unstable)


def symbols(sdk_dir, lang, unstable):
    """{"kind name": first file} of the SDK at `sdk_dir`, unstable ones left out."""
    roots, exts, skip = SOURCES[lang]
    found = {}
    for base in roots:
        top = sdk_dir / base
        if not top.exists():
            continue
        for path in sorted(top.rglob("*")):
            rel = path.relative_to(sdk_dir).as_posix()
            if not path.is_file() or not rel.endswith(exts) or re.search(skip, "/" + rel):
                continue
            if "node_modules" in rel or is_unstable(rel, unstable):
                continue
            for line in path.read_text(errors="replace").splitlines():
                for pattern in PATTERNS[lang]:
                    m = re.match(pattern, line)
                    if not m:
                        continue
                    kind, name = (m.group(1), m.group(2)) if m.lastindex == 2 else ("method", m.group(1))
                    if lang == "python" and (name.startswith("_") or line[0].isspace()):
                        continue
                    found.setdefault(f"{kind} {name}", rel)
    return found


def previous_release(root):
    """`v<from>` of the newest release in release/upgrade-path.toml. A patch
    release's `from` is its line (`0.1`): the newest earlier stable release
    of that line is the previous release."""
    with open(root / "release/upgrade-path.toml", "rb") as f:
        releases = tomllib.load(f)["release"]
    newest = releases[0]
    src = newest["from"]
    if src == "fresh":
        return None
    if re.fullmatch(r"\d+\.\d+", src):
        line = [r["version"] for r in releases[1:]
                if r["version"].startswith(src + ".") and "-" not in r["version"]]
        if not line:
            return None
        return f"v{line[0]}"
    return f"v{src}"


def checkout(root, tag, into):
    """The tag's `sdks/` (and buf.yaml) under `into`."""
    data = subprocess.run(
        ["git", "-C", str(root), "archive", "--format=tar", tag, "sdks", "buf.yaml"],
        check=True,
        capture_output=True,
    ).stdout
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(into, filter="data")


def removed(before_root, after_root, langs, unstable):
    """{lang: [(symbol, file)]} present before and absent after."""
    out = {}
    for lang in langs:
        before = symbols(before_root / "sdks" / lang, lang, unstable)
        after = symbols(after_root / "sdks" / lang, lang, unstable)
        gone = sorted((s, f) for s, f in before.items() if s not in after)
        if gone:
            out[lang] = gone
    return out


def check(root, tag):
    unstable = unstable_packages(root)
    with tempfile.TemporaryDirectory() as tmp:
        prev = pathlib.Path(tmp)
        checkout(root, tag, prev)
        gone = removed(prev, root, SUPPORTED, unstable)
    for lang, items in gone.items():
        for symbol, path in items:
            print(f"{lang}: {symbol} ({path} at {tag}) was removed", file=sys.stderr)
    return gone


def self_test():
    sources = {
        "typescript": ("packages/client/src/a.ts", "export class Client {}\nexport function connect() {}\n"),
        "python": ("src/loams/a.py", "class Client:\n    def call(self):\n        pass\ndef connect():\n    pass\n"),
        "go": ("client.go", "package loams\n\ntype Client struct{}\n\nfunc Connect() {}\n"),
        "rust": ("src/lib.rs", "pub struct Client;\npub fn connect() {}\n"),
        "cpp": ("include/loams/a.hpp", "namespace loams {\nclass Client {};\n}\n"),
        "csharp": ("src/Loams/A.cs", "public sealed class Client {\n    public void Connect() {}\n}\n"),
        "swift": ("Sources/Loams/A.swift", "public final class Client {}\npublic func connect() {}\n"),
    }
    planted = {
        "typescript": "function connect",
        "python": "def connect",
        "go": "func Connect",
        "rust": "fn connect",
        "cpp": "class Client",
        "csharp": "method Connect",
        "swift": "func connect",
    }
    # Each SDK with its planted removal.
    without = {
        "typescript": "export class Client {}\n",
        "python": "class Client:\n    def call(self):\n        pass\n",
        "go": "package loams\n\ntype Client struct{}\n",
        "rust": "pub struct Client;\n",
        "cpp": "namespace loams {\n}\n",
        "csharp": "public sealed class Client {\n}\n",
        "swift": "public final class Client {}\n",
    }
    with tempfile.TemporaryDirectory() as tmp:
        before, after = pathlib.Path(tmp, "before"), pathlib.Path(tmp, "after")
        for lang, (rel, text) in sources.items():
            for root in (before, after):
                path = root / "sdks" / lang / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
        assert removed(before, after, SUPPORTED, []) == {}, "an unchanged SDK passes"
        for lang, symbol in planted.items():
            rel, text = sources[lang]
            (after / "sdks" / lang / rel).write_text(without[lang])
            gone = removed(before, after, [lang], [])
            found = [s for s, _ in gone.get(lang, [])]
            assert found == [symbol], f"{lang}: the planted removal of {symbol} gave {found}"
            (after / "sdks" / lang / rel).write_text(text)
        # A symbol moved to another file is not a removal.
        (after / "sdks/rust/src/lib.rs").write_text("pub struct Client;\n")
        (after / "sdks/rust/src/connect.rs").write_text("pub fn connect() {}\n")
        assert removed(before, after, ["rust"], []) == {}, "a move is not a removal"
        # A removal in an unstable package's files is allowed.
        graph = before / "sdks/go/graph.go"
        graph.write_text("package loams\n\nfunc GraphQuery() {}\n")
        assert removed(before, after, ["go"], ["graph"]) == {}, "unstable packages may change"
        assert "go" in removed(before, after, ["go"], []), "and a stable one may not"
    assert unstable_packages(ROOT), "buf.yaml lists the unstable packages"
    print("removed_symbols: self-test passed")


def main(argv):
    if argv[1:] == ["--self-test"]:
        self_test()
        return 0
    tag = argv[argv.index("--against") + 1] if "--against" in argv else previous_release(ROOT)
    if tag is None:
        print("removed_symbols: no previous release keeps data; nothing to compare")
        return 0
    gone = check(ROOT, tag)
    if gone:
        print(
            f"removed_symbols: {sum(len(v) for v in gone.values())} public SDK symbols of {tag} are gone; "
            "SDKs are additive-only (design §54 §13.2)",
            file=sys.stderr,
        )
        return 1
    print(f"removed_symbols: every public symbol of {tag}'s supported SDKs is still there")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
