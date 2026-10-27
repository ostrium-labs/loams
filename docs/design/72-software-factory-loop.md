# 72 — The Software Factory Loop: Mastra Agents, Studio Worktrees and Durable Runs

> **Amendment (approved 2026-10-23) by [§79](79-void-and-vite-plus.md)** (Void and Vite+; D1770–D1791 approved by the owner). Scaffolded JavaScript projects are Vite+ projects, and stage 5 waits on their `vp check`, `vp test` and `vp build` checks (D1777). Web apps are portable Void apps by default (D1778, Q1300). Stage 6 deploys Void apps on Cloudflare through `void deploy` and records them in `loams.deploy.v1` (D1779).

Status: **Approved** · 2026-10-21. The owner approved the design with every default on 2026-10-21 (and confirmed that "fx.sh" means Vercel Labs' fx, Q1142).

> "write the factory loop design and the agent marketplace will be based on our mastra integration, user can create agents publish to our marketplace etc, mastra uses loams postgres, live, engine, make mastra as our agent integration layer etc" (owner, 2026-10-21)

The same day the owner approved the coordinator's recommendations: retire SF2–SF5 in the docs (their issues #225–#228 were closed on 2026-10-19 "because the work has been redesigned; new issues will come from the new plans"), make the marketplace open source, put the "Software Factory" name to the owner with alternatives, and keep "Download Loams Desktop" as the primary call to action.

This document redesigns the factory's **agent loop**. Its companion is [§73](73-agent-marketplace.md), the Loams Agent Marketplace (any framework, Mastra first). Both build on:
- [§39](39-software-factory-and-loams-bot.md): the original factory and Loams Bot (D-SF-1–D-SF-20). Its app-embedding half (SF1) stands; its agent half (A2A agents, the Rust Loams Bot service, `factory.run` in Rust, SF2–SF5) is superseded here (§13).
- [§53](53-studio-ade.md): Studio and `loams-agentd` (D900–D922), shipped as experimental in 0.1.0.
- [§66](66-loams-mastra.md) and plan [MA1](../plans/2026-10-20-ma1-loams-mastra.md): Loams × Mastra (D1202–D1227), the Resonate engine and `@loams/mastra`.
- [§60](60-places-and-harness-composition.md) Places (D1087–D1093), [§61](61-loams-jailer.md) the Jailer (D1094–D1105), [§63](63-loams-meter.md) the Meter (D1120–D1138), [§52](52-loams-memory.md) Loams Memory (D870–D891).
- [§64](64-loams-infra-stack.md) §9.3, deploys (D1167); [§45](45-loams-live-production.md) Live deploys; §71 (app BYOC, open PR #979, D1390–D1410).

Decisions **D1420–D1443** and questions **Q1110–Q1121** are declared in the [decision log](13-decision-log.md). One number after D1443 is kept free. §73 holds D1445–D1479 and Q1122–Q1144. Plan stub: [FT1](../plans/2026-10-21-ft1-factory-loop.md), track FT. **No code is written by this document.** Approved by the owner with every default on 2026-10-21.

Markers: **(verify)** means the plan's Task 0 checks it first; **(estimate)** means computed, not measured. Status labels follow the messaging framework: **Shipped** (in v0.1.0), **Built, unreleased** (merged on `dev` or in an open PR, which is named), **Designed** (a design or plan, no code), **New** (introduced here).

---

## 1. Summary

### 1.1 The problem

Loams already has the parts a software factory needs, but no loop that joins them:
- **Studio** (shipped, experimental) runs Claude Code, Codex, opencode and other harnesses, each in its own worktree, with supervised approvals, checkpoints, run groups and diff review.
- **The factory apps** (shipped, experimental, desktop only) show eight self-hosted tools read-only. PR #844 (open) moves their connections into `loams-agentd`.
- **Durable execution** (shipped in the engine) and a **Resonate engine for Mastra** (built in `ostrium-labs/loams-mastra`, unpublished: 259 of Mastra's 290 workflow tests pass on Loams Durable).
- **Mastra on Loams Postgres** (built): Mastra's own `@mastra/pg` suite passes 3316/3316 against Loams Postgres.

§39's answer (five Rust A2A agents, a Rust Loams Bot service and a Rust `factory.run`) was never built past SF1 and was retired on 2026-10-19.

### 1.2 The answer in one paragraph

**A factory run is a Mastra workflow that turns an intent into a deployed change through six stages (intent, plan, build, review, CI, deploy), executed durably on Loams Durable by `@loams/mastra-resonate`.** The thinking is done by **Mastra agents**: Loams Bot, the supervisor the person talks to, and a small set of per-stage and per-app agents (planner, reviewer, and agents for Forgejo, Plane, Zulip and GlitchTip). These are Mastra agents by default; any agent from the marketplace (§73), in any framework or language, can take one of their roles. The **coding is done by harnesses in Studio worktrees**: the build stage opens Studio threads through `loams-agentd`, so every change is visible, reviewable, checkpointed and supervised exactly as it is when a person drives Studio. Agents reach the factory apps through **MCP** (the apps' own servers where an open one exists, and `loams-agentd`'s factory connections exposed as MCP tools) and Loams **connectors** for data. Memory is Mastra Memory on Loams Postgres now and **Loams Memory** later. Every run enters a **Place** that fixes what it may touch; every risky step waits on a person's **approval**; budgets and the kill switch are checked before every step and move onto the **Meter**; the **Jailer** becomes the policy decision point when it exists. Deploys go through D1167's deploy record to **Live apps** first and **app BYOC** targets (§71) later. Nothing merges or deploys without a person by default.

### 1.3 The loop

```
 intent ──► plan ──► build ──────────────► review ──► CI ──► deploy ──► (observe, FT2)
 Studio /    planner   Studio threads in     reviewer   repo's   loams deploy
 Loams Bot,  agent;    worktrees, one per    agent +    own CI   → Live app (FT0)
 an issue,   plan      task, any harness;    the        (Forgejo → app BYOC (FT2)
 an error    approval  supervised approvals  person;    / GitHub
             (person)                        merge      Actions); ≤3 fix
                                             approval   attempts
                                             (person)             deploy approval (person)
 ─────────────── one Mastra workflow on Resonate (Loams Durable), one Place, one budget ───────────────
```

### 1.4 What is built and what is new

| Piece | Status | Where |
|---|---|---|
| Studio threads, worktrees, run groups, checkpoints, diff review, supervised approvals | **Shipped** (experimental) | `crates/loams-agentd-*`, `web/plugins/studio`, SD1 |
| `loams-agentd-mcp`: `create_chat(s)`, `send_message`, `wait_for_turn`, `read_chat`, `interrupt_chat` and other thread tools for agents | **Shipped** | `crates/loams-agentd-mcp/src/tools.rs` |
| Loams Bot as an ACP harness in agentd (`loams-agentd loams bot-acp`, a stub A2A client) | **Shipped** (stub) | `crates/loams-agentd-harness/src/acp/loams_bot.rs`, `crates/loams-agentd-link/src/a2a/` |
| Factory app panels (eight apps, read-only, desktop) and Studio hand-off drafts from a Plane issue or GlitchTip error | **Shipped** (experimental) | `web/plugins/factory`, `plugins/packages/plugin-*-adapter` |
| Factory connections in agentd (`FactoryService`, keyring secrets, read-only) | **Built, unreleased** (open PR #844) | `crates/loams-agentd-factory` |
| Resonate execution engine for Mastra | **Built, unreleased** (merged in `loams-mastra`, npm publish pending; upstream declined, §6.4) | `loams-mastra/packages/resonate` |
| `@mastra/pg` on Loams Postgres; `@mastra/qdrant` on the engine | **Built** (conformance gate) | `loams-mastra` CI |
| Live app deploys with versions and rollback (`LiveAdminService.Deploy`, `ActivateDeployment`) | **Shipped** | `proto/loams/live/v1/admin.proto` |
| `@loams/mastra` (`LoamsVector`, `LoamsStore`, `LoamsPubSub`, `LoamsCache`, `LoamsExporter`) | **Designed** (MA2, gated on #884) | §66 §8 |
| Places, the Jailer, the Meter's gate, Loams Memory, `loams-mcp`, the deploy record | **Designed** | §60, §61, §63, §52, #847, D1167 |
| App BYOC | **Designed** (open PR #979) | §71 |
| The factory workflow, its agents, the ACP bridge for Mastra agents, the new agentd tools, the run record | **New** | this document |

---

## 2. Goals and non-goals

### 2.1 Goals

1. **One loop from intent to deploy**, each stage a durable step with a record, so a crash, a sleeping laptop or a long CI run loses nothing.
2. **Agents think, harnesses code, people decide.** Mastra agents plan, route and review; the person's chosen coding agent writes code in a worktree; a person approves the plan, the merge and the deploy.
3. **Mastra is the default agent layer** (owner, 2026-10-21). The factory's own agents are ordinary Mastra agents on `@loams/mastra`, so anything a user builds the same way can join a factory run or be published (§73). **Mastra is the default, not a requirement:** a run can use any marketplace agent, in any framework or language, that passes §73's package conformance (§4.5).
4. **Reuse what is shipped.** Studio's threads, approvals and review; agentd's MCP tools; Live deploys; Durable.
5. **Honest phases.** A minimum viable loop (FT0) that a single developer can run on Loams Desktop against one repository, then widen.

### 2.2 Non-goals

- **No A2A mesh inside Loams.** Delegation between Loams Bot and the other agents is Mastra's in-process supervisor delegation (§4.3). A2A stays available at the edge (D1425).
- **No coding inside Mastra.** A Mastra agent never edits a repository; it starts and steers a harness in a Studio worktree (D1423).
- **No Loams-hosted CI service.** The run waits on the repository's own CI (D1433).
- **No auto-merge and no auto-deploy by default** (Q472 stands).
- **No new chat product, no second agent framework, no Rust rewrite of Mastra's runtime.**
- **No multi-tenant hosted factory.** Ostrium hosts only the console and auth; the factory runs where the user's Loams runs (desktop, their server, their BYOC account).

---

## 3. The stages

| # | Stage | Done by | Gate (default) | Output recorded |
|---|---|---|---|---|
| 1 | **Intent.** A person types a goal to Loams Bot in Studio; or starts a run from a Plane (ItsAPlan) issue or a GlitchTip error (Studio's hand-off drafts, shipped, become run starts); later a scheduled prompt (D1093) or a signal (§3.7) | Loams Bot | None | `intent { source, text, links }`, the run's Place |
| 2 | **Plan.** The planner agent reads the repository (through a read-only Studio thread or the code graph later), the issue and memory, and writes a plan: goal, tasks (each sized for one thread), acceptance checks, files likely touched, risks, deploy target | `planner` | **Plan approval** (a person; edits allowed before approving) | `plan.md` on the run branch; tasks |
| 3 | **Build.** One Studio thread per task, each in its own worktree on `factory/<run>/<task>`, with the project's default harness and model (Q1113); the thread gets the task, the plan and the acceptance checks. Optional run group of N variants per task (FT1). The thread runs the tests it can locally | Studio threads (harnesses), started by the workflow | Studio's supervised approvals for every tool call the harness makes (D911, shipped) | Thread ids, commits, checkpoint ids |
| 4 | **Review.** The reviewer agent reads the combined diff, the plan and the tests, and posts review comments into Studio's diff review (D905); comments the person accepts go back to the thread as one turn. Then a pull request is opened on the repository's host (Forgejo or GitHub) | `reviewer`, then the person | **Merge approval** (a person) after CI | Review comments, PR URL |
| 5 | **CI.** The run waits for the PR's checks. On failure it feeds the failing job's log tail to the same thread and waits again, up to `max_attempts` (3). Exhausted attempts hand the PR to a person (`factory:needs-human`), as §39's `fail_safe` did | workflow + thread | None until green | Check results, attempts |
| 6 | **Deploy.** After the person merges (or approves the merge), `loams deploy` creates a deploy record (D1167) and deploys the merge commit to the target: a Live app (FT0) or an app BYOC target (§71, FT2). Health check, then activate; failure rolls back to the previous deployment | workflow | **Deploy approval** bound to the commit and target (a person); the first deploy of a new app always | Deploy id, version, health |
| 7 | **Observe and close** (FT2). §39 §10 stages 7–8 carried forward: watch the error tracker and analytics for a window; a regression opens a gated revert PR | `glitchtip`, `analytics` agents | Revert approval | Verdict |

Rules carried over from §39 unchanged: idempotent steps keyed by run id (a branch `factory/<run>`, a PR found by that branch before one is created); compensation for a run killed or abandoned before merge (close the PR, keep the branch for inspection, say so in the thread); no model call paid twice after a crash; untrusted text from apps (issue bodies, error messages, PR comments) passed as quoted data, never as instructions.

### 3.7 Signals (FT1)

A signal is an intent that arrives without a person: a GlitchTip alert webhook, an analytics anomaly, a failing evaluation. Signals start a run only in **triage mode**: the run stops after stage 2 with the plan waiting for approval, posted to Zulip or shown in Studio. §39's deduplication by fingerprint, cooldowns and generation cap (2) apply.

---

## 4. The agents

### 4.1 Mastra is the default agent layer (D1422)

Every factory agent is a Mastra `Agent` (from `@mastra/core`, pinned to the conformance-tested set, D1223) with:
- **storage** through `@mastra/pg`'s `PostgresStore` on Loams Postgres now, and `LoamsStore` after MA2;
- **memory** as in §6;
- **tools** from MCP (§5) and a few typed local tools;
- **workflows** on `@loams/mastra-resonate`;
- **traces** through Mastra's observability to `LoamsExporter` after MA2 (House), and Langfuse where the factory's Langfuse app is connected.

The models are the user's: Mastra's model router strings (`provider/model`) with keys held by agentd (Q1114). No model is bundled.

### 4.2 The roster

| Agent | Phase | Job | Tools (risk) |
|---|---|---|---|
| **`loams-bot`** | FT0 | The supervisor the person talks to. Understands the intent, starts and steers runs, answers status questions, explains approvals | `factory.start_run`, `factory.status`, `factory.kill` (write); the sub-agents below |
| **`planner`** | FT0 | Writes the plan and splits it into thread-sized tasks | read-only repository tools (a read-only Studio thread, `read_chat`), `memory.search` (read) |
| **`reviewer`** | FT0 | Reviews the diff against the plan and acceptance checks; drafts comments | `get_thread_diff`, `post_review_comment` (write), later code-memory review context (D883) |
| **`forgejo`** / **`github`** | FT0 | Opens the PR, reads checks and logs, comments | PR and checks tools (§5.2): `open_change_request` (write), `get_checks`, `get_check_log` (read); `merge` (destructive, approval) |
| **`plane`** | FT1 | Creates and updates issues, links the run | Plane's MCP server (§5.1) |
| **`zulip`** | FT1 | Posts plans, approvals-needed notices and summaries to a stream | Zulip tools (§5.1) |
| **`glitchtip`**, **`analytics`** | FT1/FT2 | Signals; observe and verdict | Sentry-compatible API tools, OpenPanel read tools |

§39's five platform agents survive as **roles**, not services: they are Mastra agents registered under `loams-bot`'s `agents` property and live in one process (§7).

### 4.3 Delegation: Mastra's supervisor, not A2A (D1425)

Loams Bot lists the other agents in its `agents` property, so Mastra exposes them to its model as delegation tools. Mastra documents that **a sub-agent's `requireApproval` tool or `suspend()` surfaces at the supervisor**, which is where Loams turns it into a Loams approval ([Mastra: supervisor agents](https://unpkg.com/@mastra/core@1.50.1/dist/docs/references/docs-agents-supervisor-agents.md), [agent approval](https://mastra.ai/docs/agents/agent-approval.md), read 2026-10-21; behaviour at our pinned 1.78.0 **(verify)**). A2A is not used between Loams' own agents. Mastra's server can serve any agent over A2A (`/a2a/:agentId` with a card at `/.well-known/:agentId/agent-card.json`, [Mastra A2A docs](https://mastra.ai/docs/agents/a2a), read 2026-10-21); Loams turns that on only behind an operator flag, as Q469 decided for Loams Bot.

### 4.4 Loams Bot in Studio: Mastra agents as ACP harnesses (D1426)

agentd already runs ACP agents as harnesses (`AcpAgentSpec`, which can name an npm package) and already registers `HarnessId::LoamsBot` as an ACP harness whose program is `loams-agentd loams bot-acp`, today a stub that forwards to an A2A endpoint. FT0:
- adds a small **ACP bridge for Mastra agents**, `@loams/mastra-acp` in `ostrium-labs/loams-mastra` on `@agentclientprotocol/sdk` ([ACP TypeScript library](https://agentclientprotocol.com/libraries/typescript.md), read 2026-10-21). It maps `session/new` to a Mastra thread, `session/prompt` to `agent.stream`, tool calls and text to ACP session updates, and a `tool-call-approval` chunk to ACP `session/request_permission`;
- re-points the `loams-bot` harness at the factory host started with `--acp`. The stub A2A client in `loams-agentd-link` is not deleted: it becomes §73's bridge for marketplace agents that speak only A2A.

So Loams Bot is a Studio thread like any other: the same sidebar, transcript, approvals queue and notifications, on the desktop and in the browser against a remote agentd. Because ACP's permission request becomes agentd's `ApprovalRequested`, **a Mastra tool approval and a harness tool approval reach the person through the same queue**. The same bridge is how marketplace agents appear in Studio (§73).

Mastra's own `@mastra/acp` goes the other way (a Mastra agent driving Claude Code or Codex over ACP as a tool, [Mastra ACP docs](https://mastra.ai/docs/connections/acp), read 2026-10-21). The factory does not use it for the build stage, because agentd owns worktrees, checkpoints, approvals and the transcript; it is noted for §71's serverless targets, where no agentd runs.

### 4.5 Any marketplace agent can take a role (D1443)

The roles in §4.2 are slots, not a closed list. A run's Place may name an **installed marketplace agent** (§73) for a role: a reviewer written in Pydantic AI, a Plane agent built on LangGraph, a security checker on Google ADK. Such an agent runs in its own process under agentd, as every §73 agent does, and Loams Bot reaches it through the surface its package declares:
- a Mastra agent on `@loams/mastra` (Loams-native) can be loaded in the factory host and registered under Loams Bot's `agents` property, like the built-in roles (FT1);
- any other agent is reached through agentd: over ACP as a Studio thread, which the workflow drives with the same thread tools it uses for the build stage, or over A2A through agentd's bridge (FT3). Its tool calls pass through the same gate and approvals queue, and it gets only the run's Place intersected with its own grants (D1088).

Only agents that pass §73's package conformance at the **Core** level can be given a role; a role that writes (merge, deploy) stays behind the person's approval whatever agent holds it.

---

## 5. Tools: MCP and connectors (D1424)

### 5.1 Three sources

1. **The app's own MCP server, run unmodified, where an open one exists.** Plane publishes an official server (`makeplane/plane-mcp-server`, stdio via `uvx` or remote; [Glama listing](https://glama.ai/mcp/servers/@makeplane/plane-mcp-server), read 2026-10-21); ItsAPlan compatibility **(verify)**. Forgejo has a community server (`forgejo-mcp`, issues, PRs, files, branches and Actions; its maintainer and licence **(verify)**). GitHub has an official server. GlitchTip speaks Sentry's API, so Sentry's MCP server may work against it **(verify)**.
2. **agentd's factory connections as MCP tools.** PR #844's `FactoryService` already holds each app's URL and keyring secret and runs scrubbed, read-only operations. FT0 exposes the operations the loop needs as tools in `loams-agentd-mcp`, so **no app credential ever enters the Mastra process**: the tool runs in agentd, which holds the secret.
3. **Loams connectors** (`loams.flow/v1`, the registry in `connectors/registry/`) for data rather than actions: ingesting app events into streams for signals (FT1) and for the run record's search.

The **gateway's `mcp` module** (§57, designed) becomes the server-side broker in FT2: it injects credentials per call, checks the Place, and is the Jailer's enforcement point for MCP.

### 5.2 New agentd tools for FT0

Added to `loams-agentd-mcp` and `loams.agentd.v1` (the tool runs the existing engine function; agentd stays the only thing that touches git and app credentials):

| Tool | Risk | Backed by |
|---|---|---|
| `create_chat` with `worktree: { branch }` (extends the shipped tool, which today takes a `cwd`) | write | sessions' worktree creation (shipped) |
| `get_thread_diff` (thread → unified diff against its base, with checksum) | read | diff RPCs (shipped) |
| `post_review_comment` (anchored as D905) | write | review RPCs (shipped) |
| `open_change_request` (Forgejo or GitHub PR from a thread's branch; idempotent by branch) | write | sessions' change requests (shipped for Forgejo) |
| `get_checks`, `get_check_log` (tail, scrubbed) | read | new: Forgejo and GitHub commit-status and Actions APIs through the factory connection |
| `request_approval` (a typed approval in Studio's queue, returns its id; the decision only from a person) | write | approvals (shipped) |
| `merge_change_request` | **destructive** | new; refused unless the call carries a settled approval id whose hash matches the call (§39 §8 item 2's rule, kept) |

---

## 6. Durable runs, memory and the run record

### 6.1 The workflow (D1427)

`factory.run` is written with Mastra's `createWorkflow` and `createStep`, obtained from `ResonateExecutionEngine`'s `init(resonate)` (§66 D1212), so each step is a durable `ctx.run` with a deterministic id and each wait is durable:
- **human gates** (plan, merge, deploy approvals) are workflow `suspend()` with a `resumeId`, settled when the person decides; the engine refuses a second resume (built);
- **waits on a thread** use `wait_for_turn` inside a step with a durable sleep between polls;
- **CI waits** poll with durable sleeps (60 s, backing off to 5 min) in FT0, because a desktop has no public webhook; a webhook-settled promise replaces polling on a server (FT1);
- **retries** are Mastra's `retryConfig`, which the engine leaves in charge (D1212).

Known engine gaps that shape FT0 (`loams-mastra` issues): foreach resume by index (#29) and `streamLegacy` suspend (#31). FT0 therefore runs **one task at a time through a plain step loop**, not `foreach`, and never uses `streamLegacy`. Parallel tasks (FT1) wait for #29.

### 6.2 Where it runs (D1428)

The **factory host** is one Node process running a Mastra app (`@loams/factory`, Node ≥ 22.13 as Mastra 1.x requires) that holds Loams Bot, the other agents and the workflow. It is:
- **FT0:** started by agentd on the desktop or dev box when the `loams-bot` harness is first used, as agentd starts other ACP harnesses, against the local Loams (`loams dev` or the light compose stack: Postgres, Durable). Single user. Node comes from where agentd's npm-based harnesses already get it (Q1122 records the desktop choice).
- **FT2:** also a `factory` service in the BYOC compose and Helm stacks, reached by remote agentd and the browser console, for a team.

The code lives in this monorepo under a new `agents/` pnpm workspace member (`agents/factory`), depending on the published `@loams/mastra*` packages at exact versions (Q1111). It ships with Loams Desktop and the server images.

### 6.3 Memory (D1429)

- **FT0:** Mastra `Memory` with `PostgresStore` and `PgVector` on Loams Postgres (the zero-package path, gate-tested): Loams Bot's threads, the planner's working memory per project, and semantic recall over past runs' plans and outcomes.
- **FT2:** Loams Memory through `@loams/mastra/memory` processors and tools (D1221) once MM1a ships: a **user** space for preferences ("always use pnpm", "I review on mornings"), a **project** space for conventions and lessons learned from earlier runs, and **code memory** (D882, D883) as the reviewer's deterministic review context.
- Memories are data, never instructions (D888): what an issue or a PR comment says can be remembered but never becomes a standing rule without a person's review.

### 6.4 The Resonate engine stays ours

D1211 planned to contribute the engine upstream as `@mastra/resonate`. The upstream issue (mastra-ai/mastra#27885) was closed with the maintainer comment "please maintain in your own repo". Under Q1013's default the package stays **`@loams/mastra-resonate`**, maintained in `loams-mastra`; the factory depends on it like any other user. Publishing 0.1.0 (MA1 Task 21) is an FT0 precondition.

### 6.5 The run record (D1436)

Carried from D-SF-15, now written by the workflow:
- **Live table `factory_runs`**: id, project, Place, intent, stage, state (`open`, `waiting`, `paused`, `succeeded`, `failed`, `killed`), plan link, thread ids, PR, checks, deploy id, approvals, budget `{ limit, spent }`, timestamps. The console, Studio and phones subscribe through Live.
- **Stream `factory_events`**: one CloudEvent per transition (`io.loams.dev.factory.*.v1`), an idempotent producer; later `LoamsPubSub` for Mastra's own run events (D1218).
- **Studio:** a run card in the Loams Bot thread (from ACP updates) and a **Runs** page in `web/plugins/factory` (Studio mode, D916) reading `factory_runs`.

---

## 7. Safety: Places, approvals, the Jailer, budgets and the kill switch

### 7.1 Every run enters a Place (D1430)

A run's Place (D1087) is computed **before the plan is shown**: the project and repository, the environments it may deploy to, the branch prefix `factory/<run>`, the tool allow-list (by agent), egress hosts, the approval policy and the budget. The effective capability (the agents' grants intersected with the Place's) is shown on the plan card; nothing in the run can exceed it. Until the Place record exists (§60 Q936: extend projects and environments first), FT0 computes it from the Studio project plus a versioned policy file `.loams/factory.toml` in the repository, and records it on the run.

### 7.2 Approvals (D1431)

- **Risk is declared per tool**: `read`, `write`, `egress`, `destructive` (D1098 adds `egress`). The policy maps (agent, tool, environment) to `allow`, `approve` or `deny`. Defaults: read allow; write allow outside protected environments; egress approve unless the host is in the Place; destructive approve, always.
- **Three gates are on by default and cannot be removed in FT0**: plan approval, merge approval, deploy approval.
- **Inside a build thread**, the harness's own tool calls use Studio's supervised mode (shipped, default for new threads).
- **Mastra tool calls** go through one wrapper, `gate(tool)`, which reads the policy and, for `approve`, raises Mastra's tool approval, which the ACP bridge turns into an agentd approval (§4.4). The server-side check stays where the credential is: agentd refuses a destructive tool without a settled approval whose hash matches the call.
- **No agent approves anything.** The requester and the agent acting for a person cannot approve (Q432).

### 7.3 The Jailer (FT2)

When JL1a lands, the gate stops being a policy lookup and becomes a call to the Jailer's deterministic policy decision point (D1094, D1095) in agentd, with its trifecta check on Place entry and one-way taint (D1097), its circuit breaker (D1099) and its one-operation rollback of everything a run left behind (D1103: discard `factory/<run>` branches and database branches). The overseer may only tighten.

### 7.4 Budgets and the kill switch (D1432)

- **Budgets** (D-SF-14, Q471's defaults kept): per run, tokens, money, wall time (4 h), fix attempts (3), threads (FT0: 1 at a time; FT1: up to 4); per project per day, runs (10) and money ($50); per run money $5. Checked by the workflow before every step. Spend is summed from Mastra's usage per call and agentd's per-session usage for harness threads (shipped; a harness on the user's own subscription reports tokens where its CLI does, and wall time otherwise).
- **Kill switch** by scope: a run (cancel the workflow, interrupt its threads, keep the branches), an agent (also disable that agent for all runs), or the project. One button in Studio and `/kill` in the Loams Bot thread; a step-up confirmation.
- **On the Meter** (FT1, after ME1d): `factory.runs` and `factory.model_spend` become Meter features, checked with `Gate::reserve` and `settle` (D1125) so the limit holds across processes; thresholds at 80% and 100% (D1129) notify. The workflow's own counter stays as a second check, as §39 kept the AI gateway's cap.

---

## 8. Deploy targets (D1434)

- **Through the deploy record.** Every deploy is a `loams.deploy.v1` record (D1167: app, environment, artifact digest, config and secret revisions, state) created by `loams deploy` as a durable workflow. **This amends D1167's wording**: the deploy stage it serves is this document's stage 6, not SF4's.
- **FT0 target, Live apps.** `LiveAdminService.Deploy` with the built bundle, a health check, then `ActivateDeployment`; rollback activates the previous deployment (shipped). A Live app is the natural first target because it needs nothing outside the user's Loams.
- **FT2 targets, app BYOC** (§71, when merged): the Workers module shape on Cloudflare and AWS first (AB0), with §71's free-tier guards. The deploy record names the target; whether a new app defaults to app BYOC or to the data plane is one of §71's open questions, answered there, not here.
- **Never in this design:** deploys to arbitrary Kubernetes clusters by the factory, or GitOps writes outside the repository's own environment branches (§39's Argo CD path stays available for teams that already run it, as a generic callback).

---

## 9. Phases and effort

Estimates are engineer-weeks for one experienced engineer **(estimate)**.

| Phase | Scope | Gates | Effort |
|---|---|---|---|
| **FT0: the minimum viable loop** | §10 | `@loams/mastra-resonate` 0.1.0 on npm (MA1 Task 21); PR #844 merged; Studio SD1 | **10** |
| **FT1: the team loop** | Plane, Zulip and GlitchTip agents through MCP; signals in triage mode; parallel tasks and run groups (after `loams-mastra` #29); webhooks for CI on a server; the Meter's gate for budgets; a `factory` Compose profile running the apps from `images.lock`; the browser console | ME1d for Meter budgets; #29 | 10 |
| **FT2: guarded and remembered** | The Jailer as PDP (JL1a, JL1c); the Place record (§60); Loams Memory and code memory in review; observe and close with gated reverts; app BYOC deploy targets (AB0); the `factory` service in BYOC compose and Helm; `LoamsExporter` traces | JL1a, MM1a, MA2, §71 merged | 12 |
| **FT3: reach** | Phones (run list, approvals, kill); an opt-in A2A edge for Loams Bot; non-Mastra marketplace agents in run roles through agentd (§4.5); factory templates as marketplace listings (§73 MK1) | §73 MK1 | 6 |

---

## 10. The minimum viable loop (FT0)

**Who and where:** one developer, Loams Desktop (or a dev box with remote agentd and the browser console), a local light stack, one repository on Forgejo or GitHub with its own CI, and one Live app (the repository builds a Live bundle) as the deploy target. A repository that is not a Live app runs the loop up to the merge in FT0 and gets its deploy target with app BYOC in FT2.

**What it does, end to end:**
1. The person opens a Loams Bot thread in Studio and types an intent ("add rate limiting to the signup endpoint"), or clicks "Start a run" on a Plane issue or GlitchTip error draft.
2. Loams Bot starts `factory.run`. The planner writes `plan.md` with one to five tasks and acceptance checks. The plan card shows the Place (repository, branch prefix, tools, deploy target, budget). The person edits or approves.
3. For each task in order, the workflow opens a Studio thread in a worktree on `factory/<run>/<n>` with the project's default harness, sends the task, and waits. The harness's tool calls ask for approval as usual.
4. The reviewer agent reads the combined diff and posts comments; the person accepts some, which go back to the thread as one turn, or skips.
5. The `forgejo` (or `github`) agent opens one PR from the run branch. The run polls checks; a failure goes back to the last thread with the log tail, up to three times.
6. Green checks raise the merge approval. The person merges in the code host or approves `merge_change_request`.
7. The deploy approval names the commit and the Live app. On approval, `loams deploy` builds and deploys the bundle, checks health and activates, or rolls back.
8. Loams Bot posts a summary with links (plan, threads, PR, checks, deploy) and the run's cost. The run record shows it all.

**What FT0 does not do:** signals without a person, parallel tasks, Plane/Zulip writes, the Meter, the Jailer, Loams Memory, app BYOC, teams, phones.

**Exit test** (FT1 plan Task 11): on GitHub-hosted runners, with Mastra's `createMockModel`, agentd's Mock harness (which edits files), a Forgejo container from `images.lock`, a fake check status set by the test, and a local Loams with Durable and Postgres: a run goes from intent to an active Live deployment; `kill -9` of the factory host at each stage resumes without a second model call or a second PR; a kill mid-build interrupts the thread and leaves the branch; a destructive tool without an approval is refused by agentd.

---

## 11. Naming (Q1110)

factory.ai (the Droids company, "Factory") published "Factory 2.0: From coding agents to software factories" on 2026-06-15 and now describes its product as "the software factory" ([factory.ai/news/software-factory](https://factory.ai/news/software-factory), read 2026-10-21); it raised a $150M Series C on 2026-04-16 ([factory.ai/news/series-c](https://factory.ai/news/series-c)). No trademark filing on "software factory" was found **(verify with counsel; Q479's check is still pending)**. Nothing is renamed by this document.

| Option | For | Against |
|---|---|---|
| **Keep "Loams Software Factory"** | Descriptive; an industry term long before factory.ai (Microsoft's "software factories" of 2004, the US DoD's software factories); already on loams.dev, the README and the desktop; always paired with "Loams" | Shares search results and vocabulary with a well-funded competitor whose whole pitch is the phrase; readers may assume we copy it |
| **Loams Shipyard** | Says build *and* ship, which is what the loop adds over Studio; nautical, distinct from "factory" | Shipyard (shipyard.build) sells ephemeral environments to developers (medium collision, **verify**); one more name to teach |
| **Loams Works** | Short, plain, fits "Loams Studio, Loams Works"; no major developer product collides **(verify)** | Generic and hard to search; says little on its own |
| **Loams Mill** | Short; a mill turns raw material into product, as the loop turns intent into deploys | Mill is a known Scala build tool (medium collision); the metaphor needs explaining |

Rejected: **Foundry** (Palantir Foundry; Azure AI Foundry, renamed Microsoft Foundry in 2026), **Forge** (Atlassian Forge, which also has a marketplace), **Loop** (Microsoft Loop).

**Owner's ruling (2026-10-21, Q1110): keep "Loams Software Factory".** The recommendation was: **keep "Loams Software Factory"** for 0.1.x, describe it by what it does ("intent to deploy, with your own agents, on ground you own") rather than by the phrase, and decide at the trademark check; if the owner prefers a rename, **Loams Shipyard** is the strongest alternative after a class 9/42 search.

---

## 12. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **The Resonate engine is young** (259/290 Mastra workflow tests; foreach resume and `streamLegacy` gaps) and maintained only by us | FT0 avoids the failing features; the factory's own tests run on Loams Durable in CI; the engine's gaps are FT1 gates |
| 2 | **Mastra moves fast** (1.x minor releases weekly) and its npm scope was compromised on 2026-06-17 (about 140 `@mastra/*` packages gained a malicious dependency; [The Hacker News](https://thehackernews.com/2026/06/144-mastra-npm-packages-compromised-via.html), [Snyk](https://snyk.io/blog/a-forgotten-contributor-account-compromised-the-entire-mastra-npm-package-scope/)) | Exact pins from `loams-mastra`'s tested set (D1223), install scripts blocked, a 3-day minimum release age, `npm audit signatures` (D1224) |
| 3 | **Two approval paths confuse people** (harness approvals and Mastra tool approvals) | One queue: both arrive as agentd approvals through ACP (§4.4) |
| 4 | **Planner output is poor**, so threads get bad tasks | The plan is always approved and editable; tasks carry acceptance checks; run groups (FT1) compare variants |
| 5 | **A runaway loop** (CI fix attempts, signals that trigger runs) | Attempts cap, budgets checked twice, signals only in triage mode, §39's cooldowns and generation cap |
| 6 | **Prompt injection** through issues, errors or PR comments | Quoted untrusted parts; no write chosen on them without the gate; destructive tools need an approval verified where the credential lives; the Jailer's taint (FT2) |
| 7 | **The name collides** with factory.ai | §11; Q1110 |
| 8 | **Node on the desktop** adds a runtime agentd must supervise | agentd already supervises npm-installed harnesses; Q1122 |

---

## 13. Supersessions

| Earlier | Now |
|---|---|
| SF2–SF5 (plans of 2026-10-02; issues #225–#228 closed 2026-10-19) | **Retired** (D1420). FT1 replaces SF4; SF2's agents become Mastra agents (§4); SF3's Loams Bot becomes a Mastra agent in Studio (§4.4); SF5's observability wiring moves to FT1–FT2 |
| SF1 | **Continues**: app panels, embedding, factory connections (#844) |
| D-SF-6, D-SF-7, D-SF-8, D-SF-12 (A2A between Loams Bot and five platform agent services) | **Superseded** by D1425: Mastra supervisor delegation in one process; A2A only at the edge, opt-in |
| D-SF-13 (`factory.run` as a Rust Resonate function) | **Superseded** by D1427: a Mastra workflow on `@loams/mastra-resonate` |
| D-SF-18 (coding as a Forgejo skill, `propose_patch`) | **Superseded** by D1423: coding in Studio threads |
| D-SF-19 (Loams Bot as a zeron `Harness` and a cordis chat plugin) | **Superseded** by D1426: Loams Bot as an ACP harness in agentd, rendered by Studio |
| D-SF-9, D-SF-10, D-SF-14, D-SF-15 | **Carried forward** with the amendments in §7 and §6.5 |
| D-SF-16, D-SF-20 (open-core split; private marketplace) | Already superseded by D1119; the marketplace is §73 (D1446) |
| D1167 ("the Software Factory's deploy stage", SF4) | **Wording amended**: the deploy stage is §72 stage 6 |

---

## 14. Decisions (approved by the owner, 2026-10-21)

| # | Decision | § |
|---|---|---|
| D1420 | **SF2–SF5 are retired** (the owner closed #225–#228 on 2026-10-19; recorded 2026-10-21). SF1 continues. §39's agent half is superseded by this document; its embedding half stands. **Approved (owner, 2026-10-21)** | 13 |
| D1421 | **The factory loop** is intent → plan → build → review → CI → deploy (observe and close in FT2), one durable run per intent, with plan, merge and deploy approvals on by default | 3 |
| D1422 | **Factory agents are Mastra agents by default** on `@loams/mastra` (Loams Postgres storage, engine vectors, Resonate workflows, House traces), the owner's direction that Mastra is Loams' agent integration layer. **Approved (owner, 2026-10-21)** | 4.1 |
| D1423 | **Agents think, harnesses code.** The build stage opens Studio threads in worktrees through agentd; no Mastra agent edits a repository | 3, 4 |
| D1424 | **Tools reach factory apps through MCP and connectors**: the apps' own open MCP servers unmodified, agentd's factory connections as MCP tools (credentials stay in agentd), connectors for data; the gateway's `mcp` module brokers in FT2 | 5 |
| D1425 | **No A2A inside Loams.** Loams Bot delegates through Mastra's supervisor `agents`; A2A only at the edge, behind an operator flag | 4.3 |
| D1426 | **Mastra agents appear in Studio as ACP harnesses** through `@loams/mastra-acp`; the `loams-bot` harness is re-pointed at the factory host; tool approvals become agentd approvals; the A2A stub client becomes §73's A2A bridge | 4.4 |
| D1427 | **`factory.run` is a Mastra workflow on `@loams/mastra-resonate`**: durable steps, approvals as suspend/resume with a `resumeId`, durable sleeps for polling; FT0 avoids `foreach` and `streamLegacy` | 6.1 |
| D1428 | **The factory host** is one Node Mastra app, `@loams/factory` in `agents/factory`, started by agentd in FT0 and a `factory` service on servers in FT2 | 6.2 |
| D1429 | **Memory**: Mastra Memory on Loams Postgres in FT0; Loams Memory processors and tools (user and project spaces, code memory for review) in FT2; memories are data, never instructions | 6.3 |
| D1430 | **Every run enters a Place**, computed before the plan is shown; FT0 derives it from the Studio project and `.loams/factory.toml` | 7.1 |
| D1431 | **Approvals**: risk per tool (`read`, `write`, `egress`, `destructive`); plan, merge and deploy approvals always on in FT0; Mastra tool approvals and harness approvals share agentd's queue; destructive tools verified where the credential lives | 7.2 |
| D1432 | **Budgets and the kill switch** keep D-SF-14 and Q471's defaults, enforced by the workflow in FT0 and by the Meter's gate from FT1 | 7.4 |
| D1433 | **CI is the repository's own** (Forgejo or GitHub Actions); the run polls in FT0 and takes webhooks on servers; at most 3 fix attempts, then a person | 3 |
| D1434 | **Deploys go through D1167's deploy record**: Live apps in FT0, app BYOC targets (§71) in FT2; D1167's "SF4" wording now means §72 stage 6 | 8 |
| D1435 | **Observe and close** (§39 §10 stages 7–8) return in FT2 with gated reverts | 3 |
| D1436 | **The run record** is the Live table `factory_runs` and the stream `factory_events` (D-SF-15's shape), shown in Studio's Runs page | 6.5 |
| D1437 | **Intents** come from Loams Bot in Studio and from Plane issues and GlitchTip errors in FT0; signals start runs only in triage mode (FT1) | 3 |
| D1438 | **The plan is a file**, `plan.md` on the run branch, approved and editable by the person before any thread starts | 3 |
| D1439 | **Review** is the reviewer agent's comments in Studio's diff review, then the person's review and merge | 3 |
| D1440 | **Phases FT0–FT3** (§9); plan [FT1](../plans/2026-10-21-ft1-factory-loop.md) specifies FT0 task by task | 9, 10 |
| D1441 | **Not built**: an internal A2A mesh, a Loams CI service, auto-merge or auto-deploy defaults, a second agent framework, a Rust Loams Bot service, a hosted multi-tenant factory | 2.2 |
| D1442 | **The name is unchanged** until the owner rules on Q1110 | 11 |
| D1443 | **The factory's agents are §73 listings** (dogfooding the marketplace from MK0), and **any installed marketplace agent, in any framework, can take a role in a run** (FT1 for Mastra agents in-process, FT3 for others through agentd) | 4.5, §73 |

The number after D1443 is kept free.

## 15. Open questions (answered by the owner on 2026-10-21: every default)

| ID | Question | Default |
|---|---|---|
| Q1110 | Keep "Loams Software Factory" or rename (Loams Shipyard, Loams Works, Loams Mill)? | **Keep** for 0.1.x; decide at the trademark check (Q479); Shipyard if renamed |
| Q1111 | Where the factory's Mastra code lives | The monorepo, `agents/factory`, on published `@loams/mastra*` packages; the ACP bridge in `loams-mastra` |
| Q1112 | The plan's home | `plan.md` on the run branch; mirrored to a Plane issue when ItsAPlan is connected (FT1) |
| Q1113 | Default harness for build threads | The Studio project's default harness and model; else Claude Code, Codex, opencode in that order of availability |
| Q1114 | Default model for Loams Bot, planner and reviewer | The user's configured provider through Mastra's model router with keys in agentd's vault; DeepSeek if configured (Q468); no bundled model |
| Q1115 | Parallelism | One task at a time in FT0; up to 4 threads or run-group variants per run in FT1 |
| Q1116 | Code hosts in FT0 | Forgejo and GitHub, polling only |
| Q1117 | Deploy targets in FT0 | Live apps only; app BYOC in FT2 after §71's AB0 |
| Q1118 | Keep the harness id `loams-bot`, and what becomes of `loams-agentd-link`'s A2A stub? | Keep the id; turn the stub into §73's A2A bridge (MK1) instead of deleting it |
| Q1119 | Run the factory host on serverless (Workers) | No: it needs agentd and worktrees; serverless is for marketplace agents (§73) |
| Q1120 | How the factory apps are run for teams | A `factory` Compose profile from `images.lock` in FT1; Helm through the operator later |
| Q1121 | Budget defaults | Keep Q471: $5 per run, $50 per project per day, raised by project owners |

Q1122 (Node on the desktop) is in §73's block because the agent host shares it.

## 16. Sources

- Mastra: [supervisor agents](https://unpkg.com/@mastra/core@1.50.1/dist/docs/references/docs-agents-supervisor-agents.md), [agent approval](https://mastra.ai/docs/agents/agent-approval.md), [A2A](https://mastra.ai/docs/agents/a2a), [ACP](https://mastra.ai/docs/connections/acp), all read 2026-10-21 (docs for 1.50–1.51; our pin is 1.78.0, so behaviour is **(verify)**).
- ACP: [TypeScript library](https://agentclientprotocol.com/libraries/typescript.md), read 2026-10-21.
- Plane MCP server: [Glama listing](https://glama.ai/mcp/servers/@makeplane/plane-mcp-server); Forgejo MCP: [LobeHub listing](https://lobehub.com/mcp/goern-forgejo-mcp), read 2026-10-21.
- factory.ai: [Factory 2.0](https://factory.ai/news/software-factory) (2026-06-15), [Series C](https://factory.ai/news/series-c) (2026-04-16), read 2026-10-21.
- Mastra npm compromise of 2026-06-17: [The Hacker News](https://thehackernews.com/2026/06/144-mastra-npm-packages-compromised-via.html), [Snyk](https://snyk.io/blog/a-forgotten-contributor-account-compromised-the-entire-mastra-npm-package-scope/), read 2026-10-21.
- This repository at `origin/dev` `b281c2dde` (2026-10-21); `ostrium-labs/loams-mastra` `main` `c1097d8` (2026-10-21): `COMPATIBILITY.md`, `packages/resonate`, issues #29–#32 and #36.
