# SPDX-License-Identifier: Apache-2.0
"""Run one Airflow workload through the Task SDK; the worker's child process.

``python -m airflow.providers.loams.executors.resonate.runner --json-path FILE [--delete] --server URL``

This mirrors Airflow's own ``airflow.sdk.execution_time.execute_workload``
(used by the ECS and Kubernetes executors) with two differences the worker
needs: the Execution API URL comes from the promise rather than local
configuration, and the process exits with the supervisor's exit code so the
worker can report it.
"""

from __future__ import annotations

import argparse
import contextlib
import os
import sys


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="airflow.providers.loams.executors.resonate.runner")
    parser.add_argument("--json-path", required=True)
    parser.add_argument("--delete", action="store_true", help="delete the file once read")
    parser.add_argument("--server", default="", help="the Execution API URL")
    parser.add_argument("--title", default="")
    args = parser.parse_args(argv)

    with open(args.json_path, encoding="utf-8") as fh:
        raw = fh.read()
    if args.delete:
        with contextlib.suppress(OSError):
            os.unlink(args.json_path)

    from airflow.executors import workloads
    from airflow.executors.base_executor import BaseExecutor
    from pydantic import TypeAdapter

    workload = TypeAdapter(workloads.ExecutorWorkload).validate_json(raw)

    with contextlib.suppress(Exception):
        # Workers have no metadata database in Airflow 3; drop any engine an
        # import may have configured, as execute_workload does.
        from airflow.settings import dispose_orm

        dispose_orm(do_log=False)

    from airflow.sdk.log import configure_logging

    configure_logging(output=sys.stdout.buffer, json_output=True)

    rc = BaseExecutor.run_workload(
        workload,
        server=args.server or None,
        subprocess_logs_to_stdout=True,
        proctitle=f"airflow worker -- LoamsResonateExecutor: {args.title or workload.display_name}",
    )
    return int(rc or 0)


if __name__ == "__main__":
    sys.exit(main())
