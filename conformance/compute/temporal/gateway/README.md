# The Temporal gateway's SDK legs

Plan DX1 Task 10 (design 74 §13): the official Temporal SDKs, at the
corpus's pins (Go v1.49.0, TypeScript 1.24.0), against `loams-temporal`.

`run.sh` has two modes:

- **loopback** (`run.sh`): a built `loams-temporal` (and, with
  `LOAMS_RESONATE_BIN`, a real Resonate server on SQLite) on loopback ports,
  in `insecure_dev`; the SDK containers use host networking and reach only
  `127.0.0.1`.
- **profile** (`run.sh --profile`): the compose profile `temporal-gateway`
  of `deploy/release/compose.compute.yaml` (D2031), which runs the release
  image `ghcr.io/ostrium-labs/loams-temporal` (D2030).
  `../../temporal-gateway.yaml` is a test overlay, never distributed, like
  `pgdog-txn.yaml`: it builds that image locally from
  `deploy/release/Containerfile.loams-temporal` (so a pull request tests its
  own gateway) and mounts `gateway.toml` here. The gateway runs in the
  engine's network namespace (`network_mode: service:loams`) on the
  `loams_temporal` database that `deploy/release/compute/durable-init.sh`
  creates on Loams Postgres's direct endpoint, the engine's Resonate on
  `127.0.0.1:8001`, a test API key and a test task-token key, so not
  `insecure_dev`. No host port: the SDK containers run on the stack's
  network against `loams:7233` with the test key in `TEMPORAL_API_KEY`,
  which the corpus workers send as a bearer token. Each SDK container is
  killed after `LEG_TIMEOUT` seconds (default 900), each TypeScript scenario runs on its own and is killed after `TS_SCENARIO_TIMEOUT` (default 240), and the gateway's and
  the engine's logs are written to the output directory at the end. The
  stack stays up afterwards (`oc_compose --gateway down -v` removes it);
  `OC_PROJECT` runs it beside another stack.

Either way, it then runs:

| Case | What it checks |
|---|---|
| `gateway_dx1_scenarios_{go,ts}` | the corpus's own generators run every DX1 scenario through the gateway |
| `gateway_hello_{go,ts}`, `gateway_signal_{go,ts}` | §74 §13.3's DX1 exit: hello (one activity) and a signal complete |
| `gateway_histories_replay_{go,ts}` | each SDK's replayer accepts every history the gateway wrote |
| `gateway_profile_serving`, `gateway_profile_no_host_ports` | profile mode only: the gateway in the profile answers, and neither it nor the engine's namespace publishes a host port (the self-test's `gateway_profile_no_host_ports` checks the compose half) |
| `gateway_dx1_differential_<lang>_<scenario>` | `compare.py`: the gateway's history equals the corpus's after `normalize.py`, a sticky queue read as its normal queue, and the SDK's name and version dropped from later tasks' `sdkMetadata` (plan ruling X1.R20) |

```sh
cargo build -p loams-temporal-frontend --bin loams-temporal
conformance/compute/temporal/gateway/run.sh                       # in-process Resonate
LOAMS_RESONATE_BIN=…/resonate conformance/compute/temporal/gateway/run.sh   # a real one
conformance/compute/temporal/gateway/run.sh --profile             # the compose profile
```

Other SDKs run as legs (`SDK_LEGS=<sdk>`, `legs/<sdk>.sh`), each with its
own workflow and a row in
[temporal-sdk-matrix.md](../../../../docs/guides/durable/temporal-sdk-matrix.md).
The Ruby leg (`SDK_LEGS=ruby run.sh --profile`, `temporal-sdk-ruby.yml`)
adds `gateway_dx1_scenarios_ruby`, `gateway_hello_ruby`, `gateway_signal_ruby`,
`gateway_histories_replay_ruby` and `gateway_dx1_differential_ruby_<scenario>`.
The differential is skipped until `corpus/histories/ruby/` is committed. The
workflow also diffs against a start-dev generation made in the same run
(`ruby-vs-upstream.junit.xml`).

Expected failures are in `xfail.toml`.

## On pull requests

`.github/workflows/temporal-gateway.yml` runs `run.sh --profile` on every
pull request that touches `crates/loams-temporal-*`,
`crates/loams-gateway/src/temporal/**` or `conformance/compute/temporal/**`
(and the overlay, `lib.sh`, `pins.env`, `durable-init.sh`,
`compose.compute.yaml`, `Containerfile.loams-temporal`, the example config
and the workflow),
through a "Changed paths" job as in `ci.yml`, and by hand. During DX2 it is
**not part of `CI required`**: a red run does not block a merge, and the
job goes into `CI required` once it has been stable. The compose half is
checked on every pull request by `conformance/compute/selftest.sh` in
`repo-checks` (which is in `CI required`): `gateway_profile_no_host_ports`,
`gateway_profile_shape` and `gateway_image_is_release_image`.
