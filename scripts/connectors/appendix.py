#!/usr/bin/env python3
"""Read and render Appendix A of `docs/design/33-connectors.md`.

CN1 plan Task 2 makes the design doc's appendix a generated artefact:
`connectors/registry/catalog.csv` holds Appendix A's 200 rows (plan Ruling 1)
and the renderer in `matrix.py` must reproduce the appendix byte for byte, so
`camel_catalog.py` and `kestra_catalog.py` also read the appendix through this
module and see exactly the rows the renderer would produce.

Two directions live here:

* `parse` reads the tables out of the design doc. That is what the two drift
  scripts check against Camel's and Kestra's own catalogs (design §33 risk
  CN-R1: "the Camel and Kestra columns are regenerated from their catalogs").
* `render` writes the same text back out of registry rows, so `--check` can
  diff the two.

Everything here is stdlib-only, like the other scripts under `scripts/`.
"""
from __future__ import annotations

import difflib
import json
import re
import sys
from pathlib import Path

# The repository root, from scripts/connectors/appendix.py.
ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = ROOT / "docs/design/33-connectors.md"
CATALOG_CSV = ROOT / "connectors/registry/catalog.csv"

NO = "·"  # the appendix's "no"; the legend at the top of Appendix A defines it
STAR = "★"  # P1, the CN1 set (design §33 D358)
HEADER_CELLS = ("Connector", "Source", "Sink", "Streaming", "Batch", "CDC",
                "Webhook", "Auth", "Camel", "Kestra", "Priority")

# The closing line of the appendix. The counts are the registry's own, so they
# are computed; the P2 and P3 counts are deliberately left to CN1 Task 2's
# generator to state, so that tail is fixed prose.
TOTALS_TAIL = "and the P2 and P3 counts are computed by CN1 Task 2's generator."


def totals_line(rows):
    """`Totals: 203 connectors; 21 ★ (P1), …` — the counts are the registry's."""
    rows = list(rows)
    starred = sum(1 for row in rows if row.starred)
    return (f"Totals: {len(rows)} connectors; {starred} \u2605 (P1), {TOTALS_TAIL}")

# The appendix's seventeen sections, in document order. The section titles are
# editorial (they are prose, not registry data), so they live beside the
# renderer; the CSV only decides which rows are in which section.
SECTIONS = (
    ("messaging", "A.1", "Messaging and streaming"),
    ("relational", "A.2", "Relational"),
    ("nosql", "A.3", "NoSQL, search, vector and graph"),
    ("warehouse", "A.4", "Warehouse, lakehouse, OLAP and compute"),
    ("object-storage", "A.5", "Object storage and files"),
    ("format", "A.6", "Formats"),
    ("cdc", "A.7", "CDC"),
    ("integration", "A.8", "Integration and orchestration"),
    ("saas", "A.9", "CDP, marketing, analytics and ads"),
    ("crm", "A.10", "CRM, sales and support"),
    ("commerce", "A.11", "Commerce and payments"),
    ("developer", "A.12", "Developer and collaboration"),
    ("observability", "A.13", "Observability and incident"),
    ("infra", "A.14", "Infrastructure and runtime"),
    ("identity", "A.15", "Identity"),
    ("ai", "A.16", "AI"),
    ("protocol", "A.17", "Generic protocols"),
    ("applications", "A.18", "Loams's own applications"),
)

# A row's `category` (or `section`) column names one of these, and several
# categories share one appendix section: design §33 §4's enum splits A.3 into
# nosql/search/vector/graph and A.4 into warehouse/lakehouse/olap/compute, but
# the appendix prints them as one table each.
CATEGORY_SECTION = {
    "messaging": "messaging",
    "relational": "relational",
    "nosql": "nosql", "search": "nosql", "vector": "nosql", "graph": "nosql",
    "warehouse": "warehouse", "lakehouse": "warehouse", "olap": "warehouse",
    "compute": "warehouse", "etl": "warehouse",
    "object-storage": "object-storage", "files": "object-storage",
    "format": "format", "formats": "format",
    "cdc": "cdc",
    "integration": "integration", "orchestration": "integration",
    "saas": "saas", "cdp": "saas", "marketing": "saas", "analytics": "saas",
    "ads": "saas",
    "crm": "crm", "sales": "crm", "support": "crm",
    "commerce": "commerce", "payments": "commerce",
    "developer": "developer", "collaboration": "developer",
    "observability": "observability", "incident": "observability",
    "infra": "infra", "infrastructure": "infra", "runtime": "infra",
    "identity": "identity",
    "ai": "ai",
    "protocol": "protocol", "generic": "protocol",
    # A.18 (D628): Loams's own Zulip, ItsPlane and Forgejo. The peer generator
    # may label them `saas` or `collaboration` like their neighbours; either way
    # the row's name places them, so these aliases only keep the check from
    # failing on a slug it has not seen.
    "applications": "applications", "own-applications": "applications",
    "loams": "applications", "internal": "applications",
    "native-app": "applications", "apps": "applications",
}

# The two sections whose prose sits between the heading and the table. Like the
# section titles this is editorial, so it is not registry data either; the
# renderer emits it so the appendix round-trips as one block.
# A section can also carry a paragraph *after* its table (A.18 does). Like the
# notes this is editorial prose from the design doc, not registry data.
SECTION_EPILOGUES = {
    "applications": "Each is a `native` sink whose credential comes from the `SecretStore` (D189) and whose upstream is reached only over its public HTTP API. That is D359's carve-out, and it is why no upstream application's own licence is a gate here: Loams neither ships nor links any of them. ItPlane's upstream is AGPL-3.0 and Forgejo's is MIT; both are read as API surfaces, and `connectors/licences.toml` records that explicitly rather than leaving it implicit. Forgejo's write capability is a **provisioning** fact, not a code fact: Forgejo derives the token scope from the HTTP method, so a `write:issue` token must be minted before the sink can run (its read scopes and its write scopes cannot live in one token).",
    # A.3's paragraph after its table (D634, 2026-10-04), which is the second section to
    # carry one. It says why the Neo4j row's source direction is native — `camel-catalog`
    # 4.22.1 marks both graph components Camel ships `producerOnly: true` — and why Grafeo
    # joined A.3 as its own row: a Rust engine embedded in the Fabric, reached over
    # Connect-RPC with GQL (ISO/IEC 39075) as its query surface. The section key is `nosql`
    # because A.3 prints nosql, search, vector and graph as one table.
    "nosql": "`Neo4j`'s source direction is served by a **native** connector over the engine's gRPC Query Service, not by `camel-neo4j`: `camel-catalog` 4.22.1 marks `camel-neo4j` (and `camel-arangodb`) `producerOnly: true`, so Camel can write to a graph and cannot read from one (D634). `Grafeo` is Loams's own graph engine, a Rust database embedded in the Fabric and reached over Connect-RPC with **GQL** — ISO/IEC 39075 — as its query surface, so the connector is not tied to one vendor's dialect. Both are P2 and neither is ★.",
}

SECTION_NOTES = {
    "format": 'Formats are codecs used by other connectors; "Source" and '
              '"Sink" mean decode and encode.',
    "ai": "AI connectors are mostly sinks used as enrichment steps (embedding, "
          "classification, extraction) inside a route, with the result written "
          "back into the event.",
    "applications": 'Loams\'s collaboration applications (design §39, SF1). They are import **targets** — the sinks a route writes into when data comes from Slack, GitHub or Jira — and sources in their own right. Camel has no component for any of them, so all three are **native** Rust connectors in `loams-flow` that speak each app\'s public REST API (D354\'s "native Rust where Loams owns the hot path"; §33\'s `runtime.kind` values). They are **P1 but not ★**: they ship in CN1, but they are not part of the précis\' 21-connector hot path, so D358\'s ★ count is unchanged.',
}

# The appendix's legend spells three auth methods in prose ("per driver",
# "per spec", "per component") while the registry stores them as slugs, since
# every other method is one word. Everything else renders verbatim.
AUTH_PROSE = {
    "per-component": "per component",
    "per-driver": "per driver",
    "per-spec": "per spec",
}

# Camel and Kestra cells name a repository or scheme, optionally with a
# parenthetical that is not part of the name: `plugin-aws` (s3),
# `debezium-*` (embedded), core (HTTP tasks). `core` and `itself` are prose
# about the runtime itself, not a component or a plugin repository, and are
# printed without backticks.
LITERAL_NAMES = frozenset({"core", "itself"})
PAREN_RE = re.compile(r"^(?P<name>[^()]*?)\s*(?P<note>\([^()]*\))$")
APPENDIX_RE = re.compile(r"^## Appendix A\.")
SECTION_RE = re.compile(r"^### (?P<number>A\.\d+) (?P<title>.+?) \((?P<count>\d+)\)\s*$")
SEPARATOR_RE = re.compile(r"^\|[-|]+\|$")
TOTALS_RE = re.compile(r"^Totals: ")


class AppendixError(Exception):
    """The design doc does not hold the appendix this script expects."""


class Row:
    """One appendix row, from either the design doc or the registry CSV."""

    __slots__ = ("section", "name", "starred", "source", "sink", "streaming",
                 "batch", "cdc", "webhook", "auth", "camel", "kestra",
                 "priority", "line")

    def __init__(self, section, name, starred=False, source=NO, sink=NO,
                 streaming=NO, batch=NO, cdc=NO, webhook=NO, auth=(),
                 camel=(), kestra=(), priority="", line=0):
        self.section = section
        self.name = name
        self.starred = starred
        self.source = source
        self.sink = sink
        self.streaming = streaming
        self.batch = batch
        self.cdc = cdc
        self.webhook = webhook
        self.auth = list(auth)
        self.camel = list(camel)
        self.kestra = list(kestra)
        self.priority = priority
        self.line = line

    @property
    def label(self):
        """The row as a reader sees it, for a finding message."""
        return f"{STAR} {self.name}" if self.starred else self.name

    def __repr__(self):  # pragma: no cover - debugging aid
        return f"<Row {self.section} {self.label} line {self.line}>"


class Section:
    """One `### A.<n> <title> (<count>)` block of the appendix."""

    __slots__ = ("number", "title", "note", "rows", "after")

    def __init__(self, number, title, note=None, rows=(), after=None):
        self.number = number
        self.title = title
        self.note = note
        self.rows = list(rows)
        self.after = after

    def render(self):
        lines = [f"### {self.number} {self.title} ({len(self.rows)})", ""]
        if self.note:
            lines += [self.note, ""]
        lines += [header_row(), separator_row()]
        lines += [render_row(row) for row in self.rows]
        lines.append("")
        if self.after:
            lines += [self.after, ""]
        return "\n".join(lines)


class Appendix:
    """The generated part of Appendix A: the sections and the totals line."""

    __slots__ = ("sections", "totals", "body")

    def __init__(self, sections, totals=None, body=None):
        self.sections = list(sections)
        self.totals = totals if totals is not None else totals_line(self.rows)
        self.body = body if body is not None else render_appendix(self.sections, totals)

    @property
    def rows(self):
        return [row for section in self.sections for row in section.rows]

    def render(self):
        return self.body


def section_key(number):
    """The section key of an appendix section number (A.6 -> `format`)."""
    for key, section_number, _ in SECTIONS:
        if section_number == number:
            return key
    return ""


def yes_no(value):
    """Normalise a CSV cell to the appendix's `Y` / `·`."""
    text = (value or "").strip()
    if text in ("", NO, "-", "n", "N", "no", "No", "false", "False", "0"):
        return NO
    if text in ("y", "Y", "yes", "Yes", "true", "True", "1"):
        return "Y"
    raise AppendixError(f"{value!r} is not a Y/· capability cell")


def split_items(text, separator=", "):
    """Split a cell on `separator`, ignoring separators inside parentheses.

    `plugin-fs` (ftp, ftps) is one plugin with two sub-modules, not two
    plugins, so the split has to see the parentheses.
    """
    items, current, depth, index = [], [], 0, 0
    while index < len(text):
        if depth == 0 and text.startswith(separator, index):
            items.append("".join(current))
            current, index = [], index + len(separator)
            continue
        if text[index] == "(":
            depth += 1
        elif text[index] == ")":
            depth -= 1
        current.append(text[index])
        index += 1
    items.append("".join(current))
    return [item.strip() for item in items if item.strip()]


def strip_backticks(cell):
    """`plugin-aws` (s3) -> plugin-aws (s3)"""
    return cell.replace("`", "")


def name_of(item):
    """The name a Camel or Kestra cell item carries, without its parenthetical.

    `plugin-aws` (s3) -> `plugin-aws`; `core (HTTP tasks)` -> `core`;
    `(itself)` -> `` (there is no name, the row is the runtime).
    """
    item = item.strip()
    if item.startswith("("):
        return ""
    match = PAREN_RE.match(item)
    return (match.group("name") if match else item).strip()


def render_item(item):
    """Render one Camel or Kestra cell item the way the appendix prints it."""
    item = item.strip()
    if not item or item == NO:
        return NO
    # `(itself)`: the row *is* the runtime, so there is nothing to name.
    if item.startswith("("):
        return item
    match = PAREN_RE.match(item)
    if match:
        name = match.group("name").strip()
        note = match.group("note").strip()
        if not name or name.lower() in LITERAL_NAMES:
            return f"{name} {note}".strip()
        return f"`{name}` {note}"
    if item.lower() in LITERAL_NAMES:
        return item
    return f"`{item}`"


def render_list(items):
    """Render a multi-value cell (`auth`, which the appendix prints plain)."""
    return ", ".join(item.strip() for item in items if item.strip()) or NO


def render_camel(items):
    return ", ".join(render_item(item) for item in items if item.strip()) or NO


def header_row():
    return "| " + " | ".join(HEADER_CELLS) + " |"


def separator_row():
    return "|" + "|".join(["---"] * len(HEADER_CELLS)) + "|"


def render_row(row):
    """Render one row: `| ★ Kafka | Y | Y | Y | · | · | · | … | `kafka` | … | P1 |`"""
    name = f"{STAR} {row.name}" if row.starred else row.name
    cells = [name, row.source, row.sink, row.streaming, row.batch, row.cdc,
             row.webhook, render_list(row.auth), render_camel(row.camel),
             render_camel(row.kestra), row.priority]
    return "| " + " | ".join(cells) + " |"


def render_appendix(sections, totals=None):
    """The whole generated appendix body, ending in the totals line.

    Each section renders with a trailing blank line, so the totals line is one
    blank line below the last table, as the appendix prints it.
    """
    sections = list(sections)
    if totals is None:
        totals = totals_line(row for section in sections for row in section.rows)
    return "\n".join(section.render() for section in sections) + "\n" + totals + "\n"


def parse(text, path=None):
    """Parse Appendix A out of the design doc."""
    where = f" in {path}" if path else ""
    lines = text.splitlines()
    try:
        start = next(index for index, line in enumerate(lines)
                     if APPENDIX_RE.match(line))
    except StopIteration:
        raise AppendixError(f"{path or 'the document'} has no '## Appendix A.' heading")
    sections = []
    first_section_line = None
    index = start
    while index < len(lines):
        match = SECTION_RE.match(lines[index])
        if not match:
            index += 1
            continue
        number, title = match.group("number"), match.group("title")
        if first_section_line is None:
            first_section_line = index
        index += 1
        # The blank line after the heading, then the optional note paragraph
        # (A.6 and A.16 have one), then the table.
        while index < len(lines) and not lines[index].strip():
            index += 1
        note = []
        while index < len(lines) and lines[index].strip() and not lines[index].startswith("|"):
            note.append(lines[index])
            index += 1
        while index < len(lines) and not lines[index].strip():
            index += 1
        rows = []
        while index < len(lines) and lines[index].startswith("|"):
            raw = lines[index]
            index += 1
            if raw.startswith("| Connector |") or SEPARATOR_RE.match(raw.strip()):
                continue
            rows.append(parse_row(raw, number, index, where))
        # The paragraph a section may carry after its table (A.18 has one),
        # stopping at the next heading or the totals line.
        after = []
        while (index < len(lines) and lines[index].strip()
               and not SECTION_RE.match(lines[index])
               and not TOTALS_RE.match(lines[index])):
            after.append(lines[index])
            index += 1
        sections.append(Section(number, title, " ".join(note) or None, rows,
                                " ".join(after) or None))
    if not sections:
        raise AppendixError(f"{path or 'the document'} has no '### A.<n>' sections{where}")
    totals = ""
    totals_line = None
    for offset in range(start, len(lines)):
        if TOTALS_RE.match(lines[offset]):
            totals = lines[offset]
            totals_line = offset
            break
    # The appendix body is compared byte for byte, so it carries the trailing
    # newline every line in the doc has.
    body = "\n".join(lines[first_section_line:totals_line + 1]) + "\n"
    if not totals:
        # Without a totals line there is nothing to anchor the comparison on;
        # --check would diff against a truncated body.
        raise AppendixError(f"{path or 'the document'} has no 'Totals: ' line")
    return Appendix(sections, totals, body)


def parse_row(raw, section, line, where=""):
    """Parse one `| … |` row into a `Row`."""
    cells = [cell.strip() for cell in raw.strip().strip("|").split("|")]
    if len(cells) != len(HEADER_CELLS):
        raise AppendixError(
            f"{where} line {line}: {len(cells)} cells, the appendix header has {len(HEADER_CELLS)}")
    name = cells[0]
    starred = name.startswith(STAR)
    if starred:
        name = name[len(STAR):].strip()
    plain = strip_backticks
    return Row(section, name, starred,
               yes_no(cells[1]), yes_no(cells[2]), yes_no(cells[3]),
               yes_no(cells[4]), yes_no(cells[5]), yes_no(cells[6]),
               split_items(plain(cells[7]), ", "),
               split_items(plain(cells[8]), ", "),
               split_items(plain(cells[9]), ", "),
               cells[10], line)


def load(path=DESIGN_DOC):
    """Read the design doc and return its `Appendix`."""
    path = Path(path)
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise AppendixError(f"{path}: {error}") from error
    return parse(text, path)


def diff(expected, actual, expected_name="design doc", actual_name="catalog.csv"):
    """A unified diff of the two renderings, or "" when they agree."""
    return "".join(difflib.unified_diff(
        expected.splitlines(keepends=True),
        actual.splitlines(keepends=True),
        fromfile=expected_name,
        tofile=actual_name,
    ))


def note(message, json_target=None):
    """Print the human summary line, unless `--json -` gave stdout to the JSON."""
    print(message, file=sys.stderr if str(json_target) == "-" else sys.stdout)


def write_json(summary, path):
    """Write a machine-readable summary; `--json -` means stdout."""
    text = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if str(path) == "-":
        sys.stdout.write(text)
        return
    destination = Path(path)
    if destination.parent and not destination.parent.exists():
        destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(text, encoding="utf-8")
