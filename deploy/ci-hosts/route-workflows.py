#!/usr/bin/env python3
"""Routes the pull-request and dev-push CI jobs to the local CI runners.

Rewrites, in the workflows listed below, every Linux `runs-on:` label into an
expression that picks `[self-hosted, loams-local]` when all of these hold, and
the original GitHub-hosted label otherwise:

- the repository variable CI_RUNNER is `local` (set it to `github` to fall back
  in one step; unset means GitHub-hosted);
- the event is a push to dev, or a pull request into dev whose branch is in this
  repository or whose author is an organisation member or owner. A pull request
  from anyone else's fork never reaches the laptops: it runs on GitHub-hosted
  runners, with the same check names, after a maintainer approves it.

Release workflows (release*.yml, sdk-release.yml, image publishing, signing),
pull requests into main (the release gate), pushes to main, dispatches and the
nightly keep GitHub-hosted runners. macOS and Windows jobs are untouched.

Swatinem/rust-cache steps get `if: runner.environment == 'github-hosted'`: on the
laptops sccache (RUSTC_WRAPPER, set by the runner) replaces it, instead of
moving gigabytes of target directory over home Wi-Fi. So do the "Free disk
space" steps: they delete the hosted image's preinstalled SDKs and
$AGENT_TOOLSDIRECTORY, which on the laptops is the tool cache the runners share.

Usage: route-workflows.py [--check]   (from the repository root; --check exits 1
when a workflow is not routed, without writing)
"""

import pathlib
import re
import sys

WORKFLOWS = [
    "byoc-static.yml",
    "ci.yml",
    "dco.yml",
    "desktop-electron.yml",
    "fabric.yml",
    "gateway.yml",
    "gt1.yml",
    "identity.yml",
    "kv-oracles.yml",
    "monorepo.yml",
    "nf1-candidate.yml",
    "nf1-pins.yml",
    "pg2-e2e.yml",
    "pg2.yml",
    "platform.yml",
    "studio-e2e.yml",
]

LOCAL = (
    "vars.CI_RUNNER == 'local' && ("
    "(github.event_name == 'push' && github.ref == 'refs/heads/dev') || "
    "(github.event_name == 'pull_request' && github.base_ref == 'dev' && ("
    "github.event.pull_request.head.repo.full_name == github.repository || "
    "github.event.pull_request.author_association == 'MEMBER' || "
    "github.event.pull_request.author_association == 'OWNER')))"
)
SELF_HOSTED = "fromJSON('[\"self-hosted\",\"loams-local\"]')"
MARK = "vars.CI_RUNNER == 'local'"
HOSTED_ONLY_IF = "runner.environment == 'github-hosted'"
HOSTED_ONLY_STEPS = re.compile(r"^(\s*)- (uses: Swatinem/rust-cache@|name: Free disk space)")

RUNS_ON = re.compile(r"^(\s*)runs-on:\s*(.+?)\s*$")
LINUX_LABEL = re.compile(r"^(ubuntu-[0-9.]+|ubuntu-latest)$")


def fallback(value: str):
    """The GitHub-hosted label as an expression, or None when not a Linux job."""
    if LINUX_LABEL.match(value):
        return f"'{value}'"
    m = re.fullmatch(r"\$\{\{\s*(.+?)\s*\}\}", value)
    if m and "ubuntu" in m.group(1) and "matrix." not in m.group(1):
        return f"({m.group(1)})"
    return None


def route(text: str) -> str:
    out = []
    for line in text.splitlines(keepends=True):
        m = RUNS_ON.match(line.rstrip("\n"))
        if m and MARK not in line:
            fb = fallback(m.group(2).split(" #")[0].strip())
            if fb:
                line = f"{m.group(1)}runs-on: ${{{{ ({LOCAL}) && {SELF_HOSTED} || {fb} }}}}\n"
        out.append(line)
    return guard_hosted_only("".join(out))


def guard_hosted_only(text: str) -> str:
    lines = text.splitlines(keepends=True)
    i = 0
    while i < len(lines):
        m = HOSTED_ONLY_STEPS.match(lines[i])
        if not m:
            i += 1
            continue
        key_indent = m.group(1) + "  "
        # The step's own keys: lines indented at key_indent until the next step or less.
        j = i + 1
        if_line = None
        while j < len(lines):
            ln = lines[j]
            if ln.strip() == "":
                j += 1
                continue
            indent = len(ln) - len(ln.lstrip(" "))
            if indent < len(key_indent) or ln.lstrip().startswith("- ") and indent < len(key_indent) + 1:
                break
            if indent == len(key_indent) and ln.lstrip().startswith("if:"):
                if_line = j
            j += 1
        if if_line is None:
            lines.insert(i + 1, f"{key_indent}if: {HOSTED_ONLY_IF}\n")
        elif HOSTED_ONLY_IF not in lines[if_line]:
            cond = lines[if_line].split("if:", 1)[1].strip()
            cond = re.sub(r"^\$\{\{\s*(.*?)\s*\}\}$", r"\1", cond)
            lines[if_line] = f"{key_indent}if: {HOSTED_ONLY_IF} && ({cond})\n"
        i += 1
    return "".join(lines)


def main() -> int:
    check = "--check" in sys.argv[1:]
    root = pathlib.Path(".github/workflows")
    stale = []
    for name in WORKFLOWS:
        path = root / name
        if not path.exists():
            continue
        old = path.read_text()
        new = route(old)
        if new != old:
            stale.append(name)
            if not check:
                path.write_text(new)
    if check and stale:
        print("not routed to the local CI runners (run deploy/ci-hosts/route-workflows.py):", ", ".join(stale))
        return 1
    if not check:
        print("routed:", ", ".join(stale) if stale else "nothing to change")
    return 0


if __name__ == "__main__":
    sys.exit(main())
