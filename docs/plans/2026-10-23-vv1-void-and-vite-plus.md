# VV1 — Void and Vite+ — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end.
>
> **Status: Approved stub** (owner, 2026-10-23, all defaults). Track **VV**, design [§79](../design/79-void-and-vite-plus.md).
> - **Approved:** D1770–D1795 and Q1300–Q1313. That includes:
>   - no Deno support (D1790);
>   - Bun compiled ahead of time for the self-hosted and container targets (D1791);
>   - Bun-compiled Loams CLIs, with the Factory host first (D1792, Q1313);
>   - the Factory's app images on the compiled-Bun image (D1793);
>   - the LGPL-2 obligations of distributed binaries, with counsel review before the first one ships (D1794, Q1312);
>   - the Bun conformance lane (D1795);
>   - Node's managed runtime on Lambda (Q1311).
>
> **Coverage:** this plan specifies **VV0**, the spikes, task by task. VV0 writes no product code. VV1–VV3 are outlined at milestone level, and each gets full task specifications after the VV0 report.

**Goal (VV0):** before any migration, find out where Void and Vite+ hold for Loams:
- which `@loams/app` and Void seams work on Cloudflare and on Node;
- how the shims call Void's handlers;
- whether the monorepo can take one `vite-plus` pin;
- whether the Loams Cloud control plane is better on Void than on `wrangler`;
- what Bun's ahead-of-time compile gains over Node for a Void app.

**Exit (VV0):** `docs/plans/vv0-report.md`, with:
- a go or no-go for each task;
- the measured numbers (build times against today's, bundle sizes, cold starts on Lambda);
- the answers to §79's (verify) items;
- any change that Task 8's numbers suggest to Q1311's answer (Node on Lambda), or Task 7's to Q1304's.

## Global Constraints

- **Worktrees and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/vv0-<task>`. Each spike's throwaway code lives under `spikes/vv0/<task>/`, which is not a workspace member. Its PR into `dev` carries only the report section.
- **Sign off every commit:** `git commit -s`. Each PR carries the label `review:greptile`.
- **No accounts and no deploys** without the owner.
  - Tasks 2, 3 and 7 deploy only to the owner's Cloudflare Free test account and AWS credit account (§70 §12.2), with scoped tokens the owner provides.
  - Every resource is deleted at the task's end, and the deletion is logged in the report.
- **Exact pins:** `vite-plus` 1.3.0, `void` 1.2.1 and Bun 1.4.4, or the newest release at least 14 days old when the task starts.
- **Node 24.21.0 or later** for Void tasks.
- **Never** use `curl | bash` installers in CI. Never use Void Cloud.

## VV0 tasks

| Task | Question | How | Pass |
|---|---|---|---|
| 1 | Does the console build, test and lint under Vite+ with no behaviour change? | Branch `web/apps/console` and `@loams/ui` onto `vite-plus` 1.3.0: `vp build`, `vp test`, `vp check`. Diff `dist/`, time each step against Vite 8.3.1, Vitest 5.0.3 and Biome. Draft the Biome-to-Oxlint parity table | The same test results, `dist/` equivalent (hashes may differ), and a parity table with every gap named |
| 2 | Do `@loams/app` ports on Cloudflare work through Void's bindings? | A portable Void app with an `items` adapter on D1, `blobs` on R2 and `queue` on Queues. A generated `void.config.ts` overlay sets `inference.bindings` and the `cloudflare` binding arrays. Then `void deploy --platform cloudflare`, a second deploy, and `void project rollback`. Check whether a Queues dead-letter queue can be bound explicitly | Void provisions exactly the declared resources, records them in `void.lock.json`, and rolls back. The DLQ answer is recorded |
| 3 | Can Loams' shims wrap Void's `node` build? | `target: "node"`. Wrap `dist/ssr/app.js` for Lambda behind a Function URL. Call a `crons/` handler from the shim's `scheduled` entry through EventBridge Scheduler. Try calling a `queues/` handler from SQS | HTTP, cron and either a queue handler or a documented no-go, with cold-start numbers |
| 4 | Does the Bun-compiled build (D1791), and the `node` fallback, run as an OCI image in the light compose stack? | `loams app build --target bun --compile --image`, and `--target node --image` (scripts in the spike), in the `apps` profile, with ports through `env.native` on the local Postgres and the bucket | The app serves and persists across a restart |
| 5 | Can Hyperdrive reach a private Loams Postgres over Cloudflare Tunnel, and TiDB Starter as MySQL? | A Void `database: "pg"` app on Hyperdrive to Loams Postgres behind the owner's Tunnel. A `database: "mysql"` app on TiDB Starter's free instance. Also note what Workers' `connect()` means for §78 §17's HTTP-driver rationale | Both connect, with read-after-write within one request and TLS mode recorded |
| 6 | Can the whole workspace take one `vite-plus` pin with `vite` aliased? | The catalog entry and override, `pnpm install`, `nx run-many -t build test typecheck`. Check electron-vite 6 (PR #1049) against `vite-plus-core` | No peer warnings left unexplained, and every target green. The desktop's answer is recorded |
| 7 | Is the Loams Cloud control plane better on Void? | Port LC2a's skeleton (one route, the per-user Durable Object, a cron, a secret, staged deploy) to Void. Compare it with `wrangler` and `@cloudflare/vitest-pool-workers` under `vp test` | A recommendation for Q1304 with the trade-offs. LC2a is not delayed |
| 8 | How much does Bun's ahead-of-time compile gain? | Build one portable Void app (the notes example's skeleton) three ways: Void `target: "node"` on `node:24-slim`; `target: "bun"` run with `bun run`; and `target: "bun"` compiled with `bun build --compile --bytecode --minify --sourcemap --format=esm --target=bun-linux-x64` on a distroless base. Then try `--bytecode-order`, both `--target=bun-linux-arm64` and `-musl`, and `dist/client/` embedded with `--asset` against shipped beside the binary. Measure cold start to the first 200 (median of 20, on the same machine), RSS at idle, and image size. Note what fails, such as native addons or files read beside the code | The numbers recorded against the targets of §79 §7.6 (cold start at least 1.5x faster, image under 120 MB). The fallbacks named, and the LGPL notice drafted for Q1312 |

## VV1: the toolchain (milestone)

- **Pins:** the `vite-plus` catalog pin and the `vite` alias; Node 24 in CI and `.nvmrc` (Q1303).
- **Deno (D1790):** remove any remaining Deno claims from READMEs and docs as they are found. Add no Deno test cells.
- **Targets:** `vp` in `web/`, `sdks/typescript/` and `plugins/`; `vp test` and `vp check` in the desktop.
- **Lint and format:** Biome and ESLint to Oxlint and Oxfmt (Q1301), with the formatting commits in `.git-blame-ignore-revs`.
- **Libraries:** tsup to `vp pack` in `@loams/ui`, and `vp pack` for SDK packages where the output is byte-compatible.
- **The fallback lane** (Q1309).
- **Docs:** `docs/monorepo.md` and `CONTRIBUTING.md` updated.
- **loams-site** on Vite+, in its own PR.

## VV2: apps on Void (milestone, inside AB0 and AB1)

- **The templates:** the portable and Cloudflare-native Void templates, and `web/examples/app-byoc-notes` as a portable Void app.
- **The deploy:** the `@loams/app` overlay; `loams app deploy --target cloudflare` through `void deploy` (D1779).
- **Bun-compiled images** for the self-hosted `apps` profile and Cloud Run (D1791), with the `bun run` and `node` fallbacks; the Bun conformance lane (D1795).
- **The `node` shims** for AWS (AB0), then Google Cloud, Azure and Vercel (AB1).
- **The Void conformance lanes** (D1787).

## VV3: options (milestone)

- **Bun-compiled Loams CLIs** (D1792), starting with the Factory host (Q1313). No Bun binary ships before counsel has reviewed the LGPL-2 obligations (D1794, Q1312). This is an owner action before the first release.
- **The `void-platform` target** (Q1306).
- **The control plane on Void** (Q1304).
- **loams-site on `void deploy`** (Q1305).
- **Void's Cloudflare build on Loams Functions** (Q1308).

## Rulings made during execution

- **VV-R1 (2026-10-24): VV1 starts before VV0's report is complete.** VV1 is the toolchain milestone, and it depends only on Tasks 1 and 6, which have both reported a go ([vv0-report.md](vv0-report.md)). Tasks 2, 3, 5 and 7 cover VV2 and VV3. They wait on the owner's test accounts and tokens, so they don't hold VV1 back. VV1 lands one PR per item:
  1. Node 24 (this ruling's PR);
  2. the `vite-plus` pin;
  3. Oxlint and Oxfmt;
  4. `vp pack`;
  5. the fallback lane;
  6. the docs.
- **VV-R2 (2026-10-24): Node 24 comes from `.nvmrc`.** The file holds `24`, and every `actions/setup-node` step reads it through `node-version-file`. The one exception is `sdk-release.yml`'s `publish-npm`, which has no checkout, so it names `24` directly. The root's and `web/`'s `engines` become `>=24.11`.
  - `@loams/client` and `agents/factory` keep their runtime `engines` (`>=22` and `>=22.13`).
  - The `SDK: TypeScript` job runs the SDK's suite once more on Node 22, as its last step (D1775).
- **Status (2026-10-24, when this work stopped):**
  - **VV0.** Tasks 1, 6 and 8 have reported (#1162, #1170, and Task 8's PR). Tasks 2, 3 and 7 have a separate agent working on their local parts. Their deploy parts, and all of Task 5, wait on the owner's Cloudflare and AWS test accounts and scoped tokens. Task 4 is not reported yet.
  - **VV1, done.** Item 1, Node 24, is this PR.
  - **VV1, left: the pin.** Item 2 lands in two steps:
    1. now: `vite-plus` 1.1.0, the newest release at least 14 days old, through the catalog for every package except the desktop, with `peerDependencyRules.allowedVersions.vite`;
    2. from 2026-10-28: the global `vite` alias, and the desktop on electron-vite 6.0.0, folding in #1049 (Bharani, with a `Co-authored-by` credit) and Dependabot's #1069 (vite 8.3.4). `vite-plus` 1.3.0 is eligible from 2026-10-30.

    Scripts move from `vite` to `vp` in the same PR (Task 1).
  - **VV1, left: the other items.**
    - Item 3: Oxlint and Oxfmt, using Task 1's parity table, `tabWidth: 2`, and the 107 `biome-ignore` rewrites.
    - Item 4: `vp pack` with `fixedExtension: false`.
    - Item 5: the fallback lane.
    - Item 6: the docs, and Deno claims as they are found. A sweep on 2026-10-24 found none in READMEs or guides.
    - loams-site, in its own PR.
  - **Dependabot.** #1070 (msw 3), #1071 (js-yaml 5, red) and #1072 (zod 4) are not toolchain changes, so they are left for their own review.

### Owner approvals (2026-10-24)

The owner approved the pending decisions below on 2026-10-24 ("approve all with your recommendations").

- **Pin `vite-plus` 1.3.0 directly once it is 14 days old (2026-10-30)**, not 1.1.0 now. The desktop and electron-vite 6 step stays on 2026-10-28. **Approved (owner, 2026-10-24).**

### Execution, 2026-10-27 (lane VV1, owner: "merge dependabot PRs to migrate to vite+ unified toolchain")

- **VV-R3: the catalog pin lands at 1.0.0 now, 1.3.0 on 2026-10-30.** On 2026-10-27 the newest `vite-plus` at least 14 days old is 1.1.0 (1.2.0 is from 2026-10-14, 1.3.0 from 2026-10-16, 1.4.0 from 2026-10-23). The owner's 2026-10-24 approval skips 1.1.0, so the workspace moves onto one catalog pin at the version `plugins/` already runs (1.0.0, which bundles Vite 8.3.1, the console's previous Vite), and a separate PR moves the pin to 1.3.0, to merge on or after 2026-10-30.
- **VV-R4: `vite: catalog:` in every package that runs tests, not a global override, until the desktop moves.** Vitest's `vite` peer is auto-installed as real Vite when a package names `vitest` without `vite`, and a scoped override doesn't reach peers. A global `vite` override would also hit the desktop's Vite 7.3.7 before electron-vite 6.0.0 is eligible (2026-10-28). The desktop PR adds the global override.
- **VV-R5: `vitest.config.ts` files become `vite.config.ts`.** `vp test` and plain Vitest both read `vite.config.ts`, so the fallback lane (D1774) and `vp` see the same config.
- **The PRs**, one per item: the pin (all packages except the desktop), Oxlint and Oxfmt, `vp pack` for `@loams/ui`, the desktop on electron-vite 6.0.0 (from 2026-10-28), `vite-plus` 1.3.0 (from 2026-10-30), and the fallback lane. Dependabot's #1069 and #1049 are superseded by the pin and desktop PRs; #1248's `vite-plus` 1.1.0 bump by the 1.3.0 PR.
