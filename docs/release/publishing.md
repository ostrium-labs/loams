# Publishing Loams: registries, secrets and the release path

Status (2026-10-19): **container images publish on every tag. `v0.1.0` is live
on npm, PyPI, crates.io, NuGet, the Go proxy, RubyGems and Maven Central, and as
the C++ tarball on the GitHub release.** The PHP split repository
`ostrium-labs/loams-php` has `main` and `v0.1.0`, but the package is not on
Packagist until the owner submits it (step 5 below). pub.dev and CocoaPods are
not turned on. The SDK release workflow publishes nothing to a registry until
the owner turns it on. Every SDK goes out under the name `loams`, or that
registry's spelling of it. The `v0.1.0-preview.N` tags publish pre-release
versions. `v0.1.0` is a normal release (owner, 2026-10-17), and the SDK READMEs
still call the APIs a preview until 1.0.
[`.github/workflows/sdk-release.yml`](../../.github/workflows/sdk-release.yml)
does the publishing. This page is the owner's checklist for each registry: the
exact trusted-publisher fields, secrets and variables. It is written for
`ostrium-labs/loams`; the decisions it follows are in
[`docs/design/13-decision-log.md`](../design/13-decision-log.md).

## What exists today

| SDK | Registry and name | Version for `v0.1.0` (for `v0.1.0-preview.2`) | Credential | Gate variable | Environment |
|---|---|---|---|---|---|
| TypeScript | npm `@loams/live`, `@loams/client` | `0.1.0`, dist-tag `latest` (`0.1.0-preview.2`, dist-tag `preview`) | trusted publishing; bootstrap secret `NPM_TOKEN` | `SDK_PUBLISH_NPM` | `npm` |
| Python | PyPI `loams` | `0.1.0` (`0.1.0a2`) | trusted publishing | `SDK_PUBLISH_PYPI` | `pypi` |
| Rust | crates.io `loams-proto`, `loams-live-proto`, `loams` | `0.1.0` (`0.1.0-preview.2`) | trusted publishing; bootstrap secret `CARGO_REGISTRY_TOKEN` | `SDK_PUBLISH_CRATES` | `crates-io` |
| C# | NuGet `Loams` | `0.1.0` (`0.1.0-preview.2`) | trusted publishing (`NuGet/login`), variable `NUGET_USER` | `SDK_PUBLISH_NUGET` | `nuget` |
| Go | `loams.dev/go` through proxy.golang.org | tag `sdks/go/v0.1.0` (`sdks/go/v0.1.0-preview.2`) | `GITHUB_TOKEN`, plus the `go-import` tag on loams.dev | `SDK_PUBLISH_GO` | none |
| Swift | SwiftPM, this repository | the release tag | none | none | none |
| C++ | GitHub release assets `loams-cpp-<v>.tar.gz` | `0.1.0` (`0.1.0-preview.2`) | `GITHUB_TOKEN` | `SDK_PUBLISH_CPP` | none |
| Ruby | RubyGems `loams` | `0.1.0` (`0.1.0.pre.2`) | trusted publishing | `SDK_PUBLISH_RUBYGEMS` | `rubygems` |
| PHP | Packagist `loams/loams`, read from the split repository `ostrium-labs/loams-php` | tag `v0.1.0` (`v0.1.0-alpha.2`) | deploy key secret `PACKAGIST_SPLIT_DEPLOY_KEY` | `SDK_PUBLISH_PACKAGIST` | `packagist` |
| Dart | pub.dev `loams` | `0.1.0` (`0.1.0-preview.2`) | trusted publishing, on a tag push only | `SDK_PUBLISH_PUBDEV` | `pub.dev` |
| Java | Maven Central `dev.loams:loams` | `0.1.0` (`0.1.0-preview.2`) | secrets `MAVEN_CENTRAL_USERNAME`, `MAVEN_CENTRAL_PASSWORD`; GPG `LOAMS_GPG_PRIVATE_KEY` | `SDK_PUBLISH_MAVEN` | `maven-central` |
| Kotlin | Maven Central `dev.loams:loams-kotlin` | `0.1.0` (`0.1.0-preview.2`) | same as Java | `SDK_PUBLISH_MAVEN` | `maven-central` |
| Objective-C | CocoaPods `Loams` (module `LoamsObjC`) | `0.1.0` (`0.1.0-preview.2`) | secret `COCOAPODS_TRUNK_TOKEN` | `SDK_PUBLISH_COCOAPODS` | `cocoapods` |

How it behaves:

- **Every run builds and dry-runs everything:** pull requests that touch the
  packaging, every tag, and every manual run. A publish job runs only on a `v*` tag push,
  or on a manual run with `dry_run: false` and a `tag`, and only when its
  `SDK_PUBLISH_*` repository variable is `true`. An unset variable skips that
  registry, so the owner can turn registries on one at a time.
- **Re-running is safe.** Each publish job first checks whether the registry
  already has the version, and skips it if so.
- **The version is the tag.**
  [`scripts/sdk/sdk-version.py`](../../scripts/sdk/sdk-version.py) checks that
  every manifest carries the tag's version in its ecosystem's spelling.
  `sdk-version.py set <version>` bumps them all in the release PR.
- **Environments.** `npm`, `pypi`, `crates-io`, `nuget`, `rubygems`, `packagist`,
  `pub.dev`, `maven-central` and `cocoapods` exist, created 2026-10-17. Each one
  allows deployments only from tags matching `v*` and has no required reviewers;
  the owner can add reviewers. A run dispatched from a branch cannot enter them,
  so **dispatch a re-run on the tag:**
  `gh workflow run sdk-release.yml --ref vX.Y.Z-preview.N -f tag=vX.Y.Z-preview.N -f dry_run=false`.
  The same applies to `release-crates.yml`, which shares `crates-io`.
- **Put secrets in the environment, not the repository**, so that only the
  publish job can read them. The exception is `LOAMS_GPG_PRIVATE_KEY`, which is
  already a repository secret shared with the desktop and package signing.

### Release gotchas

- **A tag must be on `main` before it is pushed.** The plan job refuses to
  publish a tag that `main` (`RELEASE_BRANCH`) does not contain. Run
  38727409927 for `v0.1.0-preview.2` failed this way: the tag was pushed on
  8c0bf39c before PR #630 merged it into `main`. `main` contains it now, so a
  dispatch on the tag passes the gate.
- **A release builds the workflow and the packaging as they are at the tag.**
  `v0.1.0-preview.2` predates the Ruby, PHP, Dart, Java, Kotlin and
  Objective-C packaging, so those six SDKs can publish from `v0.1.0` at the
  earliest. To reserve a name sooner, upload once by hand from `dev`, as
  described per registry below.
- **Composer cannot read `preview`.** It knows `alpha`, `beta` and `RC`, so
  the PHP split repository is tagged `v0.1.0-alpha.2`. A
  `v0.1.0-preview.2` tag would be ignored.
- **Maven orders `preview` above the release.** `ComparableVersion` puts an
  unknown qualifier after the release, so `0.1.0-preview.2` sorts *above*
  `0.1.0`. Gradle sorts it below. Consumers pin exact versions, so this is
  harmless, but tools that pick "the newest" version get it wrong. Use
  `-alpha.N` or `-rc.N` for Maven if that matters later.
- **CocoaPods trunk becomes read-only on 2026-12-02**, with a test shutdown on
  2026-11-01 to 07 (blog.cocoapods.org/CocoaPods-Specs-Repo). After that no
  pod can be pushed. Reserving `Loams` is only worth it before November, and
  Objective-C users will need SwiftPM after that.

## Owner checklist, per registry

Everything below happens on the registry's website or in the GitHub
repository settings, and only the owner can do it. Where a field asks for a
workflow, enter the file name `sdk-release.yml`, not a path. The owner of the
repository is `ostrium-labs` and the repository is `loams`.

### npm (`@loams/live`, `@loams/client`)

1. The org `loams` belongs to the npm user `loams-admin`.
2. **Bootstrap (done 2026-10-17):** the `npm` environment has the secret `NPM_TOKEN`, a granular token with read and write access to `@loams`. If the account enforces 2FA on writes, the token needs "bypass 2FA".
3. Set the variable `SDK_PUBLISH_NPM=true` and run the release. The first upload uses the token.
4. Then, for **each** package, go to npmjs.com → package → Settings → Trusted publishing → GitHub Actions, and enter:
   - Organization or user: `ostrium-labs`
   - Repository: `loams`
   - Workflow filename: `sdk-release.yml`
   - Environment name: `npm`
5. Delete the `NPM_TOKEN` secret. Optionally, set each package's publishing access to "Require 2FA and disallow tokens", since OIDC still works with that setting.

### PyPI (`loams`)

1. Go to pypi.org → Your account → Publishing → Add a new pending publisher → GitHub, and enter:
   - PyPI project name: `loams`
   - Owner: `ostrium-labs`
   - Repository name: `loams`
   - Workflow name: `sdk-release.yml`
   - Environment name: `pypi`
2. Set the variable `SDK_PUBLISH_PYPI=true`. The first upload creates the project. No secret is needed.

### crates.io (`loams-proto`, `loams-live-proto`, `loams`)

1. crates.io trusts a workflow only for a crate that already exists. For the first upload, put the secret `CARGO_REGISTRY_TOKEN` in the `crates-io` environment. Generate it at crates.io → Account Settings → API Tokens with the scopes `publish-new` and `publish-update`, the crate pattern `loams*`, and a short expiry.
2. Set the variable `SDK_PUBLISH_CRATES=true` and run the release.
3. Then, for each of the three crates, go to crate → Settings → Trusted Publishing → Add → GitHub, and enter:
   - Repository owner: `ostrium-labs`
   - Repository name: `loams`
   - Workflow filename: `sdk-release.yml`
   - Environment: `crates-io`
   For crates that `release-crates.yml` uploads, add a second publisher with the workflow `release-crates.yml`.
4. Delete `CARGO_REGISTRY_TOKEN`.

### NuGet (`Loams`)

1. Go to nuget.org → your user name → Trusted Publishing → Create, and enter:
   - Policy name: `loams-sdk-release`
   - Package owner: the user or organization that will own `Loams`
   - Repository owner: `ostrium-labs`
   - Repository: `loams`
   - Workflow file: `sdk-release.yml`
   - Environment: `nuget`
2. Set the repository variable `NUGET_USER` to the nuget.org **profile name** (not the email) of the account that created the policy. It is not a secret.
3. Set the variable `SDK_PUBLISH_NUGET=true`.
4. Optional: ask NuGet (account@nuget.org) to reserve the prefix `Loams.`.

### RubyGems (`loams`)

1. Go to rubygems.org → your profile → Trusted publishers → Pending trusted publishers → Create, and enter:
   - Gem name: `loams`
   - Repository owner: `ostrium-labs`
   - Repository name: `loams`
   - Workflow filename: `sdk-release.yml`
   - Environment: `rubygems`
   Leave the two reusable-workflow fields empty.
2. Set the variable `SDK_PUBLISH_RUBYGEMS=true`. The first push creates the gem and turns the pending publisher into a normal one. No secret is needed.
3. To reserve the name now, before a tag carries the gemspec: `cd sdks/ruby && gem build loams.gemspec && gem push loams-0.1.0.gem`, from `dev` and with MFA.

### Packagist (`loams/loams`)

Packagist reads `composer.json` from a repository's root and builds versions from
its tags. Neither works on this monorepo: its root is not the PHP package, and
`v0.1.0-preview.2` is not a Composer version. So the PHP SDK is published
through a read-only **split repository**. Each release,
[`scripts/sdk/php-split.sh`](../../scripts/sdk/php-split.sh) pushes one commit
whose tree is exactly `sdks/php`, plus the Composer tag, to that repository. The
alternative, a root `composer.json` with `"autoload": "sdks/php/src"`, would make
every `composer require` download the whole monorepo and still could not read
the tags.

1. Create the public repository `ostrium-labs/loams-php`, empty, with the description "Read-only split of sdks/php in ostrium-labs/loams; send changes there". Turn off issues and wiki.
2. Run `ssh-keygen -t ed25519 -N '' -C loams-php-split -f loams-php-split`. Add `loams-php-split.pub` to loams-php → Settings → Deploy keys, with **Allow write access**. Store the private key `loams-php-split` as the secret `PACKAGIST_SPLIT_DEPLOY_KEY` in the `packagist` environment, then delete the local files.
3. Optional: set the variable `PACKAGIST_SPLIT_REPO` if the split repository has another name. The default is `ostrium-labs/loams-php`.
4. Set the variable `SDK_PUBLISH_PACKAGIST=true` and run the release. The first push creates `main` and `v0.1.0-alpha.N`.
5. Sign in to packagist.org with GitHub, then go to Submit and enter the repository URL `https://github.com/ostrium-labs/loams-php`. The name `loams/loams` comes from `composer.json`, and the first submit claims the vendor `loams`.
6. Updates on push: in Packagist → Profile → Settings, connect GitHub so Packagist installs its hook on `loams-php` (that needs admin on the repo). Alternatively, add a webhook on `loams-php` with the payload URL `https://packagist.org/api/github?username=<packagist user>`, content type `application/json`, and as the secret the Packagist API token from the profile page.

### pub.dev (`loams`)

1. **Verified publisher `loams.dev`:** go to pub.dev → Create publisher → `loams.dev`. pub.dev sends you to Google Search Console to verify the domain. Add the **DNS TXT** record it shows, `google-site-verification=…`, on the apex `loams.dev` (in Cloudflare DNS, name `@`). Wait for it to verify, then finish creating the publisher.
2. **The first upload is manual.** pub.dev allows automated publishing only for an existing package. From a checkout whose `sdks/dart/pubspec.yaml` is at the version (`dev` today, or the next release tag), run `cd sdks/dart && dart pub login && dart pub publish`.
3. Go to pub.dev/packages/loams/admin → Transfer to publisher `loams.dev`.
4. On the same admin page, under Automated publishing, enable publishing from GitHub Actions and enter:
   - Repository: `ostrium-labs/loams`
   - Tag pattern: `v{{version}}`
   - Require GitHub Actions environment: on, Environment `pub.dev`
   Leave "publishing from workflow_dispatch" off. The job only runs on a tag push.
5. Set the variable `SDK_PUBLISH_PUBDEV=true`.

### Maven Central (`dev.loams:loams`, `dev.loams:loams-kotlin`)

1. Sign in at central.sonatype.com → Namespaces → Add namespace `dev.loams`. The portal shows a verification key. Add a **DNS TXT** record on the apex `loams.dev` (name `@`) whose value is that key, then click Verify. The record can be removed once the namespace is verified.
2. Go to Account → Generate User Token. Store the two values as the secrets `MAVEN_CENTRAL_USERNAME` and `MAVEN_CENTRAL_PASSWORD` in the `maven-central` environment. They are the token's username and password, not the login.
3. Central rejects artifacts whose signing key it cannot find. Upload [`loams-release-key.asc`](loams-release-key.asc) (the current key, fingerprint `29C5CC3F0FD66A2AD32ECB309E8EC3E0AAB5BA68`, and the previous one, `333AC94599D5A1EB2A416D1E83DC134D8CAF895D`; see [signing.md](signing.md)) at https://keyserver.ubuntu.com/ (Submit Key) and at https://keys.openpgp.org/upload. keys.openpgp.org then asks you to confirm `security@ostriumlabs.org`. The publish job also tries `gpg --send-keys` to keyserver.ubuntu.com. The workflow uses the existing repository secret `LOAMS_GPG_PRIVATE_KEY` and variable `LOAMS_GPG_KEY_ID`.
4. Set the variable `SDK_PUBLISH_MAVEN=true`. The job uploads each bundle with `publishingType=AUTOMATIC` and waits until Central reports `PUBLISHING` or `PUBLISHED`.

### CocoaPods (`Loams`)

1. Read the trunk shutdown note under [Release gotchas](#release-gotchas) first.
2. On a Mac, run `pod trunk register security@ostriumlabs.org 'Ostrium Labs' --description='loams sdk-release'` and click the link in the email. Copy the token from `~/.netrc` (the `password` for `machine trunk.cocoapods.org`), and store it as the secret `COCOAPODS_TRUNK_TOKEN` in the `cocoapods` environment. Trunk sessions expire, so re-register if a push says the session is invalid.
3. Set the variable `SDK_PUBLISH_COCOAPODS=true`. `pod trunk push` clones the tag named in [`Loams.podspec`](../../Loams.podspec), so the tag must exist on GitHub first, which it does on a tag run. The first push claims `Loams` for the registered email.

### Go and C++ (no account)

- Go: `loams.dev/go?go-get=1` must serve
  `<meta name="go-import" content="loams.dev/go git https://github.com/ostrium-labs/loams sdks/go">`
  and
  `<meta name="go-source" content="loams.dev/go https://github.com/ostrium-labs/loams https://github.com/ostrium-labs/loams/tree/main/sdks/go{/dir} https://github.com/ostrium-labs/loams/blob/main/sdks/go{/dir}/{file}#L{line}">`.
  Every `/go/...` subpath must serve them too. That page belongs to the site
  repository. Then set `SDK_PUBLISH_GO=true`.
- C++: set `SDK_PUBLISH_CPP=true`. The archive is attached to the GitHub release
  that `release-github.yml` creates.

A rule for later: a registry gets a workflow when something in this repository
is ready to be installed from it, not when an issue lists it.

## Container images (`ghcr.io`)

The release images, the contract the compose stack in
[`deploy/release/compose.yaml`](../../deploy/release/compose.yaml) and
[Running Loams](preview.md) rely on:

| Image | Containerfile | Contents |
|---|---|---|
| `ghcr.io/ostrium-labs/loams` | `deploy/release/Containerfile.loams` | The engine, built with the features `live,live-tikv,durable,durable-tikv,graph,git,tikv,pgwire,mysql-wire,stream-grpc` |
| `ghcr.io/ostrium-labs/loams-fabric` | `deploy/release/Containerfile.loams-fabric` | The House front, with `loams-house-worker` inside |
| `ghcr.io/ostrium-labs/loams-agentd` | `deploy/release/Containerfile.loams-agentd` | The agent daemon (optional) |

- **Build.** `deploy/release/build.sh` builds every image from its
  `deploy/release/Containerfile.<name>`, locally with Docker or Podman and in CI the
  same way.
- **Publish.** CI publishes on tags only, never from a pull request, with the
  workflow's own `GITHUB_TOKEN` (`packages: write`); there is no stored registry
  credential.
- **Tags.** Every image gets `:<version>` (for example `0.1.0-preview.1`, no `v` prefix) and
  the rolling `:preview`, which moves with each prerelease (`-preview.N`, `-rc.N`). A
  stable version (no `-`, for example `0.1.0`) gets `:<version>`, `:<major>.<minor>` and
  `:latest` instead, and is a normal GitHub release (the repository's "Latest"); a
  prerelease is a GitHub pre-release and never `:latest`. The rule is
  [`scripts/release/channel.sh`](../../scripts/release/channel.sh). Deployments that must
  not move pin `:<version>`.
- **Images built elsewhere.** `ghcr.io/ostrium-labs/tidb:v8.5.8-loams.1` is built by
  [`scripts/sqldb/tidb-image/build.sh`](../../scripts/sqldb/tidb-image/build.sh) from an
  annotated patch-queue tag, is public, amd64 only, and is pinned in
  [`release/sqldb-images.toml`](../../release/sqldb-images.toml), and the Postgres stack's images
  are pinned by digest in
  [`deploy/pins/loams-postgres-images.toml`](../../deploy/pins/loams-postgres-images.toml).
- **Not yet.** No attestation or cosign signature is attached to the images in
  0.1.0, and the Loams images are not promised to be multi-architecture.

### Third-party pins (design §54 §9.2)

PD, TiKV and TiDB (`deploy/release/.env.example`, `release/sqldb-images.toml`), the Loams Postgres images
(`deploy/pins/loams-postgres-images.toml`), chDB (`loams-chdb-sys`) and Grafeo (`Cargo.toml`) follow these rules:
- A pin changes in its own pull request into `dev`. It names the old and new versions and the component's own
  upgrade notes.
- The pull request carries the `ci:upgrade` label, so the upgrade gate runs on it. The repository checks fail a
  pin change without the label.
- The newest release's `[release.components]` in `release/upgrade-path.toml` equals the pin files, and no
  component goes down against the previous release that lists components.
- A bump across a component's major needs a `point_of_no_return` migration entry in that release, because its
  files do not go back.
- `scripts/release/check-upgrade-path.sh` checks the last two rules.

## What crates.io actually publishes

The crate list is declared in the workflow's `CRATES` and checked by
[`scripts/ci/publishable-crates.py`](../../scripts/ci/publishable-crates.py),
which derives the same list from the manifests and fails the run when the two
disagree. A crate is publishable when its manifest does not say
`publish = false` and none of its normal dependencies is itself unpublishable;
dev-dependencies do not count, because cargo strips them from a published
manifest.

Two crates are excluded by a `publish = false` this change adds, and both are
structural, not a judgement about their API:

- `loams`: its optional `loams-tikv`, `loams-durable` and `loams-meta-tikv`
  dependencies appear in the published manifest, and all three are
  unpublishable. `loams-tikv` and `loams-durable` depend on git revisions of
  private forks, which no registry can resolve. Uploading the binary today
  would produce a crate nobody can build.
- `loams-safekeeper`: same shape through its `tikv` feature and `loams-tikv`.

Both flags are one line each, and both come off in the change that publishes
the binary (D294's install, published as `loams-server`, D1106) or the first release after the forks
are vendored. This page is where that promise is written down; the crate
comments say it in place.

## crates.io: the workspace crates (`release-crates.yml`)

This section covers the server's library crates, not the Rust SDK above.

1. **Claim the namespace.** crates.io rejects a crate whose name is taken. The
   `loams` and `loams-*` names are free as of 2026-10-03 (the sparse index
   answers 404 for `loams`, `loams-common`, `loams-collection` and
   `loams-store`), but crates.io does not reserve them: whoever uploads
   `loams-store` first owns it, permanently.
2. **Reserve every name, by hand, with a token.** crates.io's trusted publishing
   only issues a token for a crate that already exists, so the first upload of
   each of the 18 crates is a manual `cargo publish` from a workstation with a
   freshly generated token (`cargo login`, scope `publish-write`, then revoke).
   Do this once per crate, lowest dependency first. No token is stored in this
   repository at any point.
3. **Configure the trusted publisher** on crates.io, for each crate: repository
   `ostrium-labs/loams`, workflow `release-crates.yml`, environment `crates-io`.
   From then on CI mints its own short-lived token through
   `rust-lang/crates-io-auth-action` and no credential is stored in GitHub.
4. **The GitHub environment** `crates-io` exists (2026-10-17) and accepts only
   `v*` tags, so dispatch `release-crates.yml` on the tag (`--ref vX.Y.Z`).
   Add required reviewers to it if an upload should wait for a second
   maintainer. The environment name is part of the trusted-publisher
   configuration, so a token cannot be minted outside it.
5. **Add the missing package metadata.** Every crate carries a description and
   the Apache-2.0 licence, but none declares `repository`, `homepage`,
   `documentation` or `keywords`; `cargo publish --dry-run` warns about it and
   the verify job prints a warning per missing field. Add them in
   `[workspace.package]` and inherit them per crate, with at most five
   keywords.
6. **Set the release branch variable** if releases are not tagged on the
   repository's default branch: a repository variable `RELEASE_BRANCH`. Without
   it the guard requires the tag to be on the default branch, which is the
   integration branch here.
7. **Bump the workspace version in a PR, merge it, then tag.** The guard
   compares the tag against `[workspace.package] version` and refuses a tag that
   does not match, so a mis-tagged release cannot reach the registry.

Secrets, environments and variables this path uses, in full:

| Name | Kind | Needed by | Value |
|---|---|---|---|
| `crates-io` | environment (required reviewers, protected tags `v*`) | the publish job | none; it is a gate, not a secret |
| `RELEASE_BRANCH` | repository variable (optional) | the guard, verify and publish jobs | the branch releases are tagged on; defaults to the default branch |
| `RUNNER_MAIN` | repository variable (optional, pre-existing) | verify and publish | a runner label for the long build |

`release-crates.yml` itself stores no secret: the only credential is the OIDC
token GitHub mints for it, which crates.io revokes when the job ends. The SDK
workflow's secrets are in the table at the top of this page.

## How a release runs

1. Merge the version bump. `dev` is the integration branch.
2. Tag `vX.Y.Z` and publish a GitHub Release for it. That fires the workflow's
   `release` trigger, which runs the guard and `verify`: the tag is a real
   release, it is on the release branch at the workspace version, the crate list
   matches the manifests, and every crate packages and compiles the way crates.io
   will compile it. **Nothing is uploaded by this step.**
3. Dispatch `Release crates` with `tag: vX.Y.Z` and `publish: true`. The
   same guard and verify run again, the `crates-io` environment asks for a
   reviewer's approval, and the crates upload in dependency order.
4. A failure part way through is safe to repeat: cargo refuses a version that is
   already published, and the next dispatch continues with the crates that are
   not.

## Why a fork cannot reach the registry

- The only triggers are a published GitHub Release and a manual dispatch.
  Neither can be produced by a pull request, and nothing here uses
  `pull_request`, `pull_request_target` or `workflow_run`.
- A manual dispatch needs write access to the repository, which a fork does not
  have, and its `publish` input defaults to `false` anyway.
- The code that runs is checked out from the release branch, never from the tag
  and never from a pull request head, and the checkout keeps no credentials in
  `.git/config`.
- Every step of the guard reads the tag, the branch head and the release, so a
  tag pushed to a branch that is not the release branch is refused.
- The upload token is not a stored secret: it is minted from this workflow's
  OIDC identity, and crates.io only honours it for this repository, this
  workflow and this environment.

## Related

- [`docs/marketplace/publishing.md`](../marketplace/publishing.md) is a
  different thing: listing a third-party project in the Loams Cloud
  marketplace, not publishing Loams itself.
- D292 (cargo-dist for binaries and GitHub Releases), D294 (`cargo install
  loams`), D298 (the CLI track) and D400 (the `loams` package namespace).
- D1119 (no open core; supersedes D220 and D552). A publish path still may
  not add a private, unpublished package to anything it uploads.
