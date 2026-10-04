#!/usr/bin/env python3
"""Build the release artifacts: three Linux packages, or the Windows zip.

The Linux formats are .deb, .rpm and .pkg.tar.zst, all from one nFPM config
(`release/nfpm.yaml`). The Windows artifact is a zip holding `loams.exe`, the
built web console and the licence files, because there is no Tauri desktop app
and no GUI to wrap: what Windows gets is the server binary and the console, the
same two things the Linux packages install.

Two rules this script exists to enforce:

  * Every artifact carries the same content on every platform. The binary, the
    console build, LICENSE, NOTICE and README.md come from one staging tree that
    is built once and then packed, so a .deb and a .pkg.tar.zst of the same
    release cannot drift.
  * Signing is conditional and never faked. With no key the artifacts are built
    unsigned and this script says so on stderr with `::warning::`-shaped lines
    the workflow forwards. With a key, nFPM signs the .deb itself
    (`deb.signature`) and `gpg --detach-sign` writes the `.pkg.tar.zst.sig` that
    pacman's SigLevel expects. The .rpm is not signed here: nFPM cannot sign one
    (see `effective_config`), so SignPath's `<rpm-sign>` signs it later. Every
    signature this script claims is verified to exist before the run reports
    success, and `signing.json` records which artifact has which signature so
    the publication gate never has to guess.

Usage:
    build-artifacts.py package --version 0.1.0 --binary PATH [--console DIR]
                              --format deb --format rpm --format archlinux
                              --out DIR [--target x86_64-unknown-linux-gnu]
                              [--mtime RFC3339] [--gpg-key FILE] [--gpg-key-id ID]
    build-artifacts.py windows-zip --version 0.1.0 --binary loams.exe
                              --console DIR --out DIR
    build-artifacts.py --self-test

`--nfpm PATH` overrides the nFPM binary; without it the script looks for `nfpm`
on PATH. The self-test skips the packaging cases when nFPM is absent and says
so, rather than passing quietly.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
NFPM_CONFIG = REPO_ROOT / "release" / "nfpm.yaml"
SYSTEMD_DIR = REPO_ROOT / "release" / "systemd"
DOC_FILES = ("LICENSE", "NOTICE", "README.md")

# The formats, and what each one's leading bytes must be. A package that does
# not start with its format's magic is a failed build, and catching it here is
# cheaper than catching it on a user's machine.
FORMAT_MAGIC = {
    "deb": (b"!<arch>\n",),
    "rpm": (bytes.fromhex("edabeedb"),),
    "archlinux": (bytes.fromhex("28b52ffd"),),  # zstd frame magic
}

# nFPM's own spelling for each Rust target, per packager.
TARGET_ARCH = {
    "x86_64-unknown-linux-gnu": {"deb": "amd64", "rpm": "x86_64", "archlinux": "x86_64"},
}

# The package file names nFPM produces, so the workflow can assert on them.
def package_name(fmt: str, version: str, arch: str, release: str = "1") -> str:
    if fmt == "deb":
        return f"loams_{version}_{arch}.deb"
    if fmt == "rpm":
        return f"loams-{version}-{release}.{arch}.rpm"
    if fmt == "archlinux":
        return f"loams-{version}-{release}-{arch}.pkg.tar.zst"
    raise ValueError(fmt)


def warn(message: str) -> None:
    """A GitHub Actions annotation, and the same text on stderr for a local run."""
    print(f"::warning::{message}", file=sys.stderr)


def note(message: str) -> None:
    print(message, file=sys.stderr)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


# --------------------------------------------------------------------------
# staging
# --------------------------------------------------------------------------


def build_stage(stage: Path, binary: Path, console: Path | None) -> Path:
    """Assemble the tree every Linux package is built from.

    One tree, packed three ways. The layout mirrors the destinations declared in
    release/nfpm.yaml, and build-stage refuses anything it was not given rather
    than packaging an empty directory.
    """
    if not binary.is_file():
        raise SystemExit(f"build-artifacts: the server binary is missing: {binary}")
    if binary.stat().st_size == 0:
        raise SystemExit(f"build-artifacts: the server binary is empty: {binary}")
    if not os.access(binary, os.X_OK):
        raise SystemExit(f"build-artifacts: the server binary is not executable: {binary}")

    usr_bin = stage / "usr" / "bin"
    usr_bin.mkdir(parents=True)
    shutil.copy2(binary, usr_bin / "loams")
    os.chmod(usr_bin / "loams", 0o755)

    doc = stage / "usr" / "share" / "doc" / "loams"
    doc.mkdir(parents=True)
    for name in DOC_FILES:
        source = REPO_ROOT / name
        if not source.is_file():
            raise SystemExit(f"build-artifacts: {name} is missing from the repository root")
        shutil.copy2(source, doc / name)
        os.chmod(doc / name, 0o644)

    console_dest = stage / "usr" / "share" / "loams" / "console"
    console_dest.mkdir(parents=True)
    if console is None:
        # An absent console is a real gap in the release, not a reason to fail
        # the whole package: say so loudly and let the maintainer decide.
        warn(
            "no console build was supplied, so the package will ship without "
            "/usr/share/loams/console. Pass --console to include it."
        )
        (console_dest / "README.txt").write_text(
            "The web console was not included in this package build.\n"
            "The console is published at https://console.loams.dev.\n",
            encoding="utf-8",
        )
    else:
        console_root = Path(console)
        index = console_root / "index.html"
        if not index.is_file():
            raise SystemExit(
                f"build-artifacts: {console_root} has no index.html, so it is not a "
                "console build (the Vite output directory is web/apps/console/dist)"
            )
        copied = 0
        for path in sorted(console_root.rglob("*")):
            if not path.is_file():
                continue
            target = console_dest / path.relative_to(console_root)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            os.chmod(target, 0o644)
            copied += 1
        note(f"build-artifacts: staged {copied} console file(s) from {console_root}")

    units = stage / "usr" / "lib" / "systemd" / "system"
    units.mkdir(parents=True)
    for name in ("loams.service", "loams@.service"):
        shutil.copy2(SYSTEMD_DIR / name, units / name)
        os.chmod(units / name, 0o644)

    return stage


# --------------------------------------------------------------------------
# signing
# --------------------------------------------------------------------------


def gpg_available() -> bool:
    return shutil.which("gpg") is not None


def ensure_key_imported(path: Path) -> None:
    """Import the private key into the current GnuPG home before signing with it.

    A key file on disk is not a key in the keyring, and `gpg --local-user` looks
    only in the keyring: without this, every gpg signature step fails with "No
    secret key" (verified 2026-10-04). Importing twice is a no-op with --batch.
    """
    result = subprocess.run(
        ["gpg", "--batch", "--import", str(path)], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(f"build-artifacts: gpg could not import {path}")


def read_key_id(path: Path) -> str | None:
    """The key id of a GPG key file, or None when it cannot be read.

    nFPM needs `key_id` next to `key_file` and does not look one up, and it
    parses the value with ParseUint(base 16, 64 bits), so the 40-character
    fingerprint `gpg --with-colons` prints overflows it ("value out of range",
    verified 2026-10-04 with nfpm 2.47.0). The low 16 hex digits are the long key
    id and fit, so that is what this returns.
    """
    if not gpg_available():
        return None
    fingerprint = None
    for line in subprocess.run(
        ["gpg", "--batch", "--with-colons", "--import-options", "show-only",
         "--import", str(path)],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.splitlines():
        parts = line.split(":")
        if parts[0] == "fpr" and len(parts) > 9 and parts[9]:
            fingerprint = parts[9]
            break
    if not fingerprint:
        return None
    return fingerprint[-16:]


def effective_config(destination: Path, mtime: str, gpg_key: Path | None,
                     gpg_key_id: str | None) -> Path:
    """release/nfpm.yaml plus the signing blocks, when there is a key to sign with.

    Written out as a whole file rather than patched in place, so the signature
    settings are inspectable in CI without trusting a text substitution. It stays
    YAML because nFPM expands `${...}` while decoding YAML, and a JSON config
    would reach the packer with the placeholders unexpanded (found by the
    self-test, which builds a real .deb).
    """
    import yaml  # PyYAML; the CI image has it, and the script says so if not

    config = yaml.safe_load(NFPM_CONFIG.read_text(encoding="utf-8"))
    # nFPM decodes `mtime` as a timestamp before it expands `${...}`, so the
    # value has to be a literal here rather than a placeholder in the file.
    config["mtime"] = mtime
    if gpg_key is not None:
        if gpg_key_id is None:
            raise SystemExit(
                "build-artifacts: --gpg-key was given but the key id could not be read "
                "from it, so the packages would be signed by an unidentified key"
            )
        config.setdefault("deb", {})["signature"] = {
            "key_file": str(gpg_key),
            "key_id": gpg_key_id,
            "method": "debsign",
            "type": "origin",
            "signer": "The Loams Authors",
        }
    # `rpm.signature` is deliberately NOT set. nFPM 2.47.0 cannot do it: with a
    # key it reports "Failed to create signatures ... no valid signing keys"
    # (verified 2026-10-04 with an RSA 3072 key, an armored export and a binary
    # export, and with a key carrying a signing subkey). The .rpm is signed by
    # SignPath's `<rpm-sign>` instead, which is the project's RPM signing path
    # already (D621, D625). See docs/release/signing.md.
    destination.write_text(yaml.safe_dump(config, sort_keys=False), encoding="utf-8")
    return destination


def deb_is_signed(path: Path) -> bool:
    """True when the .deb carries the `_gpgorigin` member `debsign` writes.

    An .deb with no signature is a valid .deb, so nothing about the file fails;
    the only way to know is to look for the member.
    """
    # An ar archive: the 8-byte magic, then for each member a 60-byte header
    # (name[16], mtime[12], uid[6], gid[6], mode[8], size[10], "`\n") followed by
    # the body. Headers are not contiguous, so the walk follows each member's
    # size rather than stepping by 60 (a header table scan finds the second
    # member's name one body early, which is how this function first answered
    # False for a signed .deb).
    data = path.read_bytes()
    if data[:8] != FORMAT_MAGIC["deb"][0]:
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


def rpm_is_signed(path: Path) -> bool:
    """True when an RPM carries a signature header.

    An RPM's first header after the 96-byte lead is the signature header, and an
    unsigned RPM leaves it empty (zero index entries, padded to the 8-byte
    boundary). A signed one has entries, so the entry count is the whole test.
    """
    data = path.read_bytes()
    if data[:4] != FORMAT_MAGIC["rpm"][0] or len(data) < 96 + 16:
        return False
    _, entries, _ = struct.unpack(">III", data[96:108])
    return entries > 0


def sign_arch_package(package: Path, gpg_key: Path, gpg_key_id: str) -> Path:
    """Write the detached signature pacman's SigLevel reads.

    A pacman package is signed outside the package: `gpg --detach-sign` leaves
    `loams-....pkg.tar.zst.sig` next to it, and pacman refuses an unsigned
    package when SigLevel requires a signature.
    """
    signature = package.with_name(package.name + ".sig")
    ensure_key_imported(gpg_key)
    subprocess.run(
        ["gpg", "--batch", "--yes", "--local-user", gpg_key_id,
         "--detach-sign", "--output", str(signature), str(package)],
        check=True,
    )
    return signature


# --------------------------------------------------------------------------
# packaging
# --------------------------------------------------------------------------


def verify_magic(path: Path, fmt: str) -> None:
    longest = max(len(magic) for magic in FORMAT_MAGIC[fmt])
    head = path.read_bytes()[:longest]
    for magic in FORMAT_MAGIC[fmt]:
        if head.startswith(magic):
            return
    raise SystemExit(
        f"build-artifacts: {path.name} does not start with the {fmt} magic "
        f"({head.hex(' ')}), so nFPM produced something else"
    )


def run_package(args, stage: Path, out: Path, tmp: Path, gpg_key: Path | None) -> list[Path]:
    # nFPM resolves a content `src` against the process working directory, not
    # against the config file, so the effective config is written into the
    # scratch directory and nFPM runs there. The staging tree is `scratch/staging`.
    config = effective_config(tmp / "nfpm.yaml", args.mtime, gpg_key, args.gpg_key_id)
    produced: list[Path] = []
    for fmt in args.format:
        nfpm_arch = TARGET_ARCH[args.target][fmt]
        target = out / package_name(fmt, args.version, nfpm_arch)
        result = subprocess.run(
            [args.nfpm, "package", "--packager", fmt, "--config", str(config),
             "--target", str(target)],
            env={
                **os.environ,
                "LOAMS_VERSION": args.version,
                "LOAMS_PACKAGE_ARCH": nfpm_arch,
                "LOAMS_PACKAGE_PLATFORM": "linux",
                "LOAMS_PACKAGE_RELEASE": "1",
            },
            capture_output=True,
            text=True,
            check=False,
            cwd=str(tmp),
        )
        if result.returncode != 0:
            # nFPM draws its errors on stdout in a box; forward both streams so a
            # CI log shows the real cause rather than a non-zero exit status.
            sys.stderr.write(result.stdout)
            sys.stderr.write(result.stderr)
            raise SystemExit(f"build-artifacts: nFPM failed to build the {fmt} package")
        verify_magic(target, fmt)
        if not target.stat().st_size:
            raise SystemExit(f"build-artifacts: {target} is empty")
        produced.append(target)
        note(f"build-artifacts: {target.name}  {target.stat().st_size} bytes  sha256:{sha256(target)[:16]}…")
    if gpg_key is not None and args.gpg_key_id:
        arch = next((path for path in produced if path.name.endswith(".pkg.tar.zst")), None)
        if arch is not None:
            sign_arch_package(arch, gpg_key, args.gpg_key_id)
            note(f"build-artifacts: {arch.name}.sig written")
    return produced


# How each Linux format is signed, and by whom. `signing.json` records this per
# artifact so the publication gate and the docs never have to re-derive it.
SIGNATURE_METHOD = {
    "deb": "nfpm-debsign",
    "rpm": "signpath-rpm-sign",
    "archlinux": "gpg-detached",
}


def signature_report(out: Path, gpg_key: Path | None) -> dict:
    """What is actually signed in `out`, checked against the files on disk.

    A key present is not a signature present: nFPM can be asked to sign and
    quietly produce an unsigned .deb, and gpg can exit 0 having written nothing
    useful. So each claim below is read back off the artifact.
    """
    report = {}
    for path in sorted(out.iterdir()):
        if path.name == "signing.json" or path.is_dir():
            continue
        fmt = next((f for f, suffix in (("deb", ".deb"), ("rpm", ".rpm"),
                                        ("archlinux", ".pkg.tar.zst"))
                    if path.name.endswith(suffix)), None)
        if fmt is None:
            continue
        present = False
        if fmt == "deb":
            present = deb_is_signed(path)
        elif fmt == "rpm":
            present = rpm_is_signed(path)
        else:
            signature = path.with_name(path.name + ".sig")
            present = signature.is_file() and signature.stat().st_size > 0
            if present:
                report[signature.name] = {
                    "format": "archlinux-signature",
                    "signed": True,
                    "method": SIGNATURE_METHOD[fmt],
                    "sha256": sha256(signature),
                    "size": signature.stat().st_size,
                }
        report[path.name] = {
            "format": fmt,
            "signed": present,
            "method": SIGNATURE_METHOD[fmt],
            "sha256": sha256(path),
            "size": path.stat().st_size,
        }
    (out / "signing.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n",
                                       encoding="utf-8")
    if gpg_key is None:
        warn(
            "the packages in this directory are UNSIGNED: no .deb _gpgorigin, no .rpm "
            "signature header, no .pkg.tar.zst.sig. signing.json records it. Nothing "
            "here may be published to a package repository."
        )
    else:
        for name, entry in report.items():
            if entry["format"] == "rpm" and not entry["signed"]:
                # Expected: SignPath signs the .rpm after this script runs.
                note(f"build-artifacts: {name} is unsigned here and signed by SignPath "
                     f"({SIGNATURE_METHOD['rpm']}) in the release-sign workflow")
            elif not entry["signed"]:
                warn(f"{name} is UNSIGNED even though a GPG key was supplied")
    return report


def command_package(args) -> int:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="loams-package-") as scratch:
        stage = build_stage(Path(scratch) / "staging", Path(args.binary),
                            Path(args.console) if args.console else None)
        if args.keep_stage:
            keep = out / "staging"
            if keep.exists():
                shutil.rmtree(keep)
            shutil.copytree(stage, keep)
            note(f"build-artifacts: staging tree kept at {keep}")
        gpg_key = Path(args.gpg_key) if args.gpg_key else None
        if gpg_key is None:
            warn(
                "no GPG key was supplied, so the .deb and the .rpm are UNSIGNED and no "
                ".pkg.tar.zst.sig is written. A package repository will not accept "
                "them. See docs/release/signing.md for the key handoff."
            )
        elif not gpg_key.is_file():
            raise SystemExit(f"build-artifacts: the GPG key file does not exist: {gpg_key}")
        produced = run_package(args, stage, out, Path(scratch), gpg_key)
        report = signature_report(out, gpg_key)
    print(json.dumps({
        "artifacts": [
            {"file": path.name, "sha256": sha256(path), "size": path.stat().st_size}
            for path in produced
        ],
        # signing.json, read back, is the run's own statement about signatures.
        "signing": {name: entry["signed"] for name, entry in report.items()},
    }, indent=2))
    return 0


def command_windows_zip(args) -> int:
    """The Windows artifact: loams.exe, the console, and the licence files.

    A zip, not an MSI: there is no installer UI in this project, and wrapping a
    console server binary in one would add a WiX build, a code-signing
    requirement and nothing a user of the zip does not already do by hand.
    """
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    binary = Path(args.binary)
    if not binary.is_file():
        raise SystemExit(f"build-artifacts: the Windows binary is missing: {binary}")
    console = Path(args.console)
    if not (console / "index.html").is_file():
        raise SystemExit(
            f"build-artifacts: {console} has no index.html, so it is not a console build"
        )
    name = f"loams-{args.version}-windows-x86_64.zip"
    target = out / name
    members: list[tuple[Path, str]] = [(binary, "loams.exe")]
    for doc in DOC_FILES:
        members.append((REPO_ROOT / doc, doc))
    for path in sorted(console.rglob("*")):
        if path.is_file():
            members.append((path, str(Path("console") / path.relative_to(console))))
    with zipfile.ZipFile(target, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for source, member in members:
            info = zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0))
            # 0644, and the two bits every extractor needs to mark it a regular
            # file. A zip built with default permissions loses the executable
            # bit on loams.exe, which Windows ignores, but makes the archive
            # unreadable to anything that honours Unix modes.
            info.external_attr = (0o644 | 0o100000) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, source.read_bytes())
    note(f"build-artifacts: {name}  {target.stat().st_size} bytes  sha256:{sha256(target)[:16]}…")
    print(json.dumps({
        "artifacts": [{"file": name, "sha256": sha256(target), "size": target.stat().st_size}],
        "signed": False,
    }, indent=2))
    return 0


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

SELF_TEST_NOTE = "build-artifacts: self-test passed"


def self_test(gpg_key: str | None = None) -> int:
    """Exercise staging and packing on a fixture, never on a real binary.

    The packaging cases need nFPM. Without it the staging cases still run and the
    skipped ones are reported, so a CI job that forgot to install nFPM is visible
    rather than green.
    """
    failures: list[str] = []
    skipped: list[str] = []
    nfpm = shutil.which("nfpm")
    test_key = Path(gpg_key) if gpg_key else None
    test_key_id = read_key_id(test_key) if test_key else None
    if test_key is None:
        skipped.append(
            "the signing cases need a GPG key (--self-test --gpg-key FILE); the .deb "
            "and .pkg.tar.zst signing paths are exercised in the release workflow"
        )

    with tempfile.TemporaryDirectory(prefix="loams-artifacts-test-") as scratch:
        root = Path(scratch)
        binary = root / "loams"
        binary.write_text("#!/bin/sh\necho loams\n")
        os.chmod(binary, 0o755)
        console = root / "console"
        (console / "assets").mkdir(parents=True)
        (console / "index.html").write_text("<!doctype html>\n")
        (console / "assets" / "app.js").write_text("console.log(1)\n")

        # staging
        stage = build_stage(root / "stage", binary, console)
        for expected in (
            "usr/bin/loams",
            "usr/share/loams/console/index.html",
            "usr/share/loams/console/assets/app.js",
            "usr/share/doc/loams/LICENSE",
            "usr/share/doc/loams/NOTICE",
            "usr/share/doc/loams/README.md",
            "usr/lib/systemd/system/loams.service",
            "usr/lib/systemd/system/loams@.service",
        ):
            if not (stage / expected).is_file():
                failures.append(f"stage: {expected} was not staged")
        if oct(stat.S_IMODE((stage / "usr/bin/loams").stat().st_mode)) != "0o755":
            failures.append("stage: usr/bin/loams is not 0755")
        if oct(stat.S_IMODE((stage / "usr/lib/systemd/system/loams.service").stat().st_mode)) != "0o644":
            failures.append("stage: the systemd unit is not 0644")

        # staging refuses what it was not given
        not_a_console = root / "not-a-console"
        not_a_console.mkdir()
        (not_a_console / "readme.txt").write_text("no index.html here\n")
        for label, call in (
            ("a missing binary", lambda: build_stage(root / "s1", root / "absent", console)),
            ("a console without index.html", lambda: build_stage(root / "s2", binary, not_a_console)),
        ):
            try:
                call()
            except SystemExit:
                continue
            failures.append(f"stage: {label} was accepted")

        # windows zip
        zip_args = argparse.Namespace(
            version="9.9.9", binary=str(binary), console=str(console), out=str(root / "win")
        )
        command_windows_zip(zip_args)
        archive_path = next((root / "win").glob("*.zip"))
        with zipfile.ZipFile(archive_path) as archive:
            names = set(archive.namelist())
            if archive.testzip() is not None:
                failures.append("windows-zip: the archive is corrupt")
        for member in ("loams.exe", "LICENSE", "NOTICE", "README.md",
                       "console/index.html", "console/assets/app.js"):
            if member not in names:
                failures.append(f"windows-zip: {member} is missing")

        # package names
        expected_names = {
            ("deb", "0.1.0", "amd64"): "loams_0.1.0_amd64.deb",
            ("rpm", "0.1.0", "x86_64"): "loams-0.1.0-1.x86_64.rpm",
            ("archlinux", "0.1.0", "x86_64"): "loams-0.1.0-1-x86_64.pkg.tar.zst",
        }
        for (fmt, version, arch), expected in expected_names.items():
            actual = package_name(fmt, version, arch)
            if actual != expected:
                failures.append(f"package_name: {fmt} gave {actual}, expected {expected}")

        # packing, when nFPM is on PATH
        if nfpm is None:
            skipped.append("the three Linux packages need nFPM on PATH")
        else:
            pkg_args = argparse.Namespace(
                version="9.9.9",
                binary=str(binary),
                console=str(console),
                format=["deb", "rpm", "archlinux"],
                out=str(root / "pkg"),
                target="x86_64-unknown-linux-gnu",
                mtime="2026-01-01T00:00:00Z",
                nfpm=nfpm,
                gpg_key=test_key,
                gpg_key_id=test_key_id,
                keep_stage=False,
            )
            try:
                import yaml  # noqa: F401
            except ImportError:
                skipped.append("PyYAML is not importable, so the nFPM config cannot be expanded")
            else:
                command_package(pkg_args)
                for fmt, suffix in (("deb", ".deb"), ("rpm", ".rpm"),
                                    ("archlinux", ".pkg.tar.zst")):
                    matches = list((root / "pkg").glob(f"*{suffix}"))
                    if not matches:
                        failures.append(f"nFPM produced no {fmt}")
                    else:
                        verify_magic(matches[0], fmt)
                # The report must agree with the files, whatever the key was.
                report = json.loads((root / "pkg" / "signing.json").read_text())
                for name, entry in report.items():
                    if name.endswith(".sig"):
                        if not entry["signed"]:
                            failures.append(f"signing: {name} is reported unsigned but exists")
                        continue
                    if test_key is not None and entry["format"] != "rpm":
                        if not entry["signed"]:
                            failures.append(f"signing: {name} is unsigned despite a key")
                    elif entry["signed"]:
                        failures.append(f"signing: {name} is signed with no key")
                if deb_is_signed(root / "pkg" / package_name("deb", "9.9.9", "amd64")) != (
                    test_key is not None
                ):
                    failures.append("signing: deb_is_signed disagrees with the key being present")
                if rpm_is_signed(root / "pkg" / package_name("rpm", "9.9.9", "x86_64")):
                    failures.append("signing: the .rpm claims a signature this script never makes")

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
    # Also a top-level flag so `--self-test --gpg-key FILE` exercises the signing
    # paths without going through a subcommand that would build real packages.
    parser.add_argument("--gpg-key", help=argparse.SUPPRESS)
    sub = parser.add_subparsers(dest="command")

    package = sub.add_parser("package", help="build the .deb, the .rpm and the .pkg.tar.zst")
    package.add_argument("--version", required=True)
    package.add_argument("--binary", required=True, help="the loams binary for this target")
    package.add_argument("--console", help="the built console directory (web/apps/console/dist)")
    package.add_argument("--format", action="append", required=True,
                         choices=sorted(FORMAT_MAGIC), help="repeatable")
    package.add_argument("--out", required=True)
    package.add_argument("--target", default="x86_64-unknown-linux-gnu", choices=sorted(TARGET_ARCH))
    package.add_argument("--mtime", default="1970-01-01T00:00:00Z",
                         help="RFC 3339 timestamp baked into the packages, for reproducibility")
    package.add_argument("--nfpm", default="nfpm", help="the nFPM binary to run")
    package.add_argument("--gpg-key", help="a GPG private key file; without it nothing is signed")
    package.add_argument("--gpg-key-id", help="the key's long id; read from the key when omitted")
    package.add_argument("--keep-stage", action="store_true", help="also write the staging tree to --out")

    windows = sub.add_parser("windows-zip", help="build the Windows zip")
    windows.add_argument("--version", required=True)
    windows.add_argument("--binary", required=True, help="loams.exe")
    windows.add_argument("--console", required=True)
    windows.add_argument("--out", required=True)

    args = parser.parse_args(argv)
    if args.self_test:
        return self_test(args.gpg_key)
    if args.command == "package":
        if args.gpg_key and not args.gpg_key_id:
            args.gpg_key_id = read_key_id(Path(args.gpg_key))
        return command_package(args)
    if args.command == "windows-zip":
        return command_windows_zip(args)
    parser.print_help()
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]) or 0)