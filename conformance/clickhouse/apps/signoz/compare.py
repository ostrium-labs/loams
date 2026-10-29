"""Compare SigNoz's answers on the House with the oracle's (HE1 R9.64, D1571).

    python3 -I compare.py <oracle-results.json> <house-results.json>

Both files are the driver's HE0_SIGNOZ_RESULTS: each API request's status and a
summary of its answer (how many values it holds, and its stable strings:
services, span names, attribute keys, severities). The two runs send the same
seeded telemetry, so for each request the oracle answered, the House must
answer too, hold values when the oracle's answer does, and name the same
things. A request the oracle failed (SigNoz's `histogramQuantile` is not
installed there, R5.4) is listed, not checked. Exits 1 on any difference.
Standard library only.
"""

import collections
import json
import sys


def keyed(entries):
    seen = collections.Counter()
    out = {}
    for e in entries:
        seen[e["label"]] += 1
        out[(e["label"], seen[e["label"]])] = e
    return out


def main():
    try:
        oracle = keyed(json.load(open(sys.argv[1])))
        house = keyed(json.load(open(sys.argv[2])))
    except (OSError, ValueError) as err:
        print(f"compare: no results to compare ({err}); the driver did not finish")
        return 1
    problems, checked, skipped = [], 0, []
    for key, want in oracle.items():
        label = f"{key[0][:160]} #{key[1]}"
        got = house.get(key)
        if want["status"] != 200:
            skipped.append(f"{label}: the oracle answered {want['status']}")
            continue
        checked += 1
        if got is None:
            problems.append(f"{label}: not sent on the House")
        elif got["status"] != 200:
            problems.append(f"{label}: the House answered {got['status']}")
        elif want["values"] > 0 and got["values"] == 0:
            problems.append(f"{label}: empty on the House, {want['values']} values on the oracle")
        elif set(want["names"]) != set(got["names"]):
            missing = sorted(set(want["names"]) - set(got["names"]))[:12]
            extra = sorted(set(got["names"]) - set(want["names"]))[:12]
            problems.append(f"{label}: names differ: missing {missing}, extra {extra}")
    print(f"compare: {checked} requests checked, {len(skipped)} the oracle failed, "
          f"{len(problems)} differences")
    for line in skipped:
        print(f"  oracle-failed: {line}")
    for line in problems:
        print(f"  DIFFERENT: {line}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
