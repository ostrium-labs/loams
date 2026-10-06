#!/usr/bin/env python3
"""Offline monorepo-only import checks; --source optionally audits an original checkout."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent
NATIVE = ROOT / "native"


def require(condition, message):
    if not condition:
        raise SystemExit(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=None,
                        help="Optional original checkout: verify tracked file hashes (never required by default)")
    args = parser.parse_args()
    manifests = [NATIVE / "Cargo.toml", *NATIVE.glob("crates/*/Cargo.toml"), *NATIVE.glob("apps/*/Cargo.toml")]
    workspace = tomllib.loads(manifests[0].read_text())
    require(workspace["workspace"]["package"]["publish"] is False, "Desktop publishing must remain disabled")
    for path in manifests:
        manifest = tomllib.loads(path.read_text())
        package = manifest.get("package")
        if package:
            require(package["name"] == "loams-desktop" or package["name"].startswith("loams-desktop-"), f"Unexpected crate identity: {path}")
            require(package.get("publish") == {"workspace": True} or package.get("publish") is False, f"Publishable crate: {path}")
        for line in path.read_text().splitlines():
            if "git =" in line:
                require(re.search(r'rev\s*=\s*"[0-9a-f]{40}"', line), f"Unpinned git dependency: {path}: {line}")
    for member in workspace["workspace"]["members"]:
        require((NATIVE / member / "Cargo.toml").is_file(), f"Missing member: {member}")
    lock = tomllib.loads((NATIVE / "Cargo.lock").read_text())
    for package in lock["package"]:
        if "source" not in package:
            require(package["name"].startswith("loams-desktop"), f"Unexpected local lock identity: {package['name']}")
        elif package["source"].startswith("git+"):
            require(re.search(r"#([0-9a-f]{40})$", package["source"]), f"Unresolved git revision: {package['name']}")
    for forbidden in [".git", ".github", "apps/ios", "crates/mobile", "crates/client", "crates/text", "scripts/ios", "scripts/android"]:
        require(not (NATIVE / forbidden).exists(), f"Unexpected inherited subtree: {forbidden}")
    require(not list(NATIVE.glob("**/project.json")), "Nested Nx project discovery is not allowed")
    desktop = (NATIVE / "dist/loams-desktop.desktop").read_text()
    for line in ["Name=Loams Desktop", "Exec=loams-desktop %u", "TryExec=loams-desktop", "StartupWMClass=dev.loams.desktop", "MimeType=x-scheme-handler/loams;"]:
        require(line in desktop, f"Inconsistent Linux launcher: {line}")
    import plistlib
    plist = plistlib.loads((NATIVE / "dist/macos/Info.plist").read_bytes())
    require(plist["CFBundleExecutable"] == "loams-desktop" and plist["CFBundleIdentifier"] == "dev.loams.desktop", "Inconsistent macOS identity")
    require(plist["CFBundleURLTypes"][0]["CFBundleURLSchemes"] == ["loams"], "Unexpected macOS deep link scheme")
    installer = (NATIVE / "dist/windows/loams-desktop.iss").read_text()
    require("93DB7E9E-5B92-5E45-99A1-105C32A995B8" in installer, "Windows must not reuse upstream AppId")
    require('Software\\Classes\\loams"' in installer, "Inconsistent Windows URL handler")
    require("AppUpdatesURL=" not in installer and 'Source: "{#PackageDir}\\loams-desktop-update.json"' not in installer, "Windows updater must remain unconfigured")
    project = json.loads((ROOT / "project.json").read_text())
    require(project["name"] == "loams-desktop", "Incorrect Nx project name")
    for name, target in project["targets"].items():
        require(target["executor"] == "nx:run-commands", f"Unexpected executor: {name}")
        require(target["options"]["cwd"] in {"apps/desktop", "apps/desktop/native"}, f"Unscoped cwd: {name}")
        require(not re.search(r"publish|deploy|release", name), f"Release target is forbidden: {name}")
    provenance = json.loads((ROOT / "import-provenance.json").read_text())
    if args.source:
        source = args.source.resolve()
        require(source.is_dir(), f"Optional source checkout does not exist: {source}")
        tracked = subprocess.check_output(["git", "-C", str(source), "ls-files", "-z"]).decode().split("\0")
        require(set(filter(None, tracked)) == set(provenance["source_sha256"]), "Source tracked file set changed")
        for relative, digest in provenance["source_sha256"].items():
            require(hashlib.sha256((source / relative).read_bytes()).hexdigest() == digest, f"Source file changed: {relative}")
    for relative, digest in provenance["source_sha256"].items():
        # Copyright and license texts stay byte-identical despite directory renames.
        if "license" not in Path(relative).name.lower() and "notice" not in Path(relative).name.lower():
            continue
        mapped = relative.replace("crates/loams-brand/", "crates/loams-desktop-brand/").replace("crates/loams-link/", "crates/loams-desktop-link/")
        path = NATIVE / mapped
        if path.is_file():
            require(hashlib.sha256(path.read_bytes()).hexdigest() == digest, f"Legal text was modified: {mapped}")
    print(f"Desktop import verified: {len(manifests) - 1} private crates; standalone lock; pinned git dependencies; scoped packaging/Nx; legal texts intact")
    if args.source:
        print(f"Source preserved: {len(provenance['source_sha256'])} tracked files unchanged")
    else:
        print("Verified from monorepo files only; no sibling source checkout required")


if __name__ == "__main__":
    main()
