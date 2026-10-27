#!/usr/bin/env python3
"""The release train, release/upgrade-path.toml (design §54 §5, §11; UP1 Task 11).

    upgrade_path.py check [--root DIR]
        Every migration compiled into loams-migrate is listed exactly once,
        under the release that introduced it; `from` chains back to `fresh`;
        component pins match the pin files.
    upgrade_path.py notes VERSION [--root DIR]
        The release notes' "Upgrading" section (Markdown).
    upgrade_path.py manifest VERSION [--root DIR]
        The `upgrade` object of the release manifest (JSON).
    upgrade_path.py check-changelog [--root DIR]
        CHANGELOG.md's entry for the workspace version has an "Upgrading"
        section (a release pull request into main).
    upgrade_path.py --self-test
"""
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib

PATH_FILE = "release/upgrade-path.toml"
MIGRATIONS_DIR = "crates/loams-migrate/src/migrations"


class CheckError(Exception):
    """The release train disagrees with the code or the pins."""


def load(root):
    with open(root / PATH_FILE, "rb") as f:
        return tomllib.load(f)["release"]


def compiled_migrations(root):
    """Every `fn id(&self) -> &'static str { "<id>" }` under migrations/."""
    ids = []
    for path in sorted((root / MIGRATIONS_DIR).glob("m*.rs")):
        text = path.read_text()
        found = re.findall(r'fn id\(&self\) -> &\'static str \{\s*"([^"]+)"', text)
        ids.extend(found)
    return ids


def env_value(root, name):
    for line in (root / "deploy/release/.env.example").read_text().splitlines():
        if line.startswith(f"{name}="):
            return line.split("=", 1)[1].strip()
    return None


def pins(root):
    """The third-party pins as the pin files have them."""
    out = {}
    tikv = env_value(root, "TIKV_TAG")
    if tikv:
        out["pd"] = tikv
        out["tikv"] = tikv
    tidb = env_value(root, "TIDB_IMAGE")
    if tidb and ":" in tidb:
        out["tidb"] = tidb.rsplit(":", 1)[1]
    pg = root / "deploy/pins/loams-postgres-images.toml"
    if pg.exists():
        with open(pg, "rb") as f:
            storage = tomllib.load(f).get("images", {}).get("storage", {})
        if "digest" in storage:
            out["loams_postgres"] = storage["digest"]
    chdb = root / "fabric/crates/loams-chdb-sys/build.rs"
    if chdb.exists():
        m = re.search(r'const CHDB_VERSION: &str = "([^"]+)"', chdb.read_text())
        if m:
            out["chdb"] = m.group(1)
    cargo = root / "Cargo.toml"
    if cargo.exists():
        m = re.search(r'^grafeo = \{ version = "=([^"]+)"', cargo.read_text(), re.M)
        if m:
            out["grafeo"] = m.group(1)
    return out


def pin_version(value):
    """A pin's numeric version (`v8.5.8-loams.1` -> (8, 5, 8)); None for a digest."""
    m = re.match(r"v?(\d+(?:\.\d+)*)", value)
    return tuple(int(x) for x in m.group(1).split(".")) if m else None


def check_pin_policy(newest, previous):
    """Design §54 §9.2 (UP1 Task 21): no component goes down, and one that
    crosses a major needs a point-of-no-return migration in the release."""
    pnr = any(m.get("class") == "point_of_no_return" for m in newest.get("migration", []))
    for name, value in newest.get("components", {}).items():
        old = previous.get(name)
        a, b = pin_version(value), pin_version(old) if old else None
        if a is None or b is None:
            continue
        if a < b:
            raise CheckError(f"{newest['version']}: component {name} goes down from {old} to {value}")
        if a[0] > b[0] and not pnr:
            raise CheckError(
                f"{newest['version']}: component {name} crosses a major ({old} -> {value}): its files do not go "
                "back, so the release needs a point_of_no_return migration entry"
            )


def pins_at(root, ref):
    """The pins of the pin files at git `ref`."""
    files = [".env.example", "release/sqldb-images.toml", "deploy/pins/loams-postgres-images.toml",
             "fabric/crates/loams-chdb-sys/build.rs", "Cargo.toml"]
    files = ["deploy/release/.env.example" if f == ".env.example" else f for f in files]
    with tempfile.TemporaryDirectory() as tmp:
        for f in files:
            out = subprocess.run(["git", "-C", str(root), "show", f"{ref}:{f}"], capture_output=True)
            if out.returncode == 0:
                target = pathlib.Path(tmp) / f
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(out.stdout)
        return pins(pathlib.Path(tmp))


def pins_changed(root, ref):
    """{component: (at ref, now)} for every pin that differs from `ref`."""
    before, now = pins_at(root, ref), pins(root)
    return {k: (before.get(k), now.get(k)) for k in sorted(set(before) | set(now)) if before.get(k) != now.get(k)}


def check(root):
    releases = load(root)
    versions = [r["version"] for r in releases]
    if len(set(versions)) != len(versions):
        raise CheckError("a release is listed twice")
    listed = {}
    for r in releases:
        for m in r.get("migration", []):
            if m["id"] in listed:
                raise CheckError(f"migration {m['id']} is listed under {listed[m['id']]} and {r['version']}")
            listed[m["id"]] = r["version"]
            for key in ("store", "class", "summary"):
                if key not in m:
                    raise CheckError(f"migration {m['id']} has no {key}")
            if m["class"] not in ("reversible", "point_of_no_return"):
                raise CheckError(f"migration {m['id']}: class {m['class']!r}")
    compiled = compiled_migrations(root)
    for mid in compiled:
        if mid not in listed:
            raise CheckError(f"migration {mid} is compiled into loams-migrate but not listed")
    for mid in listed:
        if mid not in compiled:
            raise CheckError(f"migration {mid} is listed but not compiled into loams-migrate")
    # Each release chains back to `fresh` through releases of the train (a
    # line `0.1` resolves to the newest earlier release of that line).
    by_version = {r["version"]: r for r in releases}
    for r in releases:
        seen = set()
        at = r
        while True:
            if at["version"] in seen:
                raise CheckError(f"{r['version']}: the chain loops at {at['version']}")
            seen.add(at["version"])
            src = at["from"]
            if src == "fresh":
                break
            if src in by_version:
                at = by_version[src]
                continue
            if re.fullmatch(r"\d+\.\d+", src):
                older = [v for v in versions if v.startswith(src + ".") and v != at["version"]
                         and "-" not in v and v < at["version"]]
                if not older:
                    raise CheckError(f"{at['version']}: no earlier release of line {src}")
                at = by_version[sorted(older)[-1]]
                continue
            raise CheckError(f"{at['version']} upgrades from {src}, which is not in the train")
    # The newest release's pins equal the pin files, and follow the policy.
    newest = releases[0]
    check_pin_policy(newest, previous_components(root, newest["version"]))
    found = pins(root)
    for name, value in newest.get("components", {}).items():
        if name in found and found[name] != value:
            raise CheckError(
                f"{newest['version']}: component {name} is {value} here but {found[name]} in the pin files"
            )
    return compiled


def release_of(root, version):
    for r in load(root):
        if r["version"] == version:
            return r
    raise CheckError(f"{version} is not in {PATH_FILE}")


def previous_components(root, version):
    releases = load(root)
    for i, r in enumerate(releases):
        if r["version"] == version:
            for older in releases[i + 1:]:
                if older.get("components"):
                    return older["components"]
    return {}


def notes(root, version):
    r = release_of(root, version)
    lines = ["### Upgrading", ""]
    if r["from"] == "fresh":
        lines.append(f"{version} keeps no data from an earlier release: start a fresh stack.")
        return "\n".join(lines) + "\n"
    if re.fullmatch(r"\d+\.\d+", r["from"]):
        lines.append(f"From any earlier **{r['from']}.x** release, in one step: `loams upgrade --to {version}`.")
    else:
        lines.append(f"From **{r['from']}** (one step at a time): `loams upgrade --to {version}`.")
    lines.append("")
    lines.append("It takes a snapshot first, and `loams upgrade rollback` restores it.")
    lines.append("")
    migrations = r.get("migration", [])
    if migrations:
        lines.append("| Migration | Store | Class | What it does |")
        lines.append("|---|---|---|---|")
        for m in migrations:
            klass = "Reversible" if m["class"] == "reversible" else "**Point of no return**"
            lines.append(f"| `{m['id']}` | {m['store']} | {klass} | {m['summary']} |")
    else:
        lines.append("No migrations.")
    lines.append("")
    before = previous_components(root, version)
    bumps = [f"{k} {before[k]} → {v}" for k, v in sorted(r.get("components", {}).items())
             if k in before and before[k] != v]
    lines.append("Third-party bumps: " + (", ".join(bumps) if bumps else "none") + ".")
    if r.get("free_factor"):
        lines.append(f"Free space needed: {r['free_factor']}× the snapshot estimate (`loams upgrade check` shows it).")
    if r.get("downtime_seconds"):
        lines.append(f"Expected downtime: about {round(r['downtime_seconds'] / 60)} minutes, excluding image pulls.")
    if any(m["class"] == "point_of_no_return" for m in migrations):
        lines.append("")
        lines.append("This release has a point of no return: after it, only the snapshot takes the stack back.")
    return "\n".join(lines) + "\n"


def manifest(root, version):
    r = release_of(root, version)
    return {
        "from": r["from"],
        "migrations": [
            {"id": m["id"], "store": m["store"], "class": m["class"], "summary": m["summary"]}
            for m in r.get("migration", [])
        ],
        "components": r.get("components", {}),
        "env_changes": r.get("env", []),
        "free_factor": r.get("free_factor"),
        "downtime_seconds": r.get("downtime_seconds"),
    }


def workspace_version(root):
    m = re.search(r'^version = "([^"]+)"', (root / "Cargo.toml").read_text(), re.M)
    return m.group(1) if m else None


def check_changelog(root):
    version = workspace_version(root)
    # Releases that keep no data (`from = "fresh"`, preview.2 and before,
    # D923) have no upgrade to describe.
    entry = next((r for r in load(root) if r["version"] == version), None)
    if entry is not None and entry["from"] == "fresh":
        return
    text = (root / "CHANGELOG.md").read_text()
    m = re.search(rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.M | re.S)
    if not m:
        raise CheckError(f"CHANGELOG.md has no entry for {version}")
    if not re.search(r"^### Upgrading\s*$", m.group(1), re.M):
        raise CheckError(
            f"CHANGELOG.md's entry for {version} has no '### Upgrading' section "
            f"(scripts/release/upgrade-notes.sh {version} renders it)"
        )


def self_test():
    real = pathlib.Path(__file__).resolve().parents[2]

    def tree(path_toml, migrations, env=None, changelog=None, version="0.1.0-preview.3"):
        d = pathlib.Path(tempfile.mkdtemp())
        (d / "release").mkdir()
        (d / PATH_FILE).write_text(path_toml)
        mdir = d / MIGRATIONS_DIR
        mdir.mkdir(parents=True)
        for i, mid in enumerate(migrations):
            (mdir / f"m{i:04d}.rs").write_text(
                f'impl X {{ fn id(&self) -> &\'static str {{\n        "{mid}"\n    }} }}\n'
            )
        (d / "deploy/release").mkdir(parents=True)
        (d / "deploy/release/.env.example").write_text(env or "TIKV_TAG=v8.5.8\n")
        (d / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
        (d / "CHANGELOG.md").write_text(changelog or "")
        return d

    good = """
[[release]]
version = "0.1.0-preview.3"
from = "0.1.0-preview.2"
[[release.migration]]
id = "0000-a"
store = "meta"
class = "reversible"
summary = "A"
[release.components]
tikv = "v8.5.8"

[[release]]
version = "0.1.0-preview.2"
from = "fresh"
"""
    assert check(tree(good, ["0000-a"])) == ["0000-a"]

    def refuses(d, needle):
        try:
            check(d)
        except CheckError as e:
            assert needle in str(e), f"{needle!r} not in {e}"
            return
        raise AssertionError(f"accepted; expected {needle!r}")

    # An unlisted migration, a broken chain, a pin mismatch.
    refuses(tree(good, ["0000-a", "0001-b"]), "0001-b is compiled into loams-migrate but not listed")
    refuses(tree(good.replace('from = "fresh"', 'from = "0.1.0-preview.1"'), ["0000-a"]),
            "not in the train")
    refuses(tree(good, ["0000-a"], env="TIKV_TAG=v8.5.9\n"), "component tikv is v8.5.8 here but v8.5.9")
    refuses(tree(good.replace("0000-a", "0000-a", 1) + """
[[release.migration]]
id = "0000-a"
store = "meta"
class = "reversible"
summary = "again"
""", ["0000-a"]), "listed under")
    # Patch lines chain to the newest earlier patch.
    line = """
[[release]]
version = "0.1.2"
from = "0.1"
[[release]]
version = "0.1.1"
from = "0.1"
[[release]]
version = "0.1.0"
from = "fresh"
"""
    check(tree(line, []))
    # The notes golden: the real 0.1.0 entry.
    golden = (real / "scripts/release/testdata/upgrade-notes-0.1.0.md").read_text()
    rendered = notes(real, "0.1.0")
    assert rendered == golden, f"notes differ from the golden:\n{rendered}"
    # The manifest's upgrade object matches the path file.
    m = manifest(real, "0.1.0")
    r = release_of(real, "0.1.0")
    assert m["from"] == r["from"]
    assert [x["id"] for x in m["migrations"]] == [x["id"] for x in r.get("migration", [])]
    assert m["components"] == r.get("components", {})
    # A CHANGELOG entry without the section fails; with it, passes.
    without = tree(good, ["0000-a"], changelog="## [0.1.0-preview.3] - unreleased\n\n### Added\n- x\n")
    try:
        check_changelog(without)
        raise AssertionError("a CHANGELOG entry without Upgrading passed")
    except CheckError as e:
        assert "Upgrading" in str(e)
    # A release that keeps no data needs no section.
    fresh = tree(good, ["0000-a"], changelog="## [0.1.0-preview.2] - unreleased\n\n### Added\n- x\n",
                 version="0.1.0-preview.2")
    check_changelog(fresh)
    with_section = tree(good, ["0000-a"],
                        changelog="## [0.1.0-preview.3] - unreleased\n\n### Upgrading\n\nx\n\n## [0.1.0-preview.2]\n")
    check_changelog(with_section)
    # The pin policy (Task 21): pin_downgrade_fails_check and
    # major_bump_requires_point_of_no_return_entry.
    old = {"version": "0.1.0-preview.3", "components": {"tikv": "v8.5.8", "tidb": "v8.5.8-loams.1"}}
    down = {"version": "0.1.0-preview.4", "components": {"tikv": "v8.5.7"}}
    try:
        check_pin_policy(down, old["components"])
        raise AssertionError("a pin downgrade passed")
    except CheckError as e:
        assert "goes down from v8.5.8 to v8.5.7" in str(e), e
    major = {"version": "0.1.0-preview.4", "components": {"tidb": "v9.0.0-loams.1"}}
    try:
        check_pin_policy(major, old["components"])
        raise AssertionError("a major bump without a point_of_no_return entry passed")
    except CheckError as e:
        assert "crosses a major" in str(e), e
    major["migration"] = [{"id": "0002-tidb-9", "class": "point_of_no_return"}]
    check_pin_policy(major, old["components"])
    check_pin_policy({"version": "x", "components": {"tikv": "v8.5.9", "loams_postgres": "sha256:ab"}},
                     {"tikv": "v8.5.8", "loams_postgres": "sha256:cd"})
    # pin_change_without_gate_label_fails reads this: the pins that changed against a ref.
    assert pins_at(real, "HEAD").get("tikv"), "the pins are read from a git ref"
    print("upgrade_path: self-test passed")


def main(argv):
    if argv[1:] == ["--self-test"]:
        self_test()
        return 0
    root = pathlib.Path(__file__).resolve().parents[2]
    if "--root" in argv:
        i = argv.index("--root")
        root = pathlib.Path(argv[i + 1])
        argv = argv[:i] + argv[i + 2:]
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        cmd = argv[1]
        if cmd == "check":
            compiled = check(root)
            print(f"upgrade-path: ok ({len(compiled)} compiled migrations listed)")
        elif cmd == "pins-changed" and len(argv) == 3:
            for name, (old, new) in pins_changed(root, argv[2]).items():
                print(f"{name}: {old} -> {new}")
        elif cmd == "notes" and len(argv) == 3:
            sys.stdout.write(notes(root, argv[2]))
        elif cmd == "manifest" and len(argv) == 3:
            print(json.dumps(manifest(root, argv[2]), indent=2))
        elif cmd == "check-changelog":
            check_changelog(root)
            print("CHANGELOG: the release's entry has its Upgrading section")
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except CheckError as e:
        print(f"upgrade-path: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
