# @loams/factory

The Software Factory host (The Greenhouse): one Node process running a Mastra app with
Loams Bot and, as plan FT1 builds them, the planner, reviewer and code-host agents and the
durable `factory.run` workflow. Design [§72](../../docs/design/72-software-factory-loop.md),
plan [FT1](../../docs/plans/2026-10-21-ft1-factory-loop.md).

Status: **experimental**, and not yet started by anything. FT0 Task 1 is the skeleton: a
Mastra instance on Loams Postgres and Loams Bot with no tools. agentd starts the host as the
`loams-bot` harness from Task 3.

- Storage: `PostgresStore` on `LOAMS_PG_URL` (Loams Postgres).
- Model: `LOAMS_FACTORY_MODEL` as `provider/model`, with the key in the host's environment
  from agentd's vault. No model is bundled.
- Workflows: on `@loams/mastra-resonate` and Loams Durable (`LOAMS_DURABLE_URL`) from
  Task 7.
- Telemetry: Mastra's PostHog telemetry is switched off (`MASTRA_TELEMETRY_DISABLED=1`).
- No app credential ever reaches this process: tools that need one run in agentd.

```sh
pnpm --filter @loams/factory test
```
