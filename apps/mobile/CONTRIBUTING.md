# Contributing to Loams mobile

Thanks for your interest. This repository holds the native phone apps; the server, protos and design live in the main repository, [`ostrium-labs/loams`](https://github.com/ostrium-labs/loams).

## Ground rules

1. **Protos come from the main repository.** Do not edit `proto/` here by hand; change the protos upstream and re-vendor them with `scripts/sync-protos.sh` (see [docs/protos.md](docs/protos.md)).
2. **Tests come with behavior.** Pure logic goes in the cores (`android/core`, `ios/Packages/LoamsCore`) with unit tests. Behaviour both apps share is pinned by the golden fixtures in `conformance/fixtures`, and both apps must pass them.
3. **Dependency licences.** Apache-2.0, MIT, BSD, ISC or compatible only. No GPL, LGPL, AGPL, SSPL, BSL or proprietary code. List new dependencies in `THIRD_PARTY_NOTICES.md`.
4. **Attribute derived code.** Code taken from another project keeps its copyright header, gets a line in [NOTICE](NOTICE), and is called out in the PR description.
5. **Security rules are not negotiable.** No cleartext outside the debug mock hosts, no user CAs, no backups of tokens or keys, no secret in logs, no decision without the user present. See [SECURITY.md](SECURITY.md).
6. **Small PRs.** One focused change per PR, with what changed and why.

## Pull requests

Fork and branch from `dev`, and open PRs against `dev`. `main` is the release branch. Committers and maintainers merge into `dev`; maintainers merge `dev` into `main`. See [GOVERNANCE.md](GOVERNANCE.md) for the contributor ladder. Use merge commits, not squash merges.

## Running and testing

See [docs/RUNNING.md](docs/RUNNING.md).

## Developer Certificate of Origin (DCO)

This project uses the [Developer Certificate of Origin](https://developercertificate.org/). There is no contributor license agreement. Sign off every commit:

```sh
git commit -s -m "android: add the pairing screen"
```

This adds a `Signed-off-by: Your Name <you@example.com>` trailer, certifying that you wrote the change or otherwise have the right to submit it under the project's licence. To sign off commits you already made, run `git rebase --signoff dev`.

## Code of Conduct

Everyone who takes part agrees to the [Code of Conduct](CODE_OF_CONDUCT.md).
