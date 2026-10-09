# Releasing Loams Desktop (Electron)

Status: **pipeline present, nothing published**. This page is what the owner reads to add the
secrets and variables, and to cut a release. The workflows are
[`desktop-electron-release.yml`](../../.github/workflows/desktop-electron-release.yml) (the release)
and [`desktop-sign.yml`](../../.github/workflows/desktop-sign.yml) (SignPath, called by it). The CI
build and test matrix, which never signs, is `desktop-electron.yml`. A third,
[`desktop-promote.yml`](../../.github/workflows/desktop-promote.yml), moves a published release onto the
update feed.

## What each platform gets

| Platform | Files | Signing | Updates |
|---|---|---|---|
| Linux x64, arm64 | AppImage, deb, rpm, pacman | rpm: SignPath (GPG certificate). AppImage, deb, pacman: detached GPG `.sig`. | AppImage updates in place. deb, rpm and pacman installs show **Download vX** (open the release page). |
| Windows x64 | NSIS installer | Authenticode through SignPath: the unpacked app first, then the installer built from it. | Updates in place. |
| macOS arm64, x64 | dmg, zip | **Unsigned, not notarized** (D677). The release notes carry the `xattr` instruction. | **Download vX** only, never self-update. |

Every release also has `SHA256SUMS` (plus `SHA256SUMS.asc` when a GPG key is set). When
`LOAMS_UPDATE_SIGNING_KEY` is set, each `latest*.yml` update manifest gets a `latest*.yml.sig`
(Ed25519, base64; the app refuses an unsigned manifest).

macOS users run, after copying the app to `/Applications`:

```sh
xattr -dr com.apple.quarantine "/Applications/Loams Desktop.app"
```

Nothing pretends to sign. If a signing secret is absent the affected files are built with
**`-unsigned`** in their name (`loams-desktop-1.2.3-linux-x64-unsigned.deb`), the notes say so, and
the run prints a warning. Set the variable `DESKTOP_REQUIRE_SIGNING` to `true` and the `plan` job
fails instead, naming what is missing.

The Windows engine is bundled only if `cargo build --release -p loams --target
x86_64-pc-windows-msvc --features live,durable,live-tikv` builds and the binary runs `--version`. Otherwise
the build sets `LOAMS_DESKTOP_NO_LOCAL_ENGINE=1` and the app shows its "not available on Windows
yet" engine state. The engine is always built with `--features live,durable,live-tikv`.

## What the owner must add

Put the **secrets** in the protected environment `desktop-release` (below), not at repository level.
Environment secrets (Settings > Environments > `desktop-release`):

| Secret | What | If absent |
|---|---|---|
| `LOAMS_GPG_PRIVATE_KEY` | ASCII-armored secret key (`gpg --armor --export-secret-keys <id>`). | No `.sig` files, no `SHA256SUMS.asc`; AppImage, deb, pacman are named `-unsigned`. |
| `LOAMS_GPG_KEY_ID` | Fingerprint or id of the signing key (optional when the keyring holds one key). | The first secret key signs. |
| `LOAMS_GPG_PASSPHRASE` | Only if the key has a passphrase. | Treated as no passphrase. |
| `SIGNPATH_API_TOKEN` | SignPath API token with the submitter role. | rpm and Windows unsigned. |
| `LOAMS_UPDATE_SIGNING_KEY` | 64 hex chars, the Ed25519 seed (Task 14). | `latest*.yml` unsigned; the in-app updater stays off. |

Repository **variables**:

| Variable | What |
|---|---|
| `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` | The same values `release-sign.yml` uses. |
| `SIGNPATH_SIGNING_POLICY_SLUG`, `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | Existing: the rpm (GPG certificate) policy and the configuration from `signpath/artifact-configuration.rpm.xml`. |
| `SIGNPATH_WINDOWS_SIGNING_POLICY_SLUG` | New: a policy with an **Authenticode** certificate. |
| `SIGNPATH_DESKTOP_WINDOWS_ARTIFACT_CONFIGURATION_SLUG` | New: slug of the configuration pasted from `signpath/artifact-configuration.desktop-windows.xml` (the unpacked app; `Loams Desktop.exe` is required in it). |
| `SIGNPATH_DESKTOP_WINDOWS_INSTALLER_ARTIFACT_CONFIGURATION_SLUG` | New: slug of the configuration pasted from `signpath/artifact-configuration.desktop-windows-installer.xml` (the installer, required in it). |
| `LOAMS_UPDATE_PUBKEY` | 64 hex chars, the public half of `LOAMS_UPDATE_SIGNING_KEY`. Baked into the app at build time. |
| `LOAMS_UPDATE_FEED` | Optional. Defaults to `https://github.com/<owner>/<repo>/releases/download/desktop-latest`. |
| `DESKTOP_REQUIRE_SIGNING` | `true` makes a release without every signature above fail. Default unset (false). |

### The `desktop-release` environment

Create it under Settings > Environments and configure:

- **Required reviewers**: at least one maintainer who is not the person pushing the tag. The plan job,
  each SignPath call and the release job run in this environment, so expect one approval prompt
  for each of those jobs. The SignPath token and GPG key are only readable after approval.
- **Deployment branches and tags**: a rule allowing only tags matching `desktop-v*` (and no branches).
- Put `SIGNPATH_API_TOKEN`, `LOAMS_GPG_PRIVATE_KEY`, `LOAMS_GPG_KEY_ID`, `LOAMS_GPG_PASSPHRASE` and
  `LOAMS_UPDATE_SIGNING_KEY` here **as environment secrets**, and nowhere else. Do not also set them as
  repository secrets: that would let any workflow run read them without approval.
- The SignPath jobs (`desktop-sign.yml`) declare the environment themselves and read
  `SIGNPATH_API_TOKEN` from it; the callers pass no secrets, because a job that calls a reusable workflow
  cannot declare an environment. **Unverified until the first real run:** confirm in that run's `plan`
  and SignPath jobs that the token resolves (a "SignPath is not configured" warning with the token
  set means it did not).
- `desktop-promote.yml` also runs in this environment, so publishing a release needs a reviewer's
  approval before clients are offered it.

SignPath setup (project, policies, GitHub App, approvers) is in [signing.md](signing.md); the policy
file is [`signpath/pipeline-policy.md`](../../signpath/pipeline-policy.md). The Windows policy is a second
signing policy on the same project, and the Windows artifact configuration has to be created in
SignPath by pasting the XML. Its `product-name` attribute spelling is unverified against SignPath's
reference; if SignPath rejects the paste, that is the first thing to check. The NSIS uninstaller
embedded in the installer is not separately signed.

The workflow does not trust SignPath's output: after the unpacked app comes back it checks
`Loams Desktop.exe` and `resources/bin/loams.exe` (when bundled), and after the installer comes back it
checks the installer, all with `Get-AuthenticodeSignature` (status must be `Valid`). If a check fails,
the run fails when `DESKTOP_REQUIRE_SIGNING` is `true`; otherwise the installer is released named
`-unsigned` and the notes say so.

## The update feed

The app reads `<LOAMS_UPDATE_FEED>/latest.yml` (Windows), `latest-linux.yml` or
`latest-linux-arm64.yml` (AppImage), `latest-mac.yml` (macOS, version probe only), and the matching
`.yml.sig`. The default feed is a rolling release of this repository:

```
https://github.com/<owner>/<repo>/releases/download/desktop-latest
```

`desktop-latest` is a fixed-name prerelease (so it never becomes GitHub's "latest release", and the
engine's own releases cannot be mistaken for it). It holds the manifests, their `.sig` files and the
payload files they name (the Windows installer, the AppImages), and **only
`desktop-promote.yml` writes to it**. It runs when a `desktop-v*` release is **published**, or on
demand: Actions > Desktop update feed (promote) > Run workflow, with the tag. It refuses a draft, and
skips a prerelease tag, and refuses a version that is not newer than the one in the feed unless dispatched with `allow_downgrade`, so a client is never pointed at a draft or an unfinished version. Files upload in order (payloads, then `.sig` files, then manifests last), so a client never sees a
manifest without its payload and signature. Assets of earlier versions are removed afterwards.

The feed is a build-time constant, so set `LOAMS_UPDATE_FEED` (if you override it) before tagging.

## Cutting a release

1. Bump `version` in `apps/desktop-electron/package.json` and merge it to the default branch.
2. Check the secrets and variables above. For the first release, run with them unset to see the
   `-unsigned` output, or set `DESKTOP_REQUIRE_SIGNING=true` to insist on all of them.
3. Tag the merged commit and push the tag: `git tag desktop-v1.2.3 && git push origin desktop-v1.2.3`.
   The tag must equal `desktop-v` plus the package version, and sit on the default branch; a tag with
   a `-suffix` (`desktop-v1.2.3-rc.1`) makes a prerelease.
4. The workflow builds both Linux architectures, Windows and both macOS architectures, waits for
   SignPath (it can wait for a human approver, up to 50 minutes per request), and creates a **draft**
   release holding every file, `SHA256SUMS`, and the signatures.
5. Open the draft, read the notes (they state exactly what is and is not signed), then publish it.
   Publishing triggers `desktop-promote.yml`, which copies the manifests onto `desktop-latest`; check
   its run, or dispatch it by hand with the tag. Re-running the release for a tag whose draft still exists
   reuses that draft (files replaced, notes refreshed); a tag that is already published is refused.

`SHA256SUMS` covers the release files only. The manifest `latest*.yml.sig` files are outside it by
design: they sign the manifest, which is what the app verifies, and a signature file cannot sit in the
list of files it signs.

Verify a download:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gpg --verify SHA256SUMS.asc SHA256SUMS
gpg --verify loams-desktop-1.2.3-linux-x64.AppImage.sig loams-desktop-1.2.3-linux-x64.AppImage
rpm --import loams-release.asc && rpm --checksig loams-desktop-1.2.3-linux-x64.rpm
```

## Helper scripts (`apps/desktop-electron/scripts/`)

| Script | Job |
|---|---|
| `checksums.mjs <dir>` | Writes `SHA256SUMS` for the files in `<dir>`. |
| `gpg-sign.sh <dir>` | Detached `.sig` per AppImage, deb, pacman, dmg and zip, and `SHA256SUMS.asc`. Throwaway keyring; skips with a warning when no key is set. |
| `sign-manifest.mjs <latest*.yml>` | Ed25519 signature file for an update manifest. |
| `refresh-manifest.mjs <latest.yml> <dir>` | Recomputes `sha512` and `size` after SignPath replaced the installer. |
| `windows-sign.cjs` | Optional local Jsign hook (`WINDOWS_SIGN_KEYSTORE`); the password is read from the environment or a file (`WINDOWS_SIGN_STOREPASS`, `WINDOWS_SIGN_STOREPASS_FILE`), never the command line, and a missing password is an error. Release signing goes through SignPath instead. |

Tests: `pnpm --filter @loams/desktop test` (the `release-scripts` suite runs `gpg-sign.sh` with a
throwaway key and `gpg --verify`), and `python3 scripts/ci/signpath-artifacts.py --self-test`.
