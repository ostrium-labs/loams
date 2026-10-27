# Debezium example events

NOTICE: these fixtures are Debezium's documented example change events,
copyright the Debezium Authors, licensed under the Apache License 2.0
(https://github.com/debezium/debezium, `LICENSE.txt`). Taken on 2026-10-26
from commit `4824cb7269292996d3e40ddad11a7e1e113278fe` of
`documentation/modules/ROOT/pages/connectors/postgresql.adoc` and
`documentation/modules/ROOT/partials/modules/all-connectors/shared-mariadb-mysql.adoc`.

| File | Source in the docs | Shape |
|---|---|---|
| `pg-key.json` | PostgreSQL, "Change event keys" | key, JSON converter wrapper (`schema` + `payload`) |
| `pg-create.json` | PostgreSQL, "_create_ events" | value, with the wrapper |
| `pg-update.json` | PostgreSQL, "_update_ events" | value, payload only |
| `pg-delete.json` | PostgreSQL, "_delete_ events" | value, payload only |
| `mysql-key.json` | MySQL/MariaDB, "Change event keys" | key, with the wrapper |
| `mysql-create.json` | MySQL/MariaDB, "_create_ events" | value, with the wrapper |
| `mysql-update.json` | MySQL/MariaDB, "_update_ events" | value, payload only |
| `mysql-delete.json` | MySQL/MariaDB, "_delete_ events" | value, payload only |
| `mysql-schema-change.json` | MySQL/MariaDB, "Schema change topic" message | value, with the wrapper |

Edits, all mechanical (made by a script, not by hand):

- `{debezium-version}` became `3.3.0.Final`, `{context}` and
  `{connector-name}` became `postgresql` or `mysql`;
- the documentation's callout markers (`<1>`, `<2>`, …) and trailing commas
  were removed so the examples are JSON;
- where the docs elide the schema (`"schema": { ... }`), the fixture is the
  `payload` alone, which is also what Debezium writes with
  `value.converter.schemas.enable=false`.

The values (names, LSNs, binlog positions, timestamps) are the docs'.
