# FT1 — The Software Factory Loop

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, tools or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved stub** (owner, 2026-10-21, all defaults). Track FT, design [§72](../design/72-software-factory-loop.md) (D1420–D1443, Q1110–Q1121), with [§73](../design/73-agent-marketplace.md) for the agent host and packages it shares. Replaces the retired SF2–SF5 (D1420). This plan specifies **FT0, the minimum viable loop**, task by task. FT1 (the team loop), FT2 (guarded and remembered) and FT3 (reach) are listed at milestone level and get full specifications when their gates are met.

**Goal (FT0):** one developer on Loams Desktop (or a dev box with remote agentd) types an intent to Loams Bot in Studio, approves a plan, watches a harness implement it in a Studio worktree, reviews it with the reviewer agent's comments, gets a pull request whose CI is green, merges, approves a deploy, and sees the change active on a Live app, with every step durable, recorded and stoppable.

**Architecture** (§72 §4–§8):
- **`agents/factory`** (new pnpm workspace member, `@loams/factory`, private, Apache-2.0): a Mastra app with the agents `loams-bot`, `planner`, `reviewer`, `forgejo`, `github`; the workflow `factory.run`; the `gate()` tool wrapper; the budget counter; a `bin/factory-acp.js` entry that serves `loams-bot` over ACP.
- **`ostrium-labs/loams-mastra`**: `packages/acp` (`@loams/mastra-acp`), the ACP bridge for Mastra agents (§72 §4.4), shared with §73's agent host.
- **`crates/loams-agentd-harness`**: the `loams-bot` ACP spec launches the factory host.
- **`crates/loams-agentd-mcp`** and **`proto/loams/agentd/v1`**: the new tools of §72 §5.2.
- **`crates/loams-agentd-factory`** (from PR #844): check status and check logs for Forgejo and GitHub.
- **`web/plugins/factory`**: the Runs page (Studio mode).
- **Live app `factory`** in the local stack: the `factory_runs` table; the stream `factory_events`.

**Tech stack:**
- Node ≥ 22.13; TypeScript and pnpm as the workspace pins them; `@mastra/core`, `@mastra/pg`, `@mastra/memory`, `@mastra/mcp` at the exact versions of `loams-mastra`'s `test/conformance/pins.json` (`@mastra/core` 1.78.0 as of 2026-10-21); `@loams/mastra-resonate` 0.1.0; `@agentclientprotocol/sdk` at a release at least 14 days old.
- Rust 1.97 for the agentd changes, workspace dependencies only.
- Docker Compose v2 for the e2e job; Forgejo's image from `deploy/factory/images.lock`.

**Spec:** [§72](../design/72-software-factory-loop.md) (all of it); [§53](../design/53-studio-ade.md) §5–§6; [§66](../design/66-loams-mastra.md) §7; [§39](../design/39-software-factory-and-loams-bot.md) §8, §10–§12 for the carried-forward rules.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/ft0-<task>`, one branch and one PR per task into `dev` (or into `loams-mastra`'s `main` for Task 2). CodeRabbit reviews; merge on green. `git commit -s`.
- **Light Rust builds.** `cargo build -p loams-agentd-mcp` (or the one crate a task changes); never `--workspace` locally; never build in `/tmp`.
- **No credential in the Mastra process.** App tokens stay in agentd's keyring; tools that need them run in agentd. Model keys come from agentd's vault as environment variables of the host process only.
- **Three approvals always on**: plan, merge, deploy. No setting turns them off in FT0.
- **No auto-merge, no auto-deploy, no push to a protected branch**, whatever the policy file says.
- **Exact pins** for every `@mastra/*` and `@loams/*` package; install scripts blocked (`pnpm` `allowBuilds` false by default); every third-party release at least 3 days old (D1224).
- **GitHub-hosted runners only.** No job needs a model key: tests use Mastra's `createMockModel` and agentd's Mock harness.

## Review Focus

1. **A destructive tool runs without a settled approval.** Expected: refused by agentd. Test: `merge_requires_settled_approval`.
2. **A crash repeats a model call, a PR or a deploy.** Expected: never. Tests: `resume_after_kill_at_each_stage`, `open_change_request_is_idempotent`, `deploy_step_is_idempotent`.
3. **An app credential reaches the factory host.** Expected: never. Test: `factory_host_env_has_no_app_tokens`.
4. **Untrusted text (issue body, CI log, PR comment) is treated as an instruction.** Expected: it is passed as a quoted, tagged part. Test: `untrusted_parts_are_quoted`.
5. **The kill switch leaves a thread running.** Expected: never. Test: `kill_interrupts_threads_and_cancels_workflow`.

## FT0 — The minimum viable loop (Tasks 0–11)

### Task 0 — Rulings and spikes

Check and record:
- `@loams/mastra-resonate` 0.1.0 is on npm with provenance (MA1 Task 21). If not, FT0 waits.
- PR #844 is merged (`loams-agentd-factory`, `FactoryService`). If not, Tasks 6 and 7 wait.
- Mastra at the pinned version: the supervisor `agents` property and approval propagation from a sub-agent; `requireApproval` and the `tool-call-approval` chunk; `agent.approveToolCall`; workflow `suspend()`/`resume()` through `ResonateExecutionEngine` (§72 §4.3, §6.1).
- `@agentclientprotocol/sdk`: the agent side of `session/new`, `session/prompt`, `session/update` and `session/request_permission`; how agentd's ACP harness maps `request_permission` to `ApprovalRequested` today.
- Node on the desktop and on a remote agentd (§73 Q1122).
- Live: `LiveAdminService.Deploy` and `ActivateDeployment` from a non-console client with the local owner credentials; how a Live bundle is built from a repository.
- Forgejo and GitHub: the commit-status and Actions APIs the check tools need; the token scopes.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — The `agents/` workspace and `@loams/factory` skeleton

Files:
- `pnpm-workspace.yaml`: add `agents/*`.
- `agents/factory/package.json` (`@loams/factory`, `private: true`, `type: module`, `engines.node >=22.13`), `tsconfig.json`, `src/mastra.ts` (a `Mastra` instance with `PostgresStore` on `LOAMS_PG_URL` and the Resonate engine on `LOAMS_DURABLE_URL`), `src/agents/loams-bot.ts` (an agent with no tools yet).
- `scripts/ci/npm-licences.mjs` covers `agents/` (D914).
- CI job `factory` in `.github/workflows/ci.yml`: install with frozen lockfile, typecheck, unit tests.

Tests: `mastra_instance_builds`, `loams_bot_answers_with_mock_model`.

Commit: `agents: the @loams/factory workspace and skeleton (FT0 Task 1)`.

### Task 2 — `@loams/mastra-acp` (in `ostrium-labs/loams-mastra`)

Files: `packages/acp/` with `serveAcp(agent, { stdin, stdout })`:
- `session/new` → a Mastra thread id (resource = the agentd project);
- `session/prompt` → `agent.stream` with that thread; text deltas → `agent_message_chunk`; tool calls → `tool_call` and `tool_call_update`;
- a `tool-call-approval` chunk → `session/request_permission`; the outcome → `agent.approveToolCall` or decline;
- `session/cancel` → abort the stream.

Tests (`test/acp/`): `prompt_streams_text`, `tool_call_reports_status`, `approval_maps_to_request_permission`, `decline_stops_tool`, `cancel_aborts_stream`, against the SDK's client side in-process.

Commit (in `loams-mastra`): `acp: serve a Mastra agent over ACP (FT0 Task 2)`. Publish `@loams/mastra-acp` 0.1.0 with trusted publishing.

### Task 3 — Loams Bot as the factory host's ACP agent

Files:
- `agents/factory/bin/factory-acp.js`: starts the Mastra app and `serveAcp(loamsBot)`.
- `crates/loams-agentd-harness/src/acp/loams_bot.rs`: the spec launches `node <factory>/bin/factory-acp.js` (path from agentd's data directory; `LOAMS_BOT_EXECUTABLE` still overrides) with `LOAMS_PG_URL`, `LOAMS_DURABLE_URL`, the model keys from agentd's vault, and nothing else from the environment.
- `crates/loams-agentd-link/src/a2a/`: left in place for §73's A2A bridge; `bot-acp` no longer used by the `loams-bot` spec.

Tests: `loams_bot_spec_launches_factory_host`, `factory_host_env_has_no_app_tokens`, `loams_bot_thread_round_trip` (Mock model, a Studio thread shows the reply).

Commit: `agentd: Loams Bot runs the factory host over ACP (FT0 Task 3, D1426)`.

### Task 4 — New agentd tools (§72 §5.2)

Files: `crates/loams-agentd-mcp/src/tools.rs` and the matching `loams.agentd.v1` RPCs where missing:
- `create_chat` gains `worktree: { branch: string, base?: string }`;
- `get_thread_diff`, `post_review_comment`, `open_change_request` (idempotent by branch; Forgejo and GitHub), `request_approval` (kind, summary, payload hash; returns an id), `get_approval`;
- `merge_change_request` (destructive): refused unless `approval_id` names a settled, approved approval whose payload hash equals the hash of `{repo, pr, head_sha}`.
Each tool's risk is in its description and in a `risk` annotation.

Tests: `create_chat_in_new_worktree`, `get_thread_diff_matches_checksum`, `open_change_request_is_idempotent`, `merge_requires_settled_approval`, `merge_refuses_hash_mismatch`, `request_approval_cannot_be_settled_by_agent`.

Commit: `agentd: worktree chats, diff, review, change-request, approval and merge tools (FT0 Task 4)`.

### Task 5 — Check status and logs

Files: `crates/loams-agentd-factory`: `checks(repo, sha)` and `check_log_tail(repo, run, lines)` for Forgejo (commit statuses, Actions) and GitHub (check runs, Actions logs), scrubbed as #844 scrubs errors; tools `get_checks` and `get_check_log` in `loams-agentd-mcp`.

Tests: `checks_forgejo_fixture`, `checks_github_fixture`, `check_log_is_scrubbed`, `check_log_tail_is_bounded`.

Commit: `agentd: CI check status and log tails for Forgejo and GitHub (FT0 Task 5, D1433)`.

### Task 6 — The planner, reviewer and code-host agents

Files: `agents/factory/src/agents/{planner,reviewer,forgejo,github}.ts`; `src/tools/gate.ts` (`gate(tool, { risk })` reads `.loams/factory.toml` and the Place, sets `requireApproval` for `approve`, refuses `deny`); `src/mcp.ts` (an `MCPClient` to `loams-agentd mcp`); `src/agents/loams-bot.ts` registers the four under `agents`.
- The planner writes `plan.md` with tasks and acceptance checks to the run branch through a read-only thread's output, never by editing files itself.
- Untrusted text is wrapped as `<untrusted source="…">…</untrusted>` parts.

Tests: `planner_produces_tasks_with_checks`, `reviewer_posts_anchored_comments`, `gate_requires_approval_for_destructive`, `gate_denies_unlisted_tool`, `untrusted_parts_are_quoted`.

Commit: `factory: planner, reviewer and code-host agents with the tool gate (FT0 Task 6, D1431)`.

### Task 7 — The `factory.run` workflow

Files: `agents/factory/src/workflows/factory-run.ts` with steps `intent`, `plan`, `approvePlan` (suspend), `build` (a plain loop over tasks: create worktree chat, send, `wait_for_turn` with durable sleeps), `review`, `openPr`, `ci` (poll with durable sleeps 60 s → 5 min; up to 3 fix attempts), `approveMerge` (suspend), `merge`, `approveDeploy` (suspend), `deploy` (Task 8), `close`; the budget counter checked before every step; compensation for kill and abandon. No `foreach`, no `streamLegacy`.

Tests (on Loams Durable from the local stack): `run_reaches_close_with_mock_everything`, `resume_after_kill_at_each_stage`, `second_resume_is_refused`, `ci_failure_feeds_log_back`, `attempts_exhausted_hands_to_person`, `budget_exceeded_pauses_run`.

Commit: `factory: the durable factory.run workflow (FT0 Task 7, D1427)`.

### Task 8 — The deploy step on Live

Files: `agents/factory/src/deploy/live.ts`: build the bundle, `LiveAdminService.Deploy`, health check, `ActivateDeployment`, rollback to the previous deployment on failure; the deploy record fields of D1167 kept on the run until `loams.deploy.v1` exists.

Tests: `deploy_activates_on_health`, `deploy_rolls_back_on_failed_health`, `deploy_step_is_idempotent`, `deploy_requires_settled_approval`.

Commit: `factory: deploy to a Live app with rollback (FT0 Task 8, D1434)`.

### Task 9 — The run record and the Runs page

Files: a Live app `factory` (schema `factory_runs`, mutations used by the workflow), the stream `factory_events` (CloudEvents `io.loams.dev.factory.run.*.v1`); `web/plugins/factory/src/runs/` (Studio mode): a list and a detail with stage, links, approvals and spend.

Tests: `run_record_updates_each_stage`, `events_are_idempotent`, a Playwright check in `studio-e2e` that a mocked run appears and updates.

Commit: `factory: the run record and Studio's Runs page (FT0 Task 9, D1436)`.

### Task 10 — The kill switch

Files: `factory.kill(scope)` (run, agent, project) as a Loams Bot tool and `/kill` in the thread with a confirmation; cancels the workflow, calls `interrupt_chat` on the run's threads, closes an open PR on run kill, keeps branches.

Tests: `kill_interrupts_threads_and_cancels_workflow`, `agent_kill_blocks_new_runs_using_it`, `kill_keeps_branches`.

Commit: `factory: the kill switch (FT0 Task 10, D1432)`.

### Task 11 — End-to-end gate and docs

Files: `.github/workflows/factory-e2e.yml` (GitHub-hosted, Compose: the local light stack with Durable and Postgres, Forgejo from `images.lock`, agentd with the Mock harness, the factory host with `createMockModel`; the test sets commit statuses itself); `docs/guides/studio/factory-loop.md` with the status label **experimental**; the plans README row updated.

Tests: `intent_to_live_deploy` (the §72 §10 exit test, end to end).

Commit: `factory: the FT0 end-to-end gate and guide (FT0 Task 11)`.

## FT1 — The team loop (milestone level)

Plane, Zulip and GlitchTip agents through MCP (Plane's official server; others per §72 §5.1); signals in triage mode with §39's dedupe, cooldown and generation cap; parallel tasks and run groups (after `loams-mastra` #29); CI webhooks on servers; budgets on the Meter's gate (after ME1d); Mastra marketplace agents in roles in-process; a `factory` Compose profile for the apps from `images.lock`; the browser console. Estimate: 10 engineer-weeks.

## FT2 — Guarded and remembered (milestone level)

The Jailer as the policy decision point (JL1a, JL1c) and its rollback; the Place record (§60); Loams Memory (MM1a) and code memory in review; observe and close with gated reverts; app BYOC deploy targets (§71 AB0); the `factory` service in BYOC Compose and Helm; `LoamsExporter` traces (MA2). Estimate: 12 engineer-weeks.

## FT3 — Reach (milestone level)

Phones (runs, approvals, kill); an opt-in A2A edge for Loams Bot; non-Mastra marketplace agents in run roles through agentd (§72 §4.5); factory templates as marketplace listings (§73 MK1). Estimate: 6 engineer-weeks.

## Rulings made during execution

### Task 0 (2026-10-23)

- **FT0-0-1, `@loams/mastra-resonate` is not on npm yet.** `loams-mastra` has versioned 0.1.0 (#37, #39) and its Release run 39897453449 is waiting on the owner's approval of the `npm` environment; `npm view @loams/mastra-resonate` is a 404. The gate said "FT0 waits". Ruling: only the tasks that need the engine wait. Task 1 builds the Mastra instance with an **optional execution engine** (`createFactoryMastra({ engine })`, Mastra's default engine when none is given) and no dependency on the unpublished package; Task 7 (`factory.run`) adds `@loams/mastra-resonate` 0.1.0 at its exact version and waits for the publish. Tasks 2, 4, 5 and 6 do not need the engine.
- **FT0-0-2, PR #844 is not merged.** It has been conflicting with `dev` since 2026-10-19. Ruling: Task 5 does not wait for it. `loams-agentd-sessions` already holds the code-host credentials the loop needs, in `change_request_create.rs`: the Forgejo token in the keyring (`factory:forgejo:token`, `factory:forgejo:url`, sent only to its own host) and `gh` for GitHub. The token that opened a pull request is the token that reads its checks, so Task 5 puts `checks` and `check_log_tail` in a new `loams-agentd-sessions/src/checks.rs` on those credentials, with the same redaction (`git_write::redact`). When #844 merges, its `FactoryService` can own the connection; the check readers stay where the change requests are. Tasks 6 and 7 do not need #844 either for FT0: the Plane and GlitchTip intents of D1437 that need `FactoryService` move to the first task after #844 merges. #844 is left open for its author.
- **FT0-0-3, Mastra 1.78.0** (read from the published package's type declarations, 2026-10-23): `Agent` takes `agents?: DynamicArgument<Record<string, SubAgent>>` (the supervisor property); a tool takes `requireApproval?: boolean | (input, ctx) => boolean`; the stream has a `tool-call-approval` chunk; `agent.approveToolCall({ runId, toolCallId })` and `agent.declineToolCall({ runId, toolCallId, reason })` resume the stream; `createMockModel({ mockText, version: 'v2' })` is exported from `@mastra/core/test-utils/llm-mock`. Approval propagation from a sub-agent to the supervisor is documented but is verified by Task 6's tests, not assumed. Workflow `suspend()`/`resume()` through `ResonateExecutionEngine` is covered by `loams-mastra`'s suspend tests (#18) and is re-checked in Task 7.
- **FT0-0-4, `@agentclientprotocol/sdk`**: the newest release at least 14 days old is **1.8.0** (2026-10-09); 1.9.0, 2.0.0 and 2.0.1 are newer. Task 2 pins 1.8.0. agentd's ACP harness already turns `session/request_permission` into an approval (`crates/loams-agentd-harness/src/acp/approvals.rs`, and in supervised mode in `acp/mod.rs`), so the bridge only has to send the request; nothing changes in agentd for that mapping.
- **FT0-0-5, Node (Q1122).** agentd's npm-based ACP adapters install once and then spawn `node <entry>` with the `node` found by `adapter_install.rs` (the one beside a usable npm). Task 3's factory host uses the same lookup, so Desktop and a remote agentd need no second Node rule.
- **FT0-0-6, Live.** `LiveAdminService.Deploy` (`proto/loams/live/v1/admin.proto`) activates the new deployment itself by compare-and-swap against `expected_active` and returns when it is active or failed; `ActivateDeployment` re-activates a stored one. So Task 8's order is: `Deploy` with `expected_active` = the current deployment, health check, and on a failed health check `ActivateDeployment` of the previous one. Whether the local owner credentials reach `live.admin` from a non-console client, and how a bundle is built from a repository, is checked at the start of Task 8.
- **FT0-0-7, Forgejo and GitHub checks.** Forgejo: `GET /api/v1/repos/{owner}/{repo}/commits/{sha}/status` (combined status) and the Actions run and job APIs for logs; the token needs `read:repository` (verified against a live Forgejo in Task 5). GitHub: `gh api repos/{owner}/{repo}/commits/{sha}/check-runs` and `gh run view --log-failed` / the jobs log endpoint, on the user's `gh` login (`repo` scope, or `actions:read` and `checks:read` for a fine-grained token). Both are read only.
- **FT0-0-8, the MCP surface of Task 4.** The RPCs the tools need exist (`CreateWorktree`, `GetCheckoutDiff`, `UpsertReviewComment`, `CreateChangeRequest`, `WatchApprovals`, `ResolveApproval`, `MergeBranch`) but have role `Owner`; the MCP run token reaches only role `Mcp` methods on its own session and its side chats. Task 4 adds `Mcp`-role methods scoped the same way (the target chat must be the caller's chat or one it created), rather than widening the existing `Owner` methods. `ResolveApproval` stays `Owner`: no agent settles an approval (Q432).

### Task 1 (2026-10-23)

- **FT0-1-1, the npm licence gate and Mastra: owner decision needed. Superseded 2026-10-24: the owner allowed BlueOak-1.0.0 (see "Owner approvals (2026-10-24)" below).** `@mastra/core` 1.78.0 (Apache-2.0) depends in production on `lru-cache` 11.5.3 and `@isaacs/ttlcache` 2.1.5, both **BlueOak-1.0.0**, which the owner kept off the npm allow list (SD1 ruling T2-2). Neither can be overridden away: `lru-cache` 10.4.3 is ISC, but `ttlcache` 1.x (ISC) is CommonJS with a default export, and `@mastra/core` imports `{ TTLCache }` from it as ESM. So `@loams/factory` is **not yet in `scripts/ci/npm-licences.mjs`'s default projects** (it would turn "Web and mobile required" red); it joins them in the PR that carries the owner's answer: allow BlueOak-1.0.0 (a permissive, OSI-approved licence), or record exact exceptions for these two packages. The same two packages are already in `ostrium-labs/loams-mastra`'s tree.
- **FT0-1-2, Mastra telemetry is off.** `@mastra/core` 1.78.0 sends feature-telemetry events to PostHog unless `MASTRA_TELEMETRY_DISABLED` is set. The factory host sets it to `1` before Mastra is used (`src/no-telemetry.ts`, imported first; test `mastra_telemetry_is_off_by_default`), and Task 3's harness spec passes it too. A person can opt in with `MASTRA_TELEMETRY_DISABLED=0`.
- **FT0-1-3, the shape of the skeleton.** `createFactoryMastra({ model, pgUrl | storage, workflows? })` (ruling FT0-0-1) and `readFactoryEnv`, which reads `LOAMS_PG_URL`, `LOAMS_DURABLE_URL` and `LOAMS_FACTORY_MODEL` (the default model as `provider/model`; no model is bundled, Q1114). Imports use `.js` specifiers under `NodeNext`, so Task 3 can run the compiled host with plain `node`. `zod` comes from the workspace catalog.

### Task 2 (2026-10-23)

- **FT0-2-1, merged as `ostrium-labs/loams-mastra#53`.** `@loams/mastra-acp` on `@agentclientprotocol/sdk` 1.8.0 (`serveAcp`, `connectAcp`). Its tests sit in `packages/acp/test` (they need no server), not `test/acp/`. An `error` chunk fails the prompt; a cancel while a permission is pending ends it `cancelled`; an agent without Mastra memory keeps the session's turns in the bridge. A changeset releases it as 0.1.0; the publish waits, with `@loams/mastra-resonate`'s, on the owner's approval of the `npm` environment (and trusted publishing for the new package name). Task 3 waits for it.

### Task 4 (2026-10-23)

- **FT0-4-1, `mcp`-role methods, ndjson only.** The seven new methods (`CreateRunWorktree`, `GetThreadDiff`, `PostReviewComment`, `OpenChangeRequest`, `RequestApproval`, `GetApproval`, `MergeChangeRequest`; wire types in `loams-agentd-proto/src/factory_loop.rs`) are role `mcp`, `renderer: false`, and each names a `chatId`, so the daemon's policy limits them to the run's own session tree as it does `QueueCommand`. They are served over ndjson, which is what the MCP shim speaks; their `loams.agentd.v1` Connect twins wait until a browser client needs them (Studio sees the approvals through `WatchApprovals`, which it already has).
- **FT0-4-2, typed approvals.** `RequestApproval` lists the approval in the same queue as harness approvals, under the tool name `factory.<kind>` with `{kind, summary, payloadHash}` as its arguments, so Studio needs no change to show it. Unlike a harness approval it has no run waiting, no 30-minute timeout and survives the end of its chat's turn and a daemon restart (table `typed_approvals`); "always" is treated as "once" and never remembered. The daemon computes the payload hash itself (SHA-256 of the canonical JSON, keys sorted), so the factory host never has to agree with it on a serialisation.
- **FT0-4-3, `MergeChangeRequest` spends the approval before it calls the code host.** The payload is rebuilt from the call's own arguments and the chat's remote (`{repo, pr, head_sha}`), compared by hash, and the approval is marked `used` atomically; only then does Forgejo (`POST …/pulls/{n}/merge` with `head_commit_id`) or `gh pr merge --match-head-commit` run. A merge the code host then refuses (the head moved) needs a new approval: the safe side of "a crash never repeats a merge".
- **FT0-4-4, build-thread worktrees.** `CreateRunWorktree` puts the worktree at `<worktrees root>/<repo>/<branch with / as ->` on the named branch (not `loams-desktop/<name>` as Studio's own `CreateWorktree` does) and answers an existing worktree of that branch, so a resumed step never makes a second. The policy now lets a run's new side chat have its `cwd` under the worktrees root as well as under a project folder.
- **FT0-4-5, `create_chat`'s `worktree`.** The tool takes `worktree: { branch, base? }`, calls `CreateRunWorktree` on the parent chat (the caller's chat by default) and creates the chat there; `cwd` is then ignored. Tool risk is in each new tool's description ("Risk: …") and, for every tool, in `annotations.risk` next to MCP's `readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint`.

### Task 5 (2026-10-23)

- **FT0-5-1, where the readers live.** As ruling FT0-0-2 says, `checks` and `check_log_tail` are in `loams-agentd-sessions/src/checks.rs` on `ChangeRequestCreator`'s credentials, behind the `mcp` methods `GetChecks` and `GetCheckLog` (tools `get_checks`, `get_check_log`, risk read). #844's `loams-agentd-factory` is untouched.
- **FT0-5-2, Forgejo.** Status from the combined commit status API. A Forgejo Actions status's `target_url` (`…/actions/runs/{run}/jobs/{job}`) gives the log id `run/job`; the log is read from that job's web `…/logs` route with the same token, only on the credential's host. That route taking an API token is **(verify)** against a live Forgejo in Task 11's e2e job; the unit test uses the fake Forgejo.
- **FT0-5-3, GitHub.** Check runs and commit statuses through `gh api`; an Actions check run's id is its job id, so `logId` is set only for check runs of the `github-actions` app, and the log is `repos/{owner}/{repo}/actions/jobs/{id}/logs`.
- **FT0-5-4, bounded and scrubbed.** A tail is at most 1,000 lines and 64 KiB (200 lines by default, the run-group tail's size), and goes through the harness's `redact_output` (tokens, bearer values, URL credentials, long hex and base64 runs) before it leaves agentd. No checks yet reads as `pending`, not `success`.

### Task 6 (2026-10-23)

- **FT0-6-1, the policy file.** `.loams/factory.toml` has `[tools] deny = […]`, `[tools] approve = […]` and `[agents.<id>] tools = […]` (an agent's allow-list). It can only tighten the D1431 defaults: a destructive or egress tool asks whatever the file says. Parsed with `smol-toml` 1.9.0 (BSD-3-Clause, already in the tree). A tool whose risk agentd's table does not know is treated as destructive, so it always asks.
- **FT0-6-2, denied means not offered.** `gate()` answers `undefined` for a denied tool and `gateTools()` drops it, so the model never sees it; `approve` is a copy of the tool with `requireApproval: true`, keeping its prototype.
- **FT0-6-3, agentd's tools arrive through the shim agentd gives the harness.** agentd passes the MCP shim to an ACP harness in `session/new`'s `mcpServers`; `shimFromAcp` takes it from there and passes only `LOAMS_AGENTD_PORT`, `LOAMS_AGENTD_RUN_TOKEN` and `LOAMS_AGENTD_SESSION_ID` (plus `PATH`) to the subprocess. `@mastra/mcp` 2.4.1 (2026-10-15; 2.5.0 is under 3 days old) is the client.
- **FT0-6-4, structured output, validated.** The planner and the reviewer answer through Mastra's `structuredOutput` and the answer is parsed again with the same zod schema (`planSchema`: 1 to 5 tasks, each with at least one check; `reviewSchema`: at most 30 comments). The reviewer anchors every comment to the checksum of the diff it read.
- **FT0-6-5, sub-agent approval propagation is not yet tested end to end.** The supervisor registration is tested (`registers the roles under Loams Bot`); that a sub-agent's `requireApproval` surfaces at Loams Bot's stream is checked in Task 7's workflow tests and Task 11's e2e, where a mock model can drive a delegation.

### Owner approvals (2026-10-24)

The owner approved the pending decisions below on 2026-10-24 ("approve all with your recommendations").

- **FT0-1-1 is superseded: BlueOak-1.0.0 is allowed on the npm licence allow-list** (it is a permissive, OSI-approved licence; this overrides SD1 ruling T2-2 for this licence only). `@loams/factory` rejoins the npm licence gate's default projects. **Approved (owner, 2026-10-24).**
- **Task 8, the Live deploy credential lives in agentd** and the deploy runs as an agentd tool, like the merge, never in the Mastra process (the plan's "No credential in the Mastra process" constraint). **Approved (owner, 2026-10-24).**

### Status (2026-10-24)

- **Merged:** Task 0 (#1127), Task 1 (#1132), Task 2 (`ostrium-labs/loams-mastra#53`), Task 6 (#1164); Task 4 is #1163.
- **Open:** Task 5 is draft #1174, stacked on #1163; its TS bindings and request fixtures need regenerating and its tests running (see the PR).
- **Waiting on the owner:** the `npm` environment approval for `loams-mastra`'s Release run (publishes `@loams/mastra-resonate` and `@loams/mastra-acp` 0.1.0; trusted publishing must be set up for `@loams/mastra-acp`), which gates Task 3 and Task 7; the BlueOak-1.0.0 decision of FT0-1-1, after which `@loams/factory` joins the npm licence gate.
- **Not started:** Tasks 3, 7, 8 (needs a ruling on where Live's admin credential lives: in agentd as another `mcp` method, or in the factory host), 9, 10, 11.

### Task 9 (2026-10-26)

- **FT0-9-1, the Live app `factory`** is `agents/factory/live/factory.js` and `schema.json` (table `factory_runs`, indexes `by_run` and `by_updated`): `runs:record` (a mutation that applies one transition `{run, seq, patch}`; a `seq` at or below the stored one changes nothing, and only the record's own fields are accepted), `runs:list` (newest first) and `runs:get`. Its unit tests run the bundle on a fake `loams:server` (`agents/factory/test/support/loams-server.ts`, as strict as Live about `int64`); running it on the real Live runtime is part of Task 11's e2e **(verify)**.
- **FT0-9-2, who writes the record.** The factory host writes it (`liveRunStore`, Connect JSON `LiveService.Mutate` on app `factory`, with `idempotencyKey` `<run>/<seq>`). The run record is not a credential-bearing action, but an engine with authentication on needs a token: FT0 sends none (the light stack's default has authentication off). A `live:call` token scoped to the app `factory`, handed to the host by agentd at launch, comes with Task 3 **(verify)**.
- **FT0-9-3, installing the app and the stream.** `CreateApp` + `Deploy` of `agents/factory/live` and `CreateStream` of `factory_events` need the owner credential, so they belong to agentd (Task 3's launcher) and to Task 11's Compose setup, not to the host. This PR ships the app and the clients only; until Task 3, a person installs them by hand.
- **FT0-9-4, the events.** One CloudEvent per transition on `factory_events` (namespace `default`, partition 0, the streams REST `…/events` route): `id` `<run>/<seq>`, `source` `/loams/factory/<project>`, `type` `io.loams.dev.factory.run.<kind>.v1` with kind `opened`, `stage`, `waiting`, `resumed`, `paused`, `succeeded`, `failed` or `killed`. A replay re-sends the same `source` and `id`, which `ProduceCloudEvents` drops as a duplicate. The record is written before the event, so a crash between them is repaired by the replay.
- **FT0-9-5, the Runs page** (`web/plugins/factory/src/runs/`, `/studio/factory/runs`, a "Runs" entry in Studio's Factory group) reads `runs:list` through the console's own transport every 2 s. The Live plugin's watch only knows `_system:query`; a subscription to `runs:list` replaces the poll when the Live client gets a typed watch of user functions. The plugin stays desktop-only (D922), so the Playwright check (`web/apps/console/test/e2e/factory-runs.spec.ts`) runs in the web e2e job against the dev server's `?desktop` preview, with `LiveService.Query` mocked at the network edge.
