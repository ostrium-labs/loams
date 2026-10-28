# The Temporal history corpus

DX1 DX0 Task 4 (design 74 §13): real Temporal histories, produced by the
pinned Go and TypeScript SDKs against the pinned Temporal server, that the
SDKs' own replayers accept. It is DX1's differential oracle: the gateway's
histories for the same scenarios must replay the same way.

## Which server produced it

The committed corpus was generated against `temporal server start-dev` (CLI
1.9.1 from the pinned `temporalio/admin-tools:1.32.1` image, which embeds
server 1.32.0), not against compat mode (the stock 1.32.1 server on Loams
Postgres). Task 3's gate makes start-dev the oracle while compat mode is not
yet go: its soak went red once
([#1185](https://github.com/ostrium-labs/loams/issues/1185)). `manifest.json`
records this (`server_image` names start-dev, `loams_postgres_major` is `start-dev`).
Once compat mode is go, the nightly regenerates the corpus against it
(`CORPUS_SERVER=compat-mode`, the default) and the regenerated corpus
replaces this one.

## Layout

| Path | What |
|---|---|
| `scenarios.json` | the scenarios in plan order, each with its phase (DX1–DX4) and whether it is Go only |
| `workers/go/` | Go module (SDK `go.temporal.io/sdk` v1.49.0): `scenarios/` holds one workflow per scenario; `main.go` is the generator (`generate`) and the exporter (`export`) |
| `workers/ts/` | npm package (`@temporalio/*` 1.24.0): `src/workflows.ts`, `src/activities.ts`, `src/generate.ts`, and the replay test `src/replay.test.ts` |
| `workers/python/` | uv project (`temporalio` 1.34.0, D2136): `corpus_py/` holds the workflows, activities, the starter (`drive.py`), the generator and worker CLI (`python -m corpus_py`) and the replayer (`replay.py`); not yet run, and `histories/python/` is empty until its first generation (D2137) |
| `replay/go/` | `replay_test.go`: every Go history replays with `worker.NewWorkflowReplayer` |
| `histories/<lang>/<scenario>.{json,binpb}` | protojson (what `temporal workflow show -o json` writes and the replayers read) and the binary `History` message; a scenario with several runs has `<scenario>.run<k>.*`, oldest first |
| `histories/generate-<lang>.json` | what each driver observed, expected errors included |
| `manifest.json` | per history: scenario, run, language, SDK, server version and image, Loams Postgres major, date, phase, SHA-256 of both files |
| `normalize.py` | the masking used to compare two generations |
| `manifest.py` | `build`, `check` (per pull request) and `stable` (two generations) |
| `generate.sh` | runs both languages against a server and rebuilds the manifest |

## Scenarios

DX1: `hello`, `two_activities_sequential`, `activity_parallel_fanout_10`,
`signal_then_complete`, `query_while_running`, `cancel_requested`, `terminated`,
`workflow_failed_application_error`, `activity_heartbeat_details`.

DX2: `timer_short`, `timer_cancelled`, `activity_retry_backoff_3_attempts`,
`activity_start_to_close_timeout`, `activity_schedule_to_start_timeout`,
`activity_heartbeat_timeout`, `workflow_task_failure_then_fix` (Go only),
`signal_with_start`, `child_workflow`, `child_workflow_cancelled`,
`continue_as_new_3`, `id_reuse_allow_duplicate_failed_only`, `id_conflict_fail`,
`local_activity`, `side_effect_and_version_marker`.

DX3: `search_attributes_upsert` (custom keyword attribute `CorpusKeyword`, which
the generator registers), `memo`, `large_payload_1mb`.

DX4: `update_accepted_completed`, `update_rejected` (the validator refuses),
`schedule_triggered` (Go only), `reset_to_first_workflow_task` (Go only).

Notes:
- Each run uses the fixed workflow ID `corpus-<lang>-<scenario>`, in a fresh
  namespace per generation. `schedule_triggered` is the exception: the server
  appends the action time to the schedule's workflow ID.
- `continue_as_new_3` has four runs (0 to 3); `id_reuse_allow_duplicate_failed_only`
  and `reset_to_first_workflow_task` two.
- TypeScript has no `SideEffect` (workflow code is deterministic by
  construction), so its `side_effect_and_version_marker` records the version
  marker only.
- `workflow_task_failure_then_fix` fails exactly one workflow task (a panic the
  "fix" removes), then completes.
- The replayers are given the original workflow ID: child workflow IDs derive
  from it.

## Normalisation

Two generations differ only in what the server or the worker chooses at run
time. `normalize.py` masks times (also inside strings and payloads), run IDs
and other UUIDs, identities, task tokens, request IDs, binary checksums and
build IDs, the namespace, task IDs, `historySizeBytes`, stack traces, sticky
queue names, a continued run's `firstWorkflowTaskBackoff`, and sorts the SDK's
flag sets. It keeps event IDs, types, order and every other attribute. The
`History` message has no batch boundaries, so none are compared.
`activity_parallel_fanout_10` compares with the order of its activity
started and completed events free: the server, not the code, picks which
parallel activity runs first.

## Regenerating

With the compat-mode stack up (`conformance/compute/temporal/run.sh`, or
`oc_up temporal`):

```sh
conformance/compute/temporal/corpus/generate.sh                       # go,ts (CORPUS_LANGS)
conformance/compute/temporal/corpus/generate.sh --langs go,ts,python  # with the Python worker
python3 conformance/compute/temporal/corpus/manifest.py check
```

Against start-dev instead, run the dev server on a network of its own with
the alias `temporal-frontend`:

```sh
. conformance/compute/lib.sh
podman network create dx0-corpus-net
podman run -d --name dx0-corpus-temporal --network dx0-corpus-net \
  --network-alias temporal-frontend "$TEMPORAL_ADMIN_TOOLS_IMAGE" \
  temporal server start-dev --ip 0.0.0.0 --headless
CORPUS_SERVER=start-dev OC_NETWORK=dx0-corpus-net \
  conformance/compute/temporal/corpus/generate.sh
```

Any change to a workflow under `workers/` changes its histories: regenerate in
the same pull request.

## Tests

- per pull request: `corpus_manifest_complete` (`manifest.py check`) and
  `normalize_selftest` (`normalize.py --self-test`), in `conformance/compute/selftest.sh`;
- nightly: `corpus_replays_go` (`go test ./replay/go/...`), `corpus_replays_ts`
  (`node --test dist/replay.test.js` in `workers/ts`) and
  `corpus_regenerates_stably` (two generations, `manifest.py stable`).
