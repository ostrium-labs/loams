#!/usr/bin/env python3
"""A small client for Airflow's REST API v2 and the Edge plugin's API, for the
conformance run (DX1 DX0 Task 6). Stdlib only; run.sh runs it in the pinned
Python image on the stack's network, so nothing is published on the host.

  af.py token
  af.py get <path>                          # JSON, printed
  af.py send <METHOD> <path> [json-body]
  af.py wait-dags <dag_id>...               # until each is parsed and active
  af.py trigger <dag_id> [conf-json]        # prints the run ID
  af.py wait-run <dag_id> <run_id> [timeout] [state...]
  af.py wait-ti <dag_id> <run_id> <task_id> <state> [timeout] [map_index]
  af.py log <dag_id> <run_id> <task_id> <try> [map_index]
  af.py workers                             # name<TAB>state per Edge worker
  af.py wait-worker <name> <state>[,<state>] [timeout]

AF_URL (http://airflow-api-server:8080), AF_USER and AF_PASSWORD (admin)
select the server; the token comes from /auth/token on every call.
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = os.environ.get("AF_URL", "http://airflow-api-server:8080").rstrip("/")
USER = os.environ.get("AF_USER", "admin")
PASSWORD = os.environ.get("AF_PASSWORD", "admin")
_token: str | None = None


def _request(method: str, path: str, body=None, accept="application/json", auth=True, timeout=30):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method)
    req.add_header("Accept", accept)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if auth:
        req.add_header("Authorization", f"Bearer {token()}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode()
    except urllib.error.HTTPError as e:
        raise SystemExit(f"af.py: {method} {path}: HTTP {e.code}: {e.read().decode()[:2000]}")
    if accept == "application/json":
        return json.loads(raw) if raw else None
    return raw


def token() -> str:
    global _token
    if _token is None:
        r = _request("POST", "/auth/token", {"username": USER, "password": PASSWORD}, auth=False)
        _token = r["access_token"]
    return _token


def q(s: str) -> str:
    return urllib.parse.quote(s, safe="")


def run_path(dag_id: str, run_id: str) -> str:
    return f"/api/v2/dags/{q(dag_id)}/dagRuns/{q(run_id)}"


def wait_dags(dag_ids: list[str], timeout=300) -> None:
    deadline = time.monotonic() + timeout
    left = set(dag_ids)
    while left:
        for d in sorted(left):
            try:
                r = _request("GET", f"/api/v2/dags/{q(d)}")
            except SystemExit:
                continue
            # Airflow 3 reports `is_stale` (2.x had `is_active`).
            if not r.get("is_stale", False) and r.get("is_active", True) and not r.get("has_import_errors"):
                if r.get("is_paused"):
                    _request("PATCH", f"/api/v2/dags/{q(d)}", {"is_paused": False})
                left.discard(d)
        if left:
            if time.monotonic() > deadline:
                errs = _request("GET", "/api/v2/importErrors")
                raise SystemExit(f"af.py: DAGs not parsed in {timeout}s: {sorted(left)}; import errors: {json.dumps(errs)[:3000]}")
            time.sleep(3)


def trigger(dag_id: str, conf: dict) -> str:
    r = _request("POST", f"/api/v2/dags/{q(dag_id)}/dagRuns", {"logical_date": None, "conf": conf})
    return r["dag_run_id"]


def wait_run(dag_id: str, run_id: str, timeout: float, states: list[str]) -> str:
    deadline = time.monotonic() + timeout
    while True:
        r = _request("GET", run_path(dag_id, run_id))
        if r["state"] in states:
            return r["state"]
        if time.monotonic() > deadline:
            return r["state"]
        time.sleep(3)


def wait_ti(dag_id, run_id, task_id, state, timeout, map_index) -> dict:
    deadline = time.monotonic() + timeout
    path = run_path(dag_id, run_id) + f"/taskInstances/{q(task_id)}"
    if map_index is not None and int(map_index) >= 0:
        path += f"/{int(map_index)}"
    while True:
        try:
            r = _request("GET", path)
        except SystemExit:
            r = {}
        if r.get("state") == state:
            return r
        if time.monotonic() > deadline:
            raise SystemExit(f"af.py: {task_id} of {run_id} is {r.get('state')!r}, not {state!r}, after {timeout}s")
        time.sleep(2)


def log(dag_id, run_id, task_id, try_number, map_index="-1") -> str:
    """The task log as text, one line per event. Airflow 3.3's log endpoint
    answers only application/json or application/x-ndjson (406 otherwise);
    JSON gives `content`, a list of structured events (or plain strings)."""
    path = run_path(dag_id, run_id) + f"/taskInstances/{q(task_id)}/logs/{int(try_number)}?full_content=true&map_index={int(map_index)}"
    r = _request("GET", path, timeout=60) or {}
    return "\n".join(_log_line(e) for e in r.get("content", []))


def _log_line(event) -> str:
    if isinstance(event, dict):
        return str(event.get("event", json.dumps(event, default=str)))
    return str(event)


def workers() -> list[dict]:
    return _request("GET", "/edge_worker/ui/worker")["workers"]


def main(argv: list[str]) -> None:
    if not argv:
        raise SystemExit(__doc__)
    cmd, args = argv[0], argv[1:]
    if cmd == "token":
        print(token())
    elif cmd == "get":
        print(json.dumps(_request("GET", args[0]), indent=1, default=str))
    elif cmd == "send":
        body = json.loads(args[2]) if len(args) > 2 else None
        print(json.dumps(_request(args[0], args[1], body), default=str))
    elif cmd == "wait-dags":
        wait_dags(args)
    elif cmd == "trigger":
        print(trigger(args[0], json.loads(args[1]) if len(args) > 1 else {}))
    elif cmd == "wait-run":
        timeout = float(args[2]) if len(args) > 2 else 600
        states = args[3:] or ["success", "failed"]
        print(wait_run(args[0], args[1], timeout, states))
    elif cmd == "wait-ti":
        timeout = float(args[4]) if len(args) > 4 else 300
        print(json.dumps(wait_ti(args[0], args[1], args[2], args[3], timeout, args[5] if len(args) > 5 else None), default=str))
    elif cmd == "log":
        print(log(*args))
    elif cmd == "workers":
        for w in workers():
            print(f"{w['worker_name']}\t{w['state']}\t{w.get('jobs_active', 0)}")
    elif cmd == "wait-worker":
        want = set(args[1].split(","))
        deadline = time.monotonic() + (float(args[2]) if len(args) > 2 else 180)
        while True:
            got = {w["worker_name"]: w["state"] for w in workers()}
            if got.get(args[0]) in want:
                print(got[args[0]])
                return
            if time.monotonic() > deadline:
                raise SystemExit(f"af.py: worker {args[0]} is {got.get(args[0])!r}, want {sorted(want)}")
            time.sleep(2)
    else:
        raise SystemExit(f"af.py: unknown command {cmd!r}\n{__doc__}")


if __name__ == "__main__":
    main(sys.argv[1:])
