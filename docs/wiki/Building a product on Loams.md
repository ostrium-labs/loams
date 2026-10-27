# Building a product on Loams

Fork or embed, then keep the fork rebaseable: put product code in separate crates and packages, keep a short documented patch queue, and upstream generic changes.

**Upstream:** extension points, bug fixes, generic adapters, tests and docs. **Keep:** brand, UI, billing, prompts and hosted operations.

Extension points (plugins, providers, `Runner` adapters, deploy targets) and the worked examples are in [ECOSYSTEM.md](https://github.com/ostrium-labs/loams/blob/dev/ECOSYSTEM.md). Loams has no open core: everything is Apache-2.0 ([CONTRIBUTING.md](https://github.com/ostrium-labs/loams/blob/dev/CONTRIBUTING.md#everything-is-open-source)).
