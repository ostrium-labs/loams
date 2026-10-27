# Linux and Windows release packaging

Status: **the packaging is complete; the credentials are not.** Everything here
that does not need an organisation owner's browser is merged. What is still
outstanding needs one, and it is listed as [the handoff](#the-handoff) with the
exact value each item needs.

This page covers how the Linux packages and the Windows zip are built and what
they contain. What is signed, and the whole of the SignPath handoff, is
[`signing.md`](signing.md) — read both. The crates.io half of the release path
is [`publishing.md`](publishing.md), which this does not change.

## What ships

Two binaries are packaged: **`loams`**, `crates/loams/src/main.rs`, and, from
0.1.0, **`git-remote-loams`**, `crates/loams-git-remote` (git's
remote helper for `loams::` URLs; see [Git push and fetch to the
bucket](#git-push-and-fetch-to-the-bucket)). The `loams` subcommands are
`dev`, `standalone`, `cluster`, `warm`, `durable` and `migrate`.
**[verified]** — read from the `Command` enum in that file.

`git-remote-loams` is installed next to `loams` in every artifact (`/usr/bin`
in the Linux packages, the top level of the Windows zip and of the macOS
tarballs) because git finds a remote helper by its name on PATH. It is not
published to crates.io (`publish = false`; owner, 2026-10-15). It is built by
its own `cargo build -p loams-git-remote` call, so its dependencies cannot
change the server binary's feature set.

**`loams-agentd`**, the agent daemon behind Studio (`crates/loams-agentd`), is
not in these packages. For a headless Linux host it ships as its own tarball per
architecture ([#874](https://github.com/ostrium-labs/loams/issues/874)),
`loams-agentd-<v>-linux-x86_64.tar.gz` and `loams-agentd-<v>-linux-aarch64.tar.gz`,
each built on that architecture's runner by `build-artifacts.py agentd-tar`. Its
top-level directory holds `loams-agentd`, `loams` (built with the packages'
features), `SHA256SUMS` over the two and `VERSION`: the layout
`loams-agentd install-runtime --from` reads
(`crates/loams-agentd/src/runtime_install.rs`), which copies and checks both
binaries. The root `LICENSE` and `NOTICE` and the daemon's own licence files
(under `crates/loams-agentd/`) are beside them. The tarballs are unsigned; the
release's `SHA256SUMS` lists them. The user's guide is
[`remote-agentd.md`](../guides/studio/remote-agentd.md#get-the-binary).

The workspace has five other `[[bin]]` targets. **None of them is packaged**,
deliberately:

| Binary | Crate | Why not packaged |
|---|---|---|
| `compat-replay` | `loams-compat` | A PgDog→Postgres and Vitess→WeSQL capture replay tool for conformance work. Developer tooling, not a server. |
| `loams-wal` | `loams-safekeeper` | The WAL service for Neon computes (§28 P4a). Run beside a compute, not by an operator installing this package. |
| `loams-specview` | `loams-specview` | Serves and replays spec and test runs. A developer tool. |
| `loams-specview-web` | `loams-specview` | The `trunk`-built browser front end for `loams-specview`, which is itself not packaged. |

**[verified]** — enumerated from the `[[bin]]` tables in every
`crates/*/Cargo.toml`.

## Linux

### Formats

| Format | Distros | Built by | Signed by |
|---|---|---|---|
| `.deb` | Debian, Ubuntu | `release/nfpm.yaml` | **this workflow**, nFPM's `debsign` |
| `.rpm` | Fedora, RHEL, openSUSE | `release/nfpm.yaml` | **SignPath's `<rpm-sign>`**, in `release-sign.yml` |
| `.pkg.tar.zst` | Arch, CachyOS | `release/nfpm.yaml` | **this workflow**, `gpg --detach-sign` |

**CachyOS gets the pacman package, not the .rpm.** CachyOS is Arch-based: it
installs with `pacman` and reads `.pkg.tar.zst`. It has no use for an .rpm, and
an .rpm offered to a CachyOS machine installs nothing. **[verified]** — this is
CachyOS's own architecture, not a packaging choice made here.

All three come from **one nFPM config**, [`release/nfpm.yaml`](../../release/nfpm.yaml),
staged from one tree. That is why a `.deb` and a `.pkg.tar.zst` of the same
release cannot drift: they are the same files, packed three ways.

nFPM, not cargo-dist: cargo-dist 0.33.0 (D292) builds archives, shell and
PowerShell installers, npm, Homebrew and MSI, and its documented installer list
has no deb, no rpm and no Arch package. nFPM is the packager half of GoReleaser,
MIT licensed, one static binary, and produces all three formats from one file.

### Why the .rpm is built unsigned

`nFPM` cannot sign an RPM. With a key configured, nFPM 2.47.0 reports *"Failed to
create signatures ... no valid signing keys"* — reproduced with an armored
export, a binary export, and a key carrying a signing subkey. **[verified]**

So the .rpm is the one artifact this workflow hands to SignPath, which is exactly
what [`release-sign.yml`](../../.github/workflows/release-sign.yml) exists for and
why its `sign` job **refuses to submit unless an .rpm is in the set** (D625).
Before this page, that check could never pass: the repository built no packages
at all. It can now.

### Contents

```
/usr/bin/loams                        the server binary, 0755
/usr/bin/git-remote-loams             git's remote helper for loams:: URLs, 0755
/usr/share/loams/console/             the built web console, 0644
/usr/share/doc/loams/LICENSE
/usr/share/doc/loams/NOTICE
/usr/share/doc/loams/README.md
/usr/lib/systemd/system/loams.service
/usr/lib/systemd/system/loams@.service
```

`/usr/lib/systemd/system` is read by every distribution targeted here — Debian,
Ubuntu, Fedora, RHEL, openSUSE, Arch and CachyOS. Arch and CachyOS also read
`/etc`, but a unit that came from a package belongs under `/usr`.

**The console is optional and may be absent.** The engine serves it at `/ui/`
from `/usr/share/loams/console` when that directory holds a build (from 0.1.2;
`--console-dir` and `--no-console` change that), same-origin with its API.
When the console build is missing, `build-artifacts.py` ships a `README.txt` in its place and warns; in CI the console step is
`continue-on-error: true`, so a TypeScript error in the console cannot stop the
server packages from being produced.

### Dependencies

| Format | Declared dependencies |
|---|---|
| .deb | `libgcc-s1`, `libc6`, `zlib1g` |
| .rpm | `libgcc_s.so.1()(64bit)`, `libc.so.6()(64bit)`, `libz.so.1()(64bit)` |
| .pkg.tar.zst | `gcc-libs`, `glibc`, `zlib` |

These are the `DT_NEEDED` set of the packaged build (`--features
$LOAMS_PACKAGE_FEATURES`, the image's set), which `readelf -d` gives as
`libz.so.1`, `libgcc_s.so.1`, `libc.so.6`, `libm.so.6` and
`ld-linux-x86-64.so.2`. **[verified]**: read off the published 0.1.1 package on
2026-10-23. The release workflow's "the binaries' dynamic dependencies" step
fails the build when the server links a library outside that set.

**nFPM does not translate package names.** `depends` in `release/nfpm.yaml` is
written into every package as it stands, so it is in Debian's vocabulary, and
`overrides` gives the .rpm and the Arch package their own. 0.1.0 and 0.1.1
shipped `libgcc-s1` and `libc6` in all three formats, which dnf and pacman
cannot resolve (#1105). `build-artifacts.py --self-test` now reads each built
package's dependency list back (the .deb's `control`, the .rpm's
`REQUIRENAME`, the Arch package's `.PKGINFO`) and compares it with nfpm.yaml.

The .rpm names sonames, not packages: the same requirement then resolves on
Fedora (`libgcc`, `zlib-ng-compat`), RHEL (`libgcc`, `zlib`) and openSUSE
(`libgcc_s1`, `libz1`), and it is what `rpmbuild` would generate from
`DT_NEEDED` itself.

**zlib is linked dynamically, on purpose.** `libz.so.1` arrived with the
`mysql-wire` feature: `opensrv-mysql` depends on `mysql_common` with its
default features, which turn on `flate2/zlib`, and that links the system zlib
through `libz-sys`. The alternative is libz-sys's `static` feature (or
`LIBZ_SYS_STATIC=1`), which builds a bundled zlib into the binary. The packages
declare zlib instead, because:

- zlib is already installed on every target. On Debian and Ubuntu, dpkg itself
  depends on `zlib1g`, and Fedora, RHEL, openSUSE and Arch install zlib with the
  base system. The dependency costs an install nothing.
- A zlib security fix (CVE-2022-37434, for example) then reaches the server
  through the distribution's own update. A static copy would wait for a Loams
  release with a newer libz-sys.
- The container image (`deploy/release/Containerfile.loams`, debian-slim)
  already has `zlib1g`, so the packages and the image link the same way.

If a target without a system zlib ever matters (a distroless or musl image),
`LIBZ_SYS_STATIC=1` on that build alone is the switch, and that build's
`DT_NEEDED` loses `libz.so.1`.

**`libm.so.6` and the loader are deliberately not declared separately.** Every
target's glibc package already ships them: `libc6` on Debian and Ubuntu (glibc
has carried libm since 2.34), and `glibc` on Fedora, RHEL, openSUSE, Arch and
CachyOS (`pacman -Qo` confirms the last). There is no `libm` package on Debian
to name, so declaring one would produce a dependency that does not exist.

The AUR `PKGBUILD` builds the default feature set, which has no `mysql-wire`
and so no zlib. Its `depends` stays `gcc-libs`.

`git-remote-loams` needs a subset of the server's libraries (no zlib), so the
one `depends` list covers both binaries. The workflow's "the binaries' dynamic
dependencies" step fails the build if the helper ever needs a library the
server does not.

The list this short is a consequence of the server's `reqwest` being
`default-features = false`, so it links no TLS or proxy library.
**[verified]** — see [the feature set](#the-feature-set).

### The feature set, and what is not in the package

Packages are built with the release image's features (#925):
`live,live-tikv,durable,durable-tikv,graph,tikv,pgwire,mysql-wire,stream-grpc`
on top of the default set (`es`, `flight`, `hnsw`, `live`, `qdrant`). The list is
`LOAMS_PACKAGE_FEATURES` in `.github/workflows/release-package.yml`, and the
Linux job fails if it differs from `ARG FEATURES` in
`deploy/release/Containerfile.loams`. So `loams dev` from a package serves what
the image serves: Live on 127.0.0.1:7710, and Durable (the embedded Resonate
server) on 127.0.0.1:8001, with data under `<data-dir>/durable/`.
`scripts/release/smoke-durable.sh` checks that Durable answers there, on Linux,
on Windows and on the macOS runner that can run its binary natively.

The 0.1.0 packages were built from the default set alone, so they had no
Durable: `loams dev` logged "this build has no durable execution" and nothing
listened on 8001.

The `depends` list above still holds. Resonate's SQLite comes from `rusqlite`
with `bundled`, which compiles it into the binary, and the image's runtime stage
installs no libraries beyond `ca-certificates`, `curl` and `tini`.

Not in the package:

| Feature | Not in the package because |
|---|---|
| `durable-mysql` | Not in the image either. |
| `postgres` | `pg-control` ships as its own image (`loams-pg-control`). |
| `failpoints` | Release builds must not contain failpoints. |

## Windows

**Unsigned, by decision (D620).** There is no Authenticode certificate in this
project and buying one is a deliberate future decision. See
[`docs/build-from-source/windows.md`](../build-from-source/windows.md) for the
full local toolchain, and [`signing.md`](signing.md) for why unsigned is the
right answer here.

The artifact is a **`.zip`, not an `.msi`**: there is no installer UI in this
project, and wrapping a console server binary in one would add a WiX build, a
code-signing requirement, and nothing a user of the zip does not already do by
hand. The zip holds `loams.exe`, `git-remote-loams.exe`, the built console,
`LICENSE`, `NOTICE` and `README.md` — the same content the Linux packages
install, minus the systemd units.

A **downloaded** copy triggers SmartScreen's *"Windows protected your PC"*. That
is an **unknown-publisher** warning, not a malware detection, and it clears per
file once the user unblocks it:

```powershell
Get-Item .\loams.exe | Unblock-File
```

A **locally built** copy does not prompt: SmartScreen is driven by a
Mark-of-the-Web, the `Zone.Identifier` stream Windows attaches to downloaded
files, and `cargo build` output has none.

The Windows job builds on `windows-latest` with the **MSVC** host
(`x86_64-pc-windows-msvc`), not the GNU host, and installs the two
prerequisites `windows.md` identifies: **`protoc`** (six `build.rs` scripts
shell out to it) and **NASM** (`aws-lc-sys` needs it on windows+x86_64 unless
`AWS_LC_SYS_PREBUILT_NASM=1`). **[verified]** — read from the build scripts and
the vendored `aws-lc-sys` builder source, as documented in `windows.md`.

## Installing

### Debian / Ubuntu

```sh
sudo apt install ./loams_0.0.1_amd64.deb
```

Verify a signed package first:

```sh
ar p loams_0.0.1_amd64.deb _gpgorigin > /tmp/origin.sig
gpg --verify /tmp/origin.sig loams_0.0.1_amd64.deb
```

The signature covers `debian-binary + control.tar.xz + data.tar.xz` — the whole
package except the `_gpgorigin` member itself, so any change to any member
invalidates it. **[verified]** — confirmed against a locally signed `.deb`.

### Fedora / RHEL / openSUSE

```sh
sudo dnf install ./loams-0.0.1-1.x86_64.rpm
rpm -q --checksig loams-0.0.1-1.x86_64.rpm
```

The `.rpm` is unsigned until SignPath has signed it, and `rpm -q --checksig`
says so. **`rpm --import` the project key first** once signing is live.

### Arch / CachyOS

```sh
sudo pacman -U ./loams-0.0.1-1-x86_64.pkg.tar.zst
```

pacman reads the detached signature `loams-0.0.1-1-x86_64.pkg.tar.zst.sig`
that sits next to the package, so **keep the two files together**. Import the
project key first once signing is live, or `SigLevel = Required` refuses the
package.

### Windows

```powershell
Expand-Archive .\loams-0.0.1-windows-x86_64.zip -DestinationPath loams
Get-Item .\loams\loams.exe, .\loams\git-remote-loams.exe | Unblock-File
.\loams\loams.exe dev
```

Add the `loams` directory to PATH for `git-remote-loams.exe` to be found.

### Git push and fetch to the bucket

Every package installs `git-remote-loams` next to `loams`. git runs a helper
named `git-remote-<scheme>` for a `<scheme>::<address>` URL, so with the helper
on PATH, plain git reaches a Loams Git repository in the bucket directly, with
no Loams server in between:

```sh
git-remote-loams --version
# The first push creates the repository (only with this variable set, so a typo
# in a URL never creates one).
LOAMS_GIT_CREATE_ON_PUSH=1 git -C myrepo push loams::<store-url>/ns/<namespace-id>/repos/<repo> main
git clone loams::<store-url>/ns/<namespace-id>/repos/<repo> work
git -C work push origin main
```

The URL's parts:

- `<store-url>` is the bucket the engine uses (`LOAMS_BUCKET`, for example
  `s3://bucket/prefix` or `file:///var/lib/loams/bucket`). The helper opens it
  itself, with the backend's credentials from the environment (`AWS_*`,
  `GOOGLE_*`, `AZURE_*`).
- `<namespace-id>` is the namespace's **numeric id**, not its name. The bucket
  keys everything by namespace id; the names live in the engine's metadata
  store, which the helper never reaches, so a name there is refused with a
  message that says so. The engine answers the id as `namespace_id` from
  `loams.collection.v1.NamespaceService/CreateNamespace` (for a namespace that
  already exists, the `already_exists` error carries it as `id`), and as `id`
  from `POST /v1/namespaces`.
- `<repo>` is the repository's id (`[a-z0-9][a-z0-9._-]{0,99}`); a push with
  `LOAMS_GIT_CREATE_ON_PUSH=1` creates the repository under that id. The
  helper also accepts a repository name from `loams.repos.v1`'s catalog, but
  that service (cargo feature `git`) is not in the release images, so there
  `CreateRepo` and the other `RepoService` calls answer
  `feature_not_in_variant`: repositories are created by `git-remote-loams`
  push, and the console does not list them.

The helper reads refs from the repository's WAL in the bucket, fetches stored
packs, and pushes by uploading one pack and committing one ref transaction
(design §36 §6.2). It runs `git` as a process, so git must be installed. On
Linux the packages put it in `/usr/bin`; from the zip or a tarball, put the
unpacked directory on PATH (or copy both binaries into a directory that is).

## systemd

Two units are installed.

`loams.service` is the single-node case: `loams dev` with its data in
`/var/lib/loams`, started with `systemctl enable --now loams`.

`loams@.service` is the instance template, and **the instance name is the
subcommand**: `systemctl enable --now loams@dev`, `loams@standalone`,
`loams@cluster`, `loams@warm`, `loams@durable`.

Configuration goes in **`/etc/default/loams`**, which both units read through
`EnvironmentFile=-`. The leading `-` means the file is optional, so the unit runs
as shipped with no configuration at all.

```sh
# /etc/default/loams
# Extra arguments are word-split into separate arguments by systemd.
LOAMS_ARGS=--listen 0.0.0.0:8080
```

**`loams.service` passes no `--listen` at all**, and that is deliberate. The
`EnvironmentFile` is optional, so on a fresh install systemd expands an unset
`${LOAMS_LISTEN}` to the empty string and the command becomes
`loams dev --listen ` with no value — which clap rejects, and the unit fails to
start. `dev`'s own default is `127.0.0.1:8080`, so the unit starts as shipped and
`LOAMS_ARGS` is how the address is changed. **[verified]** — the default is
`#[arg(long, default_value = "127.0.0.1:8080")]` on the `Dev` variant in
`crates/loams/src/main.rs`.

Both units carry the same hardening (`NoNewPrivileges`, `PrivateTmp`,
`ProtectSystem=strict`, `ProtectHome`, `RestrictSUIDSGID`, …) and deliberately
**omit** `MemoryDenyWriteExecute`: a hardening directive that silently breaks a
query is worse than none.

After installing or changing a unit:

```sh
sudo systemctl daemon-reload
```

## Repositories

[`scripts/release/make-repo.py`](../../scripts/release/make-repo.py) builds all
three repository layouts: an **apt** repository (generated in Python, so it is
testable on any machine), a **dnf** repository (`createrepo_c`), and a
**pacman** repository (`repo-add`).

Its governing rule: **an unsigned package is never published.** By default an
unsigned package is a hard failure, because the only thing that script produces
is a repository, and a repository holding an unsigned package is a public
registry publishing unsigned artifacts. `--allow-unsigned` exists for one
purpose — building a tree in CI so a maintainer can look at it — and the
workflow never publishes a tree built with it. `make-repo.py verify` is the
publication gate and has **no** bypass, because a gate with a bypass is not a
gate.

**No workflow publishes a repository yet**, on purpose. The .rpm is unsigned at
the moment this workflow finishes, and a dnf repository needs a signed .rpm;
SignPath runs afterwards, in `release-sign.yml`. Publishing apt and pacman trees
before SignPath has signed anything would put a half-signed release into a
public registry. When the handoff below is done, publishing is a separate
workflow that consumes the signed .rpm — the scripts are ready for it and the
ordering is the only thing missing.

## Reproducibility

Packages bake in the **release commit's** timestamp (`--mtime`), never the build
machine's clock, and the staging tree is built once and packed three ways.

**An unsigned package is byte-identical across builds of the same commit.**
**[verified]** — two builds of the same binary at the same `--mtime` produce an
identical `.deb`, `.rpm` and `.pkg.tar.zst`, compared by sha256.

**A signed `.deb` is not**, and this is a property of GPG rather than of this
pipeline: a signature embeds its own creation time, so the `_gpgorigin` member
differs between two runs. **[verified]** — with the same inputs, the `.deb`'s
`control` and `data` members are byte-identical between two builds and only
`_gpgorigin` differs; the unsigned `.deb` is identical outright. The pacman
`.sig` differs for the same reason, while the `.pkg.tar.zst` it signs does not.

So the reproducibility guarantee covers the package contents. Anything that
verifies a signature should compare the extracted members or the sha256 the
release publishes, not a locally rebuilt signed `.deb`.

## Verifying this page

```sh
bash scripts/release/install-nfpm.sh ./bin          # pinned nFPM, SHA-256 checked
PATH="$PWD/bin:$PATH" python3 scripts/release/build-artifacts.py --self-test
PATH="$PWD/bin:$PATH" python3 scripts/release/make-repo.py --self-test
```

Both self-tests build **real** `.deb`, `.rpm` and `.pkg.tar.zst` files from
fixtures and read every field back, so a packaging definition nFPM rejects fails
here rather than on a release. The signing paths are exercised by adding a key:

```sh
python3 scripts/release/build-artifacts.py --self-test --gpg-key /path/to/key.asc
```

The self-tests skip what they cannot run and **say so** — the three packages need
nFPM, the dnf repository needs `createrepo_c`, the pacman repository needs
`repo-add` — rather than passing quietly. `.github/workflows/ci.yml`'s
`packaging` job runs both on every pull request, and `release-package.yml` runs
them again before it builds a release.

## The handoff

Three items. Each is one value. **None can be created from this repository, and
the available automation token cannot create any of them.**

| Kind | Name | Value |
|---|---|---|
| secret (org or repo) | `LOAMS_GPG_PRIVATE_KEY` | The project's GPG private key, ASCII-armored, **no passphrase** (nFPM and `gpg` are both invoked non-interactively). This is the same key material the SignPath policy holds; create it first and hand the public half to SignPath. |
| variable (org or repo) | `LOAMS_GPG_KEY_ID` | The key's **long key id** — the low 16 hex digits of the fingerprint. nFPM parses `key_id` with `ParseUint(base 16, 64 bits)`, so the 40-character fingerprint overflows it. |
| (already documented) | `SIGNPATH_*` | The five SignPath values in [`signing.md`](signing.md#the-handoff), which sign the .rpm. |

Repository scope is enough, which removes the organisation-owner requirement:

```sh
gh secret set LOAMS_GPG_PRIVATE_KEY --repo ostrium-labs/loams < key.asc
gh variable set LOAMS_GPG_KEY_ID --repo ostrium-labs/loams --body '<16 hex digits>'
```

The irreducible human step is **creating the key and publishing its public half**
on a page users can fetch, so that `rpm --import` and `pacman -Keyring` have
something to trust. That is the same GPG certificate the SignPath signing policy
carries, so it is created once.

**What remains unsigned until then:** every `.deb` and every `.pkg.tar.zst`
(no key), the `.rpm` (until SignPath runs), and the Windows `.exe` (by decision,
permanently). `signing.json` on every release records which artifact has which
signature, read back off the finished files, so nothing has to be guessed.

## Decision-log rows

| Row | What it records |
|---|---|
| **D628** | Linux packages are built with nFPM from one config: `.deb`, `.rpm` and `.pkg.tar.zst`, default feature set (since #925, the release image's features). |
| **D629** | The `.rpm` is built unsigned and signed by SignPath; the `.deb` and the pacman package are signed with the project GPG key. |
| **D630** | Windows ships an unsigned `.zip`, not an MSI. |
| **D631** | No repository is published until SignPath has signed the `.rpm`. |

## Out of scope

- **macOS and iOS.** No artifact of either kind is built by this workflow, by
  design (D620). macOS remains build-it-yourself:
  [`docs/build-from-source/macos.md`](../build-from-source/macos.md).
- **Android.** There is **no Android or Gradle project in this repository**, so
  there is no APK to package and none was invented. Signing an `.apk` is
  something SignPath's free programme *can* do, which makes it tempting to
  imply one exists; it does not. Building an Android app is a separate project
  and a separate effort.