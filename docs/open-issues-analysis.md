# Open Issues Analysis: `ostrium-labs/loams`

> **Document Status**: Complete Reference
> **Repository**: `ostrium-labs/loams`
> **Branch**: `dev` (`f274ad91`)
> **Generated**: 2026-10-06
> **Total Open Issues**: 51
> **Closed in Recent Wave**: 7 issues (#284, #289, #290, #291, #293, #295, #374)

---

## 1. Executive Summary & Architecture Map

The `ostrium-labs/loams` repository currently has **51 open issues**. They are distributed across **7 strategic architectural tracks**, spanning core database systems, client SDKs, developer tooling, multitenancy, application layers, and DevOps.

```
                                 Open Issues (51)
                                        │
    ┌───────────┬───────────┬───────────┼───────────┬───────────┬───────────┐
    ▼           ▼           ▼           ▼           ▼           ▼           ▼
   SDK &      Core Data &   Apps, UI    Multi-      DevOps,     Software    Operations,
Unified API   Storage       & Desktop   tenancy     Git & CLI   Factory     Bugs & Owner
 (8 issues)   (10 issues)   (9 issues)  (5 issues)  (9 issues)  (4 issues)   (6 issues)
```

### High-Level Summary by Track

| Category / Track | Issues Count | Key Active Issues | Current Readiness & Status |
| :--- | :---: | :--- | :--- |
| **1. SDKs & Unified Connect API** | **8** | #281, #282, #283, #285, #286, #292, #294, #296 | **High momentum**: 8/13 languages complete and closed. Python & Go need final completion; Dart & ObjC pending; PHP deferred. |
| **2. Operations & Tooling** | **6** | #375, #314, #253, #237, #148, #118 | **Immediate win**: #375 (`check-decision-ids.sh`) is self-contained. #314 is blocked on owner secrets. |
| **3. Core Storage & Distributed Engines** | **10** | #198, #199, #200, #201, #202, #207, #208, #209, #210, #274 | **Substantial progress**: R1 (tasks 0–10 merged, 11–17 on branches), D1 (tasks 0–5 merged, 6–9 on branches). |
| **4. Developer Experience, Git & CLI** | **9** | #204, #205, #211, #212, #213, #214, #215, #271, #203 | Architecture plans written. Connector registry drafted (`cn1-t1-registry`). #271 (GT5) is deferred. |
| **5. Applications, Web, Desktop & Mobile** | **9** | #217, #218, #219, #220, #246, #264, #269, #259, #260 | Early prototypes on branches. Ready to consume newly landed client SDKs (Kotlin, Swift, TS). |
| **6. Multitenancy, BYOC & Infrastructure** | **5** | #221, #222, #223, #257, #275 | Full deployment specs for Authentik, Knative, Clever Cloud GitOps, and Headscale tailnet. |
| **7. Software Factory & AI Agents** | **4** | #225, #226, #227, #228 | A2A Protocol, unified Loams Bot chat, and durable engineering loops. |

---

## 2. Track 1: SDKs & Unified Connect API (8 issues)

### Context & Milestone Goals
The unified API transition replaces legacy REST routes and OpenAPI schemas with a single Connect/gRPC API defined under `loams.*.v1`. The project targets **13 official client libraries** tested against 28 wire-level conformance fixtures.

- **Completed & Closed (8 languages)**:
  - TypeScript / JavaScript (#284) — PR #329 + `3a020d26` (54 tests, 28/28 fixtures)
  - Rust (#287 & #374) — `sdks/rust/src/facade.rs` drift gate clean
  - Swift (#288) — SwiftPM `sdks/swift`
  - Kotlin (#289) — 85 unit tests + 28/28 fixtures
  - Java (#290) — PR #352
  - C# / .NET (#291) — 28/28 fixtures + CI workflow
  - Ruby (#293) — dynamic protobuf descriptors + 28/28 fixtures
  - C++ (#295) — R1–R9 runtime + CMake + 28/28 fixtures

### Open Issues in Track 1

#### Issue #285: `[SDK] SDK2 Task 1 — Python SDK (wave 1)`
- **Goal**: Ship `connect-python` client on PyPI (`loams`).
- **Current State**: Initial code was merged via PR #338, but was explicitly tagged *"SALVAGE SNAPSHOT — incomplete"*.
- **Remaining Work**: Implement the complete client runtime (R1–R9), error decoding, retries with idempotency tokens, stream resumption, and ensure all 28/28 required fixtures in `sdks/fixtures` pass.
- **Priority**: **High** (Wave 1 SDK).

#### Issue #286: `[SDK] SDK2 Task 2 — Go SDK (wave 1)`
- **Goal**: Ship `connect-go` client on Go proxy (`loams.dev/go`).
- **Current State**: Merged via PR #339 as *"SALVAGE SNAPSHOT — incomplete"*.
- **Remaining Work**: Finalize `sdks/go` runtime, handle typed error unmarshaling, implement retry middleware, and pass the 28/28 conformance fixtures.
- **Priority**: **High** (Wave 1 SDK).

#### Issue #292: `[SDK] SDK2 Task 8 — Dart / Flutter SDK (wave 3)`
- **Goal**: Ship Dart/Flutter client on `pub.dev` (`loams`).
- **Current State**: Starter branch `origin/sdk/dart` exists, but client runtime and fixtures are not yet implemented.
- **Remaining Work**: Implement stubs, Connect transport or gRPC fallback, and 28/28 fixture runner.
- **Priority**: Medium.

#### Issue #294: `[SDK] SDK2 Task 10 — PHP SDK (wave 3)`
- **Goal**: Ship PHP client on Packagist (`loams/loams`).
- **Current State**: Branch `origin/sdk/php` exists.
- **Status**: **Explicitly deferred** by owner (*"lets do php later"*).

#### Issue #296: `[SDK] SDK2 Task 12 — Objective-C SDK (wave 3)`
- **Goal**: Ship Objective-C client via CocoaPods / SwiftPM.
- **Current State**: Not yet started.
- **Priority**: Low / Wave 3.

#### Issue #281: `[SDK] API1 — The Unified Connect API: Consolidate Services, Remove REST and OpenAPI`
- **Goal**: Umbrella tracking issue for transitioning all server services to Connect/gRPC on a single port.
- **Status**: In progress. Tasks 0 and 1 completed; tasks 2–10 track route migration across modules.

#### Issue #282: `[SDK] SDK1 — The SDK Generation Pipeline`
- **Goal**: Buf generation pipeline, runtime contract, and conformance test runner.
- **Status**: In progress. Conformance fixtures and test runner scripts exist in `sdks/conformance` and `sdks/fixtures`.

#### Issue #283: `[SDK] SDK2 — Per-Language SDKs (index of the thirteen language issues)`
- **Goal**: Master tracking index for the 13 language SDKs.
- **Status**: 8 of 13 completed.

---

## 3. Track 2: Tooling, Upstream & Operations (6 issues)

#### Issue #375: `[tooling] check-decision-ids.sh does not resolve citations against the log, and passes an empty log`
- **Goal**: Fix script `scripts/docs/check-decision-ids.sh` to validate citations across the repository against `docs/design/13-decision-log.md`.
- **Identified Bugs**:
  1. The script exits 0 when given an empty decision log file.
  2. It does not verify that `D<number>` references in source code actually correspond to existing rows in the decision log.
  3. Identified dangling citations: `D000`, `D055`, `D730`, `D742`, `D788`.
- **Readiness**: **Ready to implement immediately** (self-contained shell/python scripting task).

#### Issue #314: `[OWNER] Configure Cloudflare console production secrets and API origin`
- **Goal**: Add Cloudflare production secrets (`LOAMS_API_ORIGIN`, API tokens) for `console.loams.dev`.
- **Status**: **Blocked on Owner**. Deployment code is already merged via PR #301 (`origin/console-cf-deploy`).

#### Issue #253: `[OPS] Code signing with SignPath (free for OSS) for desktop, CLI and mobile release artifacts`
- **Goal**: Integrate SignPath for signing Windows and macOS binaries without proprietary hardware tokens.
- **Status**: Plan documented. Awaiting release packaging pipelines.

#### Issue #237: `Tracking: implementation backlog (Codex implements, Opus reviews)`
- **Goal**: Master operational tracking issue that coordinates all plan tasks, rulesets, and code review criteria.

#### Issue #148: `loams-wal: TLS on the Postgres and HTTP listeners`
- **Goal**: Add TLS configuration to `loams-wal` (loams-safekeeper) so authentication tokens are not passed in cleartext.

#### Issue #118: `Track WeSQL upstream fixes (fork: ostrium-labs/wesql)`
- **Goal**: Upstream monitoring issue tracking sync status between `ostrium-labs/wesql` fork and upstream WeSQL.

---

## 4. Track 3: Core Storage, Metastore, Query & Routing (10 issues)

#### Issue #201: `[R] R1 — TiKV Metastore and the Reactive Core Implementation Plan`
- **Plan**: `docs/plans/2026-09-27-r1-reactive-core.md`
- **Goal**: Implement TiKV client layer (`loams-tikv`) and reactive metastore core.
- **Current State**:
  - Tasks 0–10 merged into `dev`.
  - Tasks 11–17 already implemented on feature branches: `origin/r1-t11`, `origin/r1-t12`, `origin/r1-t13`, `origin/r1-t14`, `origin/r1-t16`, `origin/r1-t17`.
- **Action**: Review, rebase onto `dev`, and merge the remaining branch stack.

#### Issue #202: `[D] D1 — Embedded Durable Execution, the Operations API and Bulk Import`
- **Plan**: `docs/plans/2026-09-27-d1-durable-execution.md`
- **Goal**: Embedded Resonate engine inside `loams-durable`, operations API, and bulk import workers.
- **Current State**:
  - Tasks 0–5 merged into `dev`.
  - Tasks 6–9 already implemented on feature branches: `origin/d1-t6`, `origin/d1-t7`, `origin/d1-t8`, `origin/d1-t9`.
- **Action**: Review, rebase onto `dev`, and merge tasks 6–9.

#### Issue #207: `[RT] RT1 — Postgres Single-Shard Slice and the Deterministic Simulator`
- **Plan**: `docs/plans/2026-10-01-rt1-postgres-slice-and-sim.md`
- **Goal**: Single-shard routing contract for sharded Postgres and deterministic simulator in `crates/loams-sim`.
- **Status**: Starter branch `origin/rt1-t0` exists; RT0 foundations landed in #311.

#### Issue #208: `[RT] RT2 — Scatter, Merge and Aggregate with the Lean Oracle, Postgres 2PC and the Change Stream`
- **Plan**: `docs/plans/2026-10-01-rt2-scatter-oracle-2pc.md`
- **Goal**: Lean formal verification kernels for k-way merge, Postgres 2-Phase Commit (2PC), and change data streaming.
- **Status**: Blocked on RT1.

#### Issue #209: `[FL] FL1 — Event Fabric Foundation (Iggy, Fluss, the Envelope and the Bridges)`
- **Plan**: `docs/plans/2026-10-01-fl1-fabric-foundation.md`
- **Goal**: Stand up the Event Fabric using Iggy / Fluss message broker engines with standardized CloudEvents envelopes.
- **Status**: Not started.

#### Issue #210: `[FL] FL2 — Loams House SQL Phase 1 (chDB, ClickHouse HTTP, Tier 1 Engines)`
- **Plan**: `docs/plans/2026-10-01-fl2-house-sql.md`
- **Goal**: Embed chDB ClickHouse SQL engine and expose ClickHouse HTTP interface.
- **Status**: Branches `origin/fl2-t0-chdb-spike` and `origin/fl2-t3-errors` exist.

#### Issue #274: `[FL] FL3 — External Iceberg REST Catalog (R2 Data Catalog)`
- **Goal**: Read/write external Apache Iceberg tables backed by Cloudflare R2 Data Catalog inside `loams-iceberg`.
- **Status**: Design complete (§42 §6).

#### Issue #200: `[PG] PG1 — Postgres Wire Access over Collections`
- **Plan**: `docs/plans/2026-09-28-pg1-postgres-wire.md`
- **Goal**: Provide native Postgres wire protocol compatibility over Loams collections using `datafusion-postgres`.
- **Status**: Spike branch `origin/spike-pgwire` merged. Implementation pending.

#### Issue #198: `[M1] M1.6 — SDKs and MCP Server Implementation Plan`
- **Goal**: Original M1.6 plan for Python/TS SDKs and stdio MCP server.
- **Status**: Partially superseded by SDK1/SDK2 and CLI1; the MCP server tools (`search`, `sql`, `memory_write`, `list_collections`, `get_documents`) remain to be unified.

#### Issue #199: `[M1] M1.7 — M1 Exit Gates Implementation Plan`
- **Goal**: End-to-end exit gate validation for Milestone 1 collections, storage, and search.
- **Status**: Awaits resolution of remaining M1 tasks.

---

## 5. Track 4: Applications, Console, Desktop & Mobile (9 issues)

#### Issue #217: `[AP] AP1a — The Console as a cordis Application`
- **Plan**: `docs/plans/2026-10-01-ap1a-cordis-console.md`
- **Goal**: Modern web console built as a modular cordis v4 micro-frontend host.
- **Status**: Branch `origin/ap1a-cordis-console` exists.

#### Issue #218: `[AP] AP1n — Native Loams Desktop on a Zeron Fork`
- **Plan**: `docs/plans/2026-10-02-ap1n-native-desktop-zeron.md`
- **Goal**: High-performance native Rust desktop application utilizing GPUI with Authentik OIDC integration.
- **Status**: Branches `origin/ap1n-native-desktop` and `origin/ap1n-fix` exist.

#### Issue #219: `[AP] AP2 — Loams for Android (Jetpack Compose, connect-kotlin)`
- **Plan**: `docs/plans/2026-10-01-ap2-android-compose.md`
- **Goal**: Android mobile operator app written in Jetpack Compose, connecting via `connect-kotlin` (now ready in `sdks/kotlin`).
- **Status**: Unblocked by Kotlin SDK completion.

#### Issue #220: `[AP] AP3 — Loams for iOS (SwiftUI, connect-swift)`
- **Plan**: `docs/plans/2026-10-01-ap3-ios-swiftui.md`
- **Goal**: iOS mobile operator app written in SwiftUI, connecting via `connect-swift` (ready in `sdks/swift`).
- **Status**: Unblocked by Swift SDK completion.

#### Issue #246: `[AP] AP1b: the Tauri web bridge (agent-driven website control, MCP toolbox)`
- **Plan**: `docs/plans/2026-10-02-ap1b-tauri-web-bridge.md`
- **Goal**: Desktop automation engine allowing AI agents to control web browsing via Tauri and MCP tools.
- **Status**: Branch `origin/ap1b-web-bridge` exists.

#### Issue #264: `[AP] macOS and iOS: unsigned CI builds, build-from-source docs, and signing with your own Apple ID`
- **Goal**: Setup reproducible unsigned builds and documentation for personal Apple ID developer signing.
- **Status**: Operations / documentation issue.

#### Issue #269: `[AP] Control-plane console on Vite+ 1.0, deployed as an SPA on Cloudflare Workers`
- **Goal**: Vite+ 1.0 SPA deployment on Cloudflare Workers (`console.loams.dev`).
- **Status**: Code merged in PR #301. Pending production credentials (#314).

#### Issue #259: `[SO] SO1 — Loams SystemOne: Engine, API and Backends`
- **Plan**: `docs/plans/2026-10-02-so1-systemone.md`
- **Goal**: Typed decision engine providing `choice`, `score`, and `noul` primitives.
- **Status**: Design §40 complete; engine implementation in `crates/loams-systemone` pending.

#### Issue #260: `[SO] SO2 — Loams SystemOne on the Desktop`
- **Plan**: `docs/plans/2026-10-02-so2-systemone-desktop.md`
- **Goal**: Embed the SystemOne decision engine into the native desktop app.
- **Status**: Blocked on SO1 and AP1n.

---

## 6. Track 5: Developer Platform, CLI, Git & Connectors (9 issues)

#### Issue #204: `[CLI] CLI1 — The Local CLI, Stacks and the Stdio MCP Server`
- **Plan**: `docs/plans/2026-10-01-cli1-local-cli-and-mcp.md`
- **Goal**: The developer CLI binary (`loams`) for managing local stacks, migrations, and exposing stdio MCP server.

#### Issue #205: `[CLI] CLI2 — Release Pipeline, Variants, Installer and Self-Update`
- **Plan**: `docs/plans/2026-10-01-cli2-release-and-install.md`
- **Goal**: Distribution infrastructure: cross-compilation matrix, standalone installer scripts, and self-update commands.

#### Issue #211: `[CN] CN1 — The Connector Registry and the ★ Connectors`
- **Plan**: `docs/plans/2026-10-01-cn1-starred-connectors.md`
- **Goal**: Connector capability registry and initial suite of 21 official data source connectors.
- **Status**: Branch `origin/cn1-t1-registry` exists.

#### Issue #212: `[RN] RN1 — The Runner Trait, InvocationObserver and the Process and Lambda Runners`
- **Plan**: `docs/plans/2026-10-01-rn1-runner-usage.md`
- **Goal**: Execution runner abstractions for sandboxed serverless processes and AWS Lambda functions.

#### Issue #213: `[GT] GT1 — The WAL Git Core and git-remote-loams`
- **Plan**: `docs/plans/2026-10-01-gt1-wal-git-core.md`
- **Goal**: Bucket-native Git storage engine using write-ahead logs and custom Git remote helper (`git-remote-loams`).

#### Issue #214: `[GT] GT2 — Smart HTTP for Stock Git, Compaction and Partial Clone`
- **Plan**: `docs/plans/2026-10-01-gt2-smart-http.md`
- **Goal**: Smart HTTP transport server, background packfile compaction, and blobless/treeless partial clone support.
- **Status**: Depends on GT1.

#### Issue #215: `[GT] GT3 — The sccache Backend and the Crates Mirror`
- **Plan**: `docs/plans/2026-10-01-gt3-build-cache-and-mirror.md`
- **Goal**: sccache WebDAV distributed caching service and local crates.io mirror.

#### Issue #271: `[GT] GT5 — The Artifacts Provider for Loams Git`
- **Goal**: Open artifact adapter (`loams-git-artifacts`) for storing large binary assets.
- **Status**: **Explicitly deferred** (`deferred` label).

#### Issue #203: `[SC] SC1 — Loams Commons: the Open-Source Showcase Suite`
- **Plan**: `docs/plans/2026-09-28-sc1-showcase-suite.md`
- **Goal**: End-to-end reference application showcases demonstrating multi-language SDK integrations.

---

## 7. Track 6: Multitenancy, BYOC & Infrastructure (5 issues)

#### Issue #221: `[MT] MT1 — Authentik as the Identity Provider`
- **Plan**: `docs/plans/2026-10-02-mt1-authentik-identity.md`
- **Goal**: Automated blueprint provisioning for self-hosted Authentik 2026.8 OIDC provider.

#### Issue #222: `[MT] MT2 — Knative Serving and Eventing for Self-Hosted Loams`
- **Plan**: `docs/plans/2026-10-02-mt2-knative.md`
- **Goal**: Knative operator and serving layer for auto-scaling Loams compute workloads.

#### Issue #223: `[MT] MT3 — GitOps with Clever Cloud's Open-Source Stack, Knative and Authentik`
- **Plan**: `docs/plans/2026-10-02-mt3-gitops-clever.md`
- **Goal**: Argo CD GitOps repository coordinating CloudNativePG, Knative, and Authentik.

#### Issue #257: `[MT] MT4 — The Open Multitenant BYOC Control Plane with GitOps`
- **Plan**: `docs/plans/2026-10-02-mt4-byoc-control-plane.md`
- **Goal**: `loams-control` operations API managing customer VPC worker instances.

#### Issue #275: `[MT] NET1 — Private Networking (pluggable tailnet: Tailscale BYOK or Headscale)`
- **Plan**: `docs/plans/2026-10-02-net1-private-networking.md`
- **Goal**: Pluggable default-deny wireguard mesh network using Headscale or customer-managed Tailscale keys.

---

## 8. Track 7: Software Factory & Autonomous Agents (4 issues)

#### Issue #225: `[SF] SF2 — The A2A Host and the Zulip, Plane and Forgejo Agents`
- **Plan**: `docs/plans/2026-10-02-sf2-a2a-agents.md`
- **Goal**: Agent-to-Agent (A2A 1.0) communication protocol host and adapters for Zulip, Plane, and Forgejo.

#### Issue #226: `[SF] SF3 — Loams Bot: One Chat in the Desktop and Mobile Apps`
- **Plan**: `docs/plans/2026-10-02-sf3-loams-bot-chat.md`
- **Goal**: Unified cross-platform chat UI embedded into Desktop, Android, iOS, and Web Console.

#### Issue #227: `[SF] SF4 — The Factory Loop: a Durable, Gated, Single-Organisation Workflow`
- **Plan**: `docs/plans/2026-10-02-sf4-factory-loop.md`
- **Goal**: Self-hosted autonomous software engineering workflow executing issue refinement, coding, testing, and review gates.

#### Issue #228: `[SF] SF5 — Observability UIs and Agents: GlitchTip, OpenPanel, Langfuse 4, OpenObserve`
- **Plan**: `docs/plans/2026-10-02-sf5-observability-ui.md`
- **Goal**: Embed error reporting (GlitchTip), product analytics (OpenPanel), and LLM tracing (Langfuse) into the console.

---

## 9. Recommended Implementation Priority Matrix

```
┌───────────────────────────────────────────────────────────────────────────────┐
│                               PHASE 1: QUICK WINS                             │
├───────────────────────────────────────────────────────────────────────────────┤
│ 1. [Tooling] Issue #375                                                       │
│    • Validate decision IDs in scripts/docs/check-decision-ids.sh.             │
│    • Clean dangling references (D000, D055, D730, D742, D788).                │
│                                                                               │
│ 2. [SDKs Wave 1] Issues #285 (Python) & #286 (Go)                             │
│    • Finalize Python and Go SDKs to match Kotlin, C#, and Ruby.               │
│    • Run and pass all 28 conformance fixtures.                                │
└───────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│                          PHASE 2: MERGE BRANCH STACKS                         │
├───────────────────────────────────────────────────────────────────────────────┤
│ 1. [Storage] Issue #201: R1 TiKV Metastore                                    │
│    • Rebase & merge origin/r1-t11 through origin/r1-t17 into dev.             │
│                                                                               │
│ 2. [Durable] Issue #202: D1 Durable Execution                                 │
│    • Rebase & merge origin/d1-t6 through origin/d1-t9 into dev.               │
└───────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│                        PHASE 3: REMAINING SDKS & APPS                         │
├───────────────────────────────────────────────────────────────────────────────┤
│ 1. [SDKs Wave 3] Issue #292 (Dart / Flutter) & Issue #296 (Objective-C)       │
│    • Note: Issue #294 (PHP) remains deferred per user instruction.            │
│                                                                               │
│ 2. [Applications] AP2 (Android) & AP3 (iOS)                                  │
│    • Consume the validated Kotlin and Swift SDK packages.                     │
└───────────────────────────────────────────────────────────────────────────────┘
```
