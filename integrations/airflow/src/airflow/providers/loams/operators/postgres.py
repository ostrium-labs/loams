# SPDX-License-Identifier: Apache-2.0
"""`LoamsPostgresOperator`: SQL on Loams Postgres.

`SQLExecuteQueryOperator` with `LoamsPostgresHook` as its hook, so the
statement runs with a short-lived login Loams issues for the task, against
the project, branch and database named here or in the `loams` connection.
Everything `SQLExecuteQueryOperator` does (templated `.sql` files,
parameters, handlers, `split_statements`, `return_last`, OpenLineage) works.
"""

from __future__ import annotations

from collections.abc import Sequence
from functools import cached_property
from typing import Any

from airflow.providers.common.sql.operators.sql import SQLExecuteQueryOperator

from airflow.providers.loams.hooks.postgres import LoamsPostgresHook

__all__ = ["LoamsPostgresOperator"]


class LoamsPostgresOperator(SQLExecuteQueryOperator):
    """Runs SQL on Loams Postgres.

    :param sql: one statement, a list, or a `.sql` file (templated).
    :param loams_conn_id: the `loams` connection.
    :param project: the project id (`prj-...`); the connection's `pg_project` by default.
    :param branch: the branch id; the project's default branch when empty.
    :param endpoint: the endpoint id; the branch's read-write endpoint when empty.
    :param role: the role the issued login belongs to.
    :param pg_database: the database on the branch; its first when empty.
    :param read_only: use the branch's read-only endpoints.
    :param kwargs: anything `SQLExecuteQueryOperator` takes.
    """

    template_fields: Sequence[str] = (
        *SQLExecuteQueryOperator.template_fields,
        "project",
        "branch",
        "endpoint",
        "role",
        "pg_database",
    )
    ui_color = "#c9d8c5"

    def __init__(
        self,
        *,
        loams_conn_id: str = LoamsPostgresHook.default_conn_name,
        project: str | None = None,
        branch: str | None = None,
        endpoint: str | None = None,
        role: str | None = None,
        pg_database: str | None = None,
        read_only: bool | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(conn_id=loams_conn_id, **kwargs)
        self.loams_conn_id = loams_conn_id
        self.project = project
        self.branch = branch
        self.endpoint = endpoint
        self.role = role
        self.pg_database = pg_database
        self.read_only = read_only

    @cached_property
    def _hook(self) -> LoamsPostgresHook:  # type: ignore[override]
        return LoamsPostgresHook(
            loams_conn_id=self.conn_id or self.loams_conn_id,
            project=self.project,
            branch=self.branch,
            endpoint=self.endpoint,
            role=self.role,
            pg_database=self.pg_database,
            read_only=self.read_only,
            **self.hook_params,
        )

    def get_db_hook(self) -> LoamsPostgresHook:
        return self._hook
