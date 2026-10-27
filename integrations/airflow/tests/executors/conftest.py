# SPDX-License-Identifier: Apache-2.0
"""Environment for the executor tests (the parent conftest sets AIRFLOW_HOME).

Most of these tests need only the Airflow-free core. The executor tests build
real Airflow 3.3 workloads, and `BaseExecutor.__init__` resolves the Execution
API signing key, so a test-only secret is set here.
"""

from __future__ import annotations

import os

os.environ.setdefault("AIRFLOW__API_AUTH__JWT_SECRET", "loams-executor-unit-tests")
os.environ.setdefault("AIRFLOW__API__BASE_URL", "http://airflow-api-server:8080")
