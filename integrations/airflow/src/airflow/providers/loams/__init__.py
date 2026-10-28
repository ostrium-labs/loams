# SPDX-License-Identifier: Apache-2.0
"""Apache Airflow provider for Loams.

Hooks, operators, triggers and sensors for Loams Postgres, Loams House,
Loams streams (through the Iggy Kafka gateway), Loams object storage, Iceberg
tables in the Lakekeeper REST catalog and Loams Graph, plus the `loams://`
asset URI scheme and the `loams-s3://` ObjectStoragePath scheme.

Status: preview. Unit-tested with mocks; not yet run end to end against a
Loams stack.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
