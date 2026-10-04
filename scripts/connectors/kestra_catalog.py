#!/usr/bin/env python3
"""Check the Kestra column of design §33 Appendix A against the plugin list.

The Kestra half of CN1 plan Task 2: "kestra_catalog.py checks Kestra cells
against the plugin repository list." Kestra ships each connector as its own
`plugin-*` repository under `github.com/kestra-io`, so the repository list is
the catalog — the design doc read 193 of them on 2026-10-01 (§33 §13) and the
appendix's Kestra column was filled from that read.

Usage:
    kestra_catalog.py [--repos FILE | --from-github] [--docs PATH] [--check]
                      [--strict] [--json PATH]

`--repos` is a newline-separated list of repository names (what
`gh repo list kestra-io --json name --jq '.[].name'` writes) and
`$LOAMS_KESTRA_REPOS` names the same file. `--from-github` runs that command
itself when `gh` is present. Without either the check skips and exits 0: the
list is fetched over the network, so like `camel_catalog.py` this runs weekly
and on demand, never on every pull request (CN1 plan Task 2).

Severities match `camel_catalog.py`: a cell naming a `plugin-*` repository that
does not exist is an error and `--check` fails; everything else is a warning,
promoted by `--strict`.
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent))

import appendix  # noqa: E402  (the sibling module needs the path first)
from appendix import DESIGN_DOC, NO, ROOT, AppendixError  # noqa: E402

ORG = "kestra-io"
ENV_REPOS = "LOAMS_KESTRA_REPOS"
# `gh` lists the organisation's repositories; `--json name` keeps the output a
# plain newline-separated list the script can read either way.
GH_COMMAND = ["gh", "repo", "list", ORG, "--limit", "1000", "--json", "name",
              "--jq", ".[].name"]
# Kestra is a companion, not one of Loams's runtimes (design §33 D354, §5: "not a
# runtime: its scheduler, retries and flow state duplicate Resonate (D210)"), and
# the Loams plugin for it is deferred with Java (Q356). So a cell that says
# `core (…)` or `(itself)` names no plugin repository and is exempt: there is
# nothing to look up and nothing to fail on.
EXEMPT_NAMES = ("core", "itself")
GH_TIMEOUT = 60


class RepoListError(Exception):
    """The plugin repository list is not one this script can read."""


def read_repos(path):
    """A newline-separated list of `plugin-*` repository names."""
    path = Path(path)
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise RepoListError(f"{path}: {error}") from error
    repos = {line.strip() for line in lines if line.strip() and not line.startswith("#")}
    if not repos:
        raise RepoListError(f"{path} lists no repositories")
    return repos, "file"


def repos_from_github():
    """The list from `gh`, or a skip: never a failure on a missing network."""
    if not shutil.which(GH_COMMAND[0]):
        return None, ("gh is not installed; install the GitHub CLI or pass --repos "
                      "<file> to check the Kestra column")
    try:
        completed = subprocess.run(GH_COMMAND, check=True, capture_output=True,
                                   text=True, timeout=GH_TIMEOUT)
    except subprocess.TimeoutExpired:
        return None, f"`{' '.join(GH_COMMAND)}` did not answer in {GH_TIMEOUT}s"
    except (subprocess.CalledProcessError, OSError) as error:
        detail = (error.stderr or "").strip().splitlines()
        return None, (f"`{' '.join(GH_COMMAND)}` failed"
                      + (f": {detail[-1]}" if detail else f": {error}"))
    repos = {line.strip() for line in completed.stdout.splitlines() if line.strip()}
    if not repos:
        return None, f"`{' '.join(GH_COMMAND)}` returned no repositories"
    return repos, "github"


def check(rows, repos):
    """Findings for every Kestra cell in the appendix."""
    findings = []
    cells = names = exempts = 0
    for row in rows:
        covers = []
        for item in row.kestra:
            name = appendix.name_of(item)
            if not name or name == NO:
                # `·`, or `(itself)`: the row *is* Kestra (design §33 D354).
                exempts += 1
                continue
            if name.lower() in EXEMPT_NAMES:
                # `core (HTTP tasks)`: Kestra's own tasks, no plugin repo.
                exempts += 1
                continue
            names += 1
            # `plugin-aws` (s3): the parenthetical is a sub-module inside one
            # repository, recorded below and never looked up as a repository.
            note = item[item.find("("):].strip() if "(" in item else ""
            if name not in repos:
                findings.append(finding(
                    "error", row, name,
                    f"no {ORG}/{name} repository in the plugin list"
                    + (f" (the cell names the sub-module {note})" if note else "")))
                continue
            covers.append((name, note))
        if covers:
            cells += 1
    return findings, cells, names, exempts


def finding(severity, row, cell, truth):
    return {
        "severity": severity,
        "section": row.section,
        "connector": row.name,
        "starred": row.starred,
        "line": row.line,
        "cell": "Kestra",
        "named": cell,
        "truth": truth,
        "message": f"{row.section} {row.label}: the Kestra cell names `{cell}` but {truth}",
    }


def report(findings):
    for item in findings:
        print(f"kestra_catalog: {item['severity']}: {item['message']}", file=sys.stderr)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repos", default=None,
                        help=f"newline-separated repository list "
                             f"(default: ${ENV_REPOS})")
    parser.add_argument("--from-github", action="store_true",
                        help=f"list {ORG}'s repositories with `gh`")
    parser.add_argument("--docs", default=None,
                        help=f"design doc to read (default: {DESIGN_DOC.relative_to(ROOT)})")
    parser.add_argument("--check", action="store_true",
                        help="exit non-zero when a named plugin repository is missing")
    parser.add_argument("--strict", action="store_true",
                        help="treat warnings as failures")
    parser.add_argument("--json", default=None,
                        help="write the machine-readable summary here ('-' for stdout)")
    args = parser.parse_args(argv)

    repos = source = None
    skip = None
    if args.repos or os.environ.get(ENV_REPOS):
        path = args.repos or os.environ[ENV_REPOS]
        try:
            repos, source = read_repos(path)
        except RepoListError as error:
            print(f"kestra_catalog: {error}", file=sys.stderr)
            return 1
    elif args.from_github:
        repos, skip = repos_from_github()
        source = "github" if repos else None
    else:
        skip = (f"no plugin list given; set {ENV_REPOS}, pass --repos <file>, or pass "
                f"--from-github to list {ORG} with `gh`")
    if not repos:
        message = f"kestra_catalog: {skip}; skipping"
        if args.json:
            appendix.write_json({"script": "kestra_catalog", "status": "skipped",
                                 "skip": message, "findings": []}, args.json)
        print(message)
        return 0

    try:
        doc = appendix.load(args.docs or DESIGN_DOC)
    except AppendixError as error:
        print(f"kestra_catalog: {error}", file=sys.stderr)
        return 1

    findings, cells, names, exempts = check(doc.rows, repos)
    errors = [item for item in findings if item["severity"] == "error"]
    warnings = [item for item in findings if item["severity"] == "warning"]
    plugins = sorted(repo for repo in repos if repo.startswith("plugin-"))
    summary = {
        "script": "kestra_catalog",
        "status": "findings" if findings else "ok",
        "repos": {"source": source, "total": len(repos), "plugins": len(plugins)},
        "design_doc": str(args.docs or DESIGN_DOC),
        "rows": len(doc.rows),
        "kestra_cells": cells,
        "names_checked": names,
        "exempt_cells": exempts,
        "counts": {"error": len(errors), "warning": len(warnings)},
        "findings": findings,
    }
    if args.json:
        appendix.write_json(summary, args.json)
    report(findings)
    appendix.note(f"kestra_catalog: {cells} Kestra cells, {names} plugin names "
                 f"checked against {len(plugins)} plugin repositories of "
                 f"{len(repos)} in {ORG} ({source}); {len(errors)} error(s), "
                 f"{len(warnings)} warning(s)", args.json)
    if args.check and (errors or (args.strict and warnings)):
        if errors:
            print(f"kestra_catalog: {len(errors)} named plugin repository(ies) do not "
                  f"exist; the registry would point at plugins that were never released",
                  file=sys.stderr)
        else:
            print(f"kestra_catalog: --strict: {len(warnings)} warning(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
