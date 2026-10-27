# SPDX-License-Identifier: Apache-2.0
"""What Airflow's ProvidersManager reads (the `apache_airflow_provider` entry point).

Keep it in step with `provider.yaml`; `tests/test_provider_info.py` checks
that the two say the same thing and that every module named here imports.
"""

from __future__ import annotations

from typing import Any

_INTEGRATION = "Loams"

_CONN_FIELDS: dict[str, dict[str, Any]] = {
    "postgres_control_endpoint": {
        "label": "Postgres control endpoint",
        "description": "pg-control's URL (loams.postgres.v1). Defaults to the endpoint.",
        "schema": {"type": ["string", "null"], "format": "url"},
    },
    "pg_project": {
        "label": "Postgres project",
        "description": "The Loams Postgres project id (prj-...).",
        "schema": {"type": ["string", "null"]},
    },
    "pg_branch": {
        "label": "Postgres branch",
        "description": "The branch id; the project's default when empty.",
        "schema": {"type": ["string", "null"]},
    },
    "house_endpoint": {
        "label": "House endpoint",
        "description": "Where loams.house.v1 is served. Defaults to the endpoint, which proxies it.",
        "schema": {"type": ["string", "null"], "format": "url"},
    },
    "graph": {
        "label": "Graph",
        "description": "The default graph for Loams Graph statements.",
        "schema": {"type": ["string", "null"]},
    },
    "kafka_bootstrap_servers": {
        "label": "Kafka gateway",
        "description": "host:port of the Iggy Kafka gateway, for example 127.0.0.1:9093.",
        "schema": {"type": ["string", "null"]},
    },
    "kafka_sasl_username": {
        "label": "Kafka SASL user",
        "description": "SASL/PLAIN user, when the gateway has SASL on.",
        "schema": {"type": ["string", "null"]},
    },
    "kafka_sasl_password": {
        "label": "Kafka SASL password",
        "schema": {"type": ["string", "null"], "format": "password"},
    },
    "s3_endpoint": {
        "label": "Object storage endpoint",
        "description": "The S3 endpoint of the bucket, for example http://127.0.0.1:9000 (RustFS).",
        "schema": {"type": ["string", "null"], "format": "url"},
    },
    "s3_bucket": {
        "label": "Bucket",
        "description": "The default bucket.",
        "schema": {"type": ["string", "null"]},
    },
    "s3_access_key_id": {
        "label": "S3 access key id",
        "schema": {"type": ["string", "null"]},
    },
    "s3_secret_access_key": {
        "label": "S3 secret access key",
        "schema": {"type": ["string", "null"], "format": "password"},
    },
    "iceberg_catalog_uri": {
        "label": "Iceberg REST catalog",
        "description": "Lakekeeper's catalog URL, for example http://127.0.0.1:8181/catalog.",
        "schema": {"type": ["string", "null"], "format": "url"},
    },
    "iceberg_warehouse": {
        "label": "Iceberg warehouse",
        "schema": {"type": ["string", "null"]},
    },
}


def get_provider_info() -> dict[str, Any]:
    return {
        "package-name": "airflow-provider-loams",
        "name": "Loams",
        "description": "`Loams <https://loams.dev/>`__: Postgres, House, streams, object storage, "
        "Iceberg tables and Graph on one open-source data platform.\n",
        "state": "ready",
        "source-date-epoch": 1793059200,
        "versions": ["0.1.0"],
        "integrations": [
            {
                "integration-name": _INTEGRATION,
                "external-doc-url": "https://loams.dev/",
                "tags": ["service"],
            }
        ],
        "hooks": [
            {
                "integration-name": _INTEGRATION,
                "python-modules": [
                    "airflow.providers.loams.hooks.base",
                    "airflow.providers.loams.hooks.postgres",
                    "airflow.providers.loams.hooks.house",
                    "airflow.providers.loams.hooks.stream",
                    "airflow.providers.loams.hooks.object_storage",
                    "airflow.providers.loams.hooks.iceberg",
                    "airflow.providers.loams.hooks.graph",
                ],
            }
        ],
        "operators": [
            {
                "integration-name": _INTEGRATION,
                "python-modules": [
                    "airflow.providers.loams.operators.postgres",
                    "airflow.providers.loams.operators.house",
                    "airflow.providers.loams.operators.stream",
                    "airflow.providers.loams.operators.object_storage",
                    "airflow.providers.loams.operators.iceberg",
                    "airflow.providers.loams.operators.graph",
                ],
            }
        ],
        "sensors": [
            {
                "integration-name": _INTEGRATION,
                "python-modules": [
                    "airflow.providers.loams.sensors.house",
                    "airflow.providers.loams.sensors.stream",
                    "airflow.providers.loams.sensors.object_storage",
                    "airflow.providers.loams.sensors.iceberg",
                ],
            }
        ],
        "triggers": [
            {
                "integration-name": _INTEGRATION,
                "python-modules": [
                    "airflow.providers.loams.triggers.house",
                    "airflow.providers.loams.triggers.stream",
                ],
            }
        ],
        "filesystems": ["airflow.providers.loams.fs.loams_s3"],
        "executors": ["airflow.providers.loams.executors.resonate_executor.LoamsResonateExecutor"],
        "asset-uris": [
            {
                "schemes": ["loams"],
                "handler": "airflow.providers.loams.assets.loams.sanitize_uri",
                "factory": "airflow.providers.loams.assets.loams.create_asset",
            }
        ],
        "connection-types": [
            {
                "hook-class-name": "airflow.providers.loams.hooks.base.LoamsHook",
                "hook-name": "Loams",
                "connection-type": "loams",
                "ui-field-behaviour": {
                    "hidden-fields": ["login"],
                    "relabeling": {"host": "Endpoint", "password": "API key", "schema": "Namespace"},
                    "placeholders": {
                        "host": "https://acme.loams.dev",
                        "password": "lk_...",
                        "schema": "default",
                    },
                },
                "conn-fields": _CONN_FIELDS,
            }
        ],
    }
