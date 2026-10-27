# SPDX-License-Identifier: Apache-2.0
"""Executors for Loams (lane AF-exec, design 74 §9.3, D2090 to D2096).

`resonate_executor.LoamsResonateExecutor` runs task instance tries as Resonate
durable promises; `resonate/` holds its Airflow-free core, the protocol client
and the worker (`loams-airflow-resonate-worker`). Preview.
"""
