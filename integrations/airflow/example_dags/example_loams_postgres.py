# SPDX-License-Identifier: Apache-2.0
"""Loams Postgres: create a table, load rows, read them back.

The `loams_default` connection names the project (extra `pg_project`); the
task gets a short-lived login from Loams for each run.
"""

from __future__ import annotations

import pendulum
from airflow.sdk import DAG, Asset, task

from airflow.providers.loams.assets.loams import loams_uri
from airflow.providers.loams.hooks.postgres import LoamsPostgresHook
from airflow.providers.loams.operators.postgres import LoamsPostgresOperator

ORDERS = Asset(loams_uri("default", "postgres", "public.orders"))

with DAG(
    dag_id="example_loams_postgres",
    start_date=pendulum.datetime(2026, 10, 1, tz="UTC"),
    schedule=None,
    catchup=False,
    tags=["example", "loams"],
) as dag:
    create = LoamsPostgresOperator(
        task_id="create_table",
        sql="""
            CREATE TABLE IF NOT EXISTS orders (
                id bigint PRIMARY KEY,
                customer text NOT NULL,
                amount numeric(12, 2) NOT NULL,
                day date NOT NULL
            )
        """,
    )

    load = LoamsPostgresOperator(
        task_id="load",
        sql="""
            INSERT INTO orders (id, customer, amount, day)
            VALUES (%(id)s, %(customer)s, %(amount)s, %(day)s)
            ON CONFLICT (id) DO UPDATE SET amount = EXCLUDED.amount
        """,
        parameters={"id": 1, "customer": "acme", "amount": 42.5, "day": "{{ ds }}"},
        outlets=[ORDERS],
    )

    @task
    def daily_total(ds: str | None = None) -> float:
        hook = LoamsPostgresHook()
        row = hook.get_first("SELECT coalesce(sum(amount), 0) FROM orders WHERE day = %s", parameters=(ds,))
        return float(row[0])

    create >> load >> daily_total()
