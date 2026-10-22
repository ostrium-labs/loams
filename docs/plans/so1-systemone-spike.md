# SO1 Task 0: spike results and reconciliation

Plan: [SO1](2026-10-02-so1-systemone.md), Task 0. Design: [§40](../design/40-loams-systemone.md), D520–D539. Run on **2026-10-18** against `dev` at `fa85a8c6`. The rulings that come out of this document are in the plan's "Rulings made during execution" table (R0.1 to R0.24); this page holds the evidence.

## Verdicts

| Gate | Verdict |
|---|---|
| Licences (§2.1) | **Pass.** Every row is unchanged; `nvkudva/laya-web` still has no licence (R0.12) |
| Laya HTTP contract (§4.2) | **Pass, with changes to absorb.** Limits are unchanged; upstream moved from 0.3.23 to 0.4.11 and added fields, flags and env vars (R0.5, R0.6) |
| `laya-serve` CPU measurements | **Not run.** The executing agent's permission policy blocked running third-party code (installing and starting `laya-serve`, building `kevala-cli`) on the build machine, and the machine's download rate was about 100 KB/s (R0.3). Commands are below, for the labelled runner |
| kevala | **Fails** on published evidence: no multilingual Laya support, and its own int8 parity figure (0.024) is above the 0.02 gate (R0.9) |
| `ort` | `2.0.0-rc.14` (2026-10-12), still a release candidate. `load-dynamic` was not tried (no third-party builds, R0.3, R0.10) |
| New: official `laya-rust` (`laya-onnx`) | **The candidate.** Upstream now ships a Rust port on `ort` and `tokenizers` that covers all three checkpoints, so the port D527 priced is already written. Choosing it changes D527: **owner question** (R0.8) |
| In-process backend | **ort-with-approval**, through upstream `laya-onnx`, not our own port. The owner approved it on 2026-10-18 (D527 amended); Task 12 waits for the parity and latency gates to be measured on the runner |
| `FluidUse` as a SwiftPM dependency alone | **No.** The `FluidUse` product pulls `FluidAudio` and about twenty model families. `LayaManager` is five self-contained files, so vendoring them (Q525) looks feasible. Feeds SO2 Task 0 (R0.11) |
| `axum` and `async-trait` | **Match** the workspace: `axum = "0.8"` (0.8.9 locked), `async-trait = "0.1"` (0.1.92) (R0.13) |
| Decision numbers | D520–D539 and Q520–Q539 are in the log, all Proposed, with no collision. The highest numbers in use are D1106 and Q948, so new decisions from the owner questions take the next ones |

## 1. Licences and pins (re-verified 2026-10-18)

Commands: `gh api repos/<owner>/<repo>` (licence, push date) and `gh api repos/<owner>/<repo>/commits/HEAD` (SHA); `curl https://huggingface.co/api/models/<id>?blobs=true` (revision, card licence, file sizes); `curl https://pypi.org/pypi/laya/json`.

| Item | Licence | Pin recorded | Notes |
|---|---|---|---|
| `NandhaKishorM/laya` (code, includes `laya-ts/` and the new `laya-rust/`) | Apache-2.0 | HEAD `d74e8be46ef5241e4ba2b16cd25e896f24f71373` = tag `v0.4.11` (2026-10-17). The plan's pin `v0.3.23` is `d8a2e59781ca135169a36095056132e273cd9938` | PyPI `laya` 0.4.11, `requires_python >=3.10`, licence Apache-2.0 |
| `convaiinnovations/laya` (weights) | Apache-2.0, tag `commercial-use` | revision `7b928d828b7b0e022f929d9bd2e44165aa270148` (2026-10-03) | `model.safetensors` 843 MB (English), `multilingual/model.safetensors` 644 MB plus a 34 MB tokenizer, `typed-decisions/model.safetensors` 843 MB; 2 373 MB in all |
| `FluidInference/laya-coreml` | Apache-2.0 | revision `7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc` (2026-09-22) | Eight buckets: `e8` about 450 MB and `fp16` about 645 MB each, L128/256/512/1024, 32 options; 4 420 MB in all |
| `FluidInference/FluidUse` | Apache-2.0 | `cfba4d3fa4b996d0b680549416caf7aba6be770c` (2026-10-14) | See §5 |
| `bvolpato/kevala` | Apache-2.0 | `e1a3b662364b951df757f6fbb6884612bc3543ee` (2026-10-17); source tarball SHA-256 `ab5c3f1f0a7634c08b4b368cc579116f64522c93670426555106bc245f4d09f7` | Still 0.1.3, not on crates.io, 9 stars |
| `bvolpato/kevala-packs` | Apache-2.0 | revision `712955050042ee5980ba3c37e2da53d79c51d9fe` (2026-10-13) | `laya-q8.kevala` 479 MB; no multilingual Laya pack |
| `mizorewww/laya-mlx` / `aac6fef/laya-mlx` | Apache-2.0 / Apache-2.0 | `9f00d592fcf843753e6d3edd2eab63ec2b44f3c9` / `20aed815fc6acde75733882e7ec0e3f28aeb9717` | Unchanged |
| `vishalmysore/layaForWeb` | Apache-2.0 | `6e67de8d8ccaf9474c9150c3f3877878d915020b` | Unchanged |
| `nvkudva/laya-web` | **None.** No `LICENSE` in the tree, and GitHub reports no licence | `b5571e05dab3b7258bb98b80bfbf7eb90943c1b5` | Still not used |

## 2. Laya's HTTP contract against §4.2 (`docs/http-api.md`, `docs/security.md` at `v0.4.11`)

**Unchanged:** the path; the request fields `state`, `questions`, `model`, `task`, `lang`, `lang_guess`, `max_len`, `head_max_len`, `min_confidence`; the five hook arguments are a `422`; the limits (2 MiB enforced while streaming, 50 000 characters measured as §4.2 says, 64 questions, 100 options per `choice`, 32 levels per `score`, 512 options per request, 16 concurrent with a `503`); `/health` without the key is `{"status":"ok"}`; `Server-Timing: inference;dur=<ms>`.

**New or changed since 0.3.23:**

| Change | Where it matters |
|---|---|
| `LAYA_TOURNAMENT_GROUP` (2 to 100; 16 measured): a `choice` with more options is answered by elimination in groups, and the option cap becomes **255** on both routes. Adds `x_tournament` (`finalists`, `rounds`, `option_order_dropped`). Upstream measured CLINC150's 150 intents at 86.7% and 50-option subsets at 94.0% against 82.7% for one pass | D525's "over 100 options excludes `laya-serve`" and Q528's coarse-to-fine choice (owner question R0.5a) |
| `LAYA_JEV_STRICT`: projects the response onto Jev's contract and drops `usage`'s truncation facts | We must not set it: `usage.truncated` feeds `state_truncated` |
| `LAYA_MAX_LOADED` (default **2**), `LAYA_IDLE_UNLOAD_SECONDS`, `LAYA_MAX_TOKEN_BUDGET` (8192; a larger `max_len`/`head_max_len` is a `422`), `LAYA_DEFAULT_MODEL` (default `multilingual`), `LAYA_EXTRA_MODELS`, `LAYA_MAX_BATCH_TOKENS` (131 072; batches split, not refused), `LAYA_AUTO_TASK`, `LAYA_SHA256_DIGESTS` | Task 5's environment (R0.5) |
| Answers add `x_jev_confidence` on `choice` and `score` (Jev's formula, computed by Laya), and, when `min_confidence` is set, `abstention` (`passed`, `abstained` or `unevaluated`) and `abstention_threshold`. `min_confidence` may be a per-bucket map (`{"choice:3-5": 0.9, "default": 0.4}`) | Task 2's rendering, Task 5's mapping (R0.5) |
| `model` holding a path or an unpublished Hub id is a `422`; a Jev id still means "router chooses" | The adapter never forwards `model` (R0.5) |
| `/health` adds `revisions`, `device_is_preference`, `checkpoint_devices`, `cpu_fallbacks` (and the idle fields) | `Degraded` detection becomes exact (R0.5) |
| Batch: at most 64 `states` | Task 10's batch route |
| `X-Inference-Time-Ms` beside `Server-Timing` | None |
| `noul` `criteria` is an object keyed only `true`/`false` (either or both); a `labels` override renames the option words (upstream #163, #249). Issue 156 was closed on 2026-09-23 for the API half; the maintainer says the English-checkpoint bias needs a retrained checkpoint | D522's workaround and §4.2's `noul_criteria_ignored` (R0.6) |
| Issue 185 (`act_probability` always 1.0) is still open | No change: never exposed |

Third-party figures on #156 (laya 0.3.20, 240 labelled pairs; not ours): on `laya` (English), `noul` with neutral `labels` in one call reaches 81.2% at p ≥ 0.9 against 72.1% for plain `noul` and 80.8% for the two-order `choice`, at the cost of one call; on `laya-typed-decisions` the two-order `choice` is best (73.3%) and one-call `labels` is worse than the default. Calibration (ECE) is unaffected by any framing.

## 3. Jev's public API (`docs.typesafe.ai/api`, read 2026-10-18)

Unchanged: `POST /v1/systemone`, bearer auth, 255 options per `choice`, 2 to 10 `score` levels, `noul` with no confidence, 401/422/429/529. **New or corrected against §2.1 and the plan:** `model` is **required** (examples: `jev-latest`, `jev-1.13.0`); the body cap is **32 MiB**; nesting is capped at 64 levels; `400 max_tokens_exceeded` and `415` exist; `noul` `criteria` is an object with optional `true` and `false` descriptions (not a string); `choice` criteria values may be a string, object, array or null. No numeric rate limits are published. (R0.7)

## 4. The in-process backend

### 4.1 kevala at `e1a3b662` (source read, not built)

- **Dependencies:** none (`crates/kevala/Cargo.toml` has an empty `[dependencies]`). `cargo deny` would pass on licences, but `deny.toml` sets `unknown-git = "deny"`, so a git dependency needs an `allow-git` entry.
- **Toolchain:** pins Rust 1.95.0, edition 2021, `rust-version = "1.85"`; release profile `lto = "fat"`, `codegen-units = 1`. 40 `unsafe` sites (SIMD kernels); our workspace's `unsafe_code = "forbid"` does not apply to a dependency.
- **Scope has grown:** the crate now carries Kev, SemIf, H2O, Clef and Gemma 4 families beside Laya, so a pinned dependency compiles all of them.
- **Multilingual:** no mmBERT code (`grep -ri 'mmbert\|multilingual' crates/kevala/src` finds nothing) and only `laya-q8.kevala` (English) is published. **Gate fails.**
- **Parity:** kevala's own README reports Laya int8 packs at max abs difference **0.024** against the `laya` SDK on 41 cases (f32 weights below 0.0001). The int8 pack is over our 0.02 gate on its author's numbers; an f32 pack would be about 1.7 GB.
- **`torchpt.rs`:** used only by `kev/convert.rs` (the Kev converter). The Laya path reads `model.safetensors` in `laya/convert.rs` and runs from `.kevala` packs, so no `torch.save` parser is on the Laya inference path.

### 4.2 Upstream `laya-rust` (crate `laya-onnx` 0.1.0, library name `laya`)

In `NandhaKishorM/laya/laya-rust/` at `v0.4.11`, Apache-2.0. A module-for-module port of the Python inference path (tokenisation, sequence building, calibration, answers), checked against golden outputs recorded from Python (`tests/golden`). It runs ONNX (opset 18, fused `model.onnx` plus `model.onnx.data`, or laya-ts's split `encoder.onnx` + `head.onnx`) on `ort = "2.0.0-rc.13"` (default features, which download a prebuilt ONNX Runtime at build time) with `tokenizers 0.23` (`onig`), `regex`, `unicode-general-category`, `serde_json` with `preserve_order`, and `ureq` behind a default `download` feature. All three checkpoints, multilingual by default. `LayaEngine` is `Send + Sync`, with the session behind a mutex. **Known gaps:** abstention (`min_confidence`), histogram calibration, `predict_long` and per-language temperatures are not ported. It is **not on crates.io** yet, although its README says it is. ONNX weights come from exporting the PyTorch checkpoints once with `tools/export_onnx.py`; no ONNX export is published on Hugging Face.

This is the "ort with our own port of the template and head" of D527, except that upstream wrote and maintains the port. D527's cost argument (a multi-week port, re-verified on every template change) no longer holds. See R0.8.

### 4.3 `ort`

crates.io: `2.0.0-rc.14` (2026-10-12), `2.0.0-rc.13` (2026-07-28); no stable 2.0. `tokenizers` stable is 0.23.2 (1.0.0-rc.2 exists).

## 5. `FluidUse`

`Package.swift` (tools 6.0, macOS 14) exports one library product, `FluidUse`, whose target depends on `FluidAudio` (`from: "0.15.8"`) and holds about twenty model families (Laya, Kev, Jev, GLiClass, Qwen, SigLIP2 and more) plus forty demo and check executables. A consumer of the product builds the whole `FluidUse` target and `FluidAudio`, though not the executables. `Sources/FluidUse/Laya/` is five files (`LayaManager`, `LayaModelStore`, `LayaSequenceBuilder`, `LayaTokenizer`, `LayaTypes`; about 57 KB) importing only `Foundation`, `Darwin`, `os` and Core ML. Vendoring those five files with their notice (Q525) looks practical; SO2 Task 0 confirms it on a Mac.

## 6. Commands for the measurements that were not run

Run on the labelled runner (or by the owner) and record the results here. Measured download rate on the build machine on 2026-10-18: about 98 KB/s (`curl -w '%{speed_download}'`), so the 190 MB CPU torch wheel plus 1.5 GB of English and multilingual weights is several hours.

```sh
# laya-serve on CPU (uv 0.13.1, Python 3.12)
uv venv --python 3.12 venv
uv pip install --python venv/bin/python --index-url https://download.pytorch.org/whl/cpu \
  --extra-index-url https://pypi.org/simple --index-strategy unsafe-best-match 'laya[serve]==0.4.11' torch
du -sh venv
LAYA_HOST=127.0.0.1 LAYA_PORT=18000 LAYA_API_KEY="$(openssl rand -hex 32)" LAYA_PRELOAD=1 \
  LAYA_MODELS=english,multilingual LAYA_MAX_LOADED=2 LAYA_THREADS=<physical cores> \
  LAYA_REVISION=7b928d828b7b0e022f929d9bd2e44165aa270148 venv/bin/laya-serve
# cold start: time to /health listing both checkpoints; RSS from /proc/<pid>/status;
# /health with the key before and after one request; p50/p95 over 200 requests each
# for 1, 5 and 10 questions on a short English and a short Hindi state.

# laya-onnx (replaces the kevala build if the owner approves R0.8)
python laya-rust/tools/export_onnx.py ...           # once per checkpoint, see laya-rust/MODELS.md
cargo build --release -p laya-sample -j 2           # in laya-rust/; then delete target/
# compare probabilities with laya-serve on the same 200 decisions: gate max abs diff < 0.02;
# native CPU p50 for a short state: gate < 150 ms; ORT_DYLIB_PATH with ort's load-dynamic feature.
```

## 7. Repository reconciliation

The code-side findings (branch target, API2, the desktop's move to Electron and `loams-agentd`, CLI1 not landed, `LOAMS_HOME`, notices files, workspace versions) are rulings R0.1, R0.2 and R0.13 to R0.23 in the plan; R0.24 is what Task 8 does while the measurements are missing.
