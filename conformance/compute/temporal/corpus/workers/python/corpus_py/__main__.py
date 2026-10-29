"""The Python half of the corpus (lane TSDK-python, D2135).

  python -m corpus_py generate <address> <namespace> <out.json> [scenario,...]
      a worker for workflows.py plus a driver (starter) per scenario, with the
      fixed workflow ID corpus-python-<scenario>; writes what each driver
      observed. The Go exporter writes the histories afterwards
      (`corpus-go export -prefix corpus-python-`), so every language's files
      come from one exporter, as for TypeScript.
  python -m corpus_py worker <address> <namespace>
      the worker alone, until interrupted.
  python -m corpus_py start <address> <namespace> <scenario>
      one scenario's driver against a worker running elsewhere; prints the
      observation as JSON.
  python -m corpus_py replay [dir]
      the SDK's Replayer on every history in dir (default
      ../../histories/python, or $CORPUS_HISTORIES); exit 1 on any
      nondeterminism or replay failure.

TEMPORAL_API_KEY, when set, is sent as `authorization: Bearer <key>` in
plain text (the Loams gateway's API keys, X1.R5; its compose profile has no
TLS on the compose network), as the Go and TypeScript workers do.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from typing import Any

from corpus_py import PREFIX, QUEUE
from corpus_py.drive import drive, observation_file
from corpus_py.scenarios import NAMES


async def connect(address: str, namespace: str) -> Any:
    from temporalio.client import Client

    key = os.environ.get("TEMPORAL_API_KEY") or None
    # tls=False explicitly: an API key alone would otherwise turn TLS on.
    return await Client.connect(address, namespace=namespace, api_key=key, tls=False)


def new_worker(client: Any) -> Any:
    from temporalio.worker import Worker

    from corpus_py.activities import ACTIVITIES
    from corpus_py.workflows import WORKFLOWS

    # One activity at a time, so the fan-out's activities complete in the
    # order they were scheduled and regenerating is stable (as in Go).
    return Worker(
        client,
        task_queue=QUEUE,
        workflows=WORKFLOWS,
        activities=ACTIVITIES,
        max_concurrent_activities=1,
    )


def pick(only: str | None) -> list[str]:
    if not only:
        return list(NAMES)
    want = set(only.split(","))
    unknown = want - set(NAMES)
    if unknown:
        raise SystemExit(f"corpus: unknown scenarios {sorted(unknown)}")
    return [n for n in NAMES if n in want]


async def generate(address: str, namespace: str, out: str, only: str | None) -> None:
    client = await connect(address, namespace)
    observations = []
    async with new_worker(client):
        for name in pick(only):
            o = await asyncio.wait_for(drive(client, name, PREFIX + name), timeout=180)
            print(f"corpus: {name}: {o.get('result', '')}", flush=True)
            observations.append(o)
    with open(out, "w", encoding="utf-8") as f:
        f.write(observation_file(namespace, observations))


async def worker(address: str, namespace: str) -> None:
    client = await connect(address, namespace)
    await new_worker(client).run()


async def start(address: str, namespace: str, name: str) -> None:
    client = await connect(address, namespace)
    o = await drive(client, pick(name)[0], PREFIX + name)
    print(json.dumps(o, indent=2))


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    cmd, args = argv[0], argv[1:]
    if cmd == "generate" and len(args) in (3, 4):
        asyncio.run(generate(args[0], args[1], args[2], args[3] if len(args) == 4 else None))
    elif cmd == "worker" and len(args) == 2:
        asyncio.run(worker(args[0], args[1]))
    elif cmd == "start" and len(args) == 3:
        asyncio.run(start(*args))
    elif cmd == "replay" and len(args) <= 1:
        from corpus_py.replay import replay_dir

        return replay_dir(args[0] if args else None)
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
