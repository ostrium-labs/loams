"""Loams: an Apache Airflow 3 executor on Resonate durable promises (preview).

- :mod:`loams_airflow_resonate.executor` holds ``ResonateExecutor`` (it imports
  Airflow, so it is not imported here).
- :mod:`loams_airflow_resonate.worker` is the worker that runs the tasks.
- :mod:`loams_airflow_resonate.ids`, :mod:`~loams_airflow_resonate.client` and
  :mod:`~loams_airflow_resonate.dispatch` work without Airflow.
"""

__version__ = "0.1.0.dev0"

#: The executor's import path, for ``[core] executor``.
EXECUTOR = "loams_airflow_resonate.executor.ResonateExecutor"
