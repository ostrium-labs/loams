#!/usr/bin/env python3
"""Turn a directory of packages into an apt, a dnf and a pacman repository.

One script, three repository layouts, because a distribution needs all three and
each has one tool that already knows its own format:

    apt      generated here, in Python. The format is a stanzas file plus a
             Release file with a checksum per file, and writing it directly is
             what makes this testable on any machine: the self-test builds a real
             one from a fixture .deb and reads every field back. dpkg-scanpackages
             and apt-ftparchive would do the same job but only exist on Debian.
    dnf      `createrepo_c`, the canonical tool, which is what DNF, YUM and
             microdnf all read. Not on PATH on most non-RPM machines, so it is
             optional here and this script says so when it is missing.
    pacman   `repo-add`, from the pacman package, which is the only thing that
             should write a pacman database. Verified 2026-10-04 against pacman
             6.x: a repository this produces answers `pacman -Si loams`.

Signing, and the rule that shapes this script: **an unsigned package is never
published.** By default an unsigned package is a hard failure, because the only
thing this script produces is a repository and a repository with an unsigned
package in it is a public registry publishing unsigned artifacts. `--allow-unsigned`
exists for exactly one purpose -- building the tree in CI so a maintainer can
look at it before the signing handoff is done -- and it prints a warning, marks
the tree in `loams-repo.json`, and the workflow never publishes a tree built
with it.

Usage:
    make-repo.py apt     --packages DIR --out DIR [--component stable]
                         [--codename loams] [--version 0.1.0] [--arch amd64]
                         [--gpg-key FILE] [--gpg-key-id ID]
    make-repo.py dnf     --packages DIR --out DIR [--arch x86_64]
    make-repo.py pacman  --packages DIR --out DIR [--arch x86_64]
                         [--gpg-key FILE] [--gpg-key-id ID]
    make-repo.py --self-test
"""
from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
ORIGIN = "ostrium-labs"
LABEL = "Loams"
ARCHIVE_MAGIC = {"deb": b"!<arch>\n", "rpm": bytes.fromhex("edabeedb")}


def warn(message: str) -> None:
    print(f"::warning::{message}", file=sys.stderr)

def ensure_key_imported(path: Path) -> None:
    """Import the private key into the current GnuPG home before signing with it.

    A key file on disk is not a key in the keyring, and `gpg --local-user` looks
    only in the keyring: without this, every signature step fails with "No secret
    key" (verified 2026-10-04). Importing twice is a no-op with --batch.
    """
    result = subprocess.run(
        ["gpg", "--batch", "--import", str(path)], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(f"make-repo: gpg could not import {path}")



def note(message: str) -> None:
    print(message, file=sys.stderr)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def md5(path: Path) -> str:
    return hashlib.md5(path.read_bytes()).hexdigest()


def sha1(path: Path) -> str:
    return hashlib.sha1(path.read_bytes()).hexdigest()


# --------------------------------------------------------------------------
# finding and checking the packages
# --------------------------------------------------------------------------

POOL = {"deb": "pool/main/l/loams"}


def find_packages(directory: Path) -> list[Path]:
    packages = sorted(p for p in directory.iterdir() if p.is_file() and _format_of(p))
    if not packages:
        raise SystemExit(f"make-repo: no packages in {directory}")
    return packages


def _format_of(path: Path) -> str | None:
    if path.name.endswith(".deb"):
        return "deb"
    if path.name.endswith(".rpm"):
        return "rpm"
    if path.name.endswith(".pkg.tar.zst"):
        return "archlinux"
    return None


def is_signed(path: Path) -> bool:
    """Whether the package carries a signature, read off the file.

    Same three tests build-artifacts.py uses, for the same reason: a key being
    present is not a signature being present.
    """
    fmt = _format_of(path)
    data = path.read_bytes()
    if fmt == "deb":
        if data[:8] != ARCHIVE_MAGIC["deb"]:
            return False
        offset = 8
        while offset + 60 <= len(data):
            name = data[offset : offset + 16].strip(b" ").rstrip(b"/")
            try:
                size = int(data[offset + 48 : offset + 58].strip() or b"0")
            except ValueError:
                return False
            if name == b"_gpgorigin":
                return True
            offset += 60 + size + (size % 2)
        return False
    if fmt == "rpm":
        import struct

        if data[:4] != ARCHIVE_MAGIC["rpm"] or len(data) < 108:
            return False
        return struct.unpack(">III", data[96:108])[1] > 0
    if fmt == "archlinux":
        signature = path.with_name(path.name + ".sig")
        return signature.is_file() and signature.stat().st_size > 0
    return False


def check_signatures(packages: list[Path], allow_unsigned: bool) -> bool:
    """Fail loudly on an unsigned package, or warn loudly under --allow-unsigned."""
    unsigned = [path.name for path in packages if not is_signed(path)]
    if not unsigned:
        return True
    listed = ", ".join(unsigned)
    if not allow_unsigned:
        raise SystemExit(
            f"make-repo: refusing to build a repository from unsigned package(s): {listed}. "
            "Publishing them would put unsigned artifacts in a public registry. Sign them "
            "(docs/release/signing.md) or pass --allow-unsigned for a tree that will not "
            "be published."
        )
    warn(
        f"UNSIGNED package(s) in this tree: {listed}. This repository is NOT publishable. "
        "It exists so a maintainer can inspect the layout before signing is configured."
    )
    return False


# --------------------------------------------------------------------------
# deb control, read out of the package
# --------------------------------------------------------------------------


def ar_members(path: Path) -> list[tuple[str, int]]:
    data = path.read_bytes()
    offset = 8
    members = []
    while offset + 60 <= len(data):
        name = data[offset : offset + 16].strip(b" ").rstrip(b"/").decode("ascii", "replace")
        try:
            size = int(data[offset + 48 : offset + 58].strip() or b"0")
        except ValueError:
            break
        members.append((name, size))
        offset += 60 + size + (size % 2)
    return members


def deb_control(path: Path) -> dict[str, str]:
    """The .deb's control fields, read out of the package itself.

    Not re-derived from nfpm.yaml: the repository has to describe the file that
    was actually built, including anything a later build-artifacts.py revision
    added to it.
    """
    import io
    import tarfile

    data = path.read_bytes()
    offset = 8
    control = None
    while offset + 60 <= len(data):
        name = data[offset : offset + 16].strip(b" ").rstrip(b"/").decode("ascii", "replace")
        size = int(data[offset + 48 : offset + 58].strip() or b"0")
        body = data[offset + 60 : offset + 60 + size]
        if name.startswith("control.tar"):
            mode = "gz" if name.endswith(".gz") else ("xz" if name.endswith(".xz") else "*")
            with tarfile.open(fileobj=io.BytesIO(body), mode=f"r:{mode}" if mode != "*" else "r:*") as tar:
                control = tar.extractfile("./control").read().decode("utf-8")
        offset += 60 + size + (size % 2)
    if control is None:
        raise SystemExit(f"make-repo: {path.name} has no control member")
    fields: dict[str, str] = {}
    key = None
    for line in control.splitlines():
        if line[:1] in (" ", "\t") and key:
            fields[key] += "\n" + line
        elif ":" in line:
            key, _, value = line.partition(":")
            key = key.strip()
            fields[key] = value.strip()
    return fields


# --------------------------------------------------------------------------
# apt
# --------------------------------------------------------------------------


def build_apt(args) -> dict:
    packages = find_packages(Path(args.packages))
    out = Path(args.out)
    debs = [p for p in packages if p.name.endswith(".deb")]
    if not debs:
        raise SystemExit("make-repo: the apt repository needs at least one .deb")
    signed = check_signatures(debs, args.allow_unsigned)

    binary_dir = out / "dists" / args.component / f"main/binary-{args.arch}"
    binary_dir.mkdir(parents=True, exist_ok=True)

    stanzas = []
    listed: list[Path] = []
    for deb in debs:
        pool = out / POOL["deb"]
        pool.mkdir(parents=True, exist_ok=True)
        target = pool / deb.name
        shutil.copy2(deb, target)
        listed.append(target)
        fields = deb_control(deb)
        relative = target.relative_to(out).as_posix()
        stanzas.append(
            "\n".join([
                "Package: " + fields.get("Package", "loams"),
                "Version: " + fields.get("Version", ""),
                "Architecture: " + fields.get("Architecture", args.arch),
                "Maintainer: " + fields.get("Maintainer", ""),
                f"Installed-Size: {fields.get('Installed-Size', '0')}",
                *([f"Depends: {fields['Depends']}"] if fields.get("Depends") else []),
                *([f"Section: {fields['Section']}"] if fields.get("Section") else []),
                *([f"Priority: {fields['Priority']}"] if fields.get("Priority") else []),
                *([f"Homepage: {fields['Homepage']}"] if fields.get("Homepage") else []),
                f"Filename: {relative}",
                f"Size: {target.stat().st_size}",
                f"MD5sum: {md5(target)}",
                f"SHA1: {sha1(target)}",
                f"SHA256: {sha256(target)}",
                "Description: " + fields.get("Description", ""),
            ])
        )
    packages_file = binary_dir / "Packages"
    packages_file.write_text("\n\n".join(stanzas) + "\n", encoding="utf-8")
    (binary_dir / "Packages.gz").write_bytes(
        gzip.compress(packages_file.read_bytes(), compresslevel=9, mtime=0)
    )

    # The Release file: one checksum line per file the client fetches.
    checksums = []
    for path in sorted(listed) + [binary_dir / "Packages", binary_dir / "Packages.gz"]:
        relative = path.relative_to(out).as_posix()
        checksums.append(
            f" {sha256(path)} {path.stat().st_size:>16} {relative}"
        )
    release = "\n".join([
        "Origin: " + ORIGIN,
        "Label: " + LABEL,
        "Suite: " + args.component,
        "Codename: " + args.codename,
        "Version: " + args.version,
        "Architectures: " + args.arch,
        "Components: main",
        "Date: " + dt.datetime.now(dt.timezone.utc).strftime("%a, %d %b %Y %H:%M:%S +0000"),
        "Acquire-By-Hash: no",
        "Description: Loams server packages (unsigned unless the repository is signed)",
        "SHA256:",
        *checksums,
        "",
    ])
    release_path = out / "dists" / args.component / "Release"
    release_path.write_text(release, encoding="utf-8")
    listed.append(release_path)
    sign_release(out, release_path, args)
    listed.extend(sorted(p for p in binary_dir.iterdir() if p.is_file()))
    for extra in sorted(out.glob("dists/*/InRelease")):
        listed.append(extra)

    report = {
        "format": "apt",
        "component": args.component,
        "arch": args.arch,
        "signed": signed,
        "packages": [p.name for p in debs],
        "files": sorted(p.relative_to(out).as_posix() for p in listed),
    }
    note(f"make-repo: apt repository at {out} ({len(debs)} package(s), signed={signed})")
    return report


def sign_release(out: Path, release: Path, args) -> None:
    """InRelease when there is a key, otherwise a loud warning and no signature.

    apt fetches InRelease when it exists and Release plus Release.gpg otherwise.
    Writing neither is the failure mode this branch exists to prevent: a client
    then falls back to trusting the repository by transport alone, silently.
    """
    if not args.gpg_key:
        warn(
            "the apt Release file is NOT signed: no InRelease and no Release.gpg were "
            "written. This repository must not be published."
        )
        return
    ensure_key_imported(Path(args.gpg_key))
    result = subprocess.run(
        ["gpg", "--batch", "--yes", "--local-user", args.gpg_key_id, "--clearsign",
         "--digest-algo", "SHA256", "--output", str(release.with_name("InRelease")),
         str(release)],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit("make-repo: gpg could not clearsign the apt Release file")
    note(f"make-repo: InRelease signed by {args.gpg_key_id}")


# --------------------------------------------------------------------------
# dnf
# --------------------------------------------------------------------------


def build_dnf(args) -> dict:
    packages = find_packages(Path(args.packages))
    out = Path(args.out)
    rpms = [p for p in packages if p.name.endswith(".rpm")]
    if not rpms:
        raise SystemExit("make-repo: the dnf repository needs at least one .rpm")
    signed = check_signatures(rpms, args.allow_unsigned)
    out.mkdir(parents=True, exist_ok=True)
    for rpm in rpms:
        shutil.copy2(rpm, out / rpm.name)
        detached = rpm.with_name(rpm.name + ".sig")
        if detached.is_file():
            shutil.copy2(detached, out / detached.name)

    tool = shutil.which("createrepo_c") or shutil.which("createrepo")
    if tool is None:
        raise SystemExit(
            "make-repo: neither createrepo_c nor createrepo is on PATH. Install "
            "createrepo-c (Debian and Ubuntu: `apt-get install createrepo-c`; Fedora: "
            "it is in createrepo_c). The dnf repository is not generated by this "
            "script on purpose: repomd.xml is a format with no second implementation "
            "to test against."
        )
    result = subprocess.run(
        [tool, "--update", "--verbose", str(out)], capture_output=True, text=True, check=False
    )
    sys.stderr.write(result.stderr)
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        raise SystemExit(f"make-repo: {Path(tool).name} failed to write repodata")

    # Read repomd.xml back and check every href exists and hashes as it claims.
    import re

    repomd = (out / "repodata" / "repomd.xml").read_text(encoding="utf-8")
    hrefs = re.findall(r'href="([^"]+)"', repomd)
    checks = dict(
        (m[1], m[0]) for m in re.findall(r'<data .*?checksum type="sha256">([0-9a-f]+)</checksum>.*?href="([^"]+)"', repomd, re.S)
    )
    if not hrefs:
        raise SystemExit("make-repo: repomd.xml lists no data files")
    for href in hrefs:
        target = out / "repodata" / href
        if not target.is_file():
            raise SystemExit(f"make-repo: repomd.xml points at {href}, which is not there")
        expected = checks.get(href)
        if expected and sha256(target) != expected:
            raise SystemExit(f"make-repo: {href} does not match the checksum repomd.xml gives it")
    note(f"make-repo: dnf repository at {out} ({len(rpms)} package(s), "
         f"{len(hrefs)} repodata file(s), signed={signed})")
    return {
        "format": "dnf",
        "arch": args.arch,
        "signed": signed,
        "packages": [p.name for p in rpms],
        "repodata": sorted(hrefs),
    }


# --------------------------------------------------------------------------
# pacman
# --------------------------------------------------------------------------


def build_pacman(args) -> dict:
    packages = find_packages(Path(args.packages))
    out = Path(args.out)
    arch_packages = [p for p in packages if p.name.endswith(".pkg.tar.zst")]
    if not arch_packages:
        raise SystemExit("make-repo: the pacman repository needs at least one .pkg.tar.zst")
    signed = check_signatures(arch_packages, args.allow_unsigned)
    out.mkdir(parents=True, exist_ok=True)
    for package in arch_packages:
        shutil.copy2(package, out / package.name)
        detached = package.with_name(package.name + ".sig")
        if detached.is_file():
            shutil.copy2(detached, out / detached.name)

    tool = shutil.which("repo-add")
    if tool is None:
        raise SystemExit(
            "make-repo: repo-add is not on PATH. Install the pacman package "
            "(Debian and Ubuntu: `apt-get install pacman-package-manager`, which ships "
            "repo-add; Arch and CachyOS: it is in the pacman package). The pacman "
            "database is written only by repo-add."
        )
    database = out / "loams.db.tar.gz"
    result = subprocess.run(
        [tool, str(database), *[str(out / p.name) for p in arch_packages]],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stdout + result.stderr)
        raise SystemExit("make-repo: repo-add failed")

    # repo-add writes loams.db and loams.files as symlinks to the .tar.gz files.
    # GitHub Pages follows neither usefully for a client, and a detached
    # signature has to sit next to the file it signs, so both are materialised
    # as real files here.
    for link in ("loams.db", "loams.files"):
        path = out / link
        if path.is_symlink():
            source = path.resolve()
            path.unlink()
            shutil.copyfile(source, path)
        if not (out / (link + ".tar.gz")).is_file():
            raise SystemExit(f"make-repo: repo-add did not produce {link}.tar.gz")

    signed_db = []
    if args.gpg_key and args.gpg_key_id:
        ensure_key_imported(Path(args.gpg_key))
        for link in ("loams.db.tar.gz", "loams.files.tar.gz"):
            target = out / link
            subprocess.run(
                ["gpg", "--batch", "--yes", "--local-user", args.gpg_key_id,
                 "--detach-sign", "--output", str(target) + ".sig", str(target)],
                check=True,
            )
            signed_db.append(link + ".sig")
    else:
        warn(
            "the pacman database is NOT signed: no loams.db.tar.gz.sig and no "
            "loams.files.tar.gz.sig. A client with SigLevel = Required will refuse "
            "this repository."
        )
    note(f"make-repo: pacman repository at {out} ({len(arch_packages)} package(s), "
         f"signed={signed})")
    return {
        "format": "pacman",
        "arch": args.arch,
        "signed": signed,
        "packages": [p.name for p in arch_packages],
        "database_signed": sorted(signed_db),
    }


# --------------------------------------------------------------------------
# verification: the gate publication has to pass
# --------------------------------------------------------------------------


def verify_tree(root: Path) -> dict:
    """Check a repository tree is publishable, or fail.

    This is the last thing between a build and a public registry, so it checks
    the signatures themselves rather than a flag a previous step set:

      * every .deb carries an `_gpgorigin`;
      * every .rpm has a non-empty signature header (SignPath's `<rpm-sign>`);
      * every .pkg.tar.zst has a `.sig` next to it;
      * the apt repository has an InRelease, or a Release plus Release.gpg;
      * the pacman repository has loams.db.tar.gz.sig and loams.files.tar.gz.sig;
      * the dnf repository's repodata exists and repomd.xml's checksums hold.

    Any failure is a non-zero exit. There is no `--allow-unsigned` here on
    purpose: this is the gate, and a gate with a bypass is not a gate.
    """
    problems: list[str] = []
    checked: dict[str, int] = {"deb": 0, "rpm": 0, "archlinux": 0}

    packages = [p for p in root.rglob("*") if p.is_file() and _format_of(p)]
    if not packages:
        problems.append(f"{root} holds no packages")
    for package in packages:
        fmt = _format_of(package)
        if not is_signed(package):
            problems.append(f"{package.relative_to(root)} is unsigned")

    apt = root / "apt"
    if apt.is_dir():
        releases = sorted(apt.glob("dists/*/Release"))
        if not releases:
            problems.append("apt: no dists/*/Release")
        for release in releases:
            inrelease = release.with_name("InRelease")
            detached = release.with_name("Release.gpg")
            if not inrelease.is_file() and not detached.is_file():
                problems.append(f"apt: {release.name} is unsigned (no InRelease, no Release.gpg)")
            binary = release.parent / "main"
            if not list(binary.glob("binary-*/Packages")):
                problems.append("apt: no binary-*/Packages")
        checked["deb"] = sum(1 for p in packages if p.name.endswith(".deb"))

    rpm_dir = root / "rpm"
    if rpm_dir.is_dir():
        repomd = rpm_dir / "repodata" / "repomd.xml"
        if not repomd.is_file():
            problems.append("dnf: repodata/repomd.xml is missing")
        checked["rpm"] = sum(1 for p in packages if p.name.endswith(".rpm"))

    pacman = root / "pacman"
    if pacman.is_dir():
        for link in ("loams.db.tar.gz", "loams.files.tar.gz"):
            target = pacman / link
            if not target.is_file():
                problems.append(f"pacman: {link} is missing")
            elif not (pacman / (link + ".sig")).is_file():
                problems.append(f"pacman: {link} is unsigned (no {link}.sig)")
        checked["archlinux"] = sum(1 for p in packages if p.name.endswith(".pkg.tar.zst"))

    if problems:
        for problem in problems:
            print(f"::error::make-repo verify: {problem}", file=sys.stderr)
        raise SystemExit(
            f"make-repo verify: {len(problems)} problem(s); this tree must not be published"
        )
    print(json.dumps({"verified": True, "packages": checked}, indent=2, sort_keys=True))
    return {"verified": True, "packages": checked}


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

SELF_TEST_NOTE = "make-repo: self-test passed"


def self_test() -> int:
    """Build all three repositories from fixture packages and read them back."""
    import importlib.util

    failures: list[str] = []
    skipped: list[str] = []
    nfpm = shutil.which("nfpm")

    spec = importlib.util.spec_from_file_location(
        "build_artifacts", REPO_ROOT / "scripts" / "release" / "build-artifacts.py"
    )
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)

    with tempfile.TemporaryDirectory(prefix="loams-repo-test-") as scratch:
        root = Path(scratch)
        binary = root / "loams"
        binary.write_text("#!/bin/sh\necho loams\n")
        binary.chmod(0o755)
        console = root / "console"
        console.mkdir()
        (console / "index.html").write_text("<!doctype html>\n")
        packages = root / "packages"
        packages.mkdir()

        if nfpm is None:
            skipped.append("the fixtures need nFPM on PATH; install it or pass through CI")
            print(SELF_TEST_NOTE)
            for item in skipped:
                warn(f"self-test: skipped {item}")
            return 0

        builder.effective_config(root / "nfpm.yaml", "2026-01-01T00:00:00Z", None, None)
        builder.build_stage(root / "staging", binary, console)
        for fmt, suffix in (("deb", ".deb"), ("rpm", ".rpm"), ("archlinux", ".pkg.tar.zst")):
            target = packages / builder.package_name(fmt, "9.9.9", builder.TARGET_ARCH[
                "x86_64-unknown-linux-gnu"][fmt])
            result = subprocess.run(
                [nfpm, "package", "--packager", fmt, "--config", str(root / "nfpm.yaml"),
                 "--target", str(target)],
                capture_output=True, text=True, check=False, cwd=str(root),
            )
            if result.returncode != 0:
                failures.append(f"fixture {fmt}: nFPM failed: {result.stdout}{result.stderr}")
            elif not is_signed(target):
                pass  # expected: no key in the self-test
        if failures:
            for failure in failures:
                print(failure, file=sys.stderr)
            return 1

        # An unsigned repository is refused, loudly.
        try:
            build_apt(argparse.Namespace(packages=str(packages), out=str(root / "apt"),
                                         component="stable", codename="loams",
                                         version="9.9.9", arch="amd64",
                                         gpg_key=None, gpg_key_id=None,
                                         allow_unsigned=False))
        except SystemExit:
            pass
        else:
            failures.append("apt: an unsigned .deb was accepted without --allow-unsigned")

        apt_out = root / "apt"
        report = build_apt(argparse.Namespace(packages=str(packages), out=str(apt_out),
                                              component="stable", codename="loams",
                                              version="9.9.9", arch="amd64",
                                              gpg_key=None, gpg_key_id=None,
                                              allow_unsigned=True))
        if report["signed"]:
            failures.append("apt: the report claims the unsigned tree is signed")
        packages_file = apt_out / "dists" / "stable" / "main" / "binary-amd64" / "Packages"
        text = packages_file.read_text()
        if "Package: loams" not in text or "Filename: pool/main/l/loams/" not in text:
            failures.append("apt: Packages does not name the package and its pool path")
        if gzip.decompress((packages_file.parent / "Packages.gz").read_bytes()) != \
                packages_file.read_bytes():
            failures.append("apt: Packages.gz is not a gzip of Packages")
        release = (apt_out / "dists" / "stable" / "Release").read_text()
        for required in ("Origin: ostrium-labs", "Suite: stable", "Codename: loams",
                         "Architectures: amd64", "Components: main", "SHA256:"):
            if required not in release:
                failures.append(f"apt: Release is missing {required!r}")
        for line in release.splitlines():
            parts = line.split()
            if len(parts) == 3 and parts[0] != "SHA256:" and "/" in parts[2]:
                digest, size, relative = parts
                target = apt_out / relative
                if not target.is_file():
                    failures.append(f"apt: Release lists {relative}, which is not there")
                elif sha256(target) != digest or target.stat().st_size != int(size):
                    failures.append(f"apt: Release's checksum for {relative} is wrong")

        if shutil.which("createrepo_c") is None and shutil.which("createrepo") is None:
            skipped.append("the dnf repository needs createrepo_c, which is not on PATH")
        else:
            dnf_out = root / "rpm"
            build_dnf(argparse.Namespace(packages=str(packages), out=str(dnf_out),
                                         arch="x86_64", allow_unsigned=True))
            if not (dnf_out / "repodata" / "repomd.xml").is_file():
                failures.append("dnf: repomd.xml was not written")

        # The gate: the unsigned tree built above must fail it.
        try:
            verify_tree(root)
        except SystemExit:
            pass
        else:
            failures.append("verify: an unsigned tree passed the publication gate")

        if shutil.which("repo-add") is None:
            skipped.append("the pacman repository needs repo-add, which is not on PATH")
        else:
            pacman_out = root / "pacman"
            build_pacman(argparse.Namespace(packages=str(packages), out=str(pacman_out),
                                            arch="x86_64", gpg_key=None,
                                            gpg_key_id=None, allow_unsigned=True))
            db = (pacman_out / "loams.db").read_bytes()
            if not db.startswith(b"\x1f\x8b"):
                failures.append("pacman: loams.db is not gzip, so pacman cannot read it")
            if (pacman_out / "loams.db").is_symlink():
                failures.append("pacman: loams.db is still a symlink")

    for failure in failures:
        print(failure, file=sys.stderr)
    for item in skipped:
        warn(f"self-test: skipped {item}")
    if failures:
        return 1
    print(SELF_TEST_NOTE)
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true", help="run the fixtures and exit")
    sub = parser.add_subparsers(dest="command")

    def common(p):
        p.add_argument("--packages", required=True, help="directory holding the built packages")
        p.add_argument("--out", required=True)
        p.add_argument("--allow-unsigned", action="store_true",
                       help="build a tree that must not be published (unsigned packages)")
        p.add_argument("--report", help="write the JSON report here as well as to stdout")

    apt = sub.add_parser("apt", help="an apt repository (Packages, Packages.gz, Release)")
    common(apt)
    apt.add_argument("--component", default="stable")
    apt.add_argument("--codename", default="loams")
    apt.add_argument("--version", required=True)
    apt.add_argument("--arch", default="amd64")
    apt.add_argument("--gpg-key", help="GPG private key; without it InRelease is not written")
    apt.add_argument("--gpg-key-id")

    dnf = sub.add_parser("dnf", help="a dnf repository (repodata, written by createrepo_c)")
    common(dnf)
    dnf.add_argument("--arch", default="x86_64")

    pacman = sub.add_parser("pacman", help="a pacman repository (loams.db, written by repo-add)")
    common(pacman)
    pacman.add_argument("--arch", default="x86_64")
    pacman.add_argument("--gpg-key", help="GPG private key; without it the database is unsigned")
    pacman.add_argument("--gpg-key-id")

    verify = sub.add_parser(
        "verify", help="check a repository tree is publishable; non-zero if it is not"
    )
    verify.add_argument("--tree", required=True)

    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    if args.command == "verify":
        return 0 if verify_tree(Path(args.tree)) else 1

    builder = {"apt": build_apt, "dnf": build_dnf, "pacman": build_pacman}.get(args.command)
    if builder is None:
        parser.print_help()
        return 2
    if getattr(args, "gpg_key", None) and not getattr(args, "gpg_key_id", None):
        args.gpg_key_id = _read_key_id(args.gpg_key)
        if args.gpg_key_id is None:
            raise SystemExit(
                "make-repo: --gpg-key was given but no key id could be read from it, "
                "so the signatures would name no key"
            )
    report = builder(args)
    payload = json.dumps(report, indent=2, sort_keys=True)
    print(payload)
    if args.report:
        Path(args.report).write_text(payload + "\n", encoding="utf-8")
    return 0


def _read_key_id(path: str) -> str | None:
    result = subprocess.run(
        ["gpg", "--batch", "--with-colons", "--import-options", "show-only", "--import", path],
        capture_output=True, text=True, check=False,
    )
    for line in result.stdout.splitlines():
        parts = line.split(":")
        if parts[0] == "fpr" and len(parts) > 9 and parts[9]:
            return parts[9][-16:]
    return None


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]) or 0)