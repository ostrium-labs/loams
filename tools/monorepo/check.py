#!/usr/bin/env python3
"""Source-independent checks for monorepo boundaries and Nx configuration."""
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKIP = {".git", ".worktrees", ".nx", "node_modules", "target", "build", "dist", "lib", ".gradle", "__pycache__"}


def project_errors(root: Path, projects: dict[str, dict]) -> list[str]:
    errors = []
    names = {}
    for directory, config in projects.items():
        name = config["name"]
        if name in names:
            errors.append(f"Duplicate Nx project {name}: {names[name]}, {directory}")
        names[name] = directory
        for target, settings in config.get("targets", {}).items():
            cwd = settings.get("options", {}).get("cwd")
            if cwd is not None:
                destination = (root / cwd).resolve()
                if not destination.is_relative_to(root.resolve()) or not destination.is_dir():
                    errors.append(f"{name}:{target} has invalid cwd {cwd}")
    for config in projects.values():
        for dependency in config.get("implicitDependencies", []):
            if dependency not in names and not any(c in dependency for c in "*!?"):
                errors.append(f"{config['name']} depends on unknown project {dependency}")
    return errors


def discover(root: Path) -> dict[str, dict]:
    projects = {}
    for directory, folders, files in os.walk(root):
        folders[:] = [name for name in folders if name not in SKIP]
        path = Path(directory)
        config = None
        if "project.json" in files:
            config = json.loads((path / "project.json").read_text())
        elif "package.json" in files and (path == root / "web" or path.is_relative_to(root / "web")
                                         or path.is_relative_to(root / "sdks/typescript/packages")):
            package = json.loads((path / "package.json").read_text())
            config = {"name": package["name"]}
        if config is not None:
            projects[str(path.relative_to(root))] = config
    return projects


def main() -> int:
    errors = []
    projects = discover(ROOT)
    errors.extend(project_errors(ROOT, projects))
    required = {"loams-engine", "loams-contracts", "mobile-android", "mobile-ios",
                "mobile-mock", "mobile-contracts", "mobile-fixtures", "plugins", "plugins-dashboard-ui", "@loams/console"}
    missing = required - {p["name"] for p in projects.values()}
    if missing:
        errors.append("Missing projects: " + ", ".join(sorted(missing)))
    package = json.loads((ROOT / "package.json").read_text())
    if package.get("packageManager") != "pnpm@11.27.1" or package.get("devDependencies", {}).get("nx") != "23.2.1":
        errors.append("Root package manager or Nx pin changed unexpectedly")
    for obsolete in ["web/pnpm-lock.yaml", "web/pnpm-workspace.yaml", "plugins/package-lock.json"]:
        if (ROOT / obsolete).exists():
            errors.append("Obsolete competing workspace/lockfile: " + obsolete)
    for subtree in ["apps/mobile", "plugins"]:
        for directory, folders, files in os.walk(ROOT / subtree):
            if ".git" in folders or ".git" in files:
                errors.append("Nested Git metadata: " + str(Path(directory).relative_to(ROOT)))
            folders[:] = [name for name in folders if name not in SKIP]
    for notice in ["crates/loams-agentd/LICENSE", "crates/loams-agentd/NOTICE",
                   "crates/loams-agentd/THIRD_PARTY_NOTICES.md", "apps/mobile/LICENSE", "apps/mobile/NOTICE",
                   "plugins/LICENSE", "plugins/NOTICE"]:
        if not (ROOT / notice).is_file():
            errors.append("Missing scoped license/notice: " + notice)
    # The zeron fork moved into the root workspace as crates/loams-agentd* (D783).
    if (ROOT / "apps/desktop").exists():
        errors.append("apps/desktop must stay deleted: the daemon lives in crates/loams-agentd* (D783)")
    if errors:
        print("\n".join(errors))
        return 1
    print(f"Monorepo checks passed: {len(projects)} projects, scoped licenses, no sibling dependency.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
