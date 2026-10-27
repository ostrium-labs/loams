# Temporal SDKs against the Loams Temporal gateway

Preview. One row per official Temporal SDK leg beyond Go and TypeScript (whose
per-method matrix is in [temporal-gateway.md](temporal-gateway.md)). Each row
is the corpus's DX1 scenarios run by that SDK's own workers and starter
against `loams-temporal` in the compose profile `temporal-gateway`, the SDK's
replayer on the gateway's histories, and the differential against that SDK's
corpus generated on upstream Temporal. "Not yet run" means exactly that: the
code exists, CI has not yet reported a result for it. Each SDK lane edits only
its own row.

| SDK | Pin | Leg | DX1 scenarios | Replayer | Differential | Last run |
|---|---|---|---|---|---|---|
| Ruby | temporalio gem 1.9.0 (on sdk-core; Ruby 3.4.10, bundler 4.0.22) | `conformance/compute/temporal/gateway/legs/ruby.sh` (`SDK_LEGS=ruby run.sh --profile`, `temporal-sdk-ruby.yml`) | not yet run | not yet run | not yet run (no Ruby corpus committed yet; CI diffs against a start-dev generation in the same run) | — |

Notes, Ruby:

- The Ruby corpus worker is `conformance/compute/temporal/corpus/workers/ruby/`
  (workflows, activities, a worker and starter in `generate.rb`, the
  `WorkflowReplayer` check in `replay.rb`). It covers every scenario that is
  not Go only. Ruby has no `SideEffect`, so `side_effect_and_version_marker`
  records the version marker only, as in TypeScript.
- `generate-ruby.sh` generates `histories/ruby/` against upstream Temporal
  (compat mode, or `CORPUS_SERVER=start-dev`) and rebuilds only the Ruby
  entries of `manifest.json`. Until a generation is committed, the
  differential against the committed corpus is skipped, and
  `temporal-sdk-ruby.yml` diffs the gateway's Ruby histories against a
  start-dev generation made in the same workflow run.
- The SDK switches TLS on whenever an API key is set. The corpus worker
  passes `tls: false`, because the profile serves plaintext on the stack's
  network.
