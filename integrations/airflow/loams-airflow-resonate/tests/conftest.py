"""Test environment.

Most tests need no Airflow. ``test_executor.py`` imports Airflow when it is
installed (CI installs 3.3.2 with its constraints) and is skipped otherwise.
Airflow reads its configuration at import time, so the environment it needs
is set here, before any test module is imported.
"""

from __future__ import annotations

import os
import tempfile

_home = tempfile.mkdtemp(prefix="af-resonate-test-")
os.environ.setdefault("AIRFLOW_HOME", _home)
os.environ.setdefault("AIRFLOW__CORE__UNIT_TEST_MODE", "True")
os.environ.setdefault("AIRFLOW__CORE__LOAD_EXAMPLES", "False")
os.environ.setdefault("AIRFLOW__DATABASE__SQL_ALCHEMY_CONN", f"sqlite:///{_home}/airflow.db")
# BaseExecutor.__init__ resolves the Execution API signing key.
os.environ.setdefault("AIRFLOW__API_AUTH__JWT_SECRET", "loams-airflow-resonate-unit-tests")
os.environ.setdefault("AIRFLOW__API__BASE_URL", "http://airflow-api-server:8080")
