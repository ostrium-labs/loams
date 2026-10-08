# Releasing Loams Desktop (Electron)

Status: **pipeline present, nothing published**. This page is what the owner reads to add the
secrets and variables, and to cut a release. The workflows are
[`desktop-electron-release.yml`](../../.github/workflows/desktop-electron-release.yml) (the release)
and [`desktop-sign.yml`](../../.github/workflows/desktop-sign.yml) (SignPath, called by it). The CI
build and test matrix, which never signs, is `desktop-electron.yml`.

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
x86_64-pc-windows-msvc --features live,durable` builds and the binary runs `--version`. Otherwise
the build sets `LOAMS_DESKTOP_NO_LOCAL_ENGINE=1` and the app shows its "not available on Windows
yet" engine state. The engine is always built with `--features live,durable`.

## What the owner must add

Repository **secrets** (Settings > Secrets and variables > Actions):

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
| `SIGNPATH_DESKTOP_WINDOWS_ARTIFACT_CONFIGURATION_SLUG` | New: slug of the configuration pasted from `signpath/artifact-configuration.desktop-windows.xml`. |
| `LOAMS_UPDATE_PUBKEY` | 64 hex chars, the public half of `LOAMS_UPDATE_SIGNING_KEY`. Baked into the app at build time. |
| `LOAMS_UPDATE_FEED` | Optional. Defaults to `https://github.com/<owner>/<repo>/releases/latest/download`. |
| `DESKTOP_REQUIRE_SIGNING` | `true` makes a release without every signature above fail. Default unset (false). |

SignPath setup (project, policies, GitHub App, approvers) is in [signing.md](signing.md); the policy
file is [`signpath/pipeline-policy.md`](../../signpath/pipeline-policy.md). The Windows policy is a second
signing policy on the same project, and the Windows artifact configuration has to be created in
SignPath by pasting the XML. Its `product-name` attribute spelling is unverified against SignPath's
reference; if SignPath rejects the paste, that is the first thing to check. The NSIS uninstaller
embedded in the installer is not separately signed.

## The update feed

The app reads `<LOAMS_UPDATE_FEED>/latest.yml` (Windows), `latest-linux.yml` or
`latest-linux-arm64.yml` (AppImage), `latest-mac.yml` (macOS, version probe only), and the matching
`.yml.sig`. For GitHub releases the feed is the latest-download URL:

```
https://github.com/<owner>/<repo>/releases/latest/download
```

GitHub redirects `releases/latest/download/<file>` to the newest **published, non-draft,
non-prerelease** release, so:

- the draft must be published before anyone is offered the update, and ticked **Set as latest release**;
- if the same repository also publishes other releases (the engine's `vX.Y.Z`), "latest" can resolve to
  the wrong one. In that case host the desktop files at a stable URL of your own and set
  `LOAMS_UPDATE_FEED` to it. The feed is a build-time constant, so set it before tagging.

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
