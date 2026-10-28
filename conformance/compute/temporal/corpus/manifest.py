#!/usr/bin/env python3
"""The corpus manifest (DX0 Task 4): build it after a generation, check it on
every pull request.

  manifest.py build  --server-version V --server-image I --pg-major M [--date D] [--languages a,b]
  manifest.py check                      # corpus_manifest_complete
  manifest.py stable <dir-a> <dir-b>     # corpus_regenerates_stably

`build` lists every history under histories/<lang>/ with its scenario, run,
language, SDK version, server version and image, Loams Postgres major (or
`start-dev` for the CLI's dev server), generation date, phase and the
SHA-256 of its .json and .binpb. `check` fails when a file is not listed, a
listed file is missing or its hash differs, a .json has no .binpb, a
scenario of scenarios.json is missing in a language (Go-only ones in Go
and Java only), or the workers' scenario lists differ from scenarios.json. `stable`
compares two generations after normalize.py's masking. Stdlib only.

Go and TypeScript are always required. Every other language (`dotnet`, `java`, `ruby`, `php`) is
checked once `histories/<lang>/` has files, so its worker can land before its
committed corpus; `build --languages dotnet` (or `java`) rebuilds only that language's
entries and keeps the others as they are.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
HIST = HERE / "histories"
MANIFEST = HERE / "manifest.json"
SDK = {"go": "go.temporal.io/sdk", "ts": "@temporalio/worker", "python": "temporalio", "dotnet": "Temporalio", "java": "io.temporal:temporal-sdk", "ruby": "temporalio", "php": "temporal/sdk"}
# Languages whose histories every corpus must have. Another language (an SDK
# leg added later, D2137) is required once histories/<lang>/ has a file:
# until its first generation against the oracle, only its worker's scenario
# list is checked.
REQUIRED = ("go", "ts")
RUN = re.compile(r"^(?P<scenario>[a-z0-9_]+?)(\.run(?P<run>[0-9]+))?\.json$")


def scenarios() -> list[dict]:
    return json.loads((HERE / "scenarios.json").read_text())["scenarios"]


def sdk_versions() -> dict[str, str]:
    gomod = (HERE / "workers/go/go.mod").read_text()
    go = re.search(r"go\.temporal\.io/sdk v([0-9.]+)", gomod)
    pkg = json.loads((HERE / "workers/ts/package.json").read_text())
    py = re.search(r'"temporalio==([0-9.]+)"', (HERE / "workers/python/pyproject.toml").read_text())
    dotnet = re.search(r'Include="Temporalio" Version="([0-9.]+)"', (HERE / "workers/dotnet/Corpus.csproj").read_text())
    java = re.search(r"<temporal\.sdk\.version>([0-9.]+)</temporal\.sdk\.version>",
                     (HERE / "workers/java/pom.xml").read_text())
    rb = re.search(r"gem 'temporalio', '([0-9.]+)'", (HERE / "workers/ruby/Gemfile").read_text())
    php = json.loads((HERE / "workers/php/composer.json").read_text())["require"]["temporal/sdk"]
    return {
        "go": go.group(1) if go else "?",
        "ts": pkg["dependencies"]["@temporalio/worker"],
        "python": py.group(1) if py else "?",
        "dotnet": dotnet.group(1) if dotnet else "?",
        "java": java.group(1) if java else "?",
        "ruby": rb.group(1) if rb else "?",
        "php": php,
    }


def languages() -> list[str]:
    """REQUIRED, then every other language with a history directory that has files."""
    extra = sorted(
        d.name for d in HIST.iterdir()
        if d.is_dir() and d.name not in REQUIRED and any(d.glob("*.json"))
    ) if HIST.is_dir() else []
    return [*REQUIRED, *extra]


def pin(key: str) -> str:
    """A value of conformance/compute/pins.env."""
    for line in (HERE.parents[1] / "pins.env").read_text().splitlines():
        if line.startswith(f"{key}="):
            return line.split("=", 1)[1].strip()
    return "?"


def sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def histories() -> list[tuple[str, Path]]:
    out = []
    for lang in languages():
        for p in sorted((HIST / lang).glob("*.json")):
            out.append((lang, p))
    return out


def build(server_version: str, server_image: str, pg_major: str, date: str,
          only: list[str] | None = None) -> None:
    phase = {s["name"]: s["phase"] for s in scenarios()}
    sdks = sdk_versions()
    entries = []
    if only and MANIFEST.exists():
        entries = [e for e in json.loads(MANIFEST.read_text())["histories"] if e["language"] not in only]
    for lang, p in histories():
        if only and lang not in only:
            continue
        m = RUN.match(p.name)
        if not m or m["scenario"] not in phase:
            raise SystemExit(f"manifest: {p} is not a known scenario")
        bin_ = p.with_suffix(".binpb")
        entries.append({
            "file": str(p.relative_to(HERE)),
            "binpb": str(bin_.relative_to(HERE)),
            "scenario": m["scenario"],
            "run": int(m["run"] or 1),
            "language": lang,
            "sdk": f"{SDK[lang]}@{sdks[lang]}",
            "server_version": server_version,
            "server_image": server_image,
            "loams_postgres_major": pg_major,
            "generated": date,
            "phase": phase[m["scenario"]],
            "sha256_json": sha(p),
            "sha256_binpb": sha(bin_),
        })
    order = {lang: i for i, lang in enumerate(SDK)}
    entries.sort(key=lambda e: (order.get(e["language"], len(order)), e["file"]))
    doc = {
        "//": "Generated by manifest.py build after generate.sh; checked by manifest.py check (corpus_manifest_complete). Do not edit by hand.",
        "histories": sorted(entries, key=lambda e: (e["language"] not in REQUIRED, REQUIRED.index(e["language"]) if e["language"] in REQUIRED else 0, e["language"], e["file"])),
    }
    MANIFEST.write_text(json.dumps(doc, indent=2) + "\n")
    print(f"manifest: {len(entries)} histories")


def worker_lists() -> dict[str, list[str]]:
    go = re.findall(r'\{"([a-z0-9_]+)", DX[1-4], \w+, (?:true|false)\}',
                    (HERE / "workers/go/scenarios/scenarios.go").read_text())
    ts = re.findall(r"\['([a-z0-9_]+)', 'DX[1-4]'\]", (HERE / "workers/ts/src/generate.ts").read_text())
    py = re.findall(r'^    \("([a-z0-9_]+)", "DX[1-4]"\),$',
                    (HERE / "workers/python/corpus_py/scenarios.py").read_text(), re.M)
    dotnet = re.findall(r'\("([a-z0-9_]+)", "DX[1-4]"\)', (HERE / "workers/dotnet/Scenarios.cs").read_text())
    java = re.findall(r'new Scenario\("([a-z0-9_]+)", "DX[1-4]"\)',
                      (HERE / "workers/java/src/main/java/dev/loams/conformance/temporal/corpus/Scenarios.java").read_text())
    rb = re.findall(r"^    \['([a-z0-9_]+)', 'DX[1-4]'\],?$",
                    (HERE / "workers/ruby/lib/scenarios.rb").read_text(), re.M)
    php = re.findall(r"^        \['([a-z0-9_]+)', 'DX[1-4]'\],$",
                     (HERE / "workers/php/src/Scenarios.php").read_text(), re.M)
    return {"go": go, "ts": ts, "python": py, "dotnet": dotnet, "java": java, "ruby": rb, "php": php}


def check() -> list[str]:
    problems = []
    want = scenarios()
    lists = worker_lists()
    if lists["go"] != [s["name"] for s in want]:
        problems.append("workers/go/scenarios/scenarios.go does not list scenarios.json's scenarios in order")
    if lists["ts"] != [s["name"] for s in want if not s["go_only"]]:
        problems.append("workers/ts/src/generate.ts does not list scenarios.json's non-Go-only scenarios in order")
    if lists["python"] != [s["name"] for s in want if not s["go_only"]]:
        problems.append("workers/python/corpus_py/scenarios.py does not list scenarios.json's non-Go-only scenarios in order")
    # Java runs every scenario, the Go-only ones included.
    if lists["java"] != [s["name"] for s in want]:
        problems.append("workers/java's Scenarios.java does not list scenarios.json's scenarios in order")
    sdks = sdk_versions()
    if sdks["java"] != pin("TEMPORAL_SDK_JAVA"):
        problems.append(f"workers/java pins temporal-sdk {sdks['java']}, pins.env TEMPORAL_SDK_JAVA {pin('TEMPORAL_SDK_JAVA')}")
    if sdks["python"] != pin("TEMPORAL_SDK_PY"):
        problems.append(f"workers/python pins temporalio {sdks['python']}, pins.env TEMPORAL_SDK_PY {pin('TEMPORAL_SDK_PY')}")
    if lists["dotnet"] != [s["name"] for s in want if not s["go_only"]]:
        problems.append("workers/dotnet/Scenarios.cs does not list scenarios.json's non-Go-only scenarios in order")
    if sdks["dotnet"] != pin("TEMPORAL_SDK_DOTNET"):
        problems.append(f"workers/dotnet pins Temporalio {sdks['dotnet']}, pins.env TEMPORAL_SDK_DOTNET {pin('TEMPORAL_SDK_DOTNET')}")
    if lists["ruby"] != [s["name"] for s in want if not s["go_only"]]:
        problems.append("workers/ruby/lib/scenarios.rb does not list scenarios.json's non-Go-only scenarios in order")
    if sdks["ruby"] != pin("TEMPORAL_SDK_RUBY"):
        problems.append(f"workers/ruby pins temporalio {sdks['ruby']}, pins.env TEMPORAL_SDK_RUBY {pin('TEMPORAL_SDK_RUBY')}")
    if lists["php"] != [s["name"] for s in want if not s["go_only"]]:
        problems.append("workers/php/src/Scenarios.php does not list scenarios.json's non-Go-only scenarios in order")
    if sdks["php"] != pin("TEMPORAL_SDK_PHP"):
        problems.append(f"workers/php pins temporal/sdk {sdks['php']}, pins.env TEMPORAL_SDK_PHP {pin('TEMPORAL_SDK_PHP')}")
    if not MANIFEST.exists():
        return problems + ["manifest.json is missing (run generate.sh)"]
    doc = json.loads(MANIFEST.read_text())
    listed = {e["file"]: e for e in doc["histories"]}
    for lang, p in histories():
        rel = str(p.relative_to(HERE))
        if rel not in listed:
            problems.append(f"{rel} is not in manifest.json")
    for rel, e in listed.items():
        p, b = HERE / rel, HERE / e["binpb"]
        if not p.exists():
            problems.append(f"{rel} is listed but missing")
            continue
        if not b.exists():
            problems.append(f"{e['binpb']} is missing")
        elif sha(b) != e["sha256_binpb"]:
            problems.append(f"{e['binpb']}: hash differs from the manifest")
        if sha(p) != e["sha256_json"]:
            problems.append(f"{rel}: hash differs from the manifest")
        for k in ("scenario", "language", "sdk", "server_version", "server_image", "loams_postgres_major", "generated", "phase"):
            if not e.get(k):
                problems.append(f"{rel}: no {k}")
    have = {(e["language"], e["scenario"]) for e in listed.values()}
    langs = languages()
    for s in want:
        # Go and Java have every scenario; the others lack the Go-only ones.
        for lang in (lang for lang in langs if lang in ("go", "java") or not s["go_only"]):
            if (lang, s["name"]) not in have:
                problems.append(f"scenario {s['name']} has no {lang} history")
    for b in sorted(HIST.glob("*/*.binpb")):
        if not b.with_suffix(".json").exists():
            problems.append(f"{b.relative_to(HERE)} has no .json")
    return problems


def stable(a: Path, b: Path) -> list[str]:
    sys.path.insert(0, str(HERE))
    import normalize  # noqa: PLC0415

    problems = []
    for lang in sorted({*REQUIRED, *(d.name for d in a.iterdir() if d.is_dir())}):
        fa = {p.name for p in (a / lang).glob("*.json")}
        fb = {p.name for p in (b / lang).glob("*.json")}
        if fa != fb:
            problems.append(f"{lang}: the generations have different files: {sorted(fa ^ fb)}")
        for name in sorted(fa & fb):
            x, y = normalize.load(a / lang / name), normalize.load(b / lang / name)
            if name.split(".")[0] in normalize.ORDER_FREE:
                x, y = normalize.activity_order_free(x), normalize.activity_order_free(y)
            for d in normalize.compare(x, y):
                problems.append(f"{lang}/{name}: {d}")
    return problems


def main(argv: list[str]) -> int:
    if argv[:1] == ["build"]:
        args = dict(zip(argv[1::2], argv[2::2]))
        only = args["--languages"].split(",") if args.get("--languages") else None
        build(args["--server-version"], args["--server-image"], args["--pg-major"],
              args.get("--date", dt.date.today().isoformat()), only)
        return 0
    if argv == ["check"]:
        problems = check()
    elif argv[:1] == ["stable"] and len(argv) == 3:
        problems = stable(Path(argv[1]), Path(argv[2]))
    else:
        print(__doc__, file=sys.stderr)
        return 2
    for p in problems:
        print(f"manifest: {p}")
    print(f"manifest: {argv[0]} {'FAIL' if problems else 'ok'}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
