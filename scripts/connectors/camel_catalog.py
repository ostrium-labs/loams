#!/usr/bin/env python3
"""Check the Camel column of design §33 Appendix A against Camel's own sources.

CN1 plan Task 2: "camel_catalog.py reads camel-catalog 4.22.x's component JSON
(Maven artifact, Apache-2.0) and checks every Camel cell names a real component
and that its producer/consumer support agrees with Sink/Source." That is risk
CN-R1 in the design doc — "the Camel and Kestra columns are regenerated from
their catalogs" — and this is the script that regenerates the truth.

Two truth sources, because neither alone is complete:

  camel-catalog   the JSON models in `org.apache.camel:camel-catalog` (Apache-2.0,
                  Maven Central). It carries `producerOnly`/`consumerOnly`, so it
                  is the only source that can answer the direction question — but
                  it omits components whose model it does not generate.
  apache/camel    the module tree at a pinned tag. `components/*` holds the leaf
                  modules and the group directories (`camel-aws`, `camel-azure`,
                  `camel-debezium`, `camel-ai`, …) whose own sub-module
                  directories hold the grouped components (`camel-aws2-redshift`,
                  `camel-azure-eventgrid`, `camel-mcp-server`, …). Searching only
                  `components/*` misses every grouped component. A module's scheme
                  is its directory name with the leading `camel-` removed.

Severity, over the two:

  error    the name is in neither source, or in camel-catalog but not in
           apache/camel: the manifest would name a component that does not exist.
  warning  the name is a module in apache/camel that camel-catalog has no entry
           for: a catalog gap, not a doc error. Direction findings are warnings
           too, and only read the catalog, so a gap can never become one.
  --strict promotes warnings to failures.

Usage:
    camel_catalog.py [--catalog PATH] [--modules PATH] [--tag TAG] [--repo REPO]
                     [--check] [--strict] [--json PATH]

`$LOAMS_CAMEL_CATALOG` names the jar (or an extracted catalog directory) and
`$LOAMS_CAMEL_MODULES` names a dump of the module list; without a reachable
module tree the check skips and exits 0, mirroring the plan's "tests skip without
services" convention and `loams_fabric_testing::stack()`. Both sources are
fetched over the network, so this runs weekly and on demand, never on every pull
request (CN1 plan Task 2).
"""
from __future__ import annotations

import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent))

import appendix  # noqa: E402  (the sibling module needs the path first)
from appendix import DESIGN_DOC, NO, ROOT, AppendixError  # noqa: E402

# Where `camel-catalog` keeps its model JSON. `components` is the endpoint
# components; the others are the catalog entries a reader can also name — the
# formats section of the appendix names data formats (`csv`, `avro`,
# `protobuf`), and `others` holds the remaining models (`redis` is the Redis
# aggregation repository there, not the Redis connector).
MODEL_SECTIONS = ("components", "others", "dataformats", "languages",
                  "transformers")
CATALOG_PREFIX = "org/apache/camel/catalog/"
# The `kind` a catalog JSON's model object carries. `component` is the only
# kind with a producer/consumer split; the others are named but cannot answer
# it, which is what the kind-mismatch warning reports.
MODEL_KINDS = ("component", "dataformat", "other", "language", "transformer")
# The artifact and version CN1 Task 0 pins (design §33 §5: Camel 4.22.1, 319
# component modules).
CATALOG_ARTIFACT = "org.apache.camel:camel-catalog"
CATALOG_VERSION = "4.22.1"
CATALOG_TAG = f"camel-{CATALOG_VERSION}"
CATALOG_REPO = "apache/camel"
ENV_CATALOG = "LOAMS_CAMEL_CATALOG"
ENV_MODULES = "LOAMS_CAMEL_MODULES"
# The Maven coordinates CI downloads from (Apache-2.0, Maven Central).
MAVEN_URL = ("https://repo1.maven.org/maven2/org/apache/camel/camel-catalog/"
             f"{CATALOG_VERSION}/camel-catalog-{CATALOG_VERSION}.jar")
GITHUB_API = "https://api.github.com"
API_TIMEOUT = 60
# A bare name whose catalog neighbours are a family (`kubernetes-*` has 17
# schemes, `mail-*` has 1) is a bare-name-of-a-family mistake, not a gap: the
# module publishes the family, not the prefix. Three is the smallest count that
# cannot be a single sibling module.
FAMILY_MINIMUM = 3


class CatalogError(Exception):
    """The catalog path is not a catalog this script can read."""


class ModuleError(Exception):
    """The apache/camel module tree could not be read."""


class Entry:
    """One catalog model: a Camel component, data format, or other model."""

    __slots__ = ("scheme", "kind", "title", "artifact", "version",
                 "producer_only", "consumer_only", "deprecated", "source")

    def __init__(self, document, source):
        model = None
        for value in document.values():
            if isinstance(value, dict) and "kind" in value:
                model = value
                break
        if model is None:
            raise CatalogError(f"{source}: no catalog model object")
        # A component is addressed by its URI scheme; the other models have no
        # scheme and are named (`csv` the data format, `redis` the repository).
        self.scheme = model.get("scheme") or model.get("name")
        self.kind = model.get("kind")
        self.title = model.get("title")
        self.artifact = model.get("artifactId")
        self.version = model.get("version")
        self.producer_only = model.get("producerOnly")
        self.consumer_only = model.get("consumerOnly")
        self.deprecated = bool(model.get("deprecated"))
        self.source = source

    def describe(self):
        parts = [self.kind or "model", self.scheme]
        if self.title and self.title != self.scheme:
            parts.append(self.title)
        text = ", ".join(parts)
        if self.deprecated:
            text += " (deprecated upstream)"
        return text


def directions(entry):
    """(produces, consumes) for a catalog entry, or (None, None) if unknown.

    Camel records the endpoint direction on the component itself:
    `producerOnly` means the component has no consumer endpoint and
    `consumerOnly` that it has no producer endpoint. A model kind with neither
    flag cannot answer the question and is reported as unknown rather than
    guessed.
    """
    if entry.producer_only is None and entry.consumer_only is None:
        return None, None
    produces = entry.producer_only is True or entry.consumer_only is not True
    consumes = entry.consumer_only is True or entry.producer_only is not True
    return produces, consumes


def scheme_of(name):
    """The scheme a Camel cell names.

    `camel-iggy` is the module and directory name; the component's URI scheme
    is `iggy` (design §33 §4's `RuntimeRef::Camel { scheme }`). Names that are
    not module names keep their spelling, so `aws2-s3` stays `aws2-s3`.
    """
    name = name.strip()
    if name.startswith("camel-"):
        return name[len("camel-"):]
    return name


def module_scheme(directory):
    """The scheme a module directory publishes: `camel-aws2-s3` -> `aws2-s3`."""
    return scheme_of(directory)


def section_of(root, path):
    """The catalog section a JSON file is in (`components`, `dataformats`, …)."""
    parts = path.parts
    if "catalog" in parts:
        index = len(parts) - 1 - parts[::-1].index("catalog")
        if index + 1 < len(parts) - 1:
            return parts[index + 1]
        return ""
    # A directory of components passed directly (`--catalog .../components`).
    return root.name if root.name in MODEL_SECTIONS else ""


def read_entry(raw, source, skipped):
    """One catalog JSON to an `Entry`, or None when it holds no model object.

    Some catalog sections hold plain configuration JSON with no `kind`; those
    are counted and skipped rather than refused, so a catalog that grows a
    section does not fail this check.
    """
    try:
        document = json.loads(raw)
    except (UnicodeError, ValueError) as error:
        raise CatalogError(f"{source}: {error}") from error
    entry = Entry(document, source)
    if not entry.scheme or entry.kind not in MODEL_KINDS:
        skipped.append(source)
        return None
    return entry


def load_catalog(path):
    """Read a `camel-catalog` jar or an extracted catalog directory.

    Returns (entries, source, skipped) where `skipped` names the JSON files that
    hold no catalog model.
    """
    path = Path(path)
    if not path.exists():
        raise CatalogError(f"{path} does not exist")
    entries = {}
    skipped = []
    if path.is_dir():
        source = "directory"
        for json_path in sorted(path.rglob("*.json")):
            if section_of(path, json_path) not in MODEL_SECTIONS:
                continue
            try:
                raw = json_path.read_bytes()
            except OSError as error:
                raise CatalogError(f"{json_path}: {error}") from error
            entry = read_entry(raw, str(json_path), skipped)
            if entry is not None:
                entries.setdefault(entry.scheme, entry)
    elif zipfile.is_zipfile(path):
        source = "jar"
        with zipfile.ZipFile(path) as jar:
            for info in sorted(jar.infolist(), key=lambda item: item.filename):
                if not info.filename.endswith(".json"):
                    continue
                if not info.filename.startswith(CATALOG_PREFIX):
                    continue
                section = info.filename[len(CATALOG_PREFIX):].split("/", 1)[0]
                if section not in MODEL_SECTIONS:
                    continue
                entry = read_entry(raw=jar.read(info), source=info.filename,
                                   skipped=skipped)
                if entry is not None:
                    entries.setdefault(entry.scheme, entry)
    else:
        raise CatalogError(f"{path} is neither a jar nor a directory")
    if not entries:
        raise CatalogError(f"{path} holds no catalog models under {CATALOG_PREFIX}")
    return entries, source, skipped


# --------------------------------------------------------------------------
# The second truth source: the apache/camel module tree at a pinned tag.


class ModuleTree:
    """Scheme name -> module directory, from `components/*` and its groups."""

    def __init__(self, repo=CATALOG_REPO, tag=CATALOG_TAG):
        self.repo = repo
        self.tag = tag
        self.names = {}
        self.directories = {}     # module directory -> its path in the tree
        self.groups = []          # top-level directories whose children were read
        self.calls = 0
        self.capped = False
        # True when every wanted name was either answered or known to have no
        # module. Without the catalog there is nothing to sweep the group
        # directories for, so the tree is partial and an unanswered name is
        # unverified rather than wrong.
        self.complete = True
        self.transport = None

    def add(self, scheme, path):
        self.names.setdefault(scheme, path)

    def add_directory(self, directory, path):
        self.directories.setdefault(directory, path)
        self.add(module_scheme(directory), path)

    def link(self, entries):
        """Tie each catalog scheme to the module its `artifactId` names.

        A module directory is not always the scheme's own name — `camel-ftp`
        publishes `ftp`, `ftps` and `sftp`, `camel-cassandraql` publishes `cql` —
        so the catalog's own artifactId is what maps those. Only schemes the
        catalog already has are added, and only for a module that is really in
        the tree, so this can neither invent a scheme nor excuse a missing one.
        """
        for scheme, entry in entries.items():
            artifact = entry.artifact
            if artifact and artifact in self.directories:
                self.add(scheme, self.directories[artifact])

    def resolve(self, name):
        """Every module a cell name can mean: itself, or a `prefix-*` family."""
        scheme = scheme_of(name)
        if scheme in self.names:
            return [self.names[scheme]]
        if "*" in scheme or "?" in scheme:
            return [self.names[key] for key in sorted(self.names)
                    if fnmatch.fnmatchcase(key, scheme)]
        return []


def github(path):
    """One GitHub contents call: `gh api` when available, else the plain API.

    Returns (names, transport). Raises `ModuleError` with the reason when
    neither route answers, so the caller can skip instead of failing.
    """
    if shutil.which("gh"):
        try:
            completed = subprocess.run(["gh", "api", path, "--jq", ".[].name"],
                                       check=True, capture_output=True, text=True,
                                       timeout=API_TIMEOUT)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as error:
            detail = getattr(error, "stderr", "") or ""
            message = detail.strip().splitlines()
            raise ModuleError(
                f"`gh api {path}` failed"
                + (f": {message[-1]}" if message else f": {error}")) from error
        return [line for line in completed.stdout.splitlines() if line], "gh api"
    request = urllib.request.Request(f"{GITHUB_API}/{path}",
                                     headers={"Accept": "application/vnd.github+json",
                                              "User-Agent": "loams-camel-catalog"})
    try:
        with urllib.request.urlopen(request, timeout=API_TIMEOUT) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise ModuleError(
            f"{GITHUB_API}/{path} failed: {error}; install the GitHub CLI or "
            f"pass --modules <file>") from error
    return [item["name"] for item in payload if item.get("type") == "dir"], "api.github.com"


def read_modules(path):
    """A dump of module names, one per line (`camel-aws2-s3` or `camel-g/camel-x`)."""
    path = Path(path)
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ModuleError(f"{path}: {error}") from error
    tree = ModuleTree()
    tree.transport = "file"
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        directory = line.rsplit("/", 1)[-1]
        if not directory.startswith("camel-"):
            continue
        tree.add_directory(directory, line)
    if not tree.names:
        raise ModuleError(f"{path} lists no camel-* module directories")
    return tree


def normalise_tag(tag):
    """`4.22.1` -> `camel-4.22.1`: Camel's tags carry the `camel-` prefix."""
    return tag if tag.startswith("camel-") else f"camel-{tag}"


def fetch_modules(repo, tag, entries, wanted, sweep_limit):
    """The module tree, reading only the group directories the names need.

    `components/*` is one call. A name can be a sub-module of a group
    directory (`camel-aws2-redshift` under `camel-aws`), so the group
    directories that could hold a wanted name are read next, longest prefix
    first, and anything still unresolved is looked for in an alphabetical sweep
    of the remaining directories. A group need not share a prefix with its
    children — `camel-ai` holds `camel-mcp-server` and `camel-openai` — which is
    why the sweep exists. The sweep only runs when the catalog says which
    module a name belongs to; without a catalog there is nothing to sweep for.
    """
    tree = ModuleTree(repo, tag)
    tree.calls += 1
    names, tree.transport = github(
        f"repos/{repo}/contents/components?ref={tag}")
    directories = sorted(name for name in names if name.startswith("camel-"))
    for directory in directories:
        tree.add_directory(directory, f"components/{directory}")
    tree.link(entries)
    # Only a name nothing has answered yet is worth another call, so the group
    # directories read are the ones that could hold one of those.
    missing = [name for name in wanted if not tree.resolve(name)]
    for directory in sorted(directories, key=lambda item: (-len(item), item)):
        if not missing:
            break
        if _could_hold(directory, missing):
            _read_group(tree, directory)
            tree.link(entries)
            missing = [name for name in missing if not tree.resolve(name)]
    if missing and entries:
        for directory in directories:
            if not missing:
                break
            if directory in tree.groups or tree.calls >= sweep_limit:
                continue
            _read_group(tree, directory)
            tree.link(entries)
            missing = [name for name in missing if not tree.resolve(name)]
        tree.capped = bool(missing)
    tree.complete = not missing
    return tree


def _could_hold(directory, wanted):
    """Whether a group directory's children could answer a wanted name."""
    stripped = module_scheme(directory)
    for name in wanted:
        literal = scheme_of(name).split("*", 1)[0].split("?", 1)[0].rstrip("-")
        if stripped == literal or literal.startswith(stripped):
            return True
    return False


def _read_group(tree, directory):
    """Read one group directory's sub-module directories."""
    if directory in tree.groups:
        return
    tree.groups.append(directory)
    tree.calls += 1
    children, _ = github(f"repos/{tree.repo}/contents/components/{directory}"
                         f"?ref={tree.tag}")
    for child in children:
        if not child.startswith("camel-"):
            continue
        tree.add_directory(child, f"components/{directory}/{child}")


# --------------------------------------------------------------------------
# The checks.


def resolve(name, entries):
    """Every catalog entry a cell name can mean: itself, or a `prefix-*` family.

    The appendix names families as globs (`langchain4j-*`, `debezium-*`), and a
    family that matches nothing is as wrong as a name that does not exist.
    """
    scheme = scheme_of(name)
    if scheme in entries:
        return [entries[scheme]]
    if "*" in scheme or "?" in scheme:
        return [entry for key, entry in sorted(entries.items())
                if fnmatch.fnmatchcase(key, scheme)]
    return []


def gap_truth(name, entries):
    """Why a module the catalog does not describe is still probably right.

    Two shapes are found in Appendix A and are the difference between a catalog
    gap and a cell that needs an edit:

    * the module publishes a *different* scheme than the cell names — the
      catalog entry whose artifactId is this module says so;
    * the cell names a bare prefix that the catalog only publishes as a family
      (`kubernetes` against seventeen `kubernetes-*` schemes).

    Returns (truth, needs_edit, alternatives).
    """
    scheme = scheme_of(name)
    module = f"camel-{scheme}"
    # Only a component has a URI scheme. A data format or another model is named
    # by its Java model name — `parquetAvro`, `avro-binary`, `imap` — which is
    # not what a route writes, so those never rename a scheme.
    renamed = sorted(entry.scheme for entry in entries.values()
                     if entry.kind == "component" and entry.artifact == module
                     and entry.scheme != scheme)
    if renamed:
        listed = ", ".join(f"`{item}`" for item in renamed[:3])
        more = f" and {len(renamed) - 3} more" if len(renamed) > 3 else ""
        return (f"module {module} publishes {listed}{more}, not `{scheme}`",
                True, renamed)
    family = sorted(scheme for scheme in entries
                    if scheme.startswith(f"{scheme_of(name)}-"))
    if len(family) >= FAMILY_MINIMUM:
        return (f"the catalog publishes no `{scheme_of(name)}` scheme but "
                f"{len(family)} `{scheme_of(name)}-*` ones "
                f"({', '.join(f'`{item}`' for item in family[:3])}, …)",
                True, family)
    return (f"no entry for it in {CATALOG_ARTIFACT} {CATALOG_VERSION}",
            False, [])


def check(rows, entries, modules):
    """Findings for every Camel cell in the appendix.

    Existence is decided by both sources; the direction only ever comes from
    camel-catalog, and only for a name both sources agree on.
    """
    findings = []
    cells = names = 0
    for row in rows:
        # A.6's note defines its Source and Sink as decode and encode, so a
        # data format is the covering thing there; anywhere else a data format
        # is a mismatch worth reporting.
        codecs = appendix.section_key(row.section) == "format"
        covers = []
        for item in row.camel:
            name = appendix.name_of(item)
            if not name or name == NO or name.lower() in appendix.LITERAL_NAMES:
                # `·` (nothing covers it), `(itself)` and `core (...)`: the row
                # is the runtime, or nothing (design §33 D354).
                continue
            names += 1
            models = resolve(name, entries)
            found = modules.resolve(name)
            if not models and not found:
                if not modules.complete:
                    # The group directories were not all read (there was no
                    # catalog to say which module a name belongs to), so this is
                    # unverified rather than wrong.
                    findings.append(finding(
                        "warning", row, name,
                        f"unverified: neither {CATALOG_ARTIFACT} {CATALOG_VERSION} "
                        f"nor a read part of {modules.repo}@{modules.tag} has "
                        f"'{scheme_of(name)}'"))
                    continue
                findings.append(finding(
                    "error", row, name,
                    f"neither {CATALOG_ARTIFACT} {CATALOG_VERSION} nor "
                    f"{modules.repo}@{modules.tag} has '{scheme_of(name)}'"))
                continue
            if models and not found and not modules.complete:
                findings.append(finding(
                    "warning", row, name,
                    f"unverified: {CATALOG_ARTIFACT} has '{scheme_of(name)}' but "
                    f"the module behind it is in a group directory that was not "
                    f"read"))
                continue
            if models and not found:
                findings.append(finding(
                    "error", row, name,
                    f"{CATALOG_ARTIFACT} {CATALOG_VERSION} has "
                    f"'{scheme_of(name)}' ({', '.join(entry.artifact or '?' for entry in models)}) "
                    f"but {modules.repo}@{modules.tag} has no module for it"))
                continue
            if found and not models:
                if not entries:
                    # No catalog, so there is no gap to report and no direction
                    # to check; the name is a module and that is all this run can
                    # say about it.
                    continue
                # A catalog gap, never a direction finding: there is no catalog
                # entry, so there is no producer/consumer truth to disagree with.
                truth, needs_edit, alternatives = gap_truth(name, entries)
                findings.append(finding("warning", row, name, truth,
                                        needs_edit=needs_edit,
                                        alternatives=alternatives,
                                        modules=found))
                continue
            components = [entry for entry in models if entry.kind == "component"]
            if components:
                covers.extend(directions(entry) + (entry.scheme,) for entry in components)
                continue
            formats = [entry for entry in models if entry.kind == "dataformat"]
            if formats and codecs:
                covers.append((True, True, f"{join_names([f.scheme for f in formats])} "
                                           f"(data format, decode and encode)"))
                continue
            # Nothing in the family is an endpoint component. A family with one
            # real component is fine, so this only fires when none is left.
            if len(models) == 1:
                truth = (f"the catalog has it as kind '{models[0].kind}' "
                         f"({models[0].artifact or 'no artifact'}), not an endpoint "
                         f"component")
            else:
                truth = (f"no endpoint component matches it: "
                         f"{join_names([entry.describe() for entry in models])}")
            findings.append(finding("warning", row, name, truth, modules=found))
        if not covers:
            continue
        cells += 1
        if row.sink == "Y" and not any(produces for produces, _, _ in covers):
            findings.append(finding(
                "warning", row, ", ".join(label for _, _, label in covers),
                f"Sink is Y but nothing the cell names can produce "
                f"({describe_direction(covers, 0)})"))
        if row.source == "Y" and not any(consumes for _, consumes, _ in covers):
            findings.append(finding(
                "warning", row, ", ".join(label for _, _, label in covers),
                f"Source is Y but nothing the cell names can consume "
                f"({describe_direction(covers, 1)})"))
    return findings, cells, names


def describe_direction(covers, index):
    """Why nothing a cell names can do what the row needs."""
    verb = "produce" if index == 0 else "consume"
    opposite = "consumer-only" if index == 0 else "producer-only"
    kinds = {True: [], False: [], None: []}
    for produces, consumes, label in covers:
        kinds[(produces, consumes)[index]].append(label)
    parts = []
    if kinds[True]:
        parts.append(f"{join_names(kinds[True])} can {verb}")
    if kinds[False]:
        parts.append(f"{join_names(kinds[False])} "
                     f"{'are' if len(kinds[False]) > 1 else 'is'} {opposite}")
    if kinds[None]:
        parts.append(f"the catalog records no direction for {join_names(kinds[None])}")
    return "; ".join(parts)


def join_names(names):
    if len(names) == 1:
        return names[0]
    return ", ".join(names[:-1]) + " and " + names[-1]


def finding(severity, row, cell, truth, needs_edit=False, alternatives=(),
            modules=()):
    return {
        "severity": severity,
        "section": row.section,
        "connector": row.name,
        "starred": row.starred,
        "line": row.line,
        "cell": "Camel",
        "named": cell,
        "truth": truth,
        # True when the cell itself looks wrong, as opposed to the catalog
        # being behind; `docs/design/33-connectors.md` needs an edit for these.
        "needs_edit": needs_edit,
        "alternatives": list(alternatives),
        "modules": list(modules),
        "message": f"{row.section} {row.label}: the Camel cell names `{cell}` but {truth}",
    }


def report(findings):
    for item in findings:
        print(f"camel_catalog: {item['severity']}: {item['message']}", file=sys.stderr)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--catalog", default=None,
                        help=f"camel-catalog jar or extracted directory "
                             f"(default: ${ENV_CATALOG})")
    parser.add_argument("--modules", default=None,
                        help=f"module list dump (default: ${ENV_MODULES}, else the "
                             f"GitHub API)")
    parser.add_argument("--repo", default=CATALOG_REPO,
                        help=f"the module tree's repository (default: {CATALOG_REPO})")
    parser.add_argument("--tag", default=CATALOG_VERSION,
                        help=f"the Camel version whose module tree to read; the "
                             f"`camel-` prefix is added when missing "
                             f"(default: {CATALOG_VERSION})")
    parser.add_argument("--from-github", action="store_true",
                        help="read the module tree from the GitHub API (a network "
                             "call; --check implies it)")
    parser.add_argument("--sweep-limit", type=int, default=200,
                        help="cap on the group-directory sweep (default: 200)")
    parser.add_argument("--docs", default=None,
                        help=f"design doc to read (default: {DESIGN_DOC.relative_to(ROOT)})")
    parser.add_argument("--check", action="store_true",
                        help="exit non-zero when a named scheme is in neither source")
    parser.add_argument("--strict", action="store_true",
                        help="treat warnings as failures")
    parser.add_argument("--json", default=None,
                        help="write the machine-readable summary here ('-' for stdout)")
    args = parser.parse_args(argv)

    try:
        doc = appendix.load(args.docs or DESIGN_DOC)
    except AppendixError as error:
        print(f"camel_catalog: {error}", file=sys.stderr)
        return 1
    # The names the appendix asks about decide which group directories are
    # worth reading, so they are known before the module tree is fetched.
    wanted = sorted({appendix.name_of(item) for row in doc.rows for item in row.camel}
                    - {NO, ""} - {item for item in appendix.LITERAL_NAMES})
    wanted = [name for name in wanted if name]

    catalog_path = args.catalog or os.environ.get(ENV_CATALOG)
    entries, catalog_source, skipped = {}, None, []
    catalog_error = None
    if catalog_path:
        try:
            entries, catalog_source, skipped = load_catalog(catalog_path)
        except CatalogError as error:
            catalog_error = str(error)
    dump = args.modules or os.environ.get(ENV_MODULES)
    if not dump and not (args.from_github or args.check):
        # Nothing asked for the module tree and nothing named a catalog: this is
        # a bare run, and the module tree is a network fetch, so skip rather than
        # spend it (CN1 plan Task 2's "tests skip without services").
        message = (f"camel_catalog: no catalog and no module list given; set "
                   f"{ENV_CATALOG} and {ENV_MODULES}, or pass --check (which reads "
                   f"the module tree from the GitHub API at "
                   f"{CATALOG_REPO}@{normalise_tag(CATALOG_VERSION)}) "
                   f"to run the Camel column check; skipping")
        if args.json:
            appendix.write_json({"script": "camel_catalog", "status": "skipped",
                                 "skip": message, "findings": []}, args.json)
        print(message)
        return 0
    if not entries:
        # Without the catalog the direction checks are impossible, but the
        # existence half still runs, so this degrades instead of skipping.
        message = (f"camel_catalog: no camel-catalog given; set {ENV_CATALOG} or pass "
                   f"--catalog <camel-catalog jar> (CI downloads {MAVEN_URL}); the "
                   f"direction checks are skipped")
        if catalog_error:
            message = f"camel_catalog: {catalog_error}; the direction checks are skipped"
        appendix.note(message, args.json)

    try:
        if dump:
            modules = read_modules(dump)
            modules.link(entries)
        else:
            modules = fetch_modules(args.repo, normalise_tag(args.tag), entries,
                                    wanted, args.sweep_limit)
    except ModuleError as error:
        # The module tree is the source that decides whether a name exists at
        # all; without it every finding would be a guess, so skip (CN1 plan
        # Task 2: weekly, on demand, and never a hard failure on a fetch).
        message = (f"camel_catalog: {error}; skipping")
        if args.json:
            appendix.write_json({"script": "camel_catalog", "status": "skipped",
                                 "skip": message, "findings": []}, args.json)
        print(message)
        return 0

    findings, cells, names = check(doc.rows, entries, modules)
    errors = [item for item in findings if item["severity"] == "error"]
    warnings = [item for item in findings if item["severity"] == "warning"]
    needs_edit = [item for item in findings if item["needs_edit"]]
    versions = sorted({entry.version for entry in entries.values() if entry.version})
    summary = {
        "script": "camel_catalog",
        "status": "findings" if findings else "ok",
        "catalog": None if not entries else {
            "path": str(catalog_path),
            "source": catalog_source,
            "artifact": CATALOG_ARTIFACT,
            "expected_version": CATALOG_VERSION,
            "versions": versions,
            "entries": len(entries),
            "components": sum(1 for entry in entries.values()
                              if entry.kind == "component"),
            "skipped_files": len(skipped),
        },
        "modules": {
            "repo": modules.repo,
            "tag": modules.tag,
            "source": modules.transport,
            "modules": len(modules.names),
            "group_directories_read": len(modules.groups),
            "api_calls": modules.calls,
            "group_directories": len(modules.directories),
            "complete": modules.complete,
            "sweep_capped": modules.capped,
        },
        "design_doc": str(args.docs or DESIGN_DOC),
        "rows": len(doc.rows),
        "camel_cells": cells,
        "names_checked": names,
        "counts": {"error": len(errors), "warning": len(warnings),
                   "needs_edit": len(needs_edit)},
        "needs_edit": [item["message"] for item in needs_edit],
        "findings": findings,
    }
    if args.json:
        appendix.write_json(summary, args.json)
    report(findings)
    components = summary["catalog"]["components"] if entries else 0
    appendix.note(f"camel_catalog: {cells} Camel cells, {names} names checked against "
                 f"{len(entries)} catalog entries ({components} components, "
                 f"{versions[0] if versions else 'catalog absent'}) and "
                 f"{len(modules.names)} modules at {modules.repo}@{modules.tag} "
                 f"({modules.calls} API calls); {len(errors)} error(s), "
                 f"{len(warnings)} warning(s), {len(needs_edit)} needing a doc edit",
                 args.json)
    if args.check and (errors or (args.strict and warnings)):
        if errors:
            print(f"camel_catalog: {len(errors)} name(s) are in neither source; the "
                  f"registry would point at components that do not exist", file=sys.stderr)
        else:
            print(f"camel_catalog: --strict: {len(warnings)} warning(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
