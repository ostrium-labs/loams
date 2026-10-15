#!/usr/bin/env python3
"""The upgrade gate's fixtures (UP1 Task 19, design §54 §14 steps 2-5).

    run.py write     --out state.json   [stack flags]  # on the previous release
    run.py check     --state state.json [--expect-new-writes present|absent] [stack flags]
    run.py write-new --state state.json [stack flags]  # after the upgrade

`write` writes every product's fixture with the APIs the previous release
serves (a product it lacks is skipped and recorded) and records the expected
reads. `check` reads every written fixture back and, with
`--expect-new-writes`, whether `write-new`'s items are there (after the
upgrade, and after `rollback --keep-writes`: present; after a rollback from
the snapshot: absent). Exit 0 when everything reads as expected, 1 otherwise.

Stack flags (defaults are the engine's own loopback addresses; the gate runs
this in a container sharing the `loams` container's network namespace):
--engine, --qdrant, --es, --durable, --tidb HOST:PORT, --pg DSN, and --key
(or LOAMS_API_KEY).
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import traceback

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from lib import Skip, Stack  # noqa: E402
import fixtures  # noqa: E402


def stack_from(args) -> Stack:
    return Stack(
        engine=args.engine,
        qdrant=args.qdrant,
        es=args.es,
        durable=args.durable,
        tidb=args.tidb,
        pg=args.pg,
        key=args.key or os.environ.get("LOAMS_API_KEY") or None,
    )


def write(stack: Stack, products=None) -> dict:
    info = stack.read_instance()
    state = {"release": info.get("serverVersion"), "products": {}}
    for mod in products or fixtures.ALL:
        try:
            state["products"][mod.NAME] = {"status": "written", "expect": mod.write(stack)}
        except Skip as e:
            state["products"][mod.NAME] = {"status": "skipped", "reason": str(e)}
    return state


def check(stack: Stack, state: dict, expect_new: str | None = None, products=None) -> list[str]:
    stack.read_instance()
    by_name = {m.NAME: m for m in (products or fixtures.ALL)}
    problems = []
    for name, entry in state["products"].items():
        mod = by_name.get(name)
        if mod is None or entry["status"] != "written":
            continue
        try:
            problems += [f"{name}: {p}" for p in mod.check(stack, entry["expect"])]
            new = state.get("new", {}).get(name)
            if expect_new and new is not None:
                problems += [f"{name} (new): {p}" for p in mod.check_new(stack, new, expect_new == "present")]
        except Exception as e:  # a read that fails is a problem, not a crash
            problems.append(f"{name}: {type(e).__name__}: {e}")
    return problems


def write_new(stack: Stack, state: dict, products=None) -> dict:
    stack.read_instance()
    by_name = {m.NAME: m for m in (products or fixtures.ALL)}
    state.setdefault("new", {})
    for name, entry in state["products"].items():
        mod = by_name.get(name)
        if mod is not None and entry["status"] == "written":
            state["new"][name] = mod.write_new(stack)
    return state


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["write", "check", "write-new"])
    parser.add_argument("--out")
    parser.add_argument("--state")
    parser.add_argument("--expect-new-writes", choices=["present", "absent"])
    parser.add_argument("--engine", default="http://127.0.0.1:8080")
    parser.add_argument("--qdrant", default="http://127.0.0.1:6333")
    parser.add_argument("--es", default="http://127.0.0.1:9200")
    parser.add_argument("--durable", default="http://127.0.0.1:8001")
    parser.add_argument("--tidb", default="tidb:4000")
    parser.add_argument("--pg")
    parser.add_argument("--key")
    args = parser.parse_args(argv)
    stack = stack_from(args)
    try:
        if args.command == "write":
            state = write(stack)
            pathlib.Path(args.out).write_text(json.dumps(state, indent=2) + "\n")
            for name, entry in state["products"].items():
                print(f"{name}: {entry['status']}{' (' + entry['reason'] + ')' if 'reason' in entry else ''}")
            return 0
        state = json.loads(pathlib.Path(args.state).read_text())
        if args.command == "write-new":
            pathlib.Path(args.state).write_text(json.dumps(write_new(stack, state), indent=2) + "\n")
            print("new writes:", ", ".join(state.get("new", {})))
            return 0
        problems = check(stack, state, args.expect_new_writes)
        for p in problems:
            print(p, file=sys.stderr)
        written = [n for n, e in state["products"].items() if e["status"] == "written"]
        print(f"checked {len(written)} products ({', '.join(written)}): {len(problems)} problems")
        return 1 if problems else 0
    except Exception:
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())
