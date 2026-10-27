"""The Loams cases' app for DBOS Transact Python (DX0 Task 5).

  app.py setup                  # the app's table
  app.py start <wf> [crash_at]  # run `five`; with crash_at, SIGKILL itself when that step starts
  app.py resume <wf>            # launch (DBOS recovers pending workflows) and wait for <wf>
  app.py count <wf>             # rows per step, as JSON
  app.py enqueue <prefix> <n>   # n workflows on queue `loams` (concurrency 10)
  app.py serve <seconds>        # launch and dequeue for that long
  app.py queued <prefix>        # rows per workflow of that prefix, as JSON
  app.py set-event <wf> <v>     # a workflow that sets event `k` to v, then waits for `done`
  app.py get-event <wf> <secs>  # wait for event `k` of wf; print the value and when it arrived

Environment: DBOS_URL (the system database, also holding the app's table),
DBOS_LISTEN (1, the default, uses LISTEN/NOTIFY; 0 polls), DBOS_EXECUTOR.
Each step writes one row, so the row counts are the effect the cases check.
"""

from __future__ import annotations

import json
import os
import signal
import sys
import time

import psycopg
from dbos import DBOS, DBOSConfig, Queue, SetWorkflowID

URL = os.environ["DBOS_URL"]
CRASH_AT = int(os.environ.get("CRASH_AT", "0"))
config: DBOSConfig = {
    "name": "loams-crash-py",
    "system_database_url": URL,
    "use_listen_notify": os.environ.get("DBOS_LISTEN", "1") == "1",
    "executor_id": os.environ.get("DBOS_EXECUTOR", "local"),
}
DBOS(config=config)
queue = Queue("loams", concurrency=10)


def sql(q: str, args: tuple = ()) -> list[tuple]:
    with psycopg.connect(URL, autocommit=True) as c:
        cur = c.execute(q, args)
        return cur.fetchall() if cur.description else []


@DBOS.step()
def step(wf: str, n: int) -> None:
    if n == CRASH_AT:
        os.kill(os.getpid(), signal.SIGKILL)
    sql("INSERT INTO loams_steps (wf, step) VALUES (%s, %s)", (wf, n))


@DBOS.workflow()
def five(wf: str) -> str:
    for n in range(1, 6):
        step(wf, n)
    return "done"


@DBOS.workflow()
def one(wf: str) -> str:
    step(wf, 1)
    return wf


@DBOS.workflow()
def setter(v: str) -> str:
    DBOS.set_event("k", v)
    DBOS.recv("done", timeout_seconds=60)
    return v


def main(argv: list[str]) -> None:
    cmd = argv[0]
    if cmd == "setup":
        sql("CREATE TABLE IF NOT EXISTS loams_steps (wf text NOT NULL, step int NOT NULL)")
        return
    if cmd == "count":
        rows = sql("SELECT step, count(*) FROM loams_steps WHERE wf = %s GROUP BY step ORDER BY step", (argv[1],))
        print(json.dumps({str(s): n for s, n in rows}))
        return
    if cmd == "queued":
        rows = sql("SELECT wf, count(*) FROM loams_steps WHERE wf LIKE %s GROUP BY wf", (argv[1] + "%",))
        print(json.dumps({"workflows": len(rows), "max_runs": max((n for _, n in rows), default=0)}))
        return
    DBOS.launch()
    if cmd == "start":
        with SetWorkflowID(argv[1]):
            h = DBOS.start_workflow(five, argv[1])
        print(h.get_result())
    elif cmd == "resume":
        print(DBOS.retrieve_workflow(argv[1]).get_result())
    elif cmd == "enqueue":
        for i in range(int(argv[2])):
            wf = f"{argv[1]}-{i:05d}"
            with SetWorkflowID(wf):
                queue.enqueue(one, wf)
        print("enqueued")
    elif cmd == "serve":
        time.sleep(float(argv[1]))
    elif cmd == "set-event":
        with SetWorkflowID(argv[1]):
            h = DBOS.start_workflow(setter, argv[2])
        print(json.dumps({"set_at": time.time()}), flush=True)
        time.sleep(float(argv[3]) if len(argv) > 3 else 5)
        DBOS.send(argv[1], "ok", "done")
        h.get_result()
    elif cmd == "get-event":
        v = DBOS.get_event(argv[1], "k", timeout_seconds=float(argv[2]))
        print(json.dumps({"value": v, "got_at": time.time()}))
    else:
        raise SystemExit(f"unknown command {cmd}")
    DBOS.destroy()


if __name__ == "__main__":
    main(sys.argv[1:])
