# SO1 — Loams SystemOne: Engine, API and Backends Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Execute task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (names, paths, flags, field names, limits), use them verbatim. The code is not pre-written in this plan.

> **Status: Planned** (2026-10-02). **Slot: track SO, first plan** (proposed; D520–D539). Branches `so1-t<N>`, stacked; PRs target `dev` (R0.1). Task 0 ran on 2026-10-18: [spike results](so1-systemone-spike.md), rulings R0.1–R0.24 below. Task 0 gates Tasks 5 and 12's in-process backend only; Tasks 1 to 4 and 6 to 11 do not depend on it. The desktop half is [SO2](2026-10-02-so2-systemone-desktop.md).

**Goal:** Ship Loams SystemOne's engine side: one typed-decision API (`choice`, `score`, `noul`) in front of interchangeable backends, chosen from the host:
- **`loams.systemone.v1`** (Connect) and **`POST /v1/systemone`** (a superset of the Jev and `laya-serve` JSON), with validation limits set to the largest any backend accepts and per-backend caps applied by routing;
- the crate **`loams-systemone`**: canonical types and wire, the `DecisionBackend` trait, a registry, `HostProfile`, a pure routing function, calibration, the self-test, a model manager;
- adapters: **`LayaServeBackend`** (a supervised loopback `laya-serve` sidecar with a generated key), **`JevBackend`** (off by default, bring your own key), a `FakeBackend`, and, if Task 0's spike passes, **`LayaNativeBackend`** (kevala, in-process);
- the server route, `loams systemone …` CLI verbs and a read-only MCP tool, the local decision log and export, and the docs.

**Architecture:**
- **`proto/loams/systemone/v1/systemone.proto`**: the service and messages of design §40 §4.1, `buf lint` STANDARD and `buf breaking` FILE (the app-protos CI job).
- **`crates/loams-systemone/`** (no DataFusion, no storage, no GPUI; tokio, serde, reqwest, axum only for the test fake): `types.rs` (canonical request/response), `wire.rs` (JSON in Jev/Laya shape), `limits.rs`, `backend.rs` (trait, `Capabilities`, `BackendStatus`), `registry.rs`, `host.rs` (`HostProfile`), `route.rs` (`plan`), `calibrate.rs`, `selftest.rs` (+ `selftest_cases.json`), `models/` (lock, downloader, cache), `sidecar.rs` (supervisor), `backends/{laya_serve.rs,jev.rs,fake.rs,native.rs}`, `declog.rs`.
- **`crates/loams/`**: `systemone` module (Connect service impl, the axum route on the gateway role, config), CLI verbs.
- **`crates/loams-link`** is untouched here (SO2).

**Tech Stack:** Rust 1.97.1, edition 2024, workspace lints; connect-rust and buffa (D128); `reqwest` (rustls) for HTTP clients, `tokio`, `serde_json`, `sha2`, `async-trait` (as the workspace already uses it; Task 3 confirms), `thiserror`. Dev: `axum` for the fake sidecar, `wiremock` for Jev, `proptest`. Python side (Task 5): `uv` and `laya[serve]==<pin>` installed by the CLI, not a build dependency. kevala only if Task 0 passes (git dependency pinned to a commit, Apache-2.0).

**Spec:**
- [`docs/design/40-loams-systemone.md`](../design/40-loams-systemone.md): all of it, especially §2 (what was verified and corrected), §4, §5, §6, §9; [decision log](../design/13-decision-log.md) (D520–D539, Q520–Q539).
- [`docs/design/13-decision-log.md`](../design/13-decision-log.md): D111 (loopback), D128 (connect-rust), D220 (open core), D284 (no telemetry), D-SF-11 (no content in spans).
- [`docs/design/39-software-factory-and-loams-bot.md`](../design/39-software-factory-and-loams-bot.md): D-SF-9 (approvals), D-SF-11.
- Upstream, re-read in Task 0: Laya `docs/http-api.md`, `docs/security.md`, `README.md` (Honest Limits); `bvolpato/kevala` `docs/architecture.md`; docs.typesafe.ai primitives.

## Global Constraints

- **Local by default.** Locality is `LOCAL_ONLY` unless the request says `ALLOW_CLOUD` and the instance switch and a key exist. A forced cloud backend with local-only data is `CLOUD_NOT_ALLOWED`, never a silent fallback.
- **Loopback only for sidecars (D111).** The adapter refuses a non-loopback address. The sidecar is started with `LAYA_HOST=127.0.0.1` and a generated key in its environment; the key is never on a command line, never logged, never written to disk.
- **Policy above the trait.** Calibration, abstention, `act_probability` stripping and provenance are applied in `Registry::decide`, never in an adapter.
- **A decision is advisory (D537).** No code path in this plan lets a SystemOne answer approve, merge, delete, deploy or grant.
- **No content in logs, spans or errors.** State, questions and answers appear nowhere except the response and the opt-in local decision log.
- **Never copy** `nvkudva/laya-web` (no licence) or any code without a licence. kevala, Laya, FluidUse and `laya-ts` are Apache-2.0: copied files keep their notice in `THIRD_PARTY_NOTICES.md`.
- **Weights are verified before they are parsed** (SHA-256), and a revision directory is immutable.
- **The build machine.** One cargo build at a time, the shared target, `CARGO_BUILD_JOBS=4`. Python installs and model downloads only in Tasks 0, 5 and 9, by hand or in the labelled runner, never in unit tests. Stop and report if `/home` has under 8 GB free.
- **Commit areas:** `systemone`, `proto`, `cli`, `docs`, `ci`.

## Rulings made while writing this plan

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | **The HTTP shape is Jev's and `laya-serve`'s, plus optional fields.** We accept and ignore unknown fields; we never rename one | `laya-client` and TypeSafe clients work unchanged | A future Jev field we do not model is dropped silently; mitigated by passing unknown response fields through in `extra` for Jev |
| 2 | **`confidence` is passed through; `calibrated_confidence` is ours.** Gating uses the latter | The two vendors define confidence differently (D536) | None |
| 3 | **`criteria` order is the order sent.** Maps are parsed with `serde_json`'s `preserve_order` (and a custom visitor in the proto JSON path) | Option order feeds the prompt; `HashMap` would reorder | Wrong answers that look random; test `option_order_survives_roundtrip` |
| 4 | **Core ML's 32-option bucket is a routing rule, not a validation rule** | The same request is valid on another backend | A user with 40 options on a Mac gets the sidecar or an error instead of a 422 |
| 5 | **No table of calibration ships.** First table comes from a labelled set (Task 7) | A table fitted to someone else's data lies | Raw over-confidence until the user calibrates |
| 6 | **The Jev adapter ships behind a build-time-default-off flag and a runtime key** | Terms unknown (Q520) | None |
| 7 | **Language detection is conservative:** English only when explicit `lang` says so or the detector is sure; otherwise multilingual | The English checkpoint is confidently wrong outside English | English text sometimes takes the weaker multilingual path (0.657 vs 0.783, upstream) |

## Carried in

Design §40 and the pending log. The verified facts of §2 are inputs: re-verify any that a task depends on in Task 0 and record the result in the spike doc.

## Review Focus

1. **Cloud never happens by accident.** Tests: Task 4 (`cloud_requires_all_three_consents`, `forced_jev_with_local_only_fails_closed`, `fallback_respects_locality`).
2. **Sidecar hygiene.** Loopback, key not leaked, group kill. Tests: Task 5 (`non_loopback_is_refused`, `key_not_in_argv_or_logs`, `child_group_killed_on_drop`).
3. **The wire is compatible.** Golden fixtures from Laya's and Jev's docs. Tests: Task 2.
4. **Routing rules exactly as §6.3.** Tests: Task 4's table.
5. **Weights are verified.** Tests: Task 9 (`digest_mismatch_deletes_part`, `verify_before_rename`).
6. **No content leaks.** Tests: Task 10 (`no_state_in_spans_or_errors`).

## File structure

```
proto/loams/systemone/v1/systemone.proto
crates/loams-systemone/Cargo.toml  src/{lib,types,wire,limits,backend,registry,host,route,calibrate,selftest,sidecar,declog}.rs
crates/loams-systemone/src/selftest_cases.json  src/models/{mod,lock,download,cache}.rs  models.lock.toml
crates/loams-systemone/src/backends/{mod,laya_serve,jev,fake,native}.rs
crates/loams-systemone/tests/{wire,route,registry,sidecar,jev,calibrate,selftest,models}.rs  tests/fixtures/{laya_*.json,jev_*.json}
crates/loams/src/systemone/{mod,service,http,config,cli}.rs   crates/loams/tests/systemone/{main.rs,http.rs,connect.rs,cli.rs}
scripts/systemone/{install_laya.sh,bench.sh,parity.py}
docs/plans/so1-systemone-spike.md   docs/design/40-loams-systemone.md   docs/design/13-decision-log.md   docs/plans/README.md   CHANGELOG.md   THIRD_PARTY_NOTICES.md
```

### Task 0: Reconcile and measure (the spike)

**Files:** read `crates/loams/src/{server.rs,main.rs}`, the connect-rust service examples under `crates/loams/src`, `crates/loams-link/src`, `deny.toml`; write `docs/plans/so1-systemone-spike.md`. Fill this plan's "Rulings made during execution" table.

**Checks** (record each with the command and date in the spike doc):
- Re-verify design §2.1's licences on the day (Laya `LICENSE`, HF card, `laya-coreml`, FluidUse, kevala, `laya-ts`), record the commit SHAs you pin, and confirm `nvkudva/laya-web` still has no licence.
- Re-read Laya's `docs/http-api.md` and `docs/security.md`; diff the request/response fields and limits against §4.2; record any change (the pin is `laya==0.3.23`).
- On the build machine, install `laya[serve]` into a throwaway venv under `/mnt/Projects` (never `/tmp`), run `LAYA_HOST=127.0.0.1 LAYA_API_KEY=… laya-serve` on CPU, and measure p50/p95 for 1, 5 and 10 questions of a short state (English and Hindi), cold start, resident memory, and the disk use of the venv. Record `/health` before and after a request.
- **kevala:** build `kevala-cli` at a pinned commit (`CARGO_BUILD_JOBS=4`), convert or fetch the Laya pack, run the same inputs; compare probabilities with `laya-serve` (target: max abs difference under 0.02 on 200 decisions from the golden set), measure native CPU latency and memory, and answer: is there a multilingual pack or converter; does `cargo deny check` pass with it as a git dependency; does `torchpt.rs` run on untrusted input anywhere in the inference path.
- **`ort`:** record the current release (2.0.0-rc.13 on 2026-07-28 or later), whether `load-dynamic` works on the build machine, and a 1-day estimate of the template-parity port (not done unless the owner approves, Q523).
- Whether `FluidUse`'s `LayaManager` can be built as a SwiftPM dependency alone (feeds SO2 Task 0; Mac not required here, read `Package.swift` and the repository).
- Whether an `axum` version and `async-trait` match the workspace's.
- The final decision numbers.

**Output:** the spike doc with a one-line verdict per gate and, for the in-process backend, one of **kevala / ort-with-approval / none**. Task 12 is skipped if the verdict is none.

**Commit:** `docs: SO1 spike results and reconciliation`.

### Task 1: `loams.systemone.v1` and the mock

**Files:** `proto/loams/systemone/v1/systemone.proto`, `crates/loams-apps-mock/src/systemone.rs` (the mock returns canned, deterministic answers), buf config, `docs/design/40-loams-systemone.md` §4.1 if the proto changes.

**Produces:** the proto exactly as design §4.1 (service `SystemOneService` with `Decide`, `DecideBatch`, `ListBackends`, `SelfTest`). The mock implements all four against `FakeBackend` semantics so SO2 and the console can build before the engine exists.

**Semantics:** field numbers as in §4.1 and never reused; `reserved` ranges documented; `Answer.value` is a `oneof`; `Provenance.warnings` are stable strings listed in a comment (`state_truncated`, `options_over_20`, `noul_via_choice_ab`, `fell_back_to:<id>`, `self_test_failed`, `uncalibrated`).

**Tests:** `buf lint` and `buf breaking` clean; `mock_decide_roundtrips` (Connect JSON and binary); `proto_json_matches_http_json` (the canonical JSON of a `DecideResponse` equals the Task 2 wire fixture after the documented key mapping).

**Commit:** `proto: loams.systemone.v1 and the apps mock`.

### Task 2: Canonical types, wire format and limits

**Files:** `crates/loams-systemone/src/{lib,types,wire,limits}.rs`, `tests/wire.rs`, `tests/fixtures/*`.

**Produces:**

```rust
pub enum QuestionType { Choice, Score, Noul }
pub struct Question { pub ty: QuestionType, pub instructions: String, pub criteria: Criteria }
pub enum Criteria { None, Choice(Vec<(String, String)> /* key, description; order kept */), Score(Vec<String>) }
pub struct DecideRequest { pub state: serde_json::Value, pub questions: indexmap::IndexMap<String, Question>,
    pub backend: Option<BackendId>, pub lang: Option<String>, pub min_confidence: Option<f64>,
    pub budget: Budget, pub locality: Locality, pub extra: serde_json::Map<String, Value> /* ignored fields */ }
pub struct Answer { /* type, value, probabilities, legend, confidence, answer_confidence, calibrated_confidence, low_confidence */ }
pub struct DecideResponse { pub model: String, pub answers: IndexMap<String, Answer>, pub usage: Usage, pub loams: Provenance,
    pub extra: serde_json::Map<String, Value> /* unknown backend fields; rendered back only in Compat::Jev, never invented */ }
pub fn parse_request(body: &[u8]) -> Result<DecideRequest, SystemOneError>;       // enforces limits first
pub fn render_response(r: &DecideResponse, compat: Compat) -> serde_json::Value; // Compat::Jev drops `loams` and `calibrated_*` if asked
pub const MAX_BODY: usize = 2 * 1024 * 1024;       // 2 MiB
pub const MAX_STATE_CHARS: usize = 50_000;
pub const MAX_QUESTIONS: usize = 64;
pub const MAX_OPTIONS_PER_CHOICE: usize = 255;    // the largest any backend accepts (Jev); laya-serve's 100 and Core ML's 32 are routing rules
pub const MAX_LEVELS_PER_SCORE: usize = 32;
pub const MAX_OPTIONS_TOTAL: usize = 512;
```

**Semantics:** `criteria` for `choice` is a map or a list of keys (normalised to key plus empty description); for `score` a list; `noul` takes none. Missing or null `state` is `400`. State length is measured as the string itself or `serde_json::to_string` of an object/array with `ensure_ascii=false` semantics. A lone UTF-16 surrogate escape anywhere in the body is `400` (Laya's behaviour). Limit failures are `413`, invalid questions `422` with the question id in the message. Option order is preserved.

**Tests:** `parses_laya_doc_request` and `renders_laya_doc_response` (golden files copied from Laya's `http-api.md` sample, with attribution in the fixture header); `parses_jev_doc_request` (`state`, `model: jev-latest`, choice question); `criteria_list_equals_map_with_empty_descriptions`; `option_order_survives_roundtrip`; `unknown_fields_ignored`; `missing_state_is_400`; `lone_surrogate_is_400`; `limits_413_each` (one case per constant); `locality_unspecified_is_local_only` (HTTP omitted and proto zero value); `http_wire_maps_to_proto_and_back` (the translation of §40 §4.1 in both directions); `choice_list_forms_equal` (map, key list and `{key, description}` list give the same canonical question); `hook_arguments_are_422`; `extra_fields_roundtrip_in_jev_compat_mode`; `noul_has_no_criteria_and_extra_criteria_is_422`; `score_levels_over_32_is_413`; `object_instructions_is_422_unsupported_shape`; `noul_criteria_optional_forwarded_to_jev`; `usage_options_roundtrips`; `act_probability_never_rendered`; `calibrated_confidence_rendered_only_in_loams_mode`.

**Commit:** `systemone: canonical types, Jev/Laya wire and limits`.

### Task 3: The backend trait, the registry and the fake

**Files:** `crates/loams-systemone/src/{backend,registry}.rs`, `src/backends/{mod,fake}.rs`, `tests/registry.rs`.

**Produces:** `Capabilities`, `BackendStatus`, `DecisionBackend`, `BackendRequest`, `RawDecision`, `BackendError`, `Registry::{register, list, decide}` exactly as design §5. `FakeBackend` is deterministic (probabilities from a hash of state and option key), configurable capabilities, injectable failures, latency and truncation.

**Semantics:** `Registry::decide` validates (Task 2), asks `route::plan` (stub returning the only ready backend until Task 4), calls the backend with a deadline (default 10 s, `LOAMS_SYSTEMONE_TIMEOUT_MS`), then applies the policy stage: strip `act_probability`, apply calibration if a table is loaded (Task 7), compute `calibrated_confidence`, mark `low_confidence` against `min_confidence`, fill `Provenance`. A backend error never includes request content.

**Tests:** `registry_lists_status_and_reason`; `policy_strips_act_probability`; `low_confidence_keeps_answer`; `backend_error_has_no_state`; `timeout_maps_to_unavailable`; `batch_preserves_order`; `uncalibrated_warning_when_no_table`.

**Commit:** `systemone: DecisionBackend, registry and the fake backend`.

### Task 4: Host detection and routing

**Files:** `crates/loams-systemone/src/{host,route}.rs`, `tests/route.rs`.

**Produces:** `HostProfile::detect()` (os, arch, `apple_silicon`, NVIDIA via NVML if loadable else `nvidia-smi -L` with a 2 s timeout, threads, RAM, Python, container, role) and `route::plan` (pure) with the selection order and rules of §6.2 and §6.3, `RoutingConfig { backend: Option<BackendId>, fallback: Option<BackendId>, allow_cloud: bool, allow_cloud_fallback: bool, on_truncate: OnTruncate }` read from `LOAMS_SYSTEMONE_BACKEND`, `LOAMS_SYSTEMONE_FALLBACK`, `LOAMS_SYSTEMONE_ALLOW_CLOUD`, `LOAMS_SYSTEMONE_ALLOW_CLOUD_FALLBACK`, `LOAMS_SYSTEMONE_ON_TRUNCATE`, and the matching config keys.

**Semantics:** exactly §6.3, in order. Language: `detect_lang(state, hint) -> {English, Other, Undecided}` using script analysis plus a small, permissively-licensed language detector chosen in this task (candidates `whatlang` MIT, `lingua` Apache-2.0; record the choice and the licence); `Undecided` routes multilingual. `plan` returns `NO_BACKEND` with the list of candidates and why each was excluded.

**Tests** (table-driven; each row is a host profile, request and expected plan): `apple_silicon_defaults_to_coreml`; `nvidia_defaults_to_torch_cuda`; `cpu_only_prefers_native_if_ready_else_torch`; `browser_role_uses_server_route`; `env_backend_beats_auto`; `request_backend_beats_env`; `over_32_options_excludes_coreml`; `over_100_options_excludes_torch_leaves_jev_if_allowed`; `over_255_options_is_413`; `score_over_10_levels_excludes_jev`; `no_backend_error_lists_each_exclusion`; `over_20_options_warns_and_widens_budget`; `over_20_prefers_jev_only_when_allowed`; `omitted_locality_never_selects_cloud`; `non_english_goes_multilingual`; `undecided_latin_goes_multilingual`; `english_only_backend_excluded_for_hindi`; `truncation_warns_by_default`; `on_truncate_widen_retries_once`; `cloud_requires_all_three_consents`; `forced_jev_with_local_only_fails_closed`; `fallback_respects_locality`; `fallback_only_on_retryable`; `degraded_backend_skipped_unless_only`; `gpu_host_with_cpu_device_is_degraded`; `host_detect_does_not_load_models` (no process spawned except the bounded `nvidia-smi`).

**Commit:** `systemone: host detection and routing`.

### Task 5: The `laya-serve` adapter and sidecar supervisor

**Files:** `crates/loams-systemone/src/{sidecar,backends/laya_serve}.rs`, `scripts/systemone/install_laya.sh`, `tests/sidecar.rs` (against a fake `laya-serve` implemented in the test with axum, obeying the documented limits, status codes and `/health` rules).

**Produces:** `LayaServeBackend { base: LoopbackUrl, key: Secret, transport: Tcp | Uds }`, `SidecarSupervisor::{ensure_installed, start, stop, health}`, `loams systemone install laya` logic (a function; the CLI wiring is Task 10).

**Semantics:**
- Install: `uv` pinned, Python 3.12, venv at `$LOAMS_HOME/systemone/venv`, `pip install --require-hashes -r requirements.lock` (generated and reviewed per release, committed under `scripts/systemone/`), CUDA vs CPU torch from `HostProfile`; only with explicit consent; a dry-run prints the plan and sizes.
- Start: environment as design §7.1 (`LAYA_HOST=127.0.0.1`, `LAYA_PORT`, `LAYA_API_KEY`, `LAYA_DEVICE`, `LAYA_PRELOAD=1`, `LAYA_MODELS`, `LAYA_THREADS` = physical cores, `LAYA_MAX_CONCURRENT=16`, `LAYA_REVISION`, `USE_TF=0`, `HF_HUB_OFFLINE=1` once cached); own process group; wait up to 120 s for `/health` to list the checkpoints; backoff 1 s to 30 s, five tries in ten minutes, then `Failed`.
- Requests: map the canonical request to `laya-serve` JSON, forwarding `lang`, `max_len`, `head_max_len` and `min_confidence` (we use our own gate but forward it so `low_confidence` stays consistent), reading `Server-Timing`. Map statuses: 422 to `Invalid`, 413 to `TooLarge`, 503 to `Unavailable{retry_after}`, 401 to `Internal` plus a restart (the key is ours; a 401 means a stale process), 500 to `Internal`. A response with `usage.truncated` becomes the `state_truncated` warning.
- `noul_mode`: native, or `choice_ab` (Task 8 sets it): the adapter sends a two-option choice with keys `A` (yes) and `B` (no) and returns `noul = P(A)`.
- The adapter refuses any non-loopback base URL at construction.

**Tests:** `non_loopback_is_refused`; `key_not_in_argv_or_logs` (spawn a probe child, scan argv, env of logs and tracing output); `child_group_killed_on_drop`; `health_without_key_shows_only_status`; `restart_backoff_then_failed`; `truncation_becomes_warning`; `status_codes_map` (422, 413, 503, 401, 500); `queue_depth_16_then_unavailable`; `widened_budget_forwarded`; `noul_choice_ab_roundtrip`; `device_cpu_on_cuda_host_is_degraded` (fake `/health` says cpu); `installer_dry_run_prints_sizes_and_changes_nothing`; and an `#[ignore]` `real_laya_serve_cpu_smoke` (needs the venv) that runs the Task 8 self-test.

**Commit:** `systemone: supervised laya-serve sidecar and adapter`.

### Task 6: The Jev adapter

**Files:** `crates/loams-systemone/src/backends/jev.rs`, `tests/jev.rs`, `tests/fixtures/jev_*.json`.

**Produces:** `JevBackend { base: Url /* default https://api.typesafe.ai */, key: Secret, model: String /* default "jev-latest" */ }` with a circuit breaker; locality `Cloud`; capabilities from docs.typesafe.ai (all three types native; 255 options per `choice`, 2 to 10 levels per `score`, `noul` criteria optional; 401, 422, 429 and 529 mapped).

**Semantics:** `POST {base}/v1/systemone` with `Authorization: Bearer`, body `{model, state, questions}` only; decode `answers`, `model`, `usage` (unknown fields kept in `extra`, Ruling 1). 429, 529 and 5xx: exponential backoff with jitter, at most 2 retries, honouring `Retry-After`; 401/403 disable the backend with a clear status; the request body is never logged. The base URL must be `https` unless the host is loopback; redirects are not followed with the key attached (a redirect to another host or scheme fails the request). The key comes from the secret store or `LOAMS_JEV_API_KEY`; absent key means `NotInstalled { how: "set a key" }`. Compiled in by default but **inert without a key and the instance switch** (Ruling 6).

**Tests (wiremock):** `sends_only_model_state_questions`; `bearer_header_set_and_key_not_logged`; `decodes_choice_score_noul` (shapes from docs.typesafe.ai: `noul` has no confidence); `unknown_fields_kept`; `retry_after_honoured`; `breaker_opens_after_failures_and_recovers`; `auth_error_disables_backend`; `no_key_is_not_installed`; `locality_is_cloud`; `body_never_in_error`; `non_https_remote_base_refused`; `loopback_http_base_allowed`; `redirect_to_other_host_does_not_forward_key`; `jev_outputs_never_enter_decision_log_labels`.

**Commit:** `systemone: Jev cloud adapter (off by default)`.

### Task 7: Calibration

**Files:** `crates/loams-systemone/src/calibrate.rs`, `tests/calibrate.rs`.

**Produces:**

```rust
pub struct CalibrationTable { pub id: String, pub revision: String, pub entries: Vec<CalEntry> }
pub struct CalEntry { pub checkpoint: String, pub ty: QuestionType, pub bucket: OptionBucket, pub temperature: f64, pub n: u32, pub ece_before: f64, pub ece_after: f64 }
pub enum OptionBucket { B2, B3_4, B5_8, B9_20, B21_32, B33Plus }
pub fn fit(rows: &[LabelledDecision]) -> Result<CalibrationTable, CalError>;       // NLL by 1-D bounded search
pub fn apply(table: &CalibrationTable, ans: &mut Answer, ctx: &CalContext);        // p_i ∝ p_i^(1/T)
pub fn ece(rows: &[(f64 /* confidence */, bool /* correct */)], bins: usize) -> f64;
```

**Semantics:** needs at least 100 labelled decisions per bucket else that bucket is `uncalibrated` (reported, not fitted); tables are written to `$LOAMS_HOME/systemone/calibration/cal-<date>-<hash>.json` and loaded by revision; applying a table for a different revision is refused. For `score`, calibrate the distribution over levels and recompute the expected value from the calibrated probabilities. For `noul`, a two-class temperature. `calibrated_confidence = max(p)` (`max(p, 1-p)` for `noul`). Compare `confidence` and `answer_confidence` as gating signals by AUROC on the labelled set and write the result in the report (Q527).

**Tests:** `fit_reduces_ece_on_synthetic_overconfident_data` (generated with a known temperature of 2.5; recovered within 10%); `buckets_under_100_are_uncalibrated`; `table_for_other_revision_refused`; `apply_preserves_argmax`; `score_expectation_recomputed`; `noul_two_class`; `ece_known_value`; `auroc_known_value`; `uncalibrated_answers_carry_warning`.

**Commit:** `systemone: temperature calibration per type and option count`.

### Task 8: The self-test, gating and the `noul` quirk

**Files:** `crates/loams-systemone/src/selftest.rs`, `selftest_cases.json`, `tests/selftest.rs`.

**Produces:** the 16 cases of design §9.2 (ids `s01`..`s16`), `run(backend) -> SelfTestReport`, `Registry` wiring (run after `start` and on revision change; `Starting` until it passes), the `noul` stuck-label check that flips `noul_mode` to `choice_ab` per checkpoint, and `min_confidence` handling.

**Semantics:** each case declares `requires` (for example `min_options: 33` on the 40-option case), and a backend that cannot serve it reports `not_applicable`, which is neither pass nor fail; the empty-options case is a validation test (Task 2), not a backend case. Structure always (probabilities sum to 1 within 1e-3, `output_tokens == 0`, option sets equal the request's); argmax only for cases carrying `expect: "strict"`, which Task 0's measurements populate (an allowlist, with `informational` for the rest); the 40-option case is informational and reports the widened-budget outcome (Q528); the Hindi and German cases assert the multilingual route; the truncation case asserts the warning. A failure makes the backend `Degraded` with the case id in the reason.

**Tests:** `fake_backend_passes`; `structure_violation_degrades`; `strict_case_failure_degrades_informational_does_not`; `stuck_noul_switches_to_choice_ab_with_warning`; `non_english_cases_route_multilingual`; `truncation_case_warns`; `report_contains_no_state` (case ids only); `selftest_runs_under_two_seconds_on_fake`; `coreml_like_backend_skips_40_option_case_as_not_applicable`.

**Commit:** `systemone: self-test, noul workaround and gating`.

### Task 9: The model manager

**Files:** `crates/loams-systemone/src/models/{mod,lock,download,cache}.rs`, `models.lock.toml`, `tests/models.rs` (a local HTTP server with range support).

**Produces:** `ModelLock` (artefacts: source repo, commit SHA, files with SHA-256 and size, licence, notice, mirror URLs), `ModelManager::{status, plan, pull, verify, remove, list}`, the cache layout of design §9.5. The initial lock covers the Laya checkpoints used by `laya-serve` (English, multilingual, typed-decisions) and, as inactive entries, the Core ML buckets for SO2.

**Semantics:** consent callback before any byte is fetched (the CLI prompts; the desktop shows a dialog); resumable ranges to `*.part`; SHA-256 verified before the atomic rename and before any parser sees the file; read-only after; mirror fallback in order; `LOAMS_OFFLINE=1` never downloads; revisions are immutable; `remove` only on request; the lock is the only source of URLs (no URL from a response is followed).

**Tests:** `pull_resumes_after_interrupt`; `digest_mismatch_deletes_part`; `verify_before_rename`; `offline_never_fetches`; `mirror_used_when_primary_fails`; `consent_denied_downloads_nothing`; `revision_directory_immutable`; `lock_parses_and_urls_are_https`; `disk_full_leaves_no_partial_revision`; `list_shows_sizes_and_licences`.

**Commit:** `systemone: verified, resumable model cache`.

### Task 10: The server route, Connect service, CLI and MCP tool

**Files:** `crates/loams/src/systemone/{mod,service,http,config,cli}.rs`, `crates/loams/src/{server.rs,main.rs}`, `crates/loams/tests/systemone/{main.rs,http.rs,connect.rs,cli.rs}`.

**Produces:** the Connect `SystemOneService` and the axum route `POST /v1/systemone` (+ `POST /v1/systemone/batch`) on the gateway role, both over one `Registry`; `GET /v1/systemone/backends`; CLI `loams systemone {status, decide, selftest, install laya, calibrate, export, model list|pull|rm|add}` with `--output json`; and a read-only `systemone_decide` tool in the stdio MCP server (CLI1). Auth uses the normal gateway auth; the loopback desktop/dev listener allows a local key. Usage events: `io.loams.dev.systemone.decided.v1` (counts, backend, latency, locality; never content). Per-principal rate limit and per-org quota as configured (Q538, Q539: off by default).

**Semantics:** HTTP errors per §4.2; no internals in 500; `Server-Timing: inference;dur=<ms>` set. Enablement: in `loams dev` the route is on by default; on a server it is off unless started with `--systemone`, and stays off by default until Q539 is answered. `loams systemone status` prints the `HostProfile`, each backend's status with the one-line fix, the active route reason, and the calibration table id.

**Tests:** `http_decide_matches_laya_fixture_shape`; `jev_client_compat` (the TypeSafe request shape is accepted); `connect_decide_equals_http_decide`; `auth_required_on_gateway`; `no_state_in_spans_or_errors` (a tracing subscriber captures everything during a failing request); `batch_endpoint_ordering`; `cli_status_json_schema`; `cli_decide_stdin`; `mcp_tool_is_read_only_and_local_only` (refuses `ALLOW_CLOUD`); `usage_event_has_no_content`; `route_disabled_by_default_on_server`.

**Commit:** `systemone: serve the decision API, CLI verbs and MCP tool`.

### Task 11: The decision log and export (fine-tuning hooks)

**Files:** `crates/loams-systemone/src/declog.rs`, `crates/loams/src/systemone/cli.rs`, `tests/declog.rs`.

**Produces:** `DecisionLog` (JSONL by month in `$LOAMS_HOME/systemone/decisions/`), `loams systemone export --format laya --out dir` (typed-decisions JSONL plus a held-out split), `loams systemone label <decision-id> --choice …`, and `model add` (Task 9's lock accepts a custom entry with a digest).

**Semantics:** off by default (`systemone.log_decisions = off | local`); state stored only when `log_state = true`, else a salted hash; retention 90 days enforced on open; file mode 0600; labels are append-only rows; a label whose source is a Jev answer is refused (TypeSafe's agreement forbids using Output to train a model). Export refuses decisions without a stored state.

**Tests:** `off_by_default_writes_nothing`; `hash_only_without_log_state`; `retention_prunes`; `file_is_0600`; `label_rows_join_by_id`; `export_requires_state`; `export_format_matches_fixture` (a sample of upstream's typed-decisions format, copied with attribution); `custom_checkpoint_needs_digest`.

**Commit:** `systemone: opt-in decision log, export and custom checkpoints`.

### Task 12: The in-process Rust backend (only if Task 0 chose kevala or an approved `ort` port)

**Files:** `crates/loams-systemone/Cargo.toml` (feature `native`, off by default), `src/backends/native.rs`, `tests/native.rs`, `THIRD_PARTY_NOTICES.md`.

**Semantics:** wrap kevala's `Model` (pinned git commit) as `LayaNativeBackend`; models come from Task 9's cache; inference runs on a dedicated blocking pool with a bounded queue; capabilities report the checkpoints actually present; the self-test is the gate; the feature stays off by default until the owner decides (as with `pgwire`).

**Tests:** `native_matches_laya_serve_within_0_02` (`#[ignore]`, needs both); `native_passes_selftest`; `native_refuses_untrusted_torch_pickles`; `native_bounded_queue`; `cargo deny check` with the feature.

**Commit:** `systemone: in-process Laya backend behind a feature`.

### Task 13: Documentation and the exit gate

**Files:** `docs/design/40-loams-systemone.md` (as built), `docs/design/13-decision-log.md` (paste the pending log), `docs/plans/README.md`, `CHANGELOG.md`, `THIRD_PARTY_NOTICES.md`, a user page `docs/systemone.md`.

**Exit gate (all in CI or on the labelled runner):** Tasks 1 to 11 tests; `loams dev` serves `POST /v1/systemone` with the fake backend; on a runner with the venv, the real `laya-serve` CPU smoke passes the self-test; the Jev adapter passes against wiremock; `cargo deny check`; the licence table of design §2.1 matches `THIRD_PARTY_NOTICES.md`; a chaos run (kill the sidecar mid-request, corrupt a downloaded file, forbid cloud) passes.

**Commit:** `docs: document SystemOne and close SO1`.

## Owner rulings 2026-10-18 (on Task 0's questions)

The owner accepted all five recommendations of Task 0. They are binding for Tasks 1 to 13; the decision log and design §40 carry them.

| # | Question | Decision | Where recorded |
|---|---|---|---|
| R0.8 | The in-process backend | Upstream's **`laya-onnx`** (official Rust port on `ort`), behind an off-by-default `native` feature, enabled only once the parity, latency and `cargo deny` gates are measured. kevala is not used | D527 amended, Q523 answered; §40 §1, §7.2. Task 12 runs on this basis |
| R0.14 | `POST /v1/systemone` against D602 | A **D603 compatibility surface**, served in v1 **only on the loopback listener** (desktop and `loams dev`); servers expose `loams.systemone.v1` over Connect only | §40 §4.2 "Listener". Task 10 mounts the HTTP route on the loopback listener only |
| R0.5a | 101 to 255 options on `laya-serve` | **Tournament mode** (`LAYA_TOURNAMENT_GROUP`) as a capability with a `choice_by_tournament` warning, **after Task 8 measures it** | D525 amended; §40 §4.2, §6.3. Task 4 adds the capability; Task 8 measures before it is on |
| R0.6 | `noul` criteria wire shape | An optional **object with optional `true` and `false`** descriptions, forwarded to Jev and Laya; any other key is a `422`; `noul_criteria_ignored` is dropped | §40 §4.2. Task 2's two `noul` criteria tests change accordingly |
| R0.20 | D538 (desktop integration) | Rewritten so **`loams-agentd` hosts SystemOne** on the desktop | D538's wording lands with the SO2 re-plan, [#770](https://github.com/ostrium-labs/loams/pull/770); §40 §8 is marked superseded |

## Rulings made during execution

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| R0.1 | **PRs target `dev`, not `main`.** The status line is corrected | `dev` is the integration branch now | None; affects every task's PR |
| R0.2 | **The build machine changed.** `/mnt/Projects` does not exist; the machine has 15 GB of RAM, so cargo runs with `-j 2` (not `CARGO_BUILD_JOBS=4`) and a spike's target dir is deleted afterwards. Throwaway Python environments go under `/home/keshav/Ostrium/so1-spike` (outside the repository, never `/tmp`). The download rate measured about 100 KB/s | The global constraint's paths and job count were for another machine | Slow builds; affects Tasks 0, 5, 9, 12 |
| R0.3 | **The `laya-serve` CPU measurements, the kevala build and the `ort` `load-dynamic` check were not run.** The executing agent's permission policy blocked installing and running third-party code, and the bandwidth made the 1.7 GB of wheels and weights hours of download. The exact commands are in the spike doc §6 for the labelled runner. Verdicts below rest on upstream's published figures and on reading source | Not a judgement call this task could make alone | Tasks 5, 8 and 12 start without our own latency, memory and parity numbers (see R0.24) |
| R0.4 | **Pin `laya[serve]==0.4.11`** (tag `v0.4.11` = `d74e8be4…`, 2026-10-17) instead of 0.3.23, and the weights at Hugging Face revision `7b928d82…` (2026-10-03). Sizes for Task 9's lock: English 843 MB, multilingual 644 MB plus a 34 MB tokenizer, typed-decisions 843 MB; Core ML buckets about 450 MB (`e8`) and 645 MB (`fp16`) each at `7b8d7a2b…` | 0.3.23 is twelve releases old; the HTTP features below need 0.4 | A regression in 0.4.x; Task 5's `real_laya_serve_cpu_smoke` catches it. Affects Tasks 5, 9 |
| R0.5 | **Absorb `laya-serve` 0.4's additions** (spike §2). Task 5: set `LAYA_MAX_LOADED` to the number of checkpoints in `LAYA_MODELS` (its default 2 would rebuild a third on every switch); pass our lock's digests as `LAYA_SHA256_DIGESTS` beside `LAYA_REVISION`; never set `LAYA_JEV_STRICT` (it drops `usage.truncated`); never forward `model` (a path is a `422`); read `checkpoint_devices` and `cpu_fallbacks` from `/health` for `Degraded`; honour `Retry-After: 1` on `503`; cap forwarded `max_len`/`head_max_len` at the sidecar's `LAYA_MAX_TOKEN_BUDGET` (8192). Task 2: `x_jev_confidence`, `abstention`, `abstention_threshold` and `x_tournament` are Laya extensions, dropped like `routing` (our `low_confidence` and `loams` block replace them); a numeric `min_confidence` only (the per-bucket map is not in our surface). Task 4: see owner question R0.5a | Upstream added flags and fields since the design was verified; limits are unchanged | Missed truncation or wrong `Degraded` state; covered by Task 5's fake-sidecar tests |
| R0.5a | **Owner question: let `laya-serve` serve 101–255 options by elimination.** With `LAYA_TOURNAMENT_GROUP=16` upstream answers up to 255 options (CLINC150 at 86.7%, 50-option subsets at 94.0% against 82.7% in one pass, upstream figures). That would change D525's "over 100 options excludes `laya-serve`" and answer much of Q528. **Recommendation:** yes, as a capability (`max_options_per_choice = 255` when the sidecar runs with the flag), with a `choice_by_tournament` warning, after Task 8 measures it on our cases | It changes a design decision's routing rule | Without it, 101–255 options on a local-only request stay `NO_BACKEND` |
| R0.6 | **`noul` criteria and the stuck-label workaround.** Upstream closed issue 156 for the API half: `noul` `criteria` is an object keyed only `true`/`false`, and a `labels` override renames the option words. The checkpoint bias stays. Third-party figures: one call with neutral `labels` fixes the English `laya` checkpoint (81.2% against 72.1% at p ≥ 0.9), while the two-order `choice` stays best on `typed-decisions`. Task 8 keeps `choice_ab` as the workaround and measures `labels` as a candidate third `noul_mode`, adopted per checkpoint only on its numbers. §4.2's `noul_criteria_ignored` is wrong, because Laya reads the criteria. **Owner question (public wire):** accept `noul` `criteria` as `{"true": …, "false": …}`, the shape both Jev and Laya use, instead of the "optional string" of D521/§4.2, and forward it to both backends. **Recommendation:** yes | The design read 156 as fully open and Jev's `noul` criteria as a string; both are corrected upstream | A Jev client sending the object form gets a `422`. Affects Tasks 2 (`noul_criteria_optional_forwarded_to_jev`, `noul_has_no_criteria_and_extra_criteria_is_422`), 5, 6, 8 |
| R0.7 | **Jev's documented contract moved** (spike §3): `model` is required, the body cap is 32 MiB, nesting is capped at 64, `400 max_tokens_exceeded` and `415` exist, and `choice` criteria values may be null, objects or arrays. Task 6 always sends `model` (default `jev-latest`) and maps `400 max_tokens_exceeded` to `TooLarge`. `MAX_BODY` stays 2 MiB, D521's number, so a Jev client over 2 MiB gets a `413`, a divergence §4.2 lists | Re-read on the day | A Jev client with a large body is refused here; documented |
| R0.8 | **Owner question: the in-process backend is upstream's `laya-rust` (`laya-onnx`), not kevala and not our own `ort` port.** The official repository now ships a Rust port of the inference path on `ort` 2.0.0-rc.13 and `tokenizers` 0.23, golden-tested against Python, covering all three checkpoints (spike §4.2). That removes D527's reason for asking (a multi-week port that we would maintain) but still means an RC `ort`, a C++ runtime and an ONNX export step. Not yet on crates.io; abstention and per-language temperatures are not ported (our policy layer does abstention anyway). **Recommendation:** amend D527 and answer Q523 yes: Task 12 wraps `laya-onnx`, pinned by git revision until it is published, behind the off-by-default `native` feature, with `ort`'s `load-dynamic` and an ONNX Runtime library pinned by digest in the model lock (no build-time download), and the gates of D527 (parity under 0.02 with `laya-serve`, p50 under 150 ms, `cargo deny`) measured on the runner first | It changes a design decision | If declined, the verdict is **none** and CPU users run the sidecar (D527 step 3). Affects Task 12 |
| R0.9 | **kevala fails D527's gates** on published evidence: no mmBERT or multilingual code and only an English pack (`laya-q8.kevala`, 479 MB); its own int8 parity figure is 0.024, over the 0.02 gate. `torchpt.rs` is used only by the Kev converter, not on the Laya path. It has no dependencies but would need an `allow-git` entry in `deny.toml`, and its scope now includes Kev, SemIf, H2O, Clef and Gemma 4. kevala stays a candidate only for the opt-in browser plugin (D530) | Verified 2026-10-18 at `e1a3b662` | None for SO1; Task 12 does not use it |
| R0.10 | **`ort` is `2.0.0-rc.14`** (2026-10-12), still a release candidate; `laya-onnx` pins rc.13. `load-dynamic` is untested here (R0.3) | Recorded for Task 12 | Task 12's first step tests it |
| R0.11 | **`FluidUse` cannot be taken for `LayaManager` alone**: the product builds `FluidAudio` and about twenty model families. `Sources/FluidUse/Laya/` is five files importing only Foundation, Darwin, `os` and Core ML, so vendoring them with the notice (Q525) looks practical | For SO2 Task 0 | SO2 re-checks on a Mac |
| R0.12 | **Licences unchanged** (spike §1); `nvkudva/laya-web` still has no licence. Pins are recorded per item | Re-verified on the day | None |
| R0.13 | **Workspace versions:** `axum = "0.8"` (0.8.9), `async-trait = "0.1"` (0.1.92), `reqwest = "0.13"` with default features off (Task 6 adds `rustls` explicitly), `indexmap` 2 with `serde`, `proptest` 1, `sha2` 0.11, `thiserror` 2, `secrecy` 0.10 (in the lock), `keyring = "=4.2.0"`. **`serde_json` already has `preserve_order` workspace-wide**, so plan ruling 3 needs no feature change. `whatlang` 0.16.4 (MIT) is already in the lock through `charabia`, which makes it Task 4's cheapest choice. `wiremock` is not in the workspace: Task 6 adds it as a dev-dependency and runs `cargo deny` | Checked in `Cargo.toml` and `Cargo.lock` at `fa85a8c6` | None |
| R0.14 | **Owner question: `POST /v1/systemone` against API2.** D602 (API2, Q600) removed "any other hand-written JSON/HTTP application endpoint", and D601 puts REST-shaped compatibility surfaces on their own ports. D521's route was decided the same day and is not in D603's list. **Recommendation:** list `POST /v1/systemone` (and `/batch`) as a D603 compatibility surface (its reason is a drop-in for Jev and `laya-serve` clients), served only on the loopback dev/desktop listener in v1; on servers the API is `loams.systemone.v1` over Connect until Q539 is answered | It changes a design decision and the public API | Building the route against D602 would be reverted. Affects Tasks 2 (the wire stays: the sidecar adapter needs it anyway) and 10 |
| R0.15 | **Tasks 1 and 10 follow API2's package rules:** the proto is added to `crates/loams-proto/build.rs`, a `CATALOGUE` row goes in `crates/loams/src/api/connect.rs` (with `available` flipped by Task 10), handlers live in `crates/loams/src/api/connect_systemone.rs` (not `crates/loams/src/systemone/service.rs`), the package carries `unstable: true` and a `buf.yaml` `breaking.ignore` line until SO1 closes, facade annotations and fixtures are added, and error reasons go in `docs/api/reasons.md`. §44 §7.2 already lists `loams.systemone`, so `crates/loams/tests/route_map.rs` accepts the row | API2 plan, "Not in this plan" line, and `connect.rs`'s catalogue rule | A missing piece fails API2's Task 13 tests |
| R0.16 | **Auth is the main router's `Authenticator` layer** (API2 Task 6: `ApiKeyAuthenticator`, `DevLoopbackAuthenticator`); handlers read the `Principal` and parse no header. "The gateway role" in Task 10 means `roles.gateway` in `crates/loams/src/server.rs`, not the `loams-gateway` crate (§57's protocol gateway) | The plan predates API2's auth layer | Affects Task 10 |
| R0.17 | **CLI1 has not landed:** there is no `crates/loams-cli`, and `loams` has only server subcommands. Task 10 adds `loams systemone …` as a subcommand of `crates/loams/src/main.rs`, implemented in `crates/loams/src/systemone/cli.rs`, with `--output json` local to it; it moves under `loams_cli::ClientCommand` when CLI1 lands | The plan assumed CLI1's command tree | A second move later; small |
| R0.18 | **No stdio MCP server exists in `loams`** (API2 says so too). The only MCP server is `crates/loams-agentd-mcp` (the desktop daemon's, D788). Task 10 ships the read-only `systemone_decide` tool as a definition and handler in `loams-systemone` (local only, refuses `ALLOW_CLOUD`) with its test, and registration waits for a host: CLI1's `mcp serve` or the daemon (SO2) | No host to register in | The tool is not reachable from an MCP client until a host lands |
| R0.19 | **`$LOAMS_HOME` is not implemented anywhere** (CLI1's `home.rs` is absent). `loams-systemone` resolves it as D284 defines it: `LOAMS_HOME`, else `~/.loams`, created 0700. On the desktop the daemon's data directory is `<userData>/agentd` (D780); where models live there is SO2's choice | D284 | Two homes on a desktop until SO2 decides |
| R0.20 | **Owner question: D538 is stale.** The desktop is Electron (D652, the zeron fork is paused) with the per-user daemon `loams-agentd` (D780): supervision is in the daemon (D790), secrets are in its OS keyring (D794), MCP is the daemon's shim (D788), and `ostrium-labs/loams-desktop` is gone. **Recommendation:** rewrite D538 so `loams-agentd` hosts SystemOne on the desktop (links `loams-systemone`, supervises the sidecar, keeps the Jev key in its keyring, registers `systemone_decide` in `loams-agentd-mcp`), with the Decisions settings as a console plugin in the Electron shell; SO2 is re-planned on that basis. D653 already keeps SystemOne out of desktop v0.1 | It changes a design decision | SO2 as written cannot be executed. SO1 is unaffected beyond keeping `loams-systemone` free of server dependencies |
| R0.21 | **`crates/loams-link` is the stream link framework** (exactly-once apply into targets), not a desktop client; the plan's "`crates/loams-link` is untouched (SO2)" refers to the old desktop repository's crate. The desktop's client crate is now `crates/loams-agentd-link` | Name collision across repositories | None for SO1 |
| R0.22 | **There is no `THIRD_PARTY_NOTICES.md`.** Code copied from Apache-2.0 projects (fixtures from Laya's docs, any `laya-onnx` or FluidUse files) is recorded in `NOTICE`; programs Loams runs as separate processes (`laya-serve`) are listed in `LICENSES.md`. Tasks 2, 5, 12 and 13 use those two files | The repository's existing convention | None |
| R0.23 | **A new consumer: the jailer's overseer** uses SystemOne as stage 1, locally, with a p95 target under 300 ms (D1100, §61 §4.7). No SO1 change, but Task 4's routing and Task 10's latency figures should report against it | Design §61 landed after the plan | None |
| R0.24 | **Task 8 starts with every argmax case `informational`.** Its `strict` allowlist was to come from Task 0's measurements, which were not run (R0.3); the allowlist is filled when the runner measures `laya-serve` | Following R0.3 | A wrong argmax passes the self-test until the allowlist exists |
