# Loams mobile governance

Loams has two maintainers with equal roles: [Dinakaran V (@dina-kar)](https://github.com/dina-kar) and [Keshav (@Kesh3805)](https://github.com/Kesh3805). They make project decisions by consensus and can add trusted people to the maintainer teams.

- Anyone can open a pull request against `dev`. A first merged PR makes the contributor eligible for the `committers` team; a team maintainer adds them.
- `committers` may review and merge PRs into `dev` after required checks and review. The team has write permission.
- `maintainers` set release direction and merge `dev` into `main`. The team has maintain permission. A PR to `main` requires one approval from this team.

`dev` is the default integration branch, and `main` is the release branch. Both require PRs, passing CI and DCO, and merge commits. Neither permits force pushes or deletion.

Start partner or ecosystem proposals in [Loams Discussions](https://github.com/ostrium-labs/loams/discussions), then write an RFC issue before implementation PRs to `dev`. Contribute broadly useful changes upstream first. Forks, embedded uses, hosted services and products built on Loams are welcome. “Built on Loams” is fine; do not name a separate product “Loams”.
