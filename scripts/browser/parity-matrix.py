#!/usr/bin/env python3
"""Render the browser parity suite's JSON-lines reports as a Markdown matrix.

Usage: parity-matrix.py REPORT.jsonl [REPORT.jsonl ...]

Each line is {"provider", "scenario", "status", "detail", "elapsed_ms",
"engine"} as written by crates/loams-sidebar-browser/tests/provider_parity.
A missing report is shown as "not run", never as a pass (D2120).
"""

import json
import sys

PROVIDERS = ["local", "service"]
SCENARIOS = [
    "navigate",
    "accessibility_snapshot",
    "click_type_forms",
    "screenshot",
    "content_extraction",
    "downloads_policy",
    "network_policy_allowlist",
    "cookie_ledger_persistence",
    "profile_restart_persistence",
    "credential_boundary",
    "timeouts_budgets",
    "recording",
]
MARK = {"pass": "pass", "fail": "**FAIL**", "n/a": "n/a", "skip": "skipped"}


def main(paths):
    results = {}
    engines = set()
    for path in paths:
        try:
            with open(path, encoding="utf-8") as handle:
                for line in handle:
                    line = line.strip()
                    if not line:
                        continue
                    row = json.loads(line)
                    results[(row["scenario"], row["provider"])] = row
                    engines.add(row.get("engine", "?"))
        except FileNotFoundError:
            print(f"<!-- {path}: no report -->")
    scenarios = SCENARIOS + sorted(
        {scenario for scenario, _ in results} - set(SCENARIOS)
    )
    print(f"### Browser provider parity (Obscura {', '.join(sorted(engines)) or '?'})\n")
    print("| Scenario | " + " | ".join(PROVIDERS) + " |")
    print("|---|" + "---|" * len(PROVIDERS))
    for scenario in scenarios:
        cells = []
        for provider in PROVIDERS:
            row = results.get((scenario, provider))
            cells.append(MARK.get(row["status"], row["status"]) if row else "not run")
        print(f"| `{scenario}` | " + " | ".join(cells) + " |")
    print("\n<details><summary>Details</summary>\n")
    for (scenario, provider), row in sorted(results.items()):
        detail = row.get("detail", "").replace("\n", " ")
        print(
            f"- `{provider}` / `{scenario}`: {row['status']} "
            f"({row.get('elapsed_ms', '?')} ms) {detail}"
        )
    print("\n</details>")
    failed = any(row["status"] == "fail" for row in results.values())
    return 1 if failed and "--strict" in sys.argv else 0


if __name__ == "__main__":
    sys.exit(main([arg for arg in sys.argv[1:] if not arg.startswith("--")]))
