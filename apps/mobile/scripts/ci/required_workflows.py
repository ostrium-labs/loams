"""Wait for every applicable GitHub Actions workflow on this PR or push.

This reads the simple event and path filters used by this repository's workflows.
Unsupported filter syntax fails closed, so changing a trigger requires updating
this parser and its tests.
"""

import ast
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import Request, urlopen


def matching_glob(pattern: str, path: str) -> bool:
    expression = ""
    index = 0
    while index < len(pattern):
        if pattern.startswith("**/", index):
            expression += "(?:.*/)?"
            index += 3
        elif pattern.startswith("**", index):
            expression += ".*"
            index += 2
        elif pattern[index] == "*":
            expression += "[^/]*"
            index += 1
        elif pattern[index] == "?":
            expression += "[^/]"
            index += 1
        else:
            expression += re.escape(pattern[index])
            index += 1
    return re.fullmatch(expression, path) is not None


def _values(value: str, following: list[str]) -> list[str]:
    value = value.strip()
    if value.startswith("["):
        if not value.endswith("]"):
            raise ValueError(f"Malformed workflow list: {value}")
        items = (part.strip() for part in value[1:-1].split(","))
        return [ast.literal_eval(item) if item[0] in "\"'" else item for item in items if item]
    if value:
        return [ast.literal_eval(value) if value[0] in "\"'" else value]
    result = []
    for line in following:
        if re.match(r"^      - ", line):
            item = line.strip()[2:].strip()
            result.append(ast.literal_eval(item) if item[0] in "\"'" else item)
        elif line.strip() and not line.lstrip().startswith("#"):
            raise ValueError(f"Unsupported workflow filter: {line}")
    return result


def _trigger(path: Path, event: str) -> dict[str, list[str]] | None:
    lines = path.read_text().splitlines()
    try:
        start = lines.index("on:") + 1
    except ValueError as error:
        raise ValueError(f"Unsupported workflow trigger in {path}") from error
    section = []
    for line in lines[start:]:
        if line and not line[0].isspace() and not line.startswith("#"):
            break
        section.append(line)
    event_line = f"  {event}:"
    if event_line not in section:
        return None
    start = section.index(event_line) + 1
    fields = {}
    for position in range(start, len(section)):
        line = section[position]
        if re.match(r"^  [A-Za-z_]+:", line):
            break
        match = re.match(r"^    (paths|paths-ignore|branches|tags):\s*(.*)$", line)
        if match:
            following = []
            for candidate in section[position + 1 :]:
                if candidate and not candidate.startswith("      "):
                    break
                following.append(candidate)
            fields[match.group(1)] = _values(match.group(2), following)
    if "paths-ignore" in fields:
        raise ValueError(f"Unsupported paths-ignore in {path}")
    return fields


def selected_workflows(root: Path, event: str, branch: str, changed: list[str], active: set[str]) -> set[str]:
    expected = set()
    for relative in sorted(active):
        if relative == ".github/workflows/governance-ci.yml":
            continue
        path = root / relative
        if not path.is_file():
            continue
        trigger = _trigger(path, event)
        if trigger is None:
            continue
        if event == "push" and "tags" in trigger and "branches" not in trigger:
            continue
        if "branches" in trigger and not any(matching_glob(pattern, branch) for pattern in trigger["branches"]):
            continue
        if "paths" in trigger and not any(
            matching_glob(pattern, file) for pattern in trigger["paths"] for file in changed
        ):
            continue
        expected.add(relative)
    return expected


def runs_complete(expected: set[str], runs: list[dict]) -> tuple[bool, str | None]:
    latest = {}
    for run in runs:
        path = run["path"].rsplit("@", 1)[0]
        if path in expected and (path not in latest or run.get("created_at", "") > latest[path].get("created_at", "")):
            latest[path] = run
    for path in sorted(expected):
        run = latest.get(path)
        if run is None or run["status"] != "completed":
            continue
        if run["conclusion"] != "success":
            return False, f"{path}: {run['conclusion']}"
    return len(latest) == len(expected) and all(run["status"] == "completed" for run in latest.values()), None


def matches_pull_request(run: dict, payload: dict) -> bool:
    if run["pull_requests"]:
        return any(
            pr["number"] == payload["number"] and pr["base"]["sha"] == payload["pull_request"]["base"]["sha"]
            for pr in run["pull_requests"]
        )
    head = payload["pull_request"]["head"]
    return run.get("head_branch") == head["ref"] and (run.get("head_repository") or {}).get("full_name") == head["repo"]["full_name"]


def _api(path: str) -> dict:
    request = Request(
        f"https://api.github.com/{path}",
        headers={"Authorization": f"Bearer {os.environ['GH_TOKEN']}", "Accept": "application/vnd.github+json"},
    )
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def main() -> int:
    root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
    repo = os.environ["GITHUB_REPOSITORY"]
    event = os.environ["GITHUB_EVENT_NAME"]
    if event not in {"pull_request", "push"}:
        raise ValueError(f"Unsupported event: {event}")
    payload = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    own_run = _api(f"repos/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']}")
    branch = os.environ["GITHUB_BASE_REF"] if event == "pull_request" else os.environ["GITHUB_REF_NAME"]
    before = payload.get("before", "")
    base = before if event == "push" and before and set(before) != {"0"} else "HEAD^"
    changed = subprocess.check_output(["git", "diff", "--name-only", base, "HEAD"], text=True).splitlines()
    workflows = _api(f"repos/{repo}/actions/workflows?per_page=100")["workflows"]
    active = {workflow["path"] for workflow in workflows if workflow["state"] == "active"}
    active.discard(own_run["path"].rsplit("@", 1)[0])
    expected = selected_workflows(root, event, branch, changed, active)
    print(f"Applicable workflows: {', '.join(sorted(expected)) or '(none)'}", flush=True)
    earliest = datetime.fromisoformat(own_run["created_at"].replace("Z", "+00:00")) - timedelta(minutes=2)
    head_sha = own_run["head_sha"]
    url = f"repos/{repo}/actions/runs?{urlencode({'event': event, 'head_sha': head_sha, 'per_page': 100})}"
    for _ in range(236):
        runs = []
        for run in _api(url)["workflow_runs"]:
            created = datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))
            if created < earliest:
                continue
            if event == "pull_request" and not matches_pull_request(run, payload):
                continue
            runs.append(run)
        complete, failure = runs_complete(expected, runs)
        if failure:
            print(f"Applicable workflow failed: {failure}", file=sys.stderr)
            return 1
        if complete:
            print("All applicable workflows passed.")
            return 0
        time.sleep(30)
    print(f"Timed out waiting for: {', '.join(sorted(expected))}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
