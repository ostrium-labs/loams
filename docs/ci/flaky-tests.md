# Flaky tests

This is a survey of CI failures from 2026-10-15 12:00 to 2026-10-22 24:00 UTC. It covers every failed
job, plus every earlier attempt of a run that was re-run. I took the failing test names from each
failed job's log: nextest `FAIL` lines, `cargo test` `... FAILED` lines, and the vitest and
Playwright failure lines.

**How to read the table:**
- A test counts as flaky, not as broken by its pull request, when it failed on several unrelated
  branches, or when a run that failed passed on re-run with no change (the **re-run passed** column).
- A **Failed** count is the number of failed jobs that named the test.
- **Branches** is the number of distinct head branches those jobs ran on.

Other agents own the fixes. This file only lists the tests and how they are handled.

## Retry once (nextest, `Rust: tests`)

`.config/nextest.toml` retries these tests once, and only these, each by binary and exact test name.
There is no blanket retry. A test that passes on its retry shows as `FLAKY` in the nextest summary,
and the job passes. When a PR fixes one of these tests, it removes that test's entry.

| Test (binary :: name) | Failed | Branches | Re-run passed | Tracking |
|---|---|---|---|---|
| `loams-graph::limits` `namespace_concurrency_limit_resource_exhausted` | 10 | 8 | 1 | #1084 |
| `loams-graph::limits` `idle_namespaces_leave_no_slot_entry` | 9 | 8 | 2 | #1084 |
| `loams-agentd::security` `failed_upgrades_are_slowed` | 5 | 5 | 1 | #1089 |
| `loams-git::owner` `idle_sequencer_is_evicted_and_reloads` | 3 | 3 | 1 | #1086 |
| `loams-worker::worker` `an_expired_lease_nobody_took_is_retaken_and_the_task_continues` | 3 | 2 | 2 | #1087 |
| `loams-agentd-sessions::m5_repos_diffs_terminals` `git_history_search_is_fuzzy_complete_and_accepts_sha_prefixes` | 2 | 2 | 2 | #1088 |
| `loams-query::it` `filter_write::a_refused_batch_waits_for_retry_after` | 1 | 1 | 1 | #1090 |

The retry applies only where nextest runs the test, which is the workspace `Rust: tests` job. The
Agentd job runs `cargo test` per crate, so `failed_upgrades_are_slowed` has no retry there.

## Fix in flight (no retry added)

| Test | Failed | Branches | Re-run passed | Tracking |
|---|---|---|---|---|
| `loams-agentd-connect::parity` `connect_and_ndjson_agree` | 11 | 9 | 4 | PR #1059 (the fix) |
| `loams-meta::it` `node::a_node_keeps_serving_through_failed_snapshot_uploads` | 3 | 3 | 3 | #969, PR #1061 |

## Fixed (retry removed)

| Test | Failed | Branches | Re-run passed | Cause and fix |
|---|---|---|---|---|
| `loams-graph::links` `collection_link::link_apply_exactly_once_under_kill` | 6 (+1 hang on #1059) | 5 | 2 | #1085: the test harness waited out the dead owner's lease from a deadline it read once, while a renewal sent before the pause could still land and extend the lease. The survivor then found the lease held ("served by node N now; retry"). `wait_out_the_lease` now reads the lease again until no node holds it. Both kill tests also have a hard deadline. |
| `loams-graph::links` `stream_link::stream_link_exactly_once_under_kill` | 3 (+1 on #1058) | 3 | 2 | #1085, as above. The "Tokio context is being shutdown" panic that followed each failure came from the dead node's pool thread when the runtime shut down after the test had already failed. It was not a separate fault. |

## Listed only (not nextest, so no retry)

| Test | Job | Failed | Branches | Re-run passed | Tracking |
|---|---|---|---|---|---|
| `loams-house-worker` `pool::crash_does_not_reach_the_front` | Connectors: `Fabric: fmt, clippy, tests and deny` (`cargo test`) | **33** | **27** | 7 | #1091 |
| Desktop `test/e2e/cookies.spec.ts` "leaving a server clears its path-scoped and secure cookies" | Desktop: `Linux: package (x64)` (Playwright) | 21 | 14 | 2 | none yet |
| Desktop `test/e2e/smoke.spec.ts` "electron smoke" | Desktop: `Linux: package (x64)` | 15 | 9 | 0 | none yet |
| Studio `test/e2e/studio.spec.ts` "comment_and_send_review_to_agent" (3 min timeout) | `Studio e2e: web` | 10 | 9 | 5 | none yet |
| Console `test/diff.test.tsx` "shows the thread checkout's diff and its comments" | `Web: build, typecheck and tests` | 6 | 5 | 2 | none yet |
| Console `test/git.test.tsx` "repository_without_env_says_so" | `Web: build, typecheck and tests` | 5 | 5 | 1 | none yet |
| `reactive_checker_with_identity_changes::tikv`, `tikv::scan_bounds_and_order` (the TiKV suites; no failure in this window is named after the TiKV GC race) | `TiKV: suites` (full tier, `ci:tikv`) | 3, 2 | 1, 2 | 0, 1 | none yet |
| `loams-house-worker` `insert::concurrent_commits_all_land` | Connectors (`cargo test`) | 0 in this window | | | named by the owner; watch |

## Not flaky: stale generated files and real breakages

These failures stopped once the branch was regenerated or fixed. They are not flaky, and none of them
gets a retry:
- **Generated files out of date:** `the_committed_matrix_is_up_to_date` (5 failures on 5 branches,
  from the generated `docs/house` matrix), and `golden_go` and `golden_typescript` (the facade
  generator's goldens).
- **Branch-local failures:** `writers_queued_behind_an_append_share_the_next_one` (8 failures, 2
  branches) and `owned_statements_of_later_tasks_are_48` (7 failures, 5 branches) belonged to work in
  progress on those branches.

## Redoing the survey

1. List the runs per half day (the Actions runs API caps a query at 1,000 results).
2. For every failed job, and for every earlier attempt of a re-run run, fetch the log with
   `gh api --allow-escape-sequences repos/ostrium-labs/loams/actions/jobs/<id>/logs`.
3. Strip the ANSI colours and match the failure lines.
