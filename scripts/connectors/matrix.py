#!/usr/bin/env python3
"""Render Appendix A of `docs/design/33-connectors.md` from the registry CSV.

CN1 plan Task 2: `connectors/registry/catalog.csv` is the one source for the
200 registry rows (plan Ruling 1), and the appendix of design §33 is generated
from it, so the two cannot drift:

    matrix.py --check            # exit 0 only when the appendix matches the CSV
    matrix.py                    # print the rendered tables
    matrix.py --docs OUT.md      # write them to a file

`loams-fabric connectors gen --docs` (CN1 Task 14) writes the same tables from
Rust; this script is the Python reference that renderer is diffed against, so
the formatting rules below are the contract: the `### A.<n> <Title> (<count>)`
headings, the two section notes, the fixed header row, `Y`/`·` cells, the `★ `
prefix on the P1 names, backticked Camel and Kestra cells, `|`-joined
multi-values rendered back as `, `-joined, and the closing totals line.

With no arguments and no CSV the script skips and exits 0 (the plan's "tests
skip without services" convention). `--check` against a missing CSV is an
error, not a skip: that is a repository state, not a missing service.
"""
from __future__ import annotations

import argparse
import csv
import os
import sys
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent))

import appendix  # noqa: E402  (the sibling module needs the path first)
from appendix import (AUTH_PROSE, Appendix, AppendixError, CATEGORY_SECTION,  # noqa: E402
                      CATALOG_CSV, DESIGN_DOC, ROOT, Row, SECTIONS,
                      SECTION_EPILOGUES, SECTION_NOTES, STAR, Section, yes_no)

# Column aliases: the CSV carries Appendix A's columns plus `id`, `category`,
# `runtime`, `ref` and `status` (plan Ruling 1), and a generator may spell a
# header either way. Lookup is by name, so the column order does not matter.
NAME_COLUMNS = ("connector", "name", "display_name", "displayname")
SECTION_COLUMNS = ("section", "group", "appendix_section", "category")
STARRED_COLUMNS = ("starred", "star", "p1")
AUTH_COLUMNS = ("auth", "auth_methods", "authmethods", "auths")
CAMEL_COLUMNS = ("camel", "camel_components", "camel_component", "camel_scheme")
KESTRA_COLUMNS = ("kestra", "kestra_plugins", "kestra_plugin", "kestra_components")
PRIORITY_COLUMNS = ("priority",)
# The `|` separator keeps a multi-value cell inside one CSV field and cannot
# collide with the ` / ` in a connector's display name (`RabbitMQ / AMQP`).
FIELD_SEPARATOR = "|"


def normalise(header):
    """`Camel Components` -> `camel_components`"""
    return header.strip().lower().replace(" ", "_").replace("-", "_")


def column(row, aliases):
    for alias in aliases:
        if alias in row and row[alias] is not None and str(row[alias]).strip() != "":
            return str(row[alias])
    return ""


def appendix_sections(doc_path=DESIGN_DOC):
    """Row name -> appendix section number, from the design doc.

    §33 §4's `category` enum is not the appendix's seventeen sections: CRM,
    commerce and developer rows are all `saas`, and nosql, search, vector and
    graph share one table. Where the two disagree about a named row, the
    appendix's own placement decides which table the row renders into; the CSV
    keeps §4's enum, which `gen_registry.py` checks against the schema.
    """
    try:
        doc = appendix.load(doc_path)
    except AppendixError:
        return {}
    return {row.name: row.section for row in doc.rows}


def read_rows(path, doc_path=DESIGN_DOC):
    """Read the registry CSV into appendix rows grouped by appendix section."""
    path = Path(path)
    try:
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle)
            if not reader.fieldnames:
                raise AppendixError(f"{path} has no header row")
            fields = {normalise(name): name for name in reader.fieldnames if name}
            raw = [{(normalise(key) if key else ""): value
                    for key, value in record.items()} for record in reader]
    except OSError as error:
        raise AppendixError(f"{path}: {error}") from error
    if not any(name in fields for name in SECTION_COLUMNS):
        raise AppendixError(
            f"{path} has no section column (looked for {', '.join(SECTION_COLUMNS)})")
    placement = appendix_sections(doc_path)
    grouped = {key: [] for key, _, _ in SECTIONS}
    placed_by_name = 0
    for number, record in enumerate(raw, start=2):
        if not any((value or "").strip() for value in record.values()):
            continue
        category = column(record, SECTION_COLUMNS).strip().lower()
        if category not in CATEGORY_SECTION:
            raise AppendixError(
                f"{path} line {number}: category {category!r} is in no appendix "
                f"section (known: {', '.join(sorted(set(CATEGORY_SECTION)))})")
        section = CATEGORY_SECTION[category]
        row = row_from_record(record, number)
        section_of_row = placement.get(row.name)
        if section_of_row and appendix.section_key(section_of_row) != section:
            section = appendix.section_key(section_of_row)
            placed_by_name += 1
        grouped[section].append(row)
    return grouped, placed_by_name


def row_from_record(record, line):
    """One CSV row to one appendix row."""
    name = column(record, NAME_COLUMNS).strip()
    if not name:
        raise AppendixError(f"line {line}: no connector name column")
    starred = column(record, STARRED_COLUMNS).strip().lower() in ("y", "yes", "true", "1")
    if name.startswith(STAR):
        # A generator that wrote the display name verbatim must not produce
        # `★ ★ Kafka`; the prefix is this renderer's job.
        name, starred = name[len(STAR):].strip(), True
    priority = column(record, PRIORITY_COLUMNS).strip() or ("P1" if starred else "P3")
    if starred and priority != "P1":
        raise AppendixError(
            f"line {line}: {name} is starred but its priority is {priority} "
            f"(design §33 §4: starred implies P1)")
    return Row(section="", name=name, starred=starred,
               source=yes_no(record.get("source")),
               sink=yes_no(record.get("sink")),
               streaming=yes_no(record.get("streaming")),
               batch=yes_no(record.get("batch")),
               cdc=yes_no(record.get("cdc")),
               webhook=yes_no(record.get("webhook")),
               auth=[AUTH_PROSE.get(item.strip().lower(), item.strip())
                     for item in appendix.split_items(column(record, AUTH_COLUMNS), FIELD_SEPARATOR)],
               camel=appendix.split_items(column(record, CAMEL_COLUMNS), FIELD_SEPARATOR),
               kestra=appendix.split_items(column(record, KESTRA_COLUMNS), FIELD_SEPARATOR),
               priority=priority, line=line)


def build(grouped):
    """The appendix's sections, in document order, from the grouped rows."""
    sections = []
    for key, number, title in SECTIONS:
        rows = grouped.get(key, [])
        for row in rows:
            row.section = number
        sections.append(Section(number, title, SECTION_NOTES.get(key), rows,
                                SECTION_EPILOGUES.get(key)))
    return sections


def render(csv_path, doc_path=DESIGN_DOC):
    """The generated appendix body for a registry CSV."""
    grouped, _ = read_rows(csv_path, doc_path)
    return Appendix(build(grouped)).render()


def summary(appendix_text, doc_text, sections, placed_by_name=0):
    """The machine-readable summary CI archives."""
    rows = [row for section in sections for row in section.rows]
    return {
        "script": "matrix",
        "status": "rendered" if doc_text is None else (
            "ok" if appendix_text == doc_text else "differs"),
        "catalog_csv": str(CATALOG_CSV),
        "design_doc": str(DESIGN_DOC),
        "rows": len(rows),
        "sections": len(sections),
        "starred": sum(1 for row in rows if row.starred),
        # Rows the appendix places in a different table than their category
        # implies (CRM, commerce and developer rows are all `saas`).
        "placed_by_name": placed_by_name,
        "priorities": {level: sum(1 for row in rows if row.priority == level)
                       for level in ("P1", "P2", "P3")},
        "capabilities": {
            column_name: sum(1 for row in rows if getattr(row, column_name) == "Y")
            for column_name in ("source", "sink", "streaming", "batch", "cdc", "webhook")
        },
        "camel_cells": sum(1 for row in rows if row.camel),
        "kestra_cells": sum(1 for row in rows if row.kestra),
    }


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--csv", default=None,
                        help=f"registry CSV (default: {CATALOG_CSV.relative_to(ROOT)}, "
                             "or $LOAMS_CATALOG_CSV)")
    parser.add_argument("--docs", default=None,
                        help="write the rendered tables here instead of stdout")
    parser.add_argument("--check", action="store_true",
                        help="fail unless the design doc's appendix is byte-identical")
    parser.add_argument("--design-doc", default=None,
                        help=f"design doc to check against (default: {DESIGN_DOC.relative_to(ROOT)})")
    parser.add_argument("--json", default=None,
                        help="write a machine-readable summary here ('-' for stdout)")
    args = parser.parse_args(argv)

    csv_path = Path(args.csv or os.environ.get("LOAMS_CATALOG_CSV") or CATALOG_CSV)
    doc_path = Path(args.design_doc or DESIGN_DOC)

    if not csv_path.exists():
        message = (f"matrix: {csv_path} does not exist; set LOAMS_CATALOG_CSV or pass "
                   "--csv to render the appendix")
        if args.check:
            # A missing registry is a repository state, not an absent service,
            # so --check refuses rather than skipping silently.
            print(message, file=sys.stderr)
            return 1
        print(message + "; skipping")
        return 0

    try:
        grouped, placed_by_name = read_rows(csv_path, doc_path)
    except AppendixError as error:
        print(f"matrix: {error}", file=sys.stderr)
        return 1
    sections = build(grouped)
    rendered = Appendix(sections).render()

    if args.check:
        try:
            expected = appendix.load(doc_path).render()
        except AppendixError as error:
            print(f"matrix: {error}", file=sys.stderr)
            return 1
        report = summary(rendered, expected, sections, placed_by_name)
        if args.json:
            appendix.write_json(report, args.json)
        if rendered == expected:
            appendix.note(f"matrix: appendix A matches {csv_path} "
                          f"({report['rows']} rows in {report['sections']} sections, "
                          f"{report['starred']} starred, {report['placed_by_name']} "
                          f"placed by name)", args.json)
            return 0
        print(f"matrix: the appendix in {doc_path} does not match {csv_path}; "
              f"regenerate it with 'matrix.py --docs' and commit the result", file=sys.stderr)
        sys.stderr.write(appendix.diff(expected, rendered, str(doc_path), str(csv_path)))
        return 1

    if args.docs:
        destination = Path(args.docs)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(rendered, encoding="utf-8")
        appendix.note(f"matrix: wrote {destination} "
                      f"({len(grouped)} sections, {placed_by_name} placed by name)",
                      args.json)
    else:
        sys.stdout.write(rendered)
    if args.json:
        appendix.write_json(summary(rendered, None, sections, placed_by_name), args.json)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
