#!/usr/bin/env python3
"""CB1 ledger and cost guard (plan §6.4 and §7.5).

The ledger is a Markdown table in .he0-shared/cloud-bench-ledger.md (outside the
repository; BENCH_LEDGER overrides the path). Every run is written to it before
anything is created, and updated when it is destroyed and once its bill settles.

Commands:
  check     --cloud C --row R --estimate USD   refuse a run the plan does not allow
  start     --cloud C --row R --run-id ID --estimate USD
  destroyed --run-id ID --sweep clean|dirty [--note TEXT]
  actual    --run-id ID --usd USD
  spent     [--cloud C]

`check` exits non-zero, with the reason on stderr, when:
  3  the estimate is above 25% of the cloud's lane cap, or the cloud's spend
     (actual where billed, else the estimate) plus this estimate would pass 60%
     of the cap: the coordinator is told and nothing runs on that cloud;
  4  it is 20:00 IST or later, or before 06:00 IST (nothing overnight);
  5  a hold file exists (.he0-shared/cloud-bench-HOLD, or BENCH_HOLD);
  6  an earlier run on the cloud has no destroy time or a dirty sweep.
Only the standard library is used.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SHARED = Path(os.environ.get("BENCH_SHARED", Path.home() / "Documents/Ostriumlabs/loams-wt/.he0-shared"))
LEDGER = Path(os.environ.get("BENCH_LEDGER", SHARED / "cloud-bench-ledger.md"))
HOLD = Path(os.environ.get("BENCH_HOLD", SHARED / "cloud-bench-HOLD"))
IST = dt.timezone(dt.timedelta(hours=5, minutes=30))
COLUMNS = ["Cloud", "Run id", "Row", "Estimate (USD)", "Actual (USD)", "Created (UTC)", "Destroyed (UTC)", "Sweep", "Notes"]
RUN_ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,15}$")
STOP_LINE = 0.25  # one estimate above 25% of the cap: tell the coordinator
SPEND_LINE = 0.60  # the cloud's spend past 60% of the cap: tell the coordinator


class GuardError(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


def caps() -> dict[str, float]:
    return json.loads((HERE / "rows.json").read_text())["caps_usd"]


def now_utc(arg: str | None = None) -> dt.datetime:
    if arg:
        t = dt.datetime.fromisoformat(arg.replace("Z", "+00:00"))
        return t if t.tzinfo else t.replace(tzinfo=dt.timezone.utc)
    return dt.datetime.now(dt.timezone.utc)


def stamp(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# --- the Markdown table ----------------------------------------------------------
def read(path: Path | None = None) -> tuple[list[str], list[dict[str, str]]]:
    """Returns the lines before the table rows (title, notes, header) and the rows."""
    path = path or LEDGER
    if not path.exists():
        head = [
            "# Cloud bench ledger (CB1)",
            "",
            "| " + " | ".join(COLUMNS) + " |",
            "|" + "---|" * len(COLUMNS),
        ]
        return head, []
    lines = path.read_text().splitlines()
    head: list[str] = []
    rows: list[dict[str, str]] = []
    in_table = False
    for line in lines:
        if not in_table:
            head.append(line)
            if line.startswith("|---") or line.startswith("| ---"):
                in_table = True
            continue
        if not line.startswith("|"):
            continue  # text after the table is not kept; the ledger is the table
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        cells += [""] * (len(COLUMNS) - len(cells))
        rows.append(dict(zip(COLUMNS, cells[: len(COLUMNS)])))
    if not in_table:
        head += ["", "| " + " | ".join(COLUMNS) + " |", "|" + "---|" * len(COLUMNS)]
    return head, rows


def write(head: list[str], rows: list[dict[str, str]], path: Path | None = None) -> None:
    path = path or LEDGER
    body = ["| " + " | ".join(r.get(c, "").replace("|", "/") for c in COLUMNS) + " |" for r in rows]
    tmp = path.with_suffix(".tmp")
    tmp.write_text("\n".join(head + body) + "\n")
    tmp.replace(path)


def money(cell: str) -> float | None:
    m = re.search(r"-?\d+(?:\.\d+)?", cell or "")
    return float(m.group()) if m else None


def spent(rows: list[dict[str, str]], cloud: str) -> float:
    """Actual where billed, else the logged estimate."""
    total = 0.0
    for r in rows:
        if r["Cloud"] != cloud:
            continue
        actual = money(r["Actual (USD)"])
        total += actual if actual is not None else (money(r["Estimate (USD)"]) or 0.0)
    return round(total, 2)


# --- the guard ---------------------------------------------------------------------
def check(cloud: str, estimate: float, rows: list[dict[str, str]], when: dt.datetime, hold: Path | None = None) -> list[str]:
    hold = hold or HOLD
    cap = caps().get(cloud)
    if cap is None:
        raise GuardError(2, f"unknown cloud {cloud}")
    if hold.exists():
        raise GuardError(5, f"hold file {hold} exists: no run starts until the coordinator removes it")
    ist = when.astimezone(IST)
    if ist.hour >= 20 or ist.hour < 6:
        raise GuardError(4, f"it is {ist:%H:%M} IST: runs start between 06:00 and 20:00 IST only (plan §7.5)")
    for r in rows:
        if r["Cloud"] == cloud and r["Created (UTC)"] and not r["Destroyed (UTC)"]:
            raise GuardError(6, f"run {r['Run id']} on {cloud} has no destroy time: tear it down and sweep first")
        if r["Cloud"] == cloud and r["Sweep"].startswith("dirty"):
            raise GuardError(6, f"run {r['Run id']} on {cloud} had a dirty sweep: escalate to the coordinator")
    if estimate > STOP_LINE * cap:
        raise GuardError(3, f"estimate ${estimate:.2f} is above 25% of the {cloud} cap (${STOP_LINE * cap:.2f}): tell the coordinator")
    so_far = spent(rows, cloud)
    if so_far + estimate > SPEND_LINE * cap:
        raise GuardError(
            3,
            f"{cloud} spend ${so_far:.2f} + ${estimate:.2f} passes 60% of the cap (${SPEND_LINE * cap:.2f}): tell the coordinator",
        )
    return [f"ok: {cloud} spend ${so_far:.2f} + estimate ${estimate:.2f} of cap ${cap:.2f}; {ist:%H:%M} IST"]


def find(rows: list[dict[str, str]], run_id: str) -> dict[str, str]:
    for r in rows:
        if r["Run id"] == run_id:
            return r
    raise GuardError(2, f"run {run_id} is not in the ledger")


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check")
    c.add_argument("--cloud", required=True)
    c.add_argument("--row", required=True)
    c.add_argument("--estimate", type=float, required=True)
    c.add_argument("--now")
    s = sub.add_parser("start")
    s.add_argument("--cloud", required=True)
    s.add_argument("--row", required=True)
    s.add_argument("--run-id", required=True)
    s.add_argument("--estimate", type=float, required=True)
    s.add_argument("--now")
    d = sub.add_parser("destroyed")
    d.add_argument("--run-id", required=True)
    d.add_argument("--sweep", choices=["clean", "dirty"], required=True)
    d.add_argument("--note", default="")
    d.add_argument("--now")
    a = sub.add_parser("actual")
    a.add_argument("--run-id", required=True)
    a.add_argument("--usd", type=float, required=True)
    sp = sub.add_parser("spent")
    sp.add_argument("--cloud")
    args = p.parse_args(argv)

    try:
        head, rows = read()
        if args.cmd == "check":
            for line in check(args.cloud, args.estimate, rows, now_utc(args.now)):
                print(line)
        elif args.cmd == "start":
            if not RUN_ID.match(args.run_id):
                raise GuardError(2, "run id is lowercase letters, digits and - (up to 16)")
            if any(r["Run id"] == args.run_id for r in rows):
                raise GuardError(2, f"run id {args.run_id} is already in the ledger: use a new one")
            check(args.cloud, args.estimate, rows, now_utc(args.now))
            rows.append(
                {
                    "Cloud": args.cloud,
                    "Run id": args.run_id,
                    "Row": args.row,
                    "Estimate (USD)": f"{args.estimate:.2f}",
                    "Actual (USD)": "",
                    "Created (UTC)": stamp(now_utc(args.now)),
                    "Destroyed (UTC)": "",
                    "Sweep": "pending",
                    "Notes": "",
                }
            )
            write(head, rows)
            print(f"ledger: started {args.run_id}")
        elif args.cmd == "destroyed":
            r = find(rows, args.run_id)
            r["Destroyed (UTC)"] = stamp(now_utc(args.now))
            r["Sweep"] = args.sweep
            if args.note:
                r["Notes"] = "; ".join(x for x in [r["Notes"], args.note] if x)
            write(head, rows)
            print(f"ledger: {args.run_id} destroyed, sweep {args.sweep}")
        elif args.cmd == "actual":
            r = find(rows, args.run_id)
            r["Actual (USD)"] = f"{args.usd:.2f}"
            write(head, rows)
            cloud = r["Cloud"]
            cap = caps()[cloud]
            total = spent(rows, cloud)
            print(f"ledger: {args.run_id} actual ${args.usd:.2f}; {cloud} spend ${total:.2f} of ${cap:.2f}")
            if total > SPEND_LINE * cap:
                print(f"STOP: {cloud} spend passed 60% of its cap: tell the coordinator", file=sys.stderr)
                return 3
        elif args.cmd == "spent":
            for cloud in [args.cloud] if args.cloud else sorted(caps()):
                print(f"{cloud} {spent(rows, cloud):.2f} of {caps()[cloud]:.2f}")
    except GuardError as e:
        print(f"ledger: {e}", file=sys.stderr)
        return e.code
    return 0


if __name__ == "__main__":
    sys.exit(main())
