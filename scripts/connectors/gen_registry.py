#!/usr/bin/env python3
"""Generate and check Loams Flow's connector registry (CN1 Task 2).

Sources of truth, and what each one fixes:

  docs/design/33-connectors.md Appendix A   the 203-row matrix (D352, D358, D628, CN-R1)
  docs/design/33-connectors.md section 8    the 21 starred connectors and their runtime (§8, D358)
  connectors/schema/connector.schema.json   the manifest shape and the category/priority/status enums (CN1 Task 1)
  connectors/registry/catalog.csv           the one CSV the registry is generated from (CN1 Ruling 1)

Modes:

  --check         cross-check Appendix A, section 8, the CSV, every manifest's config.$ref,
                  the manifest ids in connectors/registry/handwritten.txt and the licence gate
                  input, print every disagreement and exit non-zero. This is what CI runs.
  (default)       write connectors/registry/<id>.yaml stubs for every CSV row that has no
                  hand-written manifest, and connectors/schemas/<id>.config.json for every row
                  that has no hand-written config schema. Never overwrites an id listed in
                  handwritten.txt.
  --write-csv     (re)write connectors/registry/catalog.csv from Appendix A.
  --docs PATH     render Appendix A's Markdown tables from the CSV (CN1 Task 14's catalog page).
  --validate      validate connectors/registry/*.yaml against the JSON Schema, using the
                  `jsonschema` module when it is installed.
  --licence-gate-test
                  run the D359 gate against synthetic component tables and assert its two
                  verdicts: a refused id on a `kind = "library"` row is refused, and the same
                  id on a `kind = "service"` row is the D359 carve-out and is accepted
                  (CN1 Task 15's `itsplane` stanza is that second case). Exit non-zero on a
                  wrong verdict.

Deterministic and idempotent: rows are emitted in Appendix A order, files are written only
when their content changes, and two consecutive runs leave `git status` untouched.

Rules this file encodes, with the authority for each:

  * one CSV, hand-written YAML for the 21 starred manifests and for A.18's three Loams
    applications, generated `status: planned` stubs for the rest (CN1 Ruling 1, D628);
  * every manifest's `config.$ref` must resolve to a file under connectors/ (D353: the
    manifest declares the shape of an instance's settings; connector.schema.json requires
    `config`), so a generated stub gets a generated placeholder config schema rather than a
    dangling reference;
  * `starred` follows §8, which is the authority on the starred set; an Appendix A row
    marked P1 that §8 does not list is reported, not silently promoted. A.18's three rows
    are P1 and unstarred by decision (D628, CN1 Task 15, CN1 Ruling 11): they build in CN1
    and D358's 21 is unchanged, so they are listed in ACKNOWLEDGED_NOT_STARRED;
  * a non-starred row runs on Camel when its Camel cell names a component, else on an
    OpenAPI-generated native connector (Ruling 1; §5's `openapi` row). A.18's three are the
    exception: Camel 4.22.1 has no component for any of them, so CN1 Task 15 pins them to
    native Rust in loams-flow (D628). Kestra is a companion, never a runtime (D354, Q356);
  * every licence id in connectors/licences.toml must be an SPDX id outside the refused set
    (D359; CN1 Global Constraints; the licence_gate_refuses_flagged test), with the carve-out
    that the deny list gates what Loams ships or links: a `kind = "service"` row is an API
    surface reached over HTTP, so a refused id on one is accepted and reported as a note.
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_PATH = REPO_ROOT / "docs" / "design" / "33-connectors.md"
SCHEMA_PATH = REPO_ROOT / "connectors" / "schema" / "connector.schema.json"
CSV_PATH = REPO_ROOT / "connectors" / "registry" / "catalog.csv"
REGISTRY_DIR = REPO_ROOT / "connectors" / "registry"
HANDWRITTEN_PATH = REGISTRY_DIR / "handwritten.txt"
# Every connector's instance-config JSON Schema lives here, one file per connector id
# (connector.schema.json's config.$ref is "a path to the connector's instance-config JSON
# Schema under connectors/", for example schemas/kafka.config.json). The 21 starred schemas
# are hand-written; the generator writes a placeholder for every other row.
CONFIG_SCHEMA_DIR = REPO_ROOT / "connectors" / "schemas"
CONFIG_REF_PREFIX = "schemas/"
# The annotation key that marks a generated placeholder config schema as this generator's.
# JSON Schema ignores unknown keywords, so it changes nothing about validation.
CONFIG_SCHEMA_GENERATED_BY = "x-loams-generated-by"
LICENCES_PATH = REPO_ROOT / "connectors" / "licences.toml"

# Appendix A's columns, then the extra columns of CN1 Ruling 1 (`id`, `category`,
# `runtime`, `ref`, `status`). Order is fixed: Ruling 1 and CN1 Task 2 name this header.
CSV_COLUMNS = [
    "id",
    "name",
    "category",
    "priority",
    "starred",
    "status",
    "runtime",
    "ref",
    "source",
    "sink",
    "streaming",
    "batch",
    "cdc",
    "webhook",
    "auth",
    "camel",
    "kestra",
]

# The Appendix A columns after Connector: Source, Sink, Streaming, Batch, CDC, Webhook,
# Auth, Camel, Kestra, Priority.
APPENDIX_COLUMNS = [
    "source",
    "sink",
    "streaming",
    "batch",
    "cdc",
    "webhook",
    "auth",
    "camel",
    "kestra",
    "priority",
]

EXPECTED_ROW_COUNT = 203  # Appendix A's own "Totals: 203 connectors" line (D628, CN1 Task 15) and CN1 Task 2's registry_has_203_entries_and_21_starred
EXPECTED_STARRED_COUNT = 21  # §8's 21 and D358's number, unchanged by A.18 (D628, CN1 Ruling 11)

# §33 Appendix A's subsection id, its heading title (without the count, which is computed),
# its default category, and the per-row category overrides the subsection needs.
# A.1/A.2/A.5-A.18 map by subsection; A.3 (NoSQL, search, vector, graph) and A.4 (warehouse,
# lakehouse, OLAP, compute) are mixed and map per row.
SUBSECTIONS: list[tuple[str, str, str | None, str | None]] = [
    ("A.1", "Messaging and streaming", "messaging", None),
    ("A.2", "Relational", "relational", None),
    ("A.3", "NoSQL, search, vector and graph", None, None),
    ("A.4", "Warehouse, lakehouse, OLAP and compute", None, None),
    ("A.5", "Object storage and files", "object-storage", None),
    ("A.6", "Formats", "format", 'Formats are codecs used by other connectors; "Source" and "Sink" mean decode and encode.'),
    ("A.7", "CDC", "cdc", None),
    ("A.8", "Integration and orchestration", "integration", None),
    ("A.9", "CDP, marketing, analytics and ads", "saas", None),
    ("A.10", "CRM, sales and support", "saas", None),
    ("A.11", "Commerce and payments", "saas", None),
    ("A.12", "Developer and collaboration", "saas", None),
    ("A.13", "Observability and incident", "observability", None),
    ("A.14", "Infrastructure and runtime", "infra", None),
    ("A.15", "Identity", "identity", None),
    ("A.16", "AI", "ai", "AI connectors are mostly sinks used as enrichment steps (embedding, classification, extraction) inside a route, with the result written back into the event."),
    ("A.17", "Generic protocols", "protocol", None),
    # A.18 (D628, CN1 Task 15): Loams's own collaboration applications, which are import
    # targets and sources in their own right. category is `saas`, the slug A.12 already
    # gives every collaboration row: §4's enum has no `collaboration` value, and these three
    # are reached exactly as A.12's rows are — over a public HTTP API, never linked.
    ("A.18", "Loams's own applications", "saas", None),
]

# Appendix A's table header and its closing line, verbatim from the design document. The
# totals line is fixed rather than interpolated: the document leaves the P2 and P3 counts to
# CN1 Task 2's generator, so a rendered count the document does not carry would be drift.
APPENDIX_HEADER_CELLS = [
    "Connector",
    "Source",
    "Sink",
    "Streaming",
    "Batch",
    "CDC",
    "Webhook",
    "Auth",
    "Camel",
    "Kestra",
    "Priority",
]
TOTALS_LINE = (
    "Totals: 203 connectors; 21 ★ (P1), and the P2 and P3 counts are computed by CN1 Task 2's generator."
)

# Per-row categories for the two mixed subsections. A.3's four families are named in
# §33 §4's category comment (nosql, search, vector, graph). A.4's warehouse/lakehouse split
# follows the subsection title ("Warehouse, lakehouse, OLAP and compute").
# ClickHouse and Hive are not in either hand list; they are OLAP/warehouse systems and get
# `warehouse` here. CN1 Task 2's `category` output is reported against §4's enum.
CATEGORY_BY_ROW: dict[str, str] = {
    # A.3 — NoSQL, search, vector and graph (20 rows).
    "MongoDB": "nosql",
    "Cassandra": "nosql",
    "ScyllaDB": "nosql",
    "DynamoDB": "nosql",
    "Redis": "nosql",
    "Valkey": "nosql",
    "Couchbase": "nosql",
    "Firestore": "nosql",
    "Cosmos DB": "nosql",
    "Elasticsearch": "search",
    "OpenSearch": "search",
    "Solr": "search",
    "Meilisearch": "search",
    "Typesense": "search",
    "Qdrant": "vector",
    "Pinecone": "vector",
    "Weaviate": "vector",
    "Milvus": "vector",
    "pgvector": "vector",
    "Neo4j": "graph",
    # A.4 — Warehouse, lakehouse, OLAP and compute (20 rows).
    "Snowflake": "warehouse",
    "BigQuery": "warehouse",
    "Redshift": "warehouse",
    "Databricks": "warehouse",
    "ClickHouse": "warehouse",
    "DuckDB": "warehouse",
    "Druid": "warehouse",
    "Pinot": "warehouse",
    "StarRocks": "warehouse",
    "Doris": "warehouse",
    "Trino": "warehouse",
    "Athena": "warehouse",
    "Synapse / Fabric": "warehouse",
    "Iceberg": "lakehouse",
    "Delta Lake": "lakehouse",
    "Hudi": "lakehouse",
    "Hive": "warehouse",
    "Spark": "warehouse",
    "Flink": "warehouse",
    "Teradata": "warehouse",
}

# Ids assigned by hand, keyed by the Appendix A (or §8) connector name. Everything else is
# a mechanical kebab slug of the name. Four ids are here because the mechanical slug is
# wrong or leaks an annotation:
#   * "Arrow IPC / Flight" is one starred connector in §8 (#10) and one module in CN1's file
#     structure (formats/arrow.rs); the mechanical slug would split it into arrow-ipc-flight.
#   * "HTTP / REST" is §8 #16, whose runtime is the native HTTP connector
#     (loams_flow::connectors::http); the id is `http`, not `http-rest`.
#   * "ADBC (generic)" is §8 #15's spelling of the Appendix A row "ADBC"; one id, `adbc`.
#   * "(licence flag, D359)" is an Appendix A annotation on the Airbyte row, not part of the
#     product name; the mechanical slug would put a decision id in a registry key.
HAND_ASSIGNED_IDS: dict[str, str] = {
    "Arrow IPC / Flight": "arrow",
    "HTTP / REST": "http",
    "ADBC (generic)": "adbc",
    "Airbyte (licence flag, D359)": "airbyte",
}

# §8's "Runtime and implementation" column, resolved to the two runtime columns of the CSV.
# §8 is the authority for the starred set (D358) and CN1's file structure pins these module
# paths, so they are written verbatim rather than derived.
STARRED_RUNTIME: dict[str, tuple[str, str]] = {
    "kafka": ("native", "loams_flow::connectors::kafka"),  # §8 #1, rdkafka 0.39
    "postgresql": ("native", "loams_flow::connectors::postgres"),  # §8 #2, tokio-postgres + ADBC
    "mysql": ("native", "loams_flow::connectors::mysql"),  # §8 #3, mysql_async
    "debezium-postgres": ("debezium", "io.debezium.connector.postgresql.PostgresConnector"),  # §8 #4
    "debezium-mysql": ("debezium", "io.debezium.connector.mysql.MySqlConnector"),  # §8 #5
    "s3": ("native", "loams_flow::connectors::s3"),  # §8 #6, object_store 0.14
    "iceberg": ("native", "loams_flow::connectors::iceberg"),  # §8 #7, iceberg-rust 0.10
    "parquet": ("native", "loams_flow::formats::parquet"),  # §8 #8
    "avro": ("native", "loams_flow::formats::avro"),  # §8 #9
    "arrow": ("native", "loams_flow::formats::arrow"),  # §8 #10, arrow-ipc + arrow-flight
    "elasticsearch": ("iggy", "elasticsearch_sink"),  # §8 #11
    "clickhouse": ("native", "loams_flow::connectors::clickhouse"),  # §8 #12
    "snowflake": ("native", "loams_flow::connectors::adbc"),  # §8 #13, ADBC Snowflake driver
    "bigquery": ("native", "loams_flow::connectors::adbc"),  # §8 #14, ADBC BigQuery driver
    "adbc": ("native", "loams_flow::connectors::adbc"),  # §8 #15, any allowed driver
    "http": ("native", "loams_flow::connectors::http"),  # §8 #16, reqwest
    "webhooks": ("native", "loams_flow::connectors::webhook"),  # §8 #17, signed webhook routes
    "jdbc": ("camel", "jdbc"),  # §8 #18, the jdbc/sql components in loams-connect
    "kinesis": ("native", "loams_flow::connectors::kinesis"),  # §8 #19, aws-sdk-kinesis
    "redis": ("native", "loams_flow::connectors::redis"),  # §8 #20, the redis crate
    "opentelemetry": ("native", "loams_flow::connectors::otlp"),  # §8 #21, OTLP receiver in ingest
    # Not in §8: Appendix A's A.8 lists the Camel runtime itself as a P1 row. It is a
    # runtime, not a Loams-owned connector, so it is not starred (D358 names 21 Loams-owned
    # hot-path connectors and the Camel runtime is not among them); its runtime is Camel.
    "camel-runtime": ("camel", "camel"),
}

# A.18's three Loams applications (D628, CN1 Task 15): hand-written manifests that build in CN1
# and are NOT part of §8's 21, so they are `starred: false` with `priority: P1` and
# `status: preview` — the status a hand-written manifest carries, because a generated stub is
# exactly what they are not. Their runtime is native Rust in `loams-flow`, not the generated
# `openapi:<id>` a non-starred row with an empty Camel cell would get: Apache Camel 4.22.1 has
# no component for any of the three (no `camel-zulip`, `camel-plane` or `camel-gitea` at tag
# `camel-4.22.1`), so D354's "buy first" has nothing to buy and CN1 Task 15 pins each module.
# The ref is Task 15's own module path, `loams_flow::connectors::<id>`, which is mechanical here
# but written from the task rather than derived, so the two cannot drift apart silently.
HANDWRITTEN_UNSTARRED: dict[str, tuple[str, str]] = {
    "zulip": ("native", "loams_flow::connectors::zulip"),  # CN1 Task 15's files list
    "itsplane": ("native", "loams_flow::connectors::itsplane"),
    "forgejo": ("native", "loams_flow::connectors::forgejo"),
}

# The one place where Appendix A and §8 disagree, and how it is reconciled (reported, never
# silently fixed). Appendix A's A.8 marks `Camel (runtime)` P1, which makes 22 P1 rows, but §8
# lists exactly 21 starred connectors and D358 calls them the 21 Loams-owned hot-path
# connectors; the Camel runtime is the runtime itself (§5), not a Loams-owned connector, so it
# is `starred: false` with `priority: P1` and `status: planned` kept. Any other P1 row that §8
# does not list fails `--check` until the document and §8 are reconciled.
#
# A.18's three rows are P1 and unstarred by decision, not by oversight: the owner asked on
# 2026-10-04 for the ability to import from Slack, GitHub and Jira into Loams's own Zulip,
# ItsPlane and Forgejo, so they ship in CN1, but they are Loams's own applications reached
# over HTTP and not part of the précis' 21-connector hot path, so D358's ★ count and CN1's
# release sizing are unchanged (D628, CN1 Task 15, CN1 Ruling 11). Appendix A's legend was
# amended in the same commit to say that P1 is CN1 and ★ is the 21.
ACKNOWLEDGED_NOT_STARRED: dict[str, str] = {
    "camel-runtime": "the Camel runtime is the runtime, not a Loams-owned connector (D354, D358)",
    "zulip": "A.18's Loams applications are P1 because they build in CN1, and unstarred because they are Loams's own applications reached over HTTP, not the précis' hot path (D628, CN1 Task 15, CN1 Ruling 11)",
    "itsplane": "A.18's Loams applications are P1 because they build in CN1, and unstarred because they are Loams's own applications reached over HTTP, not the précis' hot path (D628, CN1 Task 15, CN1 Ruling 11)",
    "forgejo": "A.18's Loams applications are P1 because they build in CN1, and unstarred because they are Loams's own applications reached over HTTP, not the précis' hot path (D628, CN1 Task 15, CN1 Ruling 11)",
}

# The ids whose manifest a human wrote although they are not starred, so their `status` is
# `preview` rather than the `planned` a generated stub carries. It is the same
# HANDWRITTEN_UNSTARRED set: a non-starred hand-written manifest is exactly A.18's three.
HANDWRITTEN_PREVIEW = set(HANDWRITTEN_UNSTARRED)

# Which component in connectors/licences.toml each CSV runtime kind means, for the check that
# every runtime the CSV names has a licence-gate entry. `native` and `openapi` are Loams's
# own Apache-2.0 code and have no third-party component entry; `debezium`, `camel` and `iggy`
# are the three third-party runtimes of §5 (D354).
RUNTIME_COMPONENT_KEY: dict[str, str] = {
    "camel": "camel",
    "iggy": "iggy-connectors",
    "debezium": "debezium-server",
}

# D359: the ids CN1's licence gate refuses for anything Loams ships or runs by default.
REFUSED_LICENCE_IDS = [
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "BUSL-1.1",
    "SSPL-1.0",
    "Elastic-2.0",
    "NOASSERTION",
    "unknown",
]

# The kinds the deny list gates. `runtime`, `driver` and `library` are all shipped, linked or
# loaded into a runtime Loams runs, which is exactly what D359 gates. `service` is not: D359's
# carve-out is that a system Loams reaches over its public HTTP API is an API surface, so its
# own source licence is the operator's concern, not a gate failure. That is the same carve-out
# the `saas_public_api` note in licences.toml states, and CN1 Task 15's `itsplane` stanza is
# its first consequence: ItsPlane's upstream is AGPL-3.0 and Loams neither ships nor links it.
SHIPPED_LICENCE_KINDS = ("runtime", "driver", "library")
LICENCE_KINDS = SHIPPED_LICENCE_KINDS + ("service",)

# The licence families D359 refuses by name, so a spelling the [deny] list does not spell out
# exactly (`AGPL-3.0` where the list says `AGPL-3.0-only`) is still the same licence and gets
# the same verdict. Only the family is compared, never any licence text.
REFUSED_LICENCE_FAMILIES = ("AGPL", "BUSL", "SSPL", "ELASTIC", "NOASSERTION", "UNKNOWN")

# §33 Appendix A's legend, as the slugs connector.schema.json's auth enum uses. A cell that
# says "per component", "per driver" or "per spec" is one of these three, not a free value.
AUTH_SLUG_FIXUPS = {
    "per component": "per-component",
    "per driver": "per-driver",
    "per spec": "per-spec",
}

ID_PATTERN = re.compile(r"^[a-z0-9-]{1,48}$")  # connector.schema.json's properties.id.pattern
SPDX_PATTERN = re.compile(r"^[A-Za-z0-9.+-]+$")  # connector.schema.json's licence id pattern


class Problem(Exception):
    """A disagreement between the design document, the CSV and the manifests."""


# --------------------------------------------------------------------------------------
# Small text helpers. Appendix A uses `Y` and the middot for yes and no, backticks around
# Camel component names and Kestra plugin names, and commas both as a list separator and
# inside a parenthetical, so the cell readers below have to be careful about depth.
# --------------------------------------------------------------------------------------


def strip_star(name: str) -> str:
    """Drop Appendix A's `★ ` marker and collapse whitespace (§8 lists the same 21 rows)."""
    return re.sub(r"\s+", " ", name.replace("★", " ")).strip()


def name_key(name: str) -> str:
    """A comparison key for a connector name: case-folded, spacing around `/` ignored.

    §8 writes `HTTP/REST` where Appendix A writes `HTTP / REST`; the two are the same row.
    """
    return re.sub(r"\s*/\s*", "/", strip_star(name).lower())


def norm_cell(cell: str) -> str:
    """Normalise an Appendix A cell for comparison: no backticks, no middot, collapsed space."""
    text = re.sub(r"\s+", " ", cell.replace("`", "")).strip()
    if text in ("", "·", "-", "—", "n/a"):
        return ""
    return text


def split_multi(cell: str) -> list[str]:
    """Split a multi-value cell on commas that are not inside parentheses.

    `plugin-fs (ftp, ftps)` is one plugin with a two-value qualifier; splitting naively would
    produce two broken cells, so the split tracks parenthesis depth.
    """
    parts: list[str] = []
    depth = 0
    current = ""
    for ch in cell:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)
        if ch == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += ch
    parts.append(current)
    return [re.sub(r"\s+", " ", p).strip() for p in parts if p.strip()]


def yes_no(cell: str) -> str:
    """`Y` or an empty field: the CSV has no `·` (Appendix A's "no")."""
    return "Y" if norm_cell(cell) == "Y" else ""


def auth_slugs(cell: str) -> list[str]:
    """Appendix A's Auth cell as the schema's auth slugs, comma-separated in the CSV."""
    slugs = []
    for part in split_multi(norm_cell(cell)):
        low = part.lower()
        slug = AUTH_SLUG_FIXUPS.get(low, low.replace(" ", "-"))
        slugs.append(slug)
    return slugs


def slugify(name: str) -> str:
    """The mechanical id: a kebab slug of the connector name, at most 48 characters.

    `/` separates alternatives and becomes one dash: "RabbitMQ / AMQP" -> rabbitmq-amqp,
    "Lambda / Cloud Run / Azure Functions" -> lambda-cloud-run-azure-functions. A name longer
    than the schema's 48 characters is cut on a dash boundary.
    """
    text = strip_star(name).lower()
    slug = re.sub(r"[^a-z0-9]+", "-", text).strip("-")
    if len(slug) > 48:
        slug = slug[:48].rstrip("-")
    return slug


def connector_id(name: str) -> str:
    """The registry id for an Appendix A or §8 connector name (HAND_ASSIGNED_IDS wins).

    The hand-assigned table is consulted on the exact name and then on the case-folded key, so
    §8's `HTTP/REST` reaches the entry Appendix A spells `HTTP / REST`.
    """
    clean = strip_star(name)
    if clean in HAND_ASSIGNED_IDS:
        return HAND_ASSIGNED_IDS[clean]
    for assigned, cid in HAND_ASSIGNED_IDS.items():
        if name_key(assigned) == name_key(clean):
            return cid
    return slugify(clean)


def yaml_scalar(value: str) -> str:
    """Quote a value only where YAML needs it, so the generated manifests stay readable."""
    if value == "":
        return '""'
    needs_quotes = (
        value[0] in "-?:,[]{}#&*!|>'\"%@`"
        or ": " in value
        or value.endswith(":")
        or value.strip() != value
    )
    return "'" + value.replace("'", "''") + "'" if needs_quotes else value


# --------------------------------------------------------------------------------------
# Parsing the design document.
# --------------------------------------------------------------------------------------


def design_lines() -> list[str]:
    if not DESIGN_PATH.exists():
        raise Problem(f"missing design document: {DESIGN_PATH}")
    return DESIGN_PATH.read_text(encoding="utf-8").splitlines()


def parse_appendix(lines: list[str]) -> list[dict]:
    """Read Appendix A's tables into one dict per row, in document order."""
    rows: list[dict] = []
    subsection = ""
    in_appendix = False
    for line in lines:
        if line.startswith("## Appendix A"):
            in_appendix = True
            continue
        if in_appendix and line.startswith("## "):
            break
        if not in_appendix:
            continue
        heading = re.match(r"^### (A\.\d+) (.+?)(?: \(\d+\))?$", line.strip())
        if heading:
            subsection = heading.group(1)
            continue
        if not line.lstrip().startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if cells[0] == "Connector" or set("".join(cells)) <= {"-", ":"}:
            continue  # the header row and the |---|---| separator
        if len(cells) != 1 + len(APPENDIX_COLUMNS):
            raise Problem(f"{subsection}: expected 11 cells, got {len(cells)}: {line}")
        row = {"subsection": subsection, "appendix_name": cells[0], "name": strip_star(cells[0])}
        row.update({key: norm_cell(cell) for key, cell in zip(APPENDIX_COLUMNS, cells[1:])})
        rows.append(row)
    if not rows:
        raise Problem("Appendix A: no table rows found")
    return rows


def parse_starred_section(lines: list[str]) -> list[str]:
    """Read §8's Connector column, the authority on the starred set (D358)."""
    names: list[str] = []
    inside = False
    for line in lines:
        if line.startswith("## 8."):
            inside = True
            continue
        if inside and line.startswith("## "):
            break
        if not inside or not line.lstrip().startswith("|"):
            continue  # §8's prose between the table and the rollout paragraph
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if cells[0] in ("#", "---"):
            continue
        if cells[0] == "" or set("".join(cells)) <= {"-", ":"}:
            continue
        names.append(cells[1])
    if len(names) != 21:
        raise Problem(f"section 8 lists {len(names)} connectors, expected 21")
    return names


# --------------------------------------------------------------------------------------
# Building the CSV rows from Appendix A.
# --------------------------------------------------------------------------------------


def category_for(row: dict) -> str:
    """The row's category: the subsection's default, or its per-row entry."""
    override = CATEGORY_BY_ROW.get(row["name"])
    if override:
        return override
    for sub, _title, default, _note in SUBSECTIONS:
        if sub == row["subsection"]:
            if default is None:
                raise Problem(f"{row['subsection']}: no category for {row['name']!r}")
            return default
    raise Problem(f"unknown subsection {row['subsection']!r} for {row['name']!r}")


def runtime_for(row: dict) -> tuple[str, str]:
    """The row's runtime and ref.

    The starred rows and the Camel runtime take §8's "Runtime and implementation" column
    (STARRED_RUNTIME), and A.18's three Loams applications take HANDWRITTEN_UNSTARRED, which
    CN1 Task 15 pins to native Rust because Camel 4.22.1 has no component for any of them.
    Every other row: a non-empty Camel cell means the unmodified Camel runtime in loams-connect
    covers it, and its ref is that cell's first Camel scheme; otherwise it is a native
    connector generated from an OpenAPI spec, per §5's openapi row (CN1 Ruling 1). Kestra is a
    companion, never a runtime (D354, Q356), so a Kestra cell never sets the runtime.
    """
    cid = row["id"]
    if cid in STARRED_RUNTIME:
        return STARRED_RUNTIME[cid]
    if cid in HANDWRITTEN_UNSTARRED:
        return HANDWRITTEN_UNSTARRED[cid]
    if row["camel"]:
        # The Camel cell is a list of component names; runtime.ref takes the first one.
        scheme = split_multi(row["camel"].replace("|", ", "))[0].split()[0]
        # A glob such as `langchain4j-*` or `debezium-*` names a family, not a URI scheme,
        # so the wildcard is dropped and the stem kept (CN1 Task 2's matrix.py reads the same).
        if scheme.endswith("*"):
            scheme = scheme[:-1].rstrip("-")
        if not scheme or scheme.startswith("("):
            raise Problem(f"{cid}: Camel cell {row['camel']!r} has no usable scheme")
        return "camel", scheme
    return "openapi", f"openapi:{cid}"


def build_rows(lines: list[str]) -> tuple[list[dict], list[str], list[str]]:
    """Build the CSV rows from Appendix A.

    Returns the rows, the informational notes (reconciliations that are already understood,
    printed so they stay visible) and the drift problems (a disagreement CI must fail on).
    """
    appendix_rows = parse_appendix(lines)
    # Every Appendix A row gets its registry id up front, so section 8's names can be matched
    # by id as well as by name.
    for row in appendix_rows:
        row["id"] = connector_id(row["name"])
    starred_names = parse_starred_section(lines)
    notes: list[str] = []
    problems: list[str] = []

    # §8 is the authority on which rows are starred (D358). Match §8's names to Appendix A's
    # rows through the id rules, and report anything that does not line up instead of
    # silently promoting or demoting a row.
    starred_ids: set[str] = set()
    for name in starred_names:
        # §8 writes "HTTP/REST" and "ADBC (generic)" where Appendix A writes "HTTP / REST"
        # and "ADBC"; try the exact name, then a name that ignores spacing around `/`, then
        # the id, and report only when none of the three finds a row.
        match = next((r for r in appendix_rows if r["name"] == strip_star(name)), None)
        if match is None:
            match = next((r for r in appendix_rows if name_key(r["name"]) == name_key(name)), None)
        if match is None:
            match = next((r for r in appendix_rows if r["id"] == connector_id(name)), None)
        if match is None:
            problems.append(
                f"section 8 lists {name!r}, which has no Appendix A row; no CSV row is starred for it"
            )
            continue
        # The matched row's id is authoritative: §8's spelling may slug differently.
        cid = match["id"]
        if cid in starred_ids:
            problems.append(f"section 8 lists {name!r} twice (both resolve to id {cid})")
        starred_ids.add(cid)

    rows: list[dict] = []
    for row in appendix_rows:
        if not ID_PATTERN.match(row["id"]):
            problems.append(f"{row['name']!r}: id {row['id']!r} does not match the schema's pattern")
        category = category_for(row)
        runtime, ref = runtime_for(row)
        starred = row["id"] in starred_ids
        # Ruling 1: `preview` for a hand-written manifest (the 21 starred ones, and A.18's
        # three, which build in CN1 and are unstarred by decision — D628, CN1 Ruling 11), and
        # `planned` for every generated stub. A P1 row that §8 does not list and that is not
        # in HANDWRITTEN_PREVIEW keeps `planned`, and the difference is reported rather than
        # silently starred.
        if starred or row["id"] in HANDWRITTEN_PREVIEW:
            status = "preview"
        else:
            status = "planned"
        # Every P1 row that §8 does not list is reported, whether or not it is a hand-written
        # manifest: A.18's three carry `status: preview` because a person wrote them, which does
        # not make them part of the précis' 21. An acknowledged exception is a note, so a reader
        # of the CI log still sees the decision; anything else fails the run.
        if not starred and row["priority"] == "P1":
            text = (
                f"{row['id']}: Appendix A marks it P1 but section 8 does not list it, so it is "
                f"starred=false (status={status}) (section 8 is the authority on the starred set, D358)"
            )
            if row["id"] in ACKNOWLEDGED_NOT_STARRED:
                notes.append(f"{text}; acknowledged exception: {ACKNOWLEDGED_NOT_STARRED[row['id']]}")
            else:
                problems.append(text)
        if not starred and row["appendix_name"].startswith("★"):
            problems.append(
                f"{row['id']}: Appendix A marks it with a star but section 8 does not list it"
            )
        rows.append(
            {
                "id": row["id"],
                "name": row["name"],
                "category": category,
                "priority": row["priority"],
                "starred": "true" if starred else "false",
                "status": status,
                "runtime": runtime,
                "ref": ref,
                "source": yes_no(row["source"]),
                "sink": yes_no(row["sink"]),
                "streaming": yes_no(row["streaming"]),
                "batch": yes_no(row["batch"]),
                "cdc": yes_no(row["cdc"]),
                "webhook": yes_no(row["webhook"]),
                "auth": "|".join(auth_slugs(row["auth"])),
                "camel": "|".join(split_multi(row["camel"])),
                "kestra": "|".join(split_multi(row["kestra"])),
                "subsection": row["subsection"],
            }
        )

    seen: dict[str, str] = {}
    for row in rows:
        if row["id"] in seen:
            raise Problem(f"duplicate id {row['id']!r}: {seen[row['id']]!r} and {row['name']!r}")
        seen[row["id"]] = row["name"]
    return rows, notes, problems


def render_csv(rows: list[dict]) -> str:
    """The CSV text: LF line endings, no quoting (no cell holds a comma, quote or newline)."""
    out = io.StringIO()
    writer = csv.writer(out, lineterminator="\n")
    writer.writerow(CSV_COLUMNS)
    for row in rows:
        writer.writerow([row[column] for column in CSV_COLUMNS])
    return out.getvalue()


# --------------------------------------------------------------------------------------
# Reading what is on disk.
# --------------------------------------------------------------------------------------


def read_csv() -> list[dict]:
    if not CSV_PATH.exists():
        raise Problem(f"missing {CSV_PATH}; run with --write-csv to create it")
    with CSV_PATH.open(newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle)
        header = next(reader, None)
        if header != CSV_COLUMNS:
            raise Problem(f"{CSV_PATH}: header is {header}, expected {CSV_COLUMNS}")
        rows = []
        for raw in reader:
            if not raw:
                continue
            if len(raw) != len(CSV_COLUMNS):
                raise Problem(f"{CSV_PATH}: row has {len(raw)} fields, expected {len(CSV_COLUMNS)}")
            rows.append(dict(zip(CSV_COLUMNS, raw)))
    return rows


def read_handwritten() -> list[str]:
    """The hand-written manifest ids: a plain, sorted, one-per-line list with no comments."""
    if not HANDWRITTEN_PATH.exists():
        return []
    ids = []
    for line in HANDWRITTEN_PATH.read_text(encoding="utf-8").splitlines():
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        ids.append(text)
    return ids


def read_schema_enums() -> dict[str, set[str]]:
    """The enums --check validates against, read from the schema so there is one source."""
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    props = schema["properties"]
    return {
        "category": set(props["category"]["enum"]),
        "priority": set(props["priority"]["enum"]),
        "status": set(props["status"]["enum"]),
        "auth": set(props["auth"]["items"]["enum"]),
        "runtime": set(props["runtime"]["properties"]["kind"]["enum"]),
    }


# --------------------------------------------------------------------------------------
# --check: every disagreement between Appendix A, section 8, the CSV and the licences.
# --------------------------------------------------------------------------------------


def check(rows_from_doc: list[dict], problems: list[str], notes: list[str]) -> None:
    enums = read_schema_enums()
    csv_rows = read_csv()
    handwritten = set(read_handwritten())

    if len(csv_rows) != EXPECTED_ROW_COUNT:
        problems.append(
            f"catalog.csv: {len(csv_rows)} data rows, expected {EXPECTED_ROW_COUNT} "
            "(Appendix A's totals line, D628; CN1 Task 2's registry_has_203_entries_and_21_starred)"
        )
    if len(rows_from_doc) != EXPECTED_ROW_COUNT:
        problems.append(f"Appendix A: {len(rows_from_doc)} rows, expected {EXPECTED_ROW_COUNT}")

    by_id = {row["id"]: row for row in csv_rows}
    if len(by_id) != len(csv_rows):
        problems.append("catalog.csv: duplicate id in a data row")
    # The starred count is the 21 of §8 and D358, not the number of P1 rows: the Camel runtime
    # row is P1 and is not starred, and A.18's three are P1 and unstarred by decision
    # (D628, CN1 Ruling 11).
    starred_count = sum(1 for row in csv_rows if row["starred"] == "true")
    if starred_count != EXPECTED_STARRED_COUNT:
        problems.append(
            f"catalog.csv: {starred_count} starred rows, expected {EXPECTED_STARRED_COUNT} "
            "(the 21 of section 8 and D358; the Camel runtime row and A.18's three Loams "
            "applications are P1 but not starred)"
        )

    for row in csv_rows:
        cid = row["id"]
        if not ID_PATTERN.match(cid):
            problems.append(f"catalog.csv: id {cid!r} does not match the schema's ^[a-z0-9-]{{1,48}}$")
        if row["category"] not in enums["category"]:
            problems.append(f"{cid}: category {row['category']!r} is not in the schema's enum")
        if row["priority"] not in enums["priority"]:
            problems.append(f"{cid}: priority {row['priority']!r} is not in the schema's enum")
        if row["status"] not in enums["status"]:
            problems.append(f"{cid}: status {row['status']!r} is not in the schema's enum")
        if row["runtime"] not in enums["runtime"]:
            problems.append(f"{cid}: runtime {row['runtime']!r} is not in the schema's enum")
        if row["starred"] not in ("true", "false"):
            problems.append(f"{cid}: starred {row['starred']!r} is neither true nor false")
        for slug in filter(None, row["auth"].split("|")):
            if slug not in enums["auth"]:
                problems.append(f"{cid}: auth {slug!r} is not in the schema's enum")

    # Every Appendix A row present in the CSV with matching cells, and every CSV row from
    # Appendix A (the table is generated from the CSV, per CN1 Task 2).
    for doc_row in rows_from_doc:
        cid = doc_row["id"]
        csv_row = by_id.get(cid)
        if csv_row is None:
            problems.append(f"{cid}: Appendix A row {doc_row['name']!r} is missing from catalog.csv")
            continue
        for column in ["name", "priority", "source", "sink", "streaming", "batch", "cdc", "webhook", "auth", "camel", "kestra", "category", "starred", "status", "runtime", "ref"]:
            if csv_row[column] != doc_row[column]:
                problems.append(
                    f"{cid}: {column}: appendix/doc {doc_row[column]!r} != csv {csv_row[column]!r}"
                )
    doc_ids = {row["id"] for row in rows_from_doc}
    for cid in by_id:
        if cid not in doc_ids:
            problems.append(f"{cid}: in catalog.csv but not in Appendix A")

    # handwritten.txt lists the manifests a human wrote; each must be a real row that the
    # generator will not overwrite — a starred one (the 21) or one of A.18's three, which are
    # hand-written and unstarred by decision (D628, CN1 Task 15).
    starred_ids = {row["id"] for row in csv_rows if row["starred"] == "true"}
    for cid in sorted(handwritten):
        if cid not in by_id:
            problems.append(f"handwritten.txt: {cid!r} is not a catalog.csv row")
        elif cid not in starred_ids and cid not in HANDWRITTEN_PREVIEW:
            problems.append(
                f"handwritten.txt: {cid!r} is neither starred nor one of the hand-written "
                "unstarred rows, so it is a generated stub's id (CN1 Ruling 1, D628)"
            )

    # D353: a manifest declares the shape of its instance's settings, so every config.$ref must
    # resolve to a file that exists — the hand-written schemas and the generated placeholders
    # alike. A dangling reference is a manifest that cannot be loaded.
    check_config_refs(problems)

    # CN-R1: the licences of the components the CSV's runtimes name must be in the gate input.
    check_licences(problems, notes)


def manifest_config_ref(manifest: Path) -> str | None:
    """The `config.$ref` of a manifest, read without a YAML library.

    --check is the CI entry point and must run on a bare Python, so it reads the one field it
    needs straight out of the block mapping: a top-level `config:` line and the indented `$ref:`
    under it. Returns None when the manifest has no such block, which the schema forbids.
    """
    lines = manifest.read_text(encoding="utf-8").splitlines()
    for index, line in enumerate(lines):
        if line.rstrip() != "config:":
            continue
        for follow in lines[index + 1:]:
            if follow.strip() and not follow.startswith((" ", "\t")):
                break  # the config block ended at the next top-level key
            match = re.match(r"^\s+\$ref:\s*(.+?)\s*$", follow)
            if match:
                return match.group(1).strip().strip("\"'")
    return None


def check_config_refs(problems: list[str]) -> None:
    """Every manifest's config.$ref must name a file that exists and parses as JSON."""
    manifests = sorted(REGISTRY_DIR.glob("*.yaml"))
    if not manifests:
        problems.append(f"no manifests under {REGISTRY_DIR.relative_to(REPO_ROOT)}")
        return
    for manifest in manifests:
        ref = manifest_config_ref(manifest)
        if ref is None:
            problems.append(f"{manifest.stem}: no config.$ref (connector.schema.json requires config)")
            continue
        if ref.startswith("/") or ".." in Path(ref).parts:
            problems.append(
                f"{manifest.stem}: config.$ref {ref!r} is not a path under connectors/"
            )
            continue
        target = REPO_ROOT / "connectors" / ref
        if not target.exists():
            problems.append(
                f"{manifest.stem}: config.$ref {ref!r} does not resolve: "
                f"{target.relative_to(REPO_ROOT)} does not exist"
            )
            continue
        try:
            json.loads(target.read_text(encoding="utf-8"))
        except json.JSONDecodeError as err:
            problems.append(f"{manifest.stem}: {target.relative_to(REPO_ROOT)} is not valid JSON: {err}")


def licence_is_flagged(spdx: str, denied: set[str]) -> bool:
    """True when `spdx` is one D359 refuses, by exact id or by licence family.

    The exact match is the [deny] list itself. The family match is what keeps the gate honest
    about an id the list does not spell exactly: `AGPL-3.0` is the same licence as
    `AGPL-3.0-only`, and a bare family id must not slip past because it is one suffix away.
    Only the id is inspected; no licence text is ever read.
    """
    if spdx in denied:
        return True
    family = spdx.split("-", 1)[0].strip().upper()
    return family in REFUSED_LICENCE_FAMILIES


def licence_gate_verdict(key: str, stanza: dict, denied: set[str]) -> tuple[str, str]:
    """One component's gate verdict: `refused`, `carve-out` or `ok`, with the reason.

    `refused` fails the check and is what CN1's `licence_gate_refuses_flagged` test asserts for
    anything of kind runtime, driver or library. `carve-out` is a `kind = "service"` row
    carrying a refused id: D359's carve-out, because the deny list gates what Loams ships or
    links and an HTTP API surface is neither, so the verdict is reported as a note and never as
    a failure. `ok` is everything else.
    """
    spdx = str(stanza.get("spdx", ""))
    kind = stanza.get("kind")
    if not licence_is_flagged(spdx, denied):
        return "ok", f"{key}: {spdx} is outside the refused set"
    if kind in SHIPPED_LICENCE_KINDS:
        return (
            "refused",
            f"components.{key} carries refused licence {spdx!r} on a kind={kind!r} component, "
            "which Loams ships or loads (D359; the gate refuses it)",
        )
    if kind == "service":
        return (
            "carve-out",
            f"components.{key} carries refused licence {spdx!r} on a kind=service component: it is "
            "reached only over its public HTTP API and is neither shipped nor linked, which is "
            "D359's carve-out (D628, CN1 Task 15); the id is recorded, not refused",
        )
    return "refused", f"components.{key} has an unknown kind {kind!r}, so the gate cannot place it"


def licence_gate_test() -> int:
    """The gate's two verdicts, asserted against synthetic stanzas (CN1 Task 15).

    `--check` reads the real licences.toml, where no refused id sits on a shipped kind — so it
    cannot by itself show that the gate would refuse one. This is the negative test CN1 Task 15
    asks for, and it pins the kind-aware rule rather than the file's current contents:

      * a `kind = "library"` stanza with an AGPL id is refused, which is D359's rule and the
        case a reviewer worries about;
      * the same id on a `kind = "service"` stanza is the carve-out and is accepted, which is
        `itsplane` (AGPL-3.0 upstream, reached over HTTP only);
      * an id the [deny] list does not spell exactly is still refused by family, so `AGPL-3.0`
        cannot slip past as a library;
      * the real licences.toml still comes back clean.
    """
    denied = set(REFUSED_LICENCE_IDS)
    cases: list[tuple[str, dict, str]] = [
        (
            "library-agpl",
            {"spdx": "AGPL-3.0", "kind": "library", "source": "https://example.invalid/agpl"},
            "refused",
        ),
        (
            "library-agpl-spelled-out",
            {"spdx": "AGPL-3.0-or-later", "kind": "library", "source": "https://example.invalid/agpl"},
            "refused",
        ),
        (
            "runtime-noassertion",
            {"spdx": "NOASSERTION", "kind": "runtime", "source": "https://example.invalid/unknown"},
            "refused",
        ),
        (
            # CN1 Task 15's itsplane stanza, by shape: an AGPL id on a service.
            "service-agpl",
            {"spdx": "AGPL-3.0", "kind": "service", "source": "https://example.invalid/itsplane"},
            "carve-out",
        ),
        (
            "service-mit",
            {"spdx": "MIT", "kind": "service", "source": "https://example.invalid/forgejo"},
            "ok",
        ),
        (
            "library-mit",
            {"spdx": "MIT", "kind": "library", "source": "https://example.invalid/forgejo"},
            "ok",
        ),
    ]
    failures = 0
    for key, stanza, expected in cases:
        verdict, reason = licence_gate_verdict(key, stanza, denied)
        status = "ok" if verdict == expected else "FAIL"
        if verdict != expected:
            failures += 1
        print(f"licence-gate-test: {status}: {key}: expected {expected}, got {verdict}: {reason}")
    # The real file, so a gate that passes its synthetic cases but rejects the shipped
    # stanzas is still caught here.
    problems: list[str] = []
    notes: list[str] = []
    check_licences(problems, notes)
    for note in notes:
        print(f"licence-gate-test: note: {note}")
    if problems:
        failures += len(problems)
        for problem in problems:
            print(f"licence-gate-test: FAIL: {problem}")
    else:
        print(f"licence-gate-test: {LICENCES_PATH.relative_to(REPO_ROOT)} is clean")
    print(f"licence-gate-test: {failures} failure(s)")
    return 1 if failures else 0


def check_licences(problems: list[str], notes: list[str]) -> None:
    """The D359 gate, in the form CN1's licence_gate_refuses_flagged test asserts it."""
    if not LICENCES_PATH.exists():
        problems.append(f"missing {LICENCES_PATH} (D359; CN1 Task 2's licence_gate_refuses_flagged)")
        return
    import tomllib

    data = tomllib.loads(LICENCES_PATH.read_text(encoding="utf-8"))
    denied = {str(v).strip() for v in data.get("deny", {}).get("ids", [])}
    for expected in REFUSED_LICENCE_IDS:
        if expected not in denied:
            problems.append(f"licences.toml: [deny] does not list {expected!r} (D359)")
    components = data.get("components", {})
    if not components:
        problems.append("licences.toml: no [components] stanzas (D359)")
    for key, stanza in components.items():
        spdx = str(stanza.get("spdx", ""))
        if not SPDX_PATTERN.match(spdx):
            problems.append(f"licences.toml: components.{key}.spdx {spdx!r} is not an SPDX id")
        if stanza.get("kind") not in LICENCE_KINDS:
            problems.append(
                f"licences.toml: components.{key}.kind {stanza.get('kind')!r} is not one of "
                + "|".join(LICENCE_KINDS)
            )
        if not stanza.get("source"):
            problems.append(f"licences.toml: components.{key} has no source URL")
        verdict, reason = licence_gate_verdict(key, stanza, denied)
        if verdict == "refused":
            problems.append(f"licences.toml: {reason}")
        elif verdict == "carve-out":
            notes.append(reason)
    csv_rows = read_csv()
    known_ids = {row["id"] for row in csv_rows}
    for key, stanza in components.items():
        for cid in stanza.get("used_by", []):
            if cid not in known_ids:
                problems.append(
                    f"licences.toml: components.{key}.used_by names {cid!r}, which is not a catalog.csv row"
                )
    for row in csv_rows:
        component = RUNTIME_COMPONENT_KEY.get(row["runtime"])
        if component and component not in components:
            problems.append(
                f"{row['id']}: runtime {row['runtime']} needs components.{component} in licences.toml (D359)"
            )


# --------------------------------------------------------------------------------------
# Default mode: the generated stubs.
# --------------------------------------------------------------------------------------


def stub_yaml(row: dict) -> str:
    """The generated stub for one row (CN1 Ruling 1: P2/P3 manifests are generated stubs).

    A stub declares no source or sink block: it carries no capability detail until a
    hand-written manifest replaces it (D353), and a stub that guessed a source position would
    break CN1's semantic rule that `cdc: true` implies a non-empty position.
    """
    lines: list[str] = [
        "# Generated by scripts/connectors/gen_registry.py from connectors/registry/catalog.csv.",
        "# Do not edit by hand: the CSV is the registry's one source of truth (CN1 Ruling 1),",
        "# and `--check` fails CI when the two disagree (CN1 Task 2: appendix_matches_csv).",
        f"# {row['id']}: a planned stub with no capability detail (D353); a hand-written manifest",
        "# replaces it, and the generator never rewrites an id listed in handwritten.txt.",
    ]
    if row["starred"] == "true":
        # A placeholder for a starred id, written before its id reached handwritten.txt.
        # It says so, so nobody reads it as the starred manifest §8 requires.
        lines.insert(
            3,
            f"# PLACEHOLDER: {row['id']} is one of section 8's 21 starred connectors, so its real "
            "manifest is hand-written (CN1 Ruling 1, D358). This file holds no capability detail",
            "# and is not the shipped manifest; it is replaced as soon as the hand-written one lands.",
        )
    lines += [
        "apiVersion: loams.flow/v1",
        "kind: Connector",
        f"id: {row['id']}",
        f"name: {yaml_scalar(row['name'])}",
        f"specVersion: {yaml_scalar('0.1.0')}",
        f"category: {row['category']}",
        f"priority: {row['priority']}",
        # A stub is never starred and never preview: the starred manifests are hand-written and
        # carry `starred: true` and `status: preview` (CN1 Ruling 1; §8, D358). A placeholder
        # written for a starred id before its manifest landed says so in the header above and
        # is listed on stderr by the generator.
        "starred: false",
        "status: planned",
        "runtime:",
        f"  kind: {row['runtime']}",
        f"  ref: {yaml_scalar(row['ref'])}",
        "licence:",
        # D359: the gate refuses AGPL, BSL, SSPL, ELv2 and unlicensed components; every runtime
        # §5 approves is Apache-2.0, and `native`/`openapi` are Loams's own Apache-2.0 code.
        "  component: Apache-2.0",
        "capabilities:",
        "  delivery:",
        "    source: at_least_once",
        "    sink: at_least_once",
        "  ordering: none",
        "  formats:",
        "    - json",
        "  schema:",
        "    registry: none",
        "    evolution: none",
        "  bulk:",
        # CN1 Ruling 3's 65536-row Arrow batch is the default a stub inherits (D356).
        "    arrow: false",
        "    max_batch_rows: 65536",
        "  backpressure: pull",
    ]
    auth = [slug for slug in row["auth"].split("|") if slug]
    if auth:
        lines.append("auth:")
        lines.extend(f"  - {slug}" for slug in auth)
    else:
        lines.append("auth: []")
    lines.append("config:")
    lines.append(f"  $ref: schemas/{row['id']}.config.json")
    lines.append("secrets: []")
    lines.append("envelope:")
    # A stub emits no concrete CloudEvents type yet (D355's rule is one type per connector);
    # the hand-written manifest for a starred connector fills in emits.
    lines.append('  emits: ""')
    lines.append('  consumes: "*"')
    lines.append("  passthrough: false")
    lines.append("limits: {}")
    lines.append("conformance: []")
    lines.append("docs: null")
    return "\n".join(lines) + "\n"


def write_stubs(rows: list[dict], problems: list[str]) -> tuple[int, int]:
    """Write a stub for every row without a hand-written manifest. Never overwrite one."""
    REGISTRY_DIR.mkdir(parents=True, exist_ok=True)
    handwritten = read_handwritten()
    handwritten_set = set(handwritten)
    written = 0
    skipped = 0
    placeholder_stars: list[str] = []
    for row in rows:
        target = REGISTRY_DIR / f"{row['id']}.yaml"
        if row["id"] in handwritten_set:
            skipped += 1
            continue
        content = stub_yaml(row)
        if target.exists() and target.read_text(encoding="utf-8") == content:
            continue
        target.write_text(content, encoding="utf-8")
        written += 1
        if row["starred"] == "true":
            placeholder_stars.append(row["id"])
    if placeholder_stars:
        print(
            "warning: wrote placeholder stubs for starred ids that are not yet listed in "
            f"{HANDWRITTEN_PATH.relative_to(REPO_ROOT)}: {', '.join(placeholder_stars)}",
            file=sys.stderr,
        )
        print(
            "warning: add them to handwritten.txt (one per line, sorted) once their "
            "hand-written manifests land; the generator then never touches them again",
            file=sys.stderr,
        )
    for cid in handwritten:
        if not (REGISTRY_DIR / f"{cid}.yaml").exists():
            problems.append(f"handwritten.txt lists {cid!r} but {cid}.yaml does not exist")
    return written, skipped


def placeholder_config_schema(row: dict) -> str:
    """The placeholder instance-config JSON Schema for a generated stub (CN1 Task 2).

    A real draft 2020-12 document, not `{"type": "object"}`, so that:
      * every manifest's `config.$ref` resolves to a file that exists, which is what
        connector.schema.json's config.$ref description promises and what `--check` asserts;
      * an editor or a validator can open it and see what is still missing.
    `additionalProperties` is true and there is no `required`, so an operator can already
    configure an unimplemented connector; nothing reads those fields until the connector ships.
    D353 is what the manifest promises here: it declares the shape of an instance's settings.

    The `x-loams-generated-by` annotation is not a validation keyword; JSON Schema ignores
    unknown keywords. It marks the file as this generator's, so a later run can refresh its own
    placeholder and never touches a schema a person has written.
    """
    document = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"https://loams.dev/schemas/{row['id']}.config.json",
        "title": f"{row['name']} instance config (placeholder)",
        "description": (
            f"Placeholder for a {row['name']} instance's settings. It is filled in when this "
            "connector's runtime is chosen: CN2 for the P2 rows, through the stock Apache Camel "
            "components in loams-connect or Iggy's plugins, and CN3 for the P3 long tail, "
            "through OpenAPI-generated native connectors (design §33 D354, §5). Until then it "
            "declares no field and requires none, and additionalProperties is true so an "
            "operator can already pass settings for an unimplemented connector; nothing reads "
            "them until the connector ships (design §33 D353; CN1 Task 2)."
        ),
        "type": "object",
        "additionalProperties": True,
        CONFIG_SCHEMA_GENERATED_BY: (
            "scripts/connectors/gen_registry.py — a CN1 Task 2 placeholder; replace this file "
            "when the connector's runtime is chosen"
        ),
    }
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


def is_generated_placeholder(path: Path) -> bool:
    """True when a config schema on disk is this generator's own placeholder."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    return isinstance(document, dict) and CONFIG_SCHEMA_GENERATED_BY in document


def write_config_schemas(rows: list[dict], problems: list[str]) -> tuple[int, int]:
    """Write a placeholder config schema for every row without a hand-written one.

    Two rules keep a person's work safe. An id listed in connectors/registry/handwritten.txt is
    owned by a human — the 21 starred schemas — and is never touched. Any other existing file is
    only rewritten when it still carries this generator's `x-loams-generated-by` marker, so a
    schema somebody has started writing or editing survives, with a note naming the id to add
    to handwritten.txt.
    """
    CONFIG_SCHEMA_DIR.mkdir(parents=True, exist_ok=True)
    handwritten_set = set(read_handwritten())
    written = 0
    skipped = 0
    for row in rows:
        target = CONFIG_SCHEMA_DIR / f"{row['id']}.config.json"
        if row["id"] in handwritten_set:
            skipped += 1
            continue
        content = placeholder_config_schema(row)
        if target.exists():
            existing = target.read_text(encoding="utf-8")
            if existing == content:
                continue  # idempotent: our own placeholder, unchanged
            if not is_generated_placeholder(target):
                print(
                    f"gen_registry: leaving the hand-written {target.relative_to(REPO_ROOT)} alone "
                    f"(id {row['id']} is not in {HANDWRITTEN_PATH.relative_to(REPO_ROOT)}; add it "
                    "there so the generator skips it)",
                    file=sys.stderr,
                )
                skipped += 1
                continue
            target.write_text(content, encoding="utf-8")  # refresh our own stale placeholder
            written += 1
            continue
        target.write_text(content, encoding="utf-8")
        written += 1
    for cid in sorted(handwritten_set):
        if not (CONFIG_SCHEMA_DIR / f"{cid}.config.json").exists():
            problems.append(
                f"handwritten.txt lists {cid!r} but {cid}.config.json does not exist under "
                f"{CONFIG_SCHEMA_DIR.relative_to(REPO_ROOT)}"
            )
    return written, skipped


# --------------------------------------------------------------------------------------
# --docs: Appendix A's tables, rendered from the CSV.
# --------------------------------------------------------------------------------------


def render_table(rows: list[dict]) -> list[str]:
    """One Markdown table: the header, the |---| separator, then one line per row."""
    out = [
        "| " + " | ".join(APPENDIX_HEADER_CELLS) + " |",
        "|" + "|".join(["---"] * len(APPENDIX_HEADER_CELLS)) + "|",
    ]
    for row in rows:
        star = "★ " if row["starred"] == "true" else ""
        cells = [
            ("Y" if row["source"] else "·"),
            ("Y" if row["sink"] else "·"),
            ("Y" if row["streaming"] else "·"),
            ("Y" if row["batch"] else "·"),
            ("Y" if row["cdc"] else "·"),
            ("Y" if row["webhook"] else "·"),
            ", ".join(auth_prose(slug) for slug in row["auth"].split("|") if slug) or "·",
            ", ".join(render_camel_item(cell) for cell in row["camel"].split("|") if cell) or "·",
            ", ".join(render_camel_item(cell) for cell in row["kestra"].split("|") if cell) or "·",
            row["priority"],
        ]
        out.append(f"| {star}{row['name']} | " + " | ".join(cells) + " |")
    return out


def auth_prose(slug: str) -> str:
    """`per-driver` back to the prose Appendix A prints.

    The CSV holds the slugs connector.schema.json's auth enum defines, because a manifest's
    `auth` array must hold enum values; the appendix prints them as prose ("per driver"). The
    other auth slugs are one word already.
    """
    return {"per-component": "per component", "per-driver": "per driver", "per-spec": "per spec"}.get(slug, slug)


def render_camel_item(item: str) -> str:
    """One Camel or Kestra cell item, the way Appendix A prints it.

    `` `plugin-aws` (sqs) ``, and `core` and `(itself)` without backticks, because they are
    prose about a runtime rather than a component or a plugin repository. scripts/connectors/
    appendix.py renders the same shapes (CN1 Task 2's matrix.py is the reference renderer).
    """
    item = item.strip()
    if not item:
        return "·"
    if item.startswith("("):
        return item  # (itself): the row *is* the runtime, so there is nothing to name
    match = re.match(r"^(?P<name>[^()]*?)\s*(?P<note>\([^()]*\))$", item)
    if match:
        name = match.group("name").strip()
        note = match.group("note").strip()
        if not name or name.lower() in ("core", "itself"):
            return f"{name} {note}".strip()
        return f"`{name}` {note}"
    if item.lower() in ("core", "itself"):
        return item
    return f"`{item}`"


def render_docs(rows: list[dict]) -> str:
    """Appendix A's sections and tables, from the CSV (CN1 Task 14's catalog page).

    The shape is the one scripts/connectors/appendix.py renders, so `gen_registry.py --docs`
    and `matrix.py --docs` produce the same bytes for the same rows: a `### A.<n> <Title>
    (<count>)` heading per subsection, the section's note where Appendix A has one, the
    fixed header row, the rows, and Appendix A's closing totals line.
    """
    out: list[str] = []
    for sub, title, _default, note in SUBSECTIONS:
        sub_rows = [row for row in rows if row["subsection"] == sub]
        if not sub_rows:
            continue
        out.append(f"### {sub} {title} ({len(sub_rows)})")
        out.append("")
        if note:
            out.extend([note, ""])
        out.extend(render_table(sub_rows))
        out.append("")
    return "\n".join(out) + "\n" + TOTALS_LINE + "\n"


# --------------------------------------------------------------------------------------
# --validate: the manifests against the JSON Schema.
# --------------------------------------------------------------------------------------


def validate_manifests() -> int:
    try:
        import jsonschema  # type: ignore
    except ImportError:
        print("validate: the `jsonschema` module is not installed; syntax-only check")
        return check_yaml_syntax()
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    import yaml  # type: ignore

    failures = 0
    checked = 0
    for path in sorted(REGISTRY_DIR.glob("*.yaml")):
        checked += 1
        try:
            document = yaml.safe_load(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as err:  # pragma: no cover - malformed YAML is the failure
            print(f"{path.name}: YAML error: {err}")
            failures += 1
            continue
        errors = sorted(jsonschema.Draft202012Validator(schema).iter_errors(document), key=str)
        for error in errors:
            print(f"{path.name}: {error.message} at {'/'.join(str(p) for p in error.absolute_path)}")
            failures += 1
    print(f"validate: {checked} manifests checked, {failures} problem(s)")
    return 1 if failures else 0


def check_yaml_syntax() -> int:
    import yaml  # type: ignore

    failures = 0
    paths = sorted(REGISTRY_DIR.glob("*.yaml"))
    for path in paths:
        try:
            yaml.safe_load(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as err:
            print(f"{path.name}: YAML error: {err}")
            failures += 1
    print(f"validate: {len(paths)} manifests parsed, {failures} problem(s)")
    return 1 if failures else 0


# --------------------------------------------------------------------------------------
# Entry point.
# --------------------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="cross-check and exit non-zero on any disagreement")
    parser.add_argument("--write-csv", action="store_true", help="write connectors/registry/catalog.csv from Appendix A")
    parser.add_argument("--docs", metavar="PATH", help="render Appendix A's Markdown tables from the CSV to PATH")
    parser.add_argument("--validate", action="store_true", help="validate connectors/registry/*.yaml against the JSON Schema")
    parser.add_argument(
        "--licence-gate-test",
        action="store_true",
        help="assert the D359 gate's two verdicts: a refused id on a shipped kind is refused, "
        "and the same id on a kind=service row is the carve-out (CN1 Task 15)",
    )
    args = parser.parse_args(argv)

    problems: list[str] = []
    notes: list[str] = []
    try:
        lines = design_lines()
        rows, notes, problems = build_rows(lines)
    except Problem as err:
        print(f"gen_registry: {err}", file=sys.stderr)
        return 2

    # The modes are exclusive: --check checks, --write-csv writes the CSV, --docs writes the
    # Markdown tables, --validate validates the manifests, --licence-gate-test asserts the gate,
    # and no flag writes the stubs. That keeps `--write-csv` from touching
    # connectors/registry/*.yaml as a side effect. --licence-gate-test runs before the design
    # document is needed, because its cases are synthetic plus the shipped licences.toml.
    if args.licence_gate_test:
        return licence_gate_test()

    if args.write_csv:
        # Written only when the content changes, so a second run leaves git status clean.
        text = render_csv(rows)
        if not CSV_PATH.exists() or CSV_PATH.read_text(encoding="utf-8") != text:
            CSV_PATH.parent.mkdir(parents=True, exist_ok=True)
            CSV_PATH.write_text(text, encoding="utf-8")
            print(f"wrote {CSV_PATH.relative_to(REPO_ROOT)} ({len(rows)} rows)")
        else:
            print(f"{CSV_PATH.relative_to(REPO_ROOT)} is up to date ({len(rows)} rows)")

    if args.docs:
        target = Path(args.docs)
        if not target.is_absolute():
            target = REPO_ROOT / target
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(render_docs(rows), encoding="utf-8")
        print(f"wrote {target} ({len(rows)} rows)")

    if args.check:
        try:
            check(rows, problems, notes)
        except Problem as err:
            print(f"gen_registry: {err}", file=sys.stderr)
            return 2
        # Notes are reconciliations that are already understood (they stay visible so a reader
        # of the CI log sees them); problems are drift and fail the run.
        for note in notes:
            print(f"gen_registry: note: {note}")
        for problem in problems:
            print(f"gen_registry: {problem}")
        if problems:
            print(f"gen_registry: {len(problems)} problem(s)", file=sys.stderr)
            return 1
        starred = sum(1 for row in rows if row["starred"] == "true")
        print(
            f"gen_registry: check clean ({len(rows)} rows, {starred} starred, "
            f"{sum(1 for row in rows if row['priority'] == 'P2')} P2, "
            f"{sum(1 for row in rows if row['priority'] == 'P3')} P3)"
        )
        return 0

    if args.validate:
        return validate_manifests()

    if args.write_csv or args.docs:
        return 0

    written, skipped = write_stubs(rows, problems)
    schemas_written, schemas_skipped = write_config_schemas(rows, problems)
    for problem in problems:
        print(f"gen_registry: {problem}", file=sys.stderr)
    starred = sum(1 for row in rows if row["starred"] == "true")
    print(
        f"gen_registry: {len(rows)} rows ({starred} starred); {written} stub(s) written, "
        f"{skipped} hand-written manifest(s) skipped"
    )
    print(
        f"gen_registry: {schemas_written} config schema placeholder(s) written, "
        f"{schemas_skipped} hand-written config schema(s) skipped"
    )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))