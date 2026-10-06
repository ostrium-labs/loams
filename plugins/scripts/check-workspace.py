
"""Check migration/build invariants without installing JavaScript dependencies."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(path):
    return json.loads(path.read_text())


def check(condition, message):
    if not condition:
        raise SystemExit(message)


manifest = load(ROOT / "package.json")
check(not (ROOT / "package-lock.json").exists(), "Unexpected npm lockfile")
check(not any(key in manifest for key in ("overrides", "devEngines", "workspaces")), "npm-only root fields remain")
check(manifest["scripts"]["test"] == "vp test --config vitest.config.ts", "Tests need the explicit aggregate config")
check(manifest["devDependencies"]["vite-plus"] == "1.0.0", "Vite+ version changed")
check(manifest["devDependencies"]["vitest"] == "5.0.1", "Plugin Vitest version changed")
check(manifest["devDependencies"]["vite"] == "npm:@voidzero-dev/vite-plus-core@1.0.0", "Plugin Vite alias changed")
paths = sorted([*ROOT.glob("packages/*/package.json"), *ROOT.glob("apps/*/package.json")])
packages = {load(path)["name"]: path for path in paths}
projects = {name: load(path.parent / "project.json")["name"] for name, path in packages.items()}
check(len(set(projects.values())) == len(paths), "Duplicate Nx project names")
aggregate = load(ROOT / "project.json")
inputs = aggregate["targets"]["build"]["inputs"]
check("{workspaceRoot}/plugins/packages/**/*" in inputs and "{workspaceRoot}/plugins/apps/**/*" in inputs,
      "Aggregate inputs must include child sources explicitly")
outputs = set(aggregate["targets"]["build"]["outputs"])
references = {(ROOT / ref["path"]).resolve() for ref in load(ROOT / "tsconfig.json")["references"]}
import_pattern = re.compile(r"(?:from\s+|import\s*\(|import\s+)[\"'](@loams-plugins/[^\"']+)[\"']")
for name, path in packages.items():
    data = load(path)
    project = load(path.parent / "project.json")
    dependencies = {key: value for section in ("dependencies", "devDependencies", "peerDependencies")
                    for key, value in data.get(section, {}).items()}
    for dependency, version in dependencies.items():
        if dependency in packages:
            check(version == "workspace:*", f"{name}: non-workspace dependency {dependency}")
            check(projects[dependency] in project["implicitDependencies"], f"{name}: missing Nx edge to {dependency}")
    for source in (path.parent / "src").rglob("*.ts*"):
        for imported in import_pattern.findall(source.read_text()):
            dependency = "/".join(imported.split("/")[:2])
            if dependency in packages:
                check(dependency in dependencies, f"{source}: undeclared {dependency}")
    if path.parent.name == "dashboard-ui":
        check(path.parent.resolve() not in references, "Dashboard must not enter the composite build")
        continue
    check(path.parent.resolve() in references, f"{name}: absent from aggregate build")
    config = load(path.parent / "tsconfig.json")
    check(config["compilerOptions"]["tsBuildInfoFile"] == "tsconfig.tsbuildinfo", f"{name}: implicit build-info output")
    prefix = "{workspaceRoot}/plugins/" + str(path.parent.relative_to(ROOT)) + "/"
    check(prefix + "tsconfig.tsbuildinfo" in outputs, f"{name}: Nx does not cache build info")
    check(prefix + config["compilerOptions"]["outDir"] in outputs, f"{name}: Nx does not cache emissions")
    check(project["targets"]["build"]["dependsOn"] == [{"projects": ["plugins"], "target": "build"}],
          f"{name}: build must defer to the single aggregate compiler")
    for ref in config.get("references", []):
        dependency = load((path.parent / ref["path"]).resolve() / "package.json")["name"]
        check(dependency in dependencies, f"{name}: undeclared TypeScript reference to {dependency}")
for source in (ROOT / "proto").rglob("*.proto"):
    check(re.search(r"package\s+bi\.v1\s*;", source.read_text()), f"{source}: RPC namespace changed")
check(not any(path.name == ".git" for path in ROOT.rglob("*")), "Nested .git artifact")
print(f"Plugin workspace invariants pass: {len(paths)} packages/apps, {len(outputs)} aggregate build outputs")
