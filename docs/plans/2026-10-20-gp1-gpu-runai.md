# GP1 — GPU Workloads with KAI Scheduler and the Open Run:ai Stack (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Stub** (2026-10-20). Track GP, design [§67](../design/67-gpu-scheduling-runai.md) (D1228–D1250, Q1015–Q1029, **approved by the owner on 2026-10-20 with every default**). The owner approved the direction (integrate with NVIDIA's open-sourced Run:ai stack) and then the design the same day. This plan specifies **GP0** (docs, an example chart and a Compose override, no Loams code) task by task. GP1 (Jobs), GP2 (operator, queues, Meter, Workload Map), GP3 (agent jobs) and GP4 (cuVS, optional) are listed at milestone level and get full task specifications when their gates are met (D1247). **GP0 starts after v0.1.1 ships (Q1015).**

**Goal (GP0):**
- a reader with a Kubernetes cluster that runs KAI Scheduler (alone or as part of Run:ai) installs `deploy/helm/loams-gpu-workers`, gets an embedding and a rerank model server on GPU shares in the right KAI queue, and points Loams at them, following `docs/guides/gpu/`;
- the same chart works on plain Kubernetes (whole GPUs, NVIDIA or AMD) and with standalone HAMi;
- a Compose user adds `deploy/release/compose.gpu.yaml` (or the AMD variant) with the `gpu` profile;
- nothing in the chart, the Compose files or the docs defaults to, mirrors or bundles an image under NVIDIA's licence, and nothing is exposed outside the cluster or the compose network;
- all of it is tested in CI without a GPU: lint, golden renders, schema validation, policy checks with failing fixtures, a docs-snippet check, and a kind run with a fake GPU operator and KAI.

**Architecture** (§67 §6.1, §10): no crate changes. One example chart, two Compose override files, one docs section, four scripts under `scripts/gpu/`, one new workflow `.github/workflows/gpu-examples.yml`, and an extra step in `.github/workflows/release-compose.yml`. Model servers are third-party OpenAI-compatible servers (vLLM by default, Q1016) that `loams-llm` and the gateway's `llm` module call; Loams processes load no model (D1231).

**Tech stack:** Helm 3 (the version and SHA-256 already pinned in `.github/workflows/fabric.yml`), kubeconform, yq (mikefarah), kind (the version pinned in `fabric.yml`), KAI Scheduler's chart `oci://ghcr.io/kai-scheduler/kai-scheduler/kai-scheduler`, `run-ai/fake-gpu-operator`'s chart `oci://ghcr.io/run-ai/fake-gpu-operator/fake-gpu-operator` (MIT), Docker Compose v2, bash with `set -euo pipefail` and shellcheck.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/gp0-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on green). `git commit -s`. Tasks 1–6 may be one PR if the reviewer prefers; Tasks 7, 8 and 9 are separate.
- **No Rust build.** GP0 touches no crate. If a task finds it needs one, stop and record a ruling.
- **Pins (D1249).** Every chart, tool and image is pinned by version and checksum or digest, at a release at least 14 days old at the time of the PR, except security fixes. Pins live in one file, `deploy/helm/loams-gpu-workers/ci/pins.env`, read by every script and workflow step.
- **Licences (D1242).**
  - No default image in `values.yaml`: `image.repository` and `image.digest` are empty and required by `values.schema.json`.
  - No `nvcr.io` reference anywhere in `deploy/` except in docs prose and in `ci/bad/` fixtures that must fail.
  - Nothing is pushed to `ghcr.io/ostrium-labs`.
  - The KAI `Queue` CRD schema vendored for kubeconform carries `Copied from kai-scheduler/KAI-Scheduler@<tag>:<path>, Apache-2.0` and a `NOTICE` line.
- **Never public.** No `LoadBalancer`, `NodePort`, `Ingress`, `hostPort`, `hostNetwork` or `hostPath` in any render; no `ports:` on the Compose GPU services.
- **Restricted pods.** `runAsNonRoot: true`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`, `seccompProfile: RuntimeDefault`, `automountServiceAccountToken: false`. If the pinned model-server image cannot run as non-root, Task 0 records it and the chart sets `runAsUser` to the image's documented user instead; root is never the default.
- **Vendor neutrality (D1243).** Every rendering test has an AMD case beside the NVIDIA case where the mode allows one.
- **Trademarks.** Docs say "works with NVIDIA Run:ai" and "KAI Scheduler". Test `docs_trademark_wording` (Task 9) fails on "Loams Run:ai".
- **GitHub-hosted runners only;** no GPU runner. The kind job uses fake GPUs.

## Review Focus

1. **An NVIDIA-licensed image reaches a default, a mirror or the bundle.** Expected: never. Tests: `policy_no_nvcr_default`, `ci/bad/nvcr-default.yaml` fails, `compose_gpu_requires_image`.
2. **Something is reachable from outside.** Expected: never. Tests: `policy_no_public_exposure`, `ci/bad/nodeport.yaml` fails, `compose_gpu_has_no_ports`.
3. **KAI fields on a pod that KAI does not schedule, or a GPU share with no scheduler that can honour it.** Expected: refused at render. Tests: `schema_share_requires_kai_or_hami`, `schema_kai_share_requires_nvidia`, `policy_kai_fields_only_with_kai_scheduler`.
4. **A share in `nonMemoryEnforced` mode described as isolated.** Expected: the chart's NOTES and the docs say memory is not enforced. Test: `notes_warn_non_memory_enforced`.
5. **The docs drift from the chart.** Expected: every values snippet in the docs is a file the CI renders. Test: `docs_snippets_match_files`.
6. **The kind run passes without KAI having scheduled anything.** Expected: the test checks the binding scheduler and the queue, not only `Running`. Tests: `kind_kai_binds_pods_in_queue`, `kind_default_scheduler_whole_gpu`.

## GP0 — Docs, the example chart and the Compose override (Tasks 0–10)

### Task 0 — Rulings

Check and record, in "Rulings made during execution":
- the pins for `ci/pins.env`: KAI chart (v0.18.x LTS, the newest patch at least 14 days old), fake-gpu-operator chart (0.2.0, which KAI's own end-to-end script uses, or 0.2.1), kubeconform (version and SHA-256 of `kubeconform-linux-amd64.tar.gz`), yq (version and SHA-256), vLLM image tags and digests for CUDA and ROCm (`vllm/vllm-openai` and the ROCm image vLLM documents), at a release at least 14 days old;
- for that vLLM version: the flags that serve embeddings (`/v1/embeddings`) and rerank (`/v1/rerank` or `/score`), the port, the user the image runs as and the writable paths it needs (a model cache under `emptyDir`);
- KAI's sharing-mode setting at the pinned tag (`global.gpuSharingMode` with `NonMemoryEnforced`, `NvFractions`, `HamiCore`; and whether `global.gpuSharing=true` is still needed, as KAI's end-to-end script sets it), and the exact annotation keys of §67 §2.2 at that tag;
- the Kubernetes floor for kubeconform: the oldest minor in §65's support window (Q997);
- `loams-llm`'s provider configuration keys for an OpenAI-compatible embedder and reranker (MM1 Task 2, PR #843, merged or not) and the gateway `llm` module's route syntax (§57 §12.2), for the docs; if neither has merged, the docs show the planned keys and say so;
- standalone HAMi's resource names for NVIDIA memory shares and for AMD (its `examples/`), at its latest release at least 14 days old.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — Chart skeleton and values schema

Files:
- `deploy/helm/loams-gpu-workers/Chart.yaml` (`apiVersion: v2`, `name: loams-gpu-workers`, `type: application`, `version: 0.1.0`, `kubeVersion` from Task 0's floor, `annotations.licenses: Apache-2.0`, a description that says "example")
- `deploy/helm/loams-gpu-workers/.helmignore`
- `deploy/helm/loams-gpu-workers/values.yaml`: `servers: []` of `{ name, serves: embeddings|rerank|chat, image: { repository, digest }, args: [], port, resources, gpu: { vendor: nvidia|amd, count, share: { memory | fraction }, nodeSelector, tolerations } }`; `placement: { scheduler: default|kai|hami, kai: { queue, sharingMode: nonMemoryEnforced|nvFractions, priorityClass, queues: { create: false, parent, parentQuota, leafQuota } } }`; `loams: { namespaceSelector, podSelector }` (who may call the servers); `modelCache: { sizeLimit }`
- `deploy/helm/loams-gpu-workers/values.schema.json`: required `image.repository` and `image.digest` (`^sha256:[0-9a-f]{64}$`); enums above; `count` and `share` exclusive (`oneOf`); `share` only with `placement.scheduler` in `[kai, hami]`; with `kai`, `share` only with `vendor: nvidia`; `memory` as a Kubernetes quantity; `fraction` in `(0, 1)`
- `deploy/helm/loams-gpu-workers/templates/_helpers.tpl` (names, labels `app.kubernetes.io/*` and `loams.dev/gpu-server: <name>`)
- `deploy/helm/loams-gpu-workers/templates/serviceaccount.yaml` (`automountServiceAccountToken: false`)
- `deploy/helm/loams-gpu-workers/templates/deployment.yaml` (one per server; restricted security context; `emptyDir` model cache; readiness on `/health`)
- `deploy/helm/loams-gpu-workers/templates/service.yaml` (ClusterIP only)
- `deploy/helm/loams-gpu-workers/templates/networkpolicy.yaml` (default deny; ingress only from `loams.namespaceSelector`/`podSelector` on the server port; egress only DNS and, if set, `modelSource` CIDRs for downloads)
- `deploy/helm/loams-gpu-workers/templates/NOTES.txt` (how to point Loams at each Service; the `nonMemoryEnforced` warning)
- `deploy/helm/loams-gpu-workers/README.md` (values table; "example, not a supported release artifact until the operator's `LoamsGpuPool`", D1230; licence and trademark notes)

Tests (in `scripts/gpu/chart-check.sh`, Task 4 wires them): `lint_strict` (`helm lint --strict` with each `ci/values-*.yaml`), `schema_requires_image_digest`, `schema_share_requires_kai_or_hami`, `schema_kai_share_requires_nvidia`, `schema_count_share_exclusive` (each a `ci/bad/*.yaml` that `helm template` must refuse), `notes_warn_non_memory_enforced`.

### Task 2 — Placement modes

Files: `deploy/helm/loams-gpu-workers/templates/_placement.tpl`, used by `deployment.yaml`.

| `placement.scheduler` | GPU amount | Rendered |
|---|---|---|
| `default` | `count: n` | `resources.limits["nvidia.com/gpu" or "amd.com/gpu"]: n`, node selector, tolerations |
| `kai` | `count: n` | the above plus `schedulerName: kai-scheduler`, label `kai.scheduler/queue: <placement.kai.queue>`, `priorityClassName` if set |
| `kai` + `nonMemoryEnforced` | `share.memory` / `share.fraction` | `schedulerName`, queue label, annotation `gpu-memory: "<MiB>"` or `gpu-fraction: "<f>"`; no `nvidia.com/gpu` limit |
| `kai` + `nvFractions` | `share.memory` | `schedulerName`, queue label, annotations `nvidia.com/container.<container>.gpu-memory.request` and `.limit` set to the quantity |
| `hami` | `share.memory` | HAMi's resource names from Task 0 (`nvidia.com/gpu: 1` with `nvidia.com/gpumem: <MiB>`, or the AMD names) |

The exact keys are Task 0's; the table is the intent. Tests: `placement_default_nvidia_whole`, `placement_default_amd_whole`, `placement_kai_whole`, `placement_kai_non_memory_memory_share`, `placement_kai_non_memory_fraction_share`, `placement_kai_nvfractions_share`, `placement_hami_nvidia_share`, `placement_hami_amd_share` (golden renders, Task 4).

### Task 3 — Optional KAI queues

Files: `deploy/helm/loams-gpu-workers/templates/kai-queues.yaml`. When `placement.kai.queues.create: true` (default `false`), render a parent `Queue` (`scheduling.run.ai/v2`) named `placement.kai.queues.parent` with `parentQuota`, and a leaf named `placement.kai.queue` with `parentQueue` and `leafQuota` (`resources.gpu.{quota,limit,overQuotaWeight}`; CPU and memory −1 unless set). With `create: false`, nothing is rendered and the docs show how to use an existing leaf queue (or KAI's `default-queue`). The chart never renders a `Queue` it was not asked to create (§67 D1234's spirit).

Tests: `queues_not_rendered_by_default`, `queues_rendered_parent_and_leaf` (golden), `queues_leaf_points_at_parent`.

### Task 4 — Golden renders and the check script

Files:
- `deploy/helm/loams-gpu-workers/ci/pins.env`
- `deploy/helm/loams-gpu-workers/ci/values-default-nvidia.yaml`, `values-default-amd.yaml`, `values-kai-whole.yaml`, `values-kai-nonmemory.yaml`, `values-kai-nvfractions.yaml`, `values-kai-queues.yaml`, `values-hami-nvidia.yaml`, `values-hami-amd.yaml` (each with an embed and a rerank server, images by digest from `pins.env` values written in)
- `deploy/helm/loams-gpu-workers/ci/bad/` (one file per refused case: `missing-digest.yaml`, `share-default-scheduler.yaml`, `kai-share-amd.yaml`, `count-and-share.yaml`, `nvcr-default.yaml`, `nodeport.yaml`)
- `deploy/helm/loams-gpu-workers/tests/golden/<values name>.yaml`
- `scripts/gpu/chart-check.sh [--update]`: `helm lint --strict` per values file; `helm template gpu deploy/helm/loams-gpu-workers -n loams-gpu -f <values>` diffed against the golden file (`--update` rewrites); each `ci/bad/` file must fail at `helm template` or at the policy check, with the expected reason (`# expect: <substring>` on its first line)

Tests: the golden diffs above; `bad_fixtures_fail_for_their_reason`; shellcheck on the script.

### Task 5 — Policy checks

Files: `scripts/gpu/chart-policy.sh <rendered.yaml>` (yq from `pins.env`), called by `chart-check.sh` for every good and bad values file.

Checks, each a named test:
- `policy_images_by_digest`: every container and init container image contains `@sha256:`;
- `policy_no_nvcr_default`: `values.yaml` has no `nvcr.io` and no non-empty `image.repository`; no rendered image starts with `nvcr.io` unless the values file is under `ci/bad/`;
- `policy_no_public_exposure`: no Service of type `LoadBalancer` or `NodePort`, no `Ingress`, no `hostPort`, `hostNetwork`, `hostPID` or `hostPath`;
- `policy_restricted_pods`: the security-context fields of the Global Constraints;
- `policy_kai_fields_only_with_kai_scheduler`: `kai.scheduler/queue`, `gpu-memory`, `gpu-fraction` and `nvidia.com/container.*` keys appear only on pods with `schedulerName: kai-scheduler`, and every such pod has the queue label;
- `policy_networkpolicy_per_server`: each Deployment's pods are selected by a NetworkPolicy with `policyTypes: [Ingress, Egress]`.

### Task 6 — Schema validation

Files:
- `deploy/kai/schemas/queue_v2.json` and `deploy/kai/schemas/README.md` (provenance, how to regenerate)
- `scripts/gpu/kai-crd-schema.sh`: fetches `deployments/kai-scheduler/crds/scheduling.run.ai_queues.yaml` at the pinned tag over HTTPS from GitHub, checks its SHA-256 against `pins.env`, and writes the JSON schema in kubeconform's layout
- `chart-check.sh` gains: `kubeconform -strict -kubernetes-version <floor> -schema-location default -schema-location 'deploy/kai/schemas/{{ .ResourceKind }}_{{ .ResourceAPIVersion }}.json'` on every good render

Tests: `kubeconform_all_good_renders`, `queue_schema_matches_pinned_crd` (re-running the script produces no diff).

### Task 7 — Compose override

Files:
- `deploy/release/compose.gpu.yaml`: services `embed` and `rerank`, `profiles: [gpu]`, `image: ${LOAMS_GPU_EMBED_IMAGE:?…}` and `${LOAMS_GPU_RERANK_IMAGE:?…}`, `command` from `LOAMS_GPU_*_ARGS`, `deploy.resources.reservations.devices: [{ driver: nvidia, count: 1, capabilities: [gpu] }]` (or `device_ids` from `LOAMS_GPU_*_DEVICE`; exclusive with `count`), the compose network only, no `ports`, a named volume for the model cache, a healthcheck
- `deploy/release/compose.gpu-amd.yaml`: the same services with `devices: [/dev/kfd, /dev/dri]` and no reservation (the `group_add` question is Task 0's)
- `deploy/release/.env.example`: commented `LOAMS_GPU_*` entries
- `deploy/release/README.md`: a "GPU model servers (optional)" section linking `docs/guides/gpu/compose.md`
- `.github/workflows/release-compose.yml`: a step "compose config (GPU overrides)" that runs `docker compose --env-file .env.example -f compose.yaml -f compose.gpu.yaml --profile gpu config -q` and the AMD variant with dummy digest-pinned images in the step's `env`, and asserts with `jq` on `config --format json`

Tests: `compose_gpu_config_resolves`, `compose_gpu_amd_config_resolves`, `compose_gpu_requires_image` (unset image → `config` fails), `compose_gpu_reservation_shape` (driver `nvidia`, `count: 1`, `capabilities: [gpu]`), `compose_gpu_amd_devices` (`/dev/kfd`, `/dev/dri`), `compose_gpu_has_no_ports`, `compose_default_has_no_gpu_services` (no `gpu` profile → no `embed` or `rerank`).

### Task 8 — kind run with fake GPUs and KAI

Files:
- `scripts/gpu/kind-smoke.sh`: create a kind cluster (two workers), install fake-gpu-operator from `pins.env` with values that give each worker one fake GPU with 24 GiB, install KAI from `pins.env` (`-n kai-scheduler`, sharing on per Task 0), create a parent and a leaf queue with this chart's `values-kai-queues.yaml` adapted, install the chart three times with a tiny digest-pinned placeholder image (`registry.k8s.io/pause`, Task 0 pins it) in place of the model server, then assert, and always delete the cluster
- `deploy/helm/loams-gpu-workers/ci/kind/values-kai-share.yaml`, `values-kai-whole.yaml`, `values-default-whole.yaml`

Tests:
- `kind_kai_binds_pods_in_queue`: the pods are `Running`, `.spec.schedulerName` is `kai-scheduler`, and the PodGroup KAI made for each has `.spec.queue` equal to the leaf queue;
- `kind_kai_shares_one_gpu`: two 8 GiB shares in `nonMemoryEnforced` mode land on the same node and fake GPU (by KAI's GPU-group annotation or label on the pod, the key ruled in Task 0);
- `kind_default_scheduler_whole_gpu`: with `scheduler: default`, the pod is bound by `default-scheduler` and consumes `nvidia.com/gpu: 1`;
- `kind_no_queue_no_kai_binding`: a pod with `schedulerName: kai-scheduler` and a queue label that names no queue stays `Pending` (KAI does not silently use another queue).

### Task 9 — Docs

Files:
- `docs/guides/gpu/index.md`: what Loams runs on GPUs and what it does not (model servers as separate pods, D1231); the three ways (Kubernetes with KAI or Run:ai, plain Kubernetes or HAMi, Compose); vendor support (NVIDIA whole and shared; AMD whole, and shared through HAMi only); licences and trademarks (§67 §11); NIM and Dynamo as optional endpoints (§67 §8)
- `docs/guides/gpu/kubernetes-kai.md`: prerequisites and the version matrix (KAI, the GPU Operator, and for `nvFractions` the fractioning operator's requirements: containerd ≥ 2.0 or CRI-O with NRI, GPU Operator ≥ 26.7.0, toolkit ≥ 1.20.1, device plugin ≥ 0.20.1, driver r615+); choosing a sharing mode and what each enforces; queues (existing leaf, `default-queue`, or the chart's); installing the chart; keeping Loams' own services on CPU node pools; pointing `loams-llm` and the gateway's `llm` module at the Services; on a full Run:ai install, using the Run:ai project's namespace and queue; troubleshooting `Pending` pods
- `docs/guides/gpu/kubernetes.md`: the default scheduler with whole GPUs (NVIDIA, AMD), MIG and time-slicing as the GPU Operator exposes them, standalone HAMi
- `docs/guides/gpu/compose.md`: the two override files, the profile, the `.env` entries, time-slicing without memory isolation, and that `loams dev` points at a server but starts none
- `scripts/gpu/docs-check.sh`: every fenced block preceded by `<!-- file: <path> -->` must equal that file's content (`docs_snippets_match_files`); every relative link in `docs/guides/gpu/` resolves (`docs_links_resolve`); no "Loams Run:ai" (`docs_trademark_wording`); no `nvcr.io` image in a code block without the licence note within the three lines above it (`docs_nvcr_has_licence_note`)

### Task 10 — The workflow

Files: `.github/workflows/gpu-examples.yml`
- triggers: `pull_request` and `push` to `dev` and `main` on paths `deploy/helm/loams-gpu-workers/**`, `deploy/kai/**`, `scripts/gpu/**`, `docs/guides/gpu/**`, the workflow itself; `schedule` nightly (kind job only); `workflow_dispatch`
- job `chart` ("GPU examples: chart, policy, schema, docs"): install Helm, yq and kubeconform from `pins.env` with SHA-256 checks; run `chart-check.sh` (Tasks 1–6) and `docs-check.sh` (Task 9); shellcheck on `scripts/gpu/*.sh`
- job `kai-kind` ("GPU examples: KAI on kind with fake GPUs"): install kind, Helm and kubectl from pins; run `kind-smoke.sh` (Task 8); `timeout-minutes: 30`
- `permissions: contents: read`; actions pinned by SHA as in the other workflows; neither job is added to `CI required` (like `release-compose.yml` and `fabric.yml`)

Tests: the workflow runs green on its own PR; a deliberately broken golden file in a draft commit fails `chart` (recorded in the PR description, then reverted).

**GP0 exit:** Review Focus all green; the docs read end to end by someone who did not write them; one manual real-GPU run recorded in `docs/guides/gpu/kubernetes-kai.md` if credits exist (Q1018), otherwise the docs say "tested with fake GPUs on kind; not yet on hardware".

## GP1–GP4 (milestone level; full tasks when their gates are met)

| Milestone | Content (§67) | Gate |
|---|---|---|
| **GP1** | `crates/loams-gpu` (`GpuRequest`, `GpuSpec`, the vendor resource table, `GpuPlacement` with `kubernetes`, `kai`, `compose`, `external`); `QueueSpec.gpu` and GPU-aware leasing in `loams-jobs`; `EngineJob::GpuBatch` in the Kubernetes engine runner; `io.loams.dev.usage.gpu.allocation.v1` and the Jobs gate's `gpu_ms` reserve; re-embedding (§21's phase D3) as the first user; placement conformance cases for every backend and vendor | JB1's `TikvJobStore` and engine-runner registry merged |
| **GP2** | `LoamsGpuPool` and `LoamsCluster.spec.gpu` in `loams-operator-api`; namespace → KAI queue sync in the `LoamsNamespace` controller; `LOC-GPU-001`–`004`; the Workload Map definition in `deploy/kai/workload-map/` and the upstream PR; the `hami` and `dra` backends; the GP0 chart deprecated in favour of the operator | OP1's `LoamsCluster`, `LoamsNamespace` and LOC harness |
| **GP3** | `submit_gpu_job` tool; the `gpu:submit` Place grant; Jailer rules (image allowlist, size, `gpu_ms` budget, approval threshold, egress, taint); dedicated tainted node pools for agent jobs; verdict events and left-behind records | JL1a and GP1 |
| **GP4** (optional) | The Rust cuVS accelerator for Lance, upstream in `lance-format/lance-cuvs`; Loams index builds above a size threshold as `GpuBatch` jobs | Q1024 approved; GP1 |

## Owner actions

- §67 and its defaults (D1228–D1250, Q1015–Q1029) were approved on 2026-10-20; nothing further for the design.
- Q1018 (default approved): approve the actual GPU budget (or apply for credits) for the manual real-GPU run.
- Q1026 (default approved: after GP2's real-GPU run): decide when Ostrium lists "Loams with KAI on your cluster" as a service.

## Rulings made during execution

(none yet)
