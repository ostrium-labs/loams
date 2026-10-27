#!/usr/bin/env python3
"""Check and list Loams Flow's connector profiles (CN2 plan Task 7).

Sources of truth:

  connectors/schema/profile.schema.json   the profile shape, the family modules and
                                          the target / http-api rule (R-CN2-2)
  connectors/profiles/<id>.yaml           one target or HTTP-API profile each
  connectors/registry/<id>.yaml           the manifests whose runtime.profile names one

Modes:

  --check   (default) check every profile and every manifest's runtime.profile, print
            every problem and exit non-zero on any. The schema is applied with the
            `jsonschema` module when it is installed; the rules below run either way.
  --list    print one line per profile: id, kind, module, the real system and its service.
  --self-test  run the checks over built-in fixtures that hold a credential, and fail if
            any printed line contains it (issue #1105).

The rules mirror `fabric/crates/loams-flow/src/profile.rs`, which is the authority at
run time:

  * the id is the file name;
  * a target's module is a family module and it has no `http` block; an http-api
    profile's module is `http` and it has one;
  * no key anywhere names a credential (`token_url` is a URL), no fixed header
    carries one, and `http.base_url` has no user information (Review Focus 8);
  * every `narrows` path names a capability, and a manifest that uses the profile
    explains each with a note that starts `narrows: <path>`;
  * a manifest's runtime.profile names an existing profile.

A refusal names the key, never the value. A file that is not valid YAML is reported by
line and column, never by the parser's excerpt of the line, which could be a credential.
Every printed line also passes through `redact`, which blanks out any value a
credential-named key or a URL's user information holds.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import re
import sys
import tempfile
from pathlib import Path

try:
    import yaml  # type: ignore
except ImportError:  # pragma: no cover - the CI image has PyYAML
    print("profiles: PyYAML is required", file=sys.stderr)
    sys.exit(2)

ROOT = Path(__file__).resolve().parents[2]
CONNECTORS = ROOT / "connectors"
SCHEMA = CONNECTORS / "schema" / "profile.schema.json"
PROFILES = CONNECTORS / "profiles"
REGISTRY = CONNECTORS / "registry"

SECRET_KEY_PARTS = (
    "password", "passwd", "secret", "token", "api_key", "apikey", "private_key",
    "access_key", "credential", "authorization", "cookie", "session",
)
NOT_SECRET_KEYS = {"token_url"}
SOURCE_FLAGS = {"streaming", "batch", "cdc", "webhook", "resumable"}
SINK_FLAGS = {"streaming", "batch", "transactional", "upsert", "delete", "idempotent", "reply"}


def is_secret_key(key: str) -> bool:
    k = key.lower().replace("-", "_")
    return k not in NOT_SECRET_KEYS and any(p in k for p in SECRET_KEY_PARTS)


SECRET_REF = re.compile(r"^\{auth\.[a-z0-9_]+\}$")


def is_secret_ref(value) -> bool:
    return isinstance(value, str) and SECRET_REF.match(value) is not None


def credential_field_paths(value, at: str = "") -> list[str]:
    """One problem per credential-named key under `value`, naming its path, never its value."""
    problems: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            here = f"{at}.{key}" if at else str(key)
            # A credential-named key whose value is a secret reference
            # (`{auth.<field>}`, resolved by the runtime) holds no credential
            # (CN2 Task 20: PostHog's `api_key` body field).
            if is_secret_ref(child):
                continue
            if at != "http.resources" and is_secret_key(str(key)):
                problems.append(f"{here}: a profile never holds a credential (Review Focus 8)")
                continue
            problems.extend(credential_field_paths(child, here))
    elif isinstance(value, list):
        for i, child in enumerate(value):
            problems.extend(credential_field_paths(child, f"{at}.{i}"))
    return problems


def narrow_problem(path: str, formats: set[str]) -> str | None:
    head, _, tail = path.partition(".")
    ok = (
        (head in ("source", "sink") and not tail)
        or (head == "source" and tail in SOURCE_FLAGS)
        or (head == "sink" and tail in SINK_FLAGS)
        or (head == "bulk" and tail == "arrow")
        or (head == "formats" and tail in formats)
    )
    return None if ok else f"narrows: {path!r} names no capability"


def yaml_error_summary(error: yaml.YAMLError) -> str:
    """What PyYAML refused and where, without the excerpt of the line it quotes."""
    mark = getattr(error, "problem_mark", None)
    where = f" at line {mark.line + 1}, column {mark.column + 1}" if mark is not None else ""
    return f"not valid YAML{where}"


def credential_values(value, under_credential: bool = False) -> set[str]:
    """Every string a credential-named key holds anywhere under `value`, and the user
    information of every URL, so `redact` can blank them out of a printed line."""
    found: set[str] = set()
    if isinstance(value, dict):
        for key, child in value.items():
            found |= credential_values(child, under_credential or is_secret_key(str(key)))
    elif isinstance(value, list):
        for child in value:
            found |= credential_values(child, under_credential)
    elif isinstance(value, str):
        if under_credential and value and not is_secret_ref(value):
            found.add(value)
        authority = value.split("://", 1)[1].split("/", 1)[0] if "://" in value else ""
        if "@" in authority:
            found.add(authority.rsplit("@", 1)[0])
    return found


def redact(line: str, doc) -> str:
    """`line` with any credential `doc` holds replaced by `[redacted]`."""
    for value in sorted(credential_values(doc), key=len, reverse=True):
        if len(value) >= 3:
            line = line.replace(value, "[redacted]")
    return line


def check_profile(path: Path, schema: dict, validator, formats: set[str]) -> tuple[dict | None, list[str]]:
    try:
        doc = yaml.safe_load(path.read_text())
    except OSError as error:
        return None, [f"cannot read: {error.strerror or type(error).__name__}"]
    except yaml.YAMLError as error:
        return None, [f"cannot read: {yaml_error_summary(error)}"]
    if not isinstance(doc, dict):
        return None, ["not a YAML mapping"]
    problems: list[str] = []
    if validator is not None:
        for error in sorted(validator.iter_errors(doc), key=lambda e: list(e.absolute_path)):
            where = ".".join(str(p) for p in error.absolute_path) or "profile"
            # The validator's message may quote the value; report the keyword only.
            problems.append(f"{where}: fails the schema's {error.validator!r}")
    modules = schema["properties"]["module"]["enum"]
    for key in ("apiVersion", "kind", "id", "profile_kind", "module", "system"):
        if key not in doc:
            problems.append(f"{key}: required")
    system = doc.get("system")
    if isinstance(system, dict):
        for key in ("name", "service"):
            if key not in system:
                problems.append(f"system.{key}: required (OR-2: the real system the suite runs against)")
    if doc.get("id") != path.stem:
        problems.append(f"id: {doc.get('id')!r} is not the file name ({path.name})")
    kind, module = doc.get("profile_kind"), doc.get("module")
    if kind not in ("target", "http-api"):
        problems.append(f"profile_kind: {kind!r} is not target or http-api")
    if module not in modules:
        problems.append(f"module: {module!r} is not a family module")
    if kind == "http-api" and (module != "http" or "http" not in doc):
        problems.append("module: an http-api profile runs on `http` and has an `http` block")
    if kind == "target" and "http" in doc:
        problems.append("http: only an http-api profile has an `http` block")
    problems.extend(credential_field_paths(doc))
    base_url = ((doc.get("http") or {}).get("base_url") or "")
    if "@" in base_url.split("://", 1)[-1].split("/", 1)[0]:
        problems.append("http.base_url: a URL with user information holds a credential")
    for narrowed in doc.get("narrows") or []:
        problem = narrow_problem(str(narrowed), formats)
        if problem:
            problems.append(problem)
    return doc, problems


def manifest_formats() -> set[str]:
    manifest_schema = json.loads((CONNECTORS / "schema" / "connector.schema.json").read_text())
    found: set[str] = set()

    def walk(node):
        if isinstance(node, dict):
            if "enum" in node and "cloudevents-binary" in node["enum"]:
                found.update(node["enum"])
            for child in node.values():
                walk(child)
        elif isinstance(node, list):
            for child in node:
                walk(child)

    walk(manifest_schema)
    return found


def check_profiles(directory: Path, schema: dict, validator, formats: set[str]) -> tuple[dict[str, dict], int]:
    """Check every profile in `directory`, printing each problem (redacted); the valid
    profiles by id, and the number of problems."""
    profiles: dict[str, dict] = {}
    failures = 0
    for path in sorted(directory.glob("*.yaml")):
        doc, problems = check_profile(path, schema, validator, formats)
        where = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path.name
        for problem in problems:
            print(redact(f"{where}: {problem}", doc))
        failures += len(problems)
        if doc is not None and not problems:
            profiles[doc["id"]] = doc
    return profiles, failures


# A value no real profile holds; the self-test fails if any printed line contains it.
SENTINEL = "s3ntinel-Credential-9f2c"

SELF_TEST_FIXTURES = {
    # A credential under a credential-named key, nested, and in a fixed header.
    "holds-a-key.yaml": f"""\
apiVersion: loams.flow/v1
kind: Profile
id: holds-a-key
profile_kind: http-api
module: http
system: {{ name: Example, service: {{ fixture: none }} }}
http:
  base_url: "https://api.example.com"
  headers: {{ Authorization: "Bearer {SENTINEL}" }}
  auth: {{ scheme: bearer, client_secret: "{SENTINEL}" }}
defaults: {{ password: "{SENTINEL}" }}
""",
    # User information in the base URL.
    "userinfo-url.yaml": f"""\
apiVersion: loams.flow/v1
kind: Profile
id: userinfo-url
profile_kind: http-api
module: http
system: {{ name: Example, service: {{ fixture: none }} }}
http: {{ base_url: "https://bot:{SENTINEL}@api.example.com/v1" }}
""",
    # Not YAML: PyYAML's own message quotes the offending line, credential and all.
    "not-yaml.yaml": f"""\
apiVersion: loams.flow/v1
defaults:
  token: {SENTINEL}: [unclosed
""",
    # A credential where a non-credential value is reported (the id).
    "wrong-name.yaml": f"""\
apiVersion: loams.flow/v1
kind: Profile
id: wrong-name-{SENTINEL}
profile_kind: target
module: nope
system: {{ name: Example, service: {{ fixture: none }} }}
defaults: {{ api_key: "{SENTINEL}" }}
""",
}


def self_test(schema: dict, validator, formats: set[str]) -> int:
    """Every fixture is refused, and no printed line carries its credential."""
    failures: list[str] = []
    with tempfile.TemporaryDirectory(prefix="loams-profiles-") as tmp:
        directory = Path(tmp)
        for name, text in SELF_TEST_FIXTURES.items():
            (directory / name).write_text(text)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            _, problems = check_profiles(directory, schema, validator, formats)
        lines = out.getvalue().splitlines()
    for name in SELF_TEST_FIXTURES:
        if not any(line.startswith(f"{name}: ") for line in lines):
            failures.append(f"{name}: not refused")
    for line in lines:
        if SENTINEL in line or SENTINEL[:12] in line:
            failures.append(f"a printed line carries the credential: {line.replace(SENTINEL, '<SENTINEL>')}")
    if problems != len(lines):
        failures.append(f"{problems} problem(s) but {len(lines)} printed line(s)")
    for failure in failures:
        print(f"profiles: self-test FAIL: {failure}")
    print(f"profiles: self-test {len(failures)} failure(s) over {len(lines)} refusal(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--check", action="store_true", help="check (the default)")
    parser.add_argument("--list", action="store_true", help="list the profiles")
    parser.add_argument("--self-test", action="store_true", help="the credential-redaction fixtures")
    args = parser.parse_args()

    schema = json.loads(SCHEMA.read_text())
    try:
        import jsonschema  # type: ignore

        validator = jsonschema.Draft202012Validator(schema)
    except ImportError:
        validator = None
        if not args.list:
            print("profiles: the `jsonschema` module is not installed; rules-only check")
    formats = manifest_formats()
    if args.self_test:
        return self_test(schema, validator, formats)

    profiles, failures = check_profiles(PROFILES, schema, validator, formats)

    if args.list:
        for pid, doc in profiles.items():
            system = doc["system"]
            print(f"{pid}\t{doc['profile_kind']}\t{doc['module']}\t{system['name']}\t{json.dumps(system['service'])}")
        return 1 if failures else 0

    for path in sorted(REGISTRY.glob("*.yaml")):
        manifest = yaml.safe_load(path.read_text()) or {}
        pid = (manifest.get("runtime") or {}).get("profile")
        if not pid:
            continue
        where = path.relative_to(ROOT)
        if pid not in profiles:
            print(f"{where}: runtime.profile {pid!r} is not a valid profile under connectors/profiles/")
            failures += 1
            continue
        notes = manifest.get("notes") or []
        for narrowed in profiles[pid].get("narrows") or []:
            prefix = f"narrows: {narrowed}"
            if not any(re.match(re.escape(prefix) + r"($|[\s:,;])", n) for n in notes):
                print(f"{where}: the profile {pid!r} narrows {narrowed} and no `{prefix} …` note says why")
                failures += 1

    if failures:
        print(f"profiles: {failures} problem(s)")
        return 1
    print(f"profiles: {len(profiles)} profile(s) ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
