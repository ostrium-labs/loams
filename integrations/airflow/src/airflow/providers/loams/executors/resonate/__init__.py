# SPDX-License-Identifier: Apache-2.0
"""The Resonate executor's parts that do not import Airflow at module level:
deterministic ids (`ids`), the protocol client (`client`), settings
(`config`), the executor core (`dispatch`) and the worker (`worker`).
`runner` is the worker's child process and imports Airflow.
"""
