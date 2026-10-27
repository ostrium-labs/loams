# 67 — GPU Workloads on Kubernetes: Loams with KAI Scheduler and the Open Run:ai Stack

Status: **Approved** · 2026-10-20 (the owner approved the direction on 2026-10-20, integrate with NVIDIA's open-sourced Run:ai stack, and then every decision and question with its default the same day). Source: the research note `pitch/research/nvidia.md` (Ostrium Labs, 2026-10-20, not in this repository), §4 "Run:ai integration", and its recommendation, rechecked against the upstream repositories on 2026-10-20 (§2, §20). Decisions **D1228–D1250** and questions **Q1015–Q1029** are in the [decision log](13-decision-log.md). The numbers follow §66 (D1202–D1227, Q1000–Q1014). Plan stub: [GP1](../plans/2026-10-20-gp1-gpu-runai.md). **This document writes no code.**

Builds on:
- [§09](09-links-and-workers.md) §2: the optional `embed()` UDF, which calls an external OpenAI-compatible endpoint ("Loams does not host models").
- [§06](06-search-and-vector.md) §5.1: Lance IVF index builds as worker tasks.
- [§17](17-ai-data-ecosystem.md) §7: GPU scheduling and model serving are left to the lab's Ray or Spark cluster today.
- [§21](21-durable-execution.md): re-embedding a collection as a durable operation, the durable track's phase D3 (§21 §6, not decision D3).
- [§24](24-cpu-time-runtime.md) §16: the `Runner` trait for functions (D375).
- [§26](26-jobs-api.md): Loams Jobs (D204–D216): queues with leases and fencing, engine jobs owned by engine runners on kube-rs.
- [§38](38-knative-authentik-gitops.md) §3.3: one Kubernetes namespace per Loams namespace (D443).
- [§52](52-loams-memory.md) and plan [MM1](../plans/2026-10-13-mm1-loams-memory.md) Task 2: `loams-llm` with `Embedder` and `Reranker` over OpenAI-compatible HTTP (D887).
- [§56](56-loams-sandbox.md): `loams-sandbox` and its adapter ladder (D990–D1009).
- [§57](57-loams-gateway.md) §12.2: the gateway's `llm` module, which routes `/v1/embeddings` by model name to providers or self-hosted servers.
- [§60](60-places-and-harness-composition.md) and [§61](61-loams-jailer.md): Places (D1087) and the Jailer PDP (D1094–D1105).
- [§63](63-loams-meter.md): Loams Meter, usage event types and the gate (D1120–D1138).
- [§64](64-loams-infra-stack.md) §9.1: Loams Run (IS4), the small non-Kubernetes scheduler (D1165).
- [§65](65-loams-operator.md): `loams-operator`, `LoamsCluster`, `LoamsNamespace` and Loams Operator Conformance (D1172–D1201).

Markers, as in §64–§66:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search did not settle it.

---

## 1. Summary

### 1.1 What was released

On **2026-10-13** NVIDIA published the whole Run:ai platform as open source under **Apache-2.0**, in [`dsx-ai-factory/run-ai`](https://github.com/dsx-ai-factory/run-ai), as part of its DSX AI Factory platform. The release has four parts that matter to Loams (§2):

1. **KAI Scheduler**, the Kubernetes-native GPU scheduler behind Run:ai, already open since 2025 and a CNCF Sandbox project: gang scheduling, hierarchical queues with quotas and over-quota weights, fair share, reclaim and preemption, topology awareness and GPU sharing.
2. **GPU fractioning**: node-level enforcement of the GPU-memory share KAI assigns to a container.
3. **Quota and tenancy** (`kai-resource-management`): projects, departments, quotas and node pools, now standalone.
4. **Workload types** (Workload Map, formerly Karta): a CRD that describes any workload kind's pods, replicas and status so KAI can gang-schedule kinds it has never seen.

Run:ai is **Kubernetes-only**. NVIDIA's prebuilt images and the NGC "Community Product" chart are distributed under the **NVIDIA Software License Agreement**, which is not the source licence.

### 1.2 What Loams wants from it

Loams schedules no GPUs today. `embed()` and the rerank stage call external model endpoints, and GPU batch work runs on someone else's Ray or Spark cluster (§17 §7). The goals (§3) are GPU workloads *in* a Loams deployment, on clusters that already run KAI or Run:ai:
- embedding and rerank model servers on **fractional GPUs**, called by `loams-llm` and the gateway's `llm` module;
- **batch embedding backfills** (re-embedding a collection, §21's phase D3) as GPU jobs;
- **agent GPU jobs**, submitted through Loams Jobs and checked by the Jailer;
- optional self-hosted model serving through Dynamo or NIM;
- House GPU analytics, later and only if an engine path appears.

### 1.3 The recommendation in one paragraph

Integrate at the **Kubernetes layer only**: Loams' GPU pods carry KAI's `schedulerName`, queue label and sharing annotations, and Loams namespaces map to KAI queues. **Never depend on the Run:ai control plane, never link Run:ai code, never fork it.** Keep every Loams API **vendor-neutral** (a `GpuRequest` with a vendor and a placement backend, of which `kai` is one), so AMD and others are not excluded. Ship in phases: **GP0** now, a docs page, an example Helm chart and a Compose override, with no core code; **GP1** a GPU job class in Loams Jobs and a `kai` placement for the Kubernetes engine runner; **GP2** a `gpu` block in `loams-operator`, namespace → queue quota sync tied to Loams Meter, and a Loams workload-type definition published upstream; **GP3** agent GPU jobs under the Jailer. A **cuVS track** (a Rust cuVS accelerator contributed to Lance) is a separate, optional decision (§13). Docker Compose users get GPUs through Compose's device reservation, with no KAI (§10).

### 1.4 Phase order (D1246)

| Phase | Content | Where | Starts |
|---|---|---|---|
| **GP0** | Docs: "GPU workloads with Loams" (Kubernetes with KAI, plain Kubernetes, Compose); the example chart `deploy/helm/loams-gpu-workers`; the opt-in `deploy/release/compose.gpu.yaml`; GPU-free CI (lint, golden render, schema validation, policy checks, a kind run with a fake GPU operator) | This repository | After v0.1.1 (Q1015) |
| **GP1** | `GpuRequest` and `GpuPlacement` in a small crate; GPU queues in Jobs (`QueueSpec.gpu`); `EngineJob::GpuBatch`; the `kubernetes` and `kai` placement backends for the Kubernetes engine runner; the `gpu.allocation` usage event and the Meter gate | `loams-jobs`, `loams-gpu` | After JB1's TiKV store and engine-runner registry exist |
| **GP2** | `LoamsGpuPool` and the `gpu` block in `loams-operator-api`; namespace → KAI queue sync; LOC's KAI co-existence profile; the Workload Map definition upstream | `loams-operator`, upstream `dsx-ai-factory/workload-map` | With OP1 or after it |
| **GP3** | Agent GPU jobs: the `submit_gpu_job` tool, the Place grant, the Jailer rules and budgets | `loams-jailer`, agentd, Studio | After JL1a and GP1 |
| **GP4** (optional) | cuVS: a Rust accelerator for Lance index builds, contributed upstream; Loams runs it as a GPU job class | upstream `lance-format/lance-cuvs`, `lance-format/lance` | Owner approval (Q1024), after GP1 |

### 1.5 What changes

| Before | After |
|---|---|
| "GPU scheduling and model serving: … on the lab's Ray or Spark cluster" (§17 §7) | Still true for training and large batch inference. Loams can also run its own embed and rerank servers and GPU batch jobs on a Kubernetes cluster with KAI, or on plain Kubernetes |
| `embed()` and rerank need an external endpoint (§09) | They may point at an in-cluster model server from a Loams GPU pool; Loams still never loads a model into its own processes (D1231) |
| Jobs has CPU queues and engine jobs (§26) | GPU queues, leased only by workers that hold a matching GPU, and `EngineJob::GpuBatch` (D1238) |
| `LoamsCluster` has no GPU components (§65 §5.2) | `LoamsGpuPool`, with a vendor-neutral `gpu` block (D1233) |
| `LoamsNamespace` renders ResourceQuota and LimitRange (D443) | It also renders the namespace's KAI child queue when the cluster's GPU scheduler is `kai` (D1235) |
| Meter has no GPU events (§63 §6.3) | `io.loams.dev.usage.gpu.allocation.v1`, billed on allocation (D1236) |

## 2. What was released, checked (2026-10-20)

The research note's table, rechecked against the GitHub API and the repositories' files on 2026-10-20. Anything this recheck did not confirm is marked.

| Component | Repository | What it is | Licence | Latest (2026-10-20) |
|---|---|---|---|---|
| **Run:ai platform** | [`dsx-ai-factory/run-ai`](https://github.com/dsx-ai-factory/run-ai) (Go) | Monorepo: `control-plane/` (APIs and services for workloads, policies, identity, config), `runai-cluster/` (in-cluster controllers and admission webhooks), `runai-installer/`, `runai-mcp-server/`, `runai-clients/`, the UI, `skills/`. Supported branch `release-3.0` | Apache-2.0 (root `LICENSE`); nested MIT (`conjungo`, `gojsonq`, `validation`) and Apache-2.0 AND CC-BY-4.0 (`skills/`) | 3.0.20 (2026-10-11) |
| **KAI Scheduler** | [`kai-scheduler/KAI-Scheduler`](https://github.com/kai-scheduler/KAI-Scheduler) (Go; `NVIDIA/KAI-Scheduler` redirects) | Kubernetes-native GPU scheduler, run beside the default scheduler: gang scheduling, hierarchical queues with quota, limit and over-quota weight, fair share, reclaim and preemption with minimum runtimes, topology, GPU sharing, DRA (off by default). CNCF Sandbox since 2025-12-21 | Apache-2.0 | v0.18.6 (2026-10-15; v0.18 is the LTS line, supported to 2027-09); v0.16.12 backport (2026-10-18) |
| **GPU fractioning operator** | [`kai-scheduler/gpu-fractioning`](https://github.com/kai-scheduler/gpu-fractioning) | Node-level enforcement of the GPU-memory share KAI assigns: an NRI plugin (`fractiond`) injects a per-container memory limit, and the operator runs MPS with memory accounting. Runs beside KAI; KAI's `NvFractions` mode waits for it | Apache-2.0 | v0.1.8 (2026-10-19) |
| **KAI Resource Management** | [`kai-scheduler/kai-resource-management`](https://github.com/kai-scheduler/kai-resource-management) | Run:ai's tenancy model, standalone: cluster-scoped `Project`, `Department`, `NodePool`, `KRMConfig`, `ManagedNodesConfig` in `kai.resources/v1alpha1`; generates per-project, per-node-pool KAI queues and creates project namespaces | Apache-2.0 | v0.18.6 (2026-10-18; its README still says no release, which is stale) |
| **Workload Map** (formerly Karta, renamed 2026-09-15) | [`dsx-ai-factory/workload-map`](https://github.com/dsx-ai-factory/workload-map) | The `Karta` CRD (`run.ai/v1alpha1`) and a Go SDK: describes a workload kind's components, pod-template paths, replica paths, status mappings (JQ) and gang instructions, so KAI's pod grouper can group kinds it has no plugin for. A catalogue of definitions (Deployment, StatefulSet, Job, CronJob, Pod, JobSet, LeaderWorkerSet, PyTorchJob, MPIJob, RayCluster, RayJob, RayService, KServe, Knative Service, NIM, Milvus, Dynamo, Grove, Agent Sandbox). Not a scheduler; pre-1.0 | Apache-2.0 | v0.2.8 (2026-09-22) |
| fake-gpu-operator (test aid) | [`run-ai/fake-gpu-operator`](https://github.com/run-ai/fake-gpu-operator) | Advertises fake GPUs on nodes; KAI's own kind-based end-to-end setup installs it from `oci://ghcr.io/run-ai/fake-gpu-operator/fake-gpu-operator` | MIT | v0.2.1 (2026-10-04) |

The research note dated the release to 2026-10-13 from NVIDIA's technical blog; the GitHub repository was created on 2026-08-25 (its public date is unknown).

### 2.1 Distribution and licences

- **Sources** are Apache-2.0, and contributions take a DCO sign-off: Run:ai (with a `dco` CI check, no CLA) and Workload Map (no CLA). KAI's contributing docs mention both a `CLA.md` and a DCO sign-off (verify which applies before Loams sends KAI a change; GP0 sends none).
- **KAI's own chart and images** are built from the Apache-2.0 sources and published on GitHub's registry: the chart at `oci://ghcr.io/kai-scheduler/kai-scheduler/kai-scheduler`, images at `ghcr.io/kai-scheduler/kai-scheduler/<component>` (amd64 and arm64, with `-fips` variants). The fractioning chart is at `oci://ghcr.io/kai-scheduler/gpu-fractioning`, images at `ghcr.io/kai-scheduler/gpu-fractioning/{operator,fractiond,metricsd}`. KAI's prose names a `gpu-sharing` subchart path that its `Chart.yaml` does not use; the `Chart.yaml` is taken as right.
- **The Run:ai platform's images and charts** are on NGC under `nvcr.io/nvidia/runai` (and an `nvidia/runai-oss` organisation), and the installer pulls from there by default. NGC's pages and Run:ai's product-support policy put them under the **NVIDIA Software License Agreement** and product-specific terms. Whether they may be redistributed is not stated in their favour anywhere checked, so Loams treats them as not redistributable (D1242).
- **Trademarks.** Run:ai's `TRADEMARKS.md`: Apache-2.0 grants no rights to the NVIDIA, Run:ai or NVIDIA Run:ai names or logos, and forks and self-hosted deployments must not imply NVIDIA's endorsement.
- **No enterprise support** comes with the open release; NVIDIA keeps a paid, support-entitled release beside it (research note §1.2).

### 2.2 The Kubernetes surface Loams uses

What a Loams pod or object carries, all verified in KAI's and the fractioning operator's docs and CRDs on 2026-10-20:

| Need | Kubernetes field | Notes |
|---|---|---|
| Be scheduled by KAI | `spec.schedulerName: kai-scheduler` | KAI runs beside the default scheduler |
| Join a queue | label `kai.scheduler/queue: <leaf queue>` | Only leaf queues take work. The chart creates `default-parent-queue` and its leaf `default-queue` (quota 0, limit −1, over-quota weight 1) |
| Queue objects | `Queue`, `scheduling.run.ai/v2`: `parentQueue`, `priority` (default 100), `preemptMinRuntime`, `reclaimMinRuntime`, `resources.{cpu,memory,gpu}.{quota,limit,overQuotaWeight}` | CPU in millicores, memory in MB, GPU in device units; −1 is unlimited. Whether a GPU quota may be fractional is (verify) |
| Gang scheduling | annotation `kai.scheduler/batch-min-member: "<n>"`, or a `PodGroup` (`scheduling.run.ai/v2alpha2`: `minMember`, `queue`, `priorityClassName`, `subGroups`) made by KAI's pod grouper | The pod grouper handles standard kinds itself and falls back to Workload Map definitions for others |
| A GPU share, mode `NonMemoryEnforced` (KAI's default `global.gpuSharingMode`) | annotations `gpu-fraction: "0.5"` or `gpu-memory: "<MiB>"` | Placement only; **memory is not enforced** |
| A GPU share, mode `NvFractions` | annotations `nvidia.com/container.<container>.gpu-memory.request` and `.limit` (quantities such as `8Gi`), and optionally `nvidia.com/container.<container>.gpu-compute.mode: time-slicing \| sm-sharing` | Needs the fractioning operator (installed by `--set global.nvFractions.set=true`), which needs Kubernetes ≥ 1.28, containerd ≥ 2.0 or CRI-O with NRI, GPU Operator ≥ 26.7.0 (26.7.1 recommended), container toolkit ≥ 1.20.1, device plugin ≥ 0.20.1 and driver r615+ (CUDA 13.4), which is not the GPU Operator's default driver |
| A GPU share, mode `HamiCore` | as KAI's sharing docs say for that mode | Needs `kai-resource-isolator` (HAMi-core, NVIDIA) |
| Whole GPUs | `resources.limits["nvidia.com/gpu"]` or `["amd.com/gpu"]` | KAI maps exactly these two names to its GPU slot |
| DRA | `ResourceClaim`s | Off by default (`--feature-gates=DynamicResourceAllocation=true` on scheduler and binder); documented for NVIDIA's DRA driver only |

Other facts that shape the design:
- **Kubernetes floor.** KAI's CI validates from v1.28.13; its chart sets no `kubeVersion`. Loams' own floor (§65, Q997) is higher, so KAI does not lower it.
- **Prerequisites.** KAI lists the NVIDIA GPU Operator as required for GPU workloads.
- **Vendors.** KAI schedules whole AMD GPUs (`amd.com/gpu`) and reads an AMD VRAM node label for memory-aware decisions, but **every fractional-sharing mode is NVIDIA-only**. Its vendor-neutrality issue (#134) is open.
- **Quotas and tenancy without Run:ai.** KAI queues are enough for Loams; Run:ai's projects and departments (`kai.resources`) generate queues for teams that use them, and Loams does not need them (§6.3).

## 3. Goals and non-goals

### 3.1 Goals

1. **Embedding and rerank workers** on fractional GPUs. Models of 1B parameters or less (for example `llama-nemotron-embed-1b-v2`) leave most of a GPU idle, so a share of GPU memory (8–16 GiB, estimate) is the right unit.
2. **Batch embedding backfills.** Re-embedding a collection after a model change (§21's phase D3) is hours of GPU work that must not be redone; it runs as a durable GPU job.
3. **Agent GPU jobs.** An agent in Studio or agentd asks for a GPU job (a fine-tune, an evaluation, a batch inference), and Loams runs it inside the Place's limits.
4. **Self-hosted model serving, optional.** Teams that run Dynamo or NIM point the gateway's `llm` module at it. Loams documents it and does not deploy it.
5. **House GPU analytics, later.** Only if chDB or the ecosystem (cuDF-Polars over `scan_loams()`) gives a path (Q1028).
6. **On clusters that run KAI or Run:ai**, without fighting the platform's own admission and quota rules, and **on plain Kubernetes and Compose** too.

### 3.2 Non-goals

- Training frameworks, distributed training orchestration, or a model registry.
- A GPU scheduler of our own. On Kubernetes, Loams uses Kubernetes (D1165).
- Any requirement on the Run:ai control plane, its REST API, its UI or its MCP server.
- Shipping, mirroring or bundling NVIDIA images, drivers, CUDA or the GPU Operator.
- GPU access inside `loams-sandbox`'s local adapters (§9.3).

## 4. Principles

1. **Kubernetes objects are the contract.** Loams writes ordinary pods, Deployments, Jobs and, when asked, KAI `Queue` objects. It never calls a scheduler API.
2. **Loams stays the queue of record.** For Jobs, leases, fencing, retries and dead letters stay in Loams (§26 §6). KAI only places the pods that lease work.
3. **Vendor-neutral at the API, specific at the backend.** A user asks for a `GpuRequest`; a placement backend turns it into scheduler-specific fields. NVIDIA names appear only in the `kai` and `nvidia` backends.
4. **The platform wins.** On a cluster where Run:ai's admission webhooks or a platform team set the scheduler, queue or node pool, Loams does not overwrite them (D1234).
5. **Allocation is billed, utilisation is shown.** What a namespace reserved is what it used (D1236).
6. **Reference, never redistribute** anything under the NVIDIA Software License Agreement (D1242).
7. **Agents never touch Kubernetes.** An agent's GPU job is a Loams job checked by the Jailer, never a pod spec the agent wrote (D1239).

## 5. Where Loams stands today

| Piece | State | Where |
|---|---|---|
| `embed()` UDF | Designed, not built | §09 §2 |
| `loams-llm` (`Embedder`, `Reranker`, OpenAI-compatible) | In review (MM1 Task 2, PR #843) | §52, MM1 |
| Gateway `llm` module (`/v1/embeddings`, routing by model) | Designed | §57 §12.2 |
| Jobs (`loams-jobs`, leases, engine runners on kube-rs) | Designed; JB1 planned | §26, [JB1](../plans/2026-10-10-jb1-jobs-and-usage.md) |
| `Runner` trait (functions) | Designed, not built (RN1) | §24 §16 |
| `loams-sandbox` | Designed; SB1 after preview.3 | §56 |
| `loams-operator` | Approved design; OP0 after v0.1.0 | §65 |
| `LoamsNamespace` (namespace, NetworkPolicy, ResourceQuota, LimitRange) | Designed (CP1 Task 15, moving to OP1) | D443, §65 §11.5 |
| Loams Meter | Approved design; ME1 stub | §63 |
| Jailer PDP | Proposed; JL1 after SB1d | §61 |
| Loams Run (non-Kubernetes placement) | Approved, gated (Q980) | §64 §9.1 |
| Anything that requests a GPU | None | — |

## 6. The integration layers

Six layers, each usable without the ones after it.

```
 (f) Jailer policy for agent GPU jobs ── Place grant, image allowlist, GPU budget ──┐
 (e) Jobs `kai` placement ── GPU queues, EngineJob::GpuBatch → Job / pods ──────────┤
 (d) Workload Map definition for LoamsGpuPool (upstream) ───────────────────────────┤ GP2–GP3
 (c) namespace → KAI queue, quota sync, Meter events ───────────────────────────────┤
 (b) operator `gpu` block on LoamsGpuPool ──────────────────────────────────────────┘
 (a) docs + example chart + Compose override (no Loams code) ──────────────────────── GP0
        │
        ▼  Kubernetes objects only: schedulerName, queue label, sharing annotations,
           Queue objects, node selectors and tolerations, extended resources or DRA claims
     KAI Scheduler (+ GPU sharing / fractioning) │ default scheduler + device plugin │ HAMi │ DRA
```

### 6.1 (a) Docs and the example chart (GP0; D1230)

A docs page set under `docs/guides/gpu/` and an example Helm chart `deploy/helm/loams-gpu-workers`. The chart runs **model servers** for embedding and rerank (vLLM by default, Q1016) as Deployments with ClusterIP Services and a default-deny NetworkPolicy that admits only the Loams namespace's pods. It has three placement modes, chosen by `placement.scheduler`:

| `placement.scheduler` | What the chart renders on the pod |
|---|---|
| `default` | The vendor's extended resource (`nvidia.com/gpu: 1`, `amd.com/gpu: 1`), node selector and tolerations. Whole GPUs only (or what MIG or time-slicing exposes) |
| `kai` | `schedulerName: kai-scheduler` and the label `kai.scheduler/queue`; with `gpu.share`, the annotations of the cluster's sharing mode (`placement.kai.sharingMode`): `gpu-memory` (MiB) or `gpu-fraction` for `nonMemoryEnforced`, `nvidia.com/container.<c>.gpu-memory.request`/`.limit` for `nvFractions`; whole GPUs keep the extended-resource limit (§2.2) |
| `hami` | Standalone HAMi's resource names (`nvidia.com/gpu` with `nvidia.com/gpumem`, or the vendor's names from HAMi's examples) for sharing without KAI (verify per vendor) |

Sharing in `nonMemoryEnforced` mode is placement only: one server can take another's GPU memory. The chart allows it, and the docs say so; the operator later allows it only inside one namespace's pool (D1232).

The chart never sets an image registry default under NVIDIA's licence; `image.repository` and `image.digest` are required values, and the docs list choices (vLLM, Hugging Face TEI, NIM with its licence terms). Loams itself (engine, gateway) stays on CPU node pools; the docs show how to point `loams-llm` and the gateway's `llm` module at the model server's Service.

GP0 needs no Loams code: the chart is an example, versioned with the repository, tested in CI, and not part of the release's supported set until GP2 replaces it with the operator (D1230).

### 6.2 (b) The operator `gpu` block (GP2; D1233, D1234)

`loams-operator-api` gains one kind and one shared block.

```yaml
apiVersion: loams.dev/v1alpha1
kind: LoamsGpuPool
metadata: { name: prod-embed, namespace: loams-system }
spec:
  cluster: prod
  role: modelServer            # modelServer | jobWorker
  modelServer:                 # role = modelServer: an OpenAI-compatible server
    image: { repository: <user-chosen>, digest: sha256:… }
    args: ["--model", "nvidia/llama-nemotron-embed-1b-v2", "--task", "embed"]
    serves: [embeddings]       # embeddings | rerank | chat
    llmRoute: { model: nemotron-embed }   # registered with the gateway's llm module
  jobWorker:                   # role = jobWorker: Loams Jobs workers for GPU queues
    queues: [reembed]
  replicas: { min: 1, max: 4 }
  gpu:
    vendor: nvidia             # nvidia | amd | intel | any
    # count: 1                 # whole devices, or a share (exclusive):
    share: { memory: 12Gi }    # memory or fraction
    scheduler: kai             # default | kai | hami | dra
    kai: { queue: auto, priorityClass: inference }  # auto = the namespace's queue (§6.3)
    adoptPlatformPlacement: false   # true: leave scheduler, queue and affinity to the platform (D1234)
    nodeSelector: { loams.dev/gpu-pool: embed }
    tolerations: [{ key: nvidia.com/gpu, operator: Exists, effect: NoSchedule }]
---
# LoamsCluster.spec.gpu: cluster-wide facts the pools must match
gpu:
  scheduler: kai               # the default for pools that do not say
  kai:
    sharingMode: nvFractions   # nonMemoryEnforced | nvFractions | hamiCore: must equal KAI's global.gpuSharingMode
    parentQueue: ""            # empty: create loams-<cluster>; a name: adopt that queue (§6.3)
```

The rules:
- **One `gpu` block type** (`GpuSpec`), also used by `QueueSpec.gpu` in Jobs (§6.5), so the operator and Jobs translate requests the same way, in one crate (`loams-gpu`, D1229).
- **Validation** (CEL): `count` and `share` are exclusive; `share` is refused with `scheduler: default`; `share` is refused unless `vendor: nvidia` while the scheduler is `kai` (every KAI sharing mode is NVIDIA-only, §2.2); `kai` settings are refused unless `scheduler: kai`.
- **Sharing mode.** KAI's sharing mode is a cluster-wide Helm setting, so it lives on `LoamsCluster.spec.gpu.kai.sharingMode`, and the operator checks it against KAI's configuration where it can read it (verify the `Config` object's field in GP2 Task 0). A `share` across namespaces' pods on one GPU is allowed only with `nvFractions`, which enforces memory.
- **Detection.** The operator reports a `GpuSchedulerAvailable` condition from the presence of KAI's CRDs (or HAMi's, or DRA's `DeviceClass`). It never installs KAI (Q1027).
- **The platform wins (D1234).** The operator's server-side-apply field manager does not own `schedulerName`, the queue label, node-pool labels or affinity on a pool with `gpu.adoptPlatformPlacement: true` (whatever its `gpu.scheduler`), so a Run:ai admission webhook or a platform mutation stays. LOC gains a Loams-only profile, `LOC-GPU-*` (§12.2).
- **Never public.** Model-server Services are ClusterIP behind the default-deny policy; §65 §11.4's exposure rule applies unchanged.

### 6.3 (c) Namespaces → KAI queues, quotas and the Meter (GP2; D1235, D1236)

**Mapping.** KAI queues are hierarchical. Loams uses two levels:

| KAI queue | Created by | Quota from |
|---|---|---|
| Parent: `loams-<cluster>` (or an existing queue named in `LoamsCluster.spec.gpu.kai.parentQueue`) | `loams-operator`, unless adopted | `LoamsCluster.spec.gpu.kai.parentQuota`, or untouched when adopted |
| Child per Loams namespace: `loams-ns-<namespace>` (the Kubernetes namespace name of D443) | The `LoamsNamespace` controller | The namespace's GPU entitlement (§63 §9): `quota` = the plan's guaranteed GPUs, `limit` = the hard cap, `overQuotaWeight` from the plan |
| Optional grandchildren per pool or per Jobs queue | Not in GP2 (Q1019) | — |

- The sync is **one-way**: Loams writes the queues it created and reads the ones it adopted. It never edits a queue it did not create.
- Pods in a Loams namespace carry the child queue's label; `queue: auto` means exactly that.
- A namespace with no GPU entitlement gets no queue; the Meter gate refuses its GPU jobs (§6.5) and the operator refuses its GPU pools.
- Run:ai *projects* (`kai-resource-management`) are not used. Teams that run the Run:ai control plane can map a Run:ai project to the same namespace by hand; Loams does not read or write Run:ai objects (Q1025).

**Usage (D1236).** A new event type in §63 §6.3's set:

| Type | From | Data (units) | Producer seam |
|---|---|---|---|
| `io.loams.dev.usage.gpu.allocation.v1` | The node agent (`loams-meter-agent`), per pod per window | `gpu_ms` (devices × share × ms allocated), `gpu_memory_mib_ms`, `vendor`, `product` (for example `NVIDIA-L40S`), `share_mode` (`whole`, `memory`, `fraction`, `mig`), `pool`, `queue`, `workload` (`model_server`, `job`, `agent_job`) | Allocation read from the pod's spec and the scheduler's binding annotations, from outside the workload |

- **Allocation, not utilisation, is billed.** Utilisation from DCGM's exporter (Apache-2.0) is shown in the console as a gauge and never billed, so a share that sits idle still counts, as CPU requests do.
- **The gate.** Jobs calls `reserve(gpu_ms, share × deadline)` when it leases a GPU job and settles on completion (§63 §10.2's Jobs row gains a GPU line). Model-server pools are platform capacity: their allocation is metered to the pool's owner namespace, not to callers; callers pay per `llm.tokens` event as today.
- **Quota and entitlement agree.** The KAI child queue's `limit` is derived from the same entitlement the Meter enforces, so the scheduler and the gate refuse at the same edge. When they disagree (a platform team edited an adopted queue), the gate's answer stands for Loams work.

### 6.4 (d) A Loams workload-type definition upstream (GP2; D1237)

KAI gang-schedules standard kinds (Deployments, Jobs, JobSets and others) without help. A `LoamsGpuPool` renders Deployments, so KAI handles it already. A Workload Map definition is still worth publishing for two reasons:
- it lets KAI's pod grouper treat the **pool** (the custom resource) as the scheduling unit, so a pool's min replicas start together or not at all;
- it puts Loams in NVIDIA's public list of integrated workload types, which is the visible artifact of the integration.

The definition lives in `deploy/kai/workload-map/loams-gpupool.yaml`: a `Karta` (`run.ai/v1alpha1`) whose root component is `LoamsGpuPool` with `additionalChildKinds` for its Deployment, a `podTemplateSpecPath` into the pool's rendered template, `replicasPath` and `minReplicasPath` from `spec.replicas`, status mappings from the pool's `Ready` condition, and `optimizationInstructions.gangScheduling` on. It is tested in Loams CI against the pinned Workload Map release (the pod-grouping result on kind, golden). KAI's pod grouper reads Workload Map definitions only as a fallback after its in-tree plugins, so the test also checks which grouper KAI chose.

Upstream, a catalogue entry is a Go builder registered in `karta/sdk/pkg/catalog/kartas/`, generated YAML (`make generate-samples`, never hand-edited), an end-to-end flow under `test/e2e/flows/` and a README row, signed off with the DCO (no CLA). If upstream declines, the file stays in Loams and the operator's chart installs it (Q1021).

### 6.5 (e) Jobs: GPU queues and the `kai` placement (GP1; D1238)

Two shapes of GPU work in Jobs (§26):

1. **GPU queues.** `QueueSpec` gains `gpu: Option<GpuSpec>`. A worker's `Lease` call declares the GPU it holds (`WorkerGpu { vendor, product, memory_mib, share }`, read from the device or the pod's downward API). The queue core leases a job to a worker only if the worker's GPU satisfies the queue's request. Workers for GPU queues run in a `LoamsGpuPool` with `role: jobWorker`, or in any pod the user runs. Loams keeps leases, fencing, retries and DLQs; KAI only places the worker pods.
2. **GPU batch engine jobs.** `EngineJob::GpuBatch { image, command, args, env, inputs, outputs, gpu: GpuSpec, parallelism, gang: bool, deadline }` runs a container to completion. Its engine runner is the existing Kubernetes engine runner (§26 §3.1), whose lifecycle is a Resonate workflow (submit, wait, collect, clean up). The runner calls a **placement backend**:

```rust
// crates/loams-gpu/src/lib.rs (sketch; the GP1 milestone's first task fixes the names)
pub trait GpuPlacement: Send + Sync + fmt::Debug + 'static {
    fn kind(&self) -> PlacementKind;                 // Kubernetes, Kai, Hami, Dra, Compose, External
    fn capabilities(&self) -> &PlacementCapabilities; // vendors, whole/memory/fraction/mig, gang, preemption
    /// Patch a pod template for this request; refuses what the backend cannot express.
    fn place(&self, req: &GpuRequest, ns: &NamespaceCx, pod: &mut PodTemplateSpec) -> Result<PlacementNote, PlacementError>;
    /// Gang semantics for n pods (KAI's min-member annotation, a JobSet, or Refused).
    fn gang(&self, req: &GpuRequest, n: u32) -> Result<GangPlan, PlacementError>;
}
```

- **`kubernetes`** (default): extended resources and node selectors; whole devices only.
- **`kai`**: `schedulerName: kai-scheduler`, the namespace's `kai.scheduler/queue` label, the sharing mode's annotations, the priority class, and `kai.scheduler/batch-min-member: "<n>"` for `gang: true`, so KAI's pod grouper makes the `PodGroup` (§2.2).
- **`hami`** and **`dra`**: later (GP2+), behind the same trait (D1243).
- **`compose`**: for the Compose stack, where the engine runner starts containers locally; renders a device reservation, whole devices only (§10).
- **`external`**: the job is handed to the user's Ray cluster through `loams[ray]` (§17), unchanged.

This is the "`kai` runner" of the request: a placement backend under Jobs' Kubernetes engine runner, not a new implementation of the function `Runner` trait. Functions on GPUs are not planned (§14).

**Re-embedding** (§21's phase D3) becomes the first user: a durable workflow that pages a collection by fragment, enqueues `GpuBatch` shards or jobs on a GPU queue, writes vectors back through the normal write path with idempotency keys, and swaps the index when done.

### 6.6 (f) Agent GPU jobs under the Jailer (GP3; D1239)

Agents never write Kubernetes objects. An agent asks through one tool, `submit_gpu_job`, which calls Jobs' `submit_engine_job(GpuBatch)` with the agent's run credentials. The Jailer PDP (§61 §4.1) decides before Jobs sees it:

| Check | Rule | Default |
|---|---|---|
| Capability | The Place's effective capability includes the grant `gpu:submit` (OpenFGA, D1087) | No Place has it until a person grants it |
| Image | The image is pinned by digest and on the Place's image allowlist | Allowlist empty |
| Size | `count` and `share` within the Place's GPU limit | 1 GPU or less |
| Budget | `gpu_ms` reserved within the run's GPU budget, a **hard stop** (§61 §4.5 rule 5) | 1 GPU-hour per run |
| Approval | Above the Place's auto-approve threshold, a durable `ApprovalRequested` (D911) | Every job asks until a person raises the threshold (Q1022) |
| Egress | The job's pod gets the Place's egress policy through the namespace's NetworkPolicy; `none` unless the Place allows more | `none` |
| Taint | A tainted run (§61 §4.5 rule 2) always asks | — |
| Pod shape | No privileged, no host paths, no host network, `restricted` Pod Security, the namespace's service account with no API token | Enforced by the runner and the namespace's admission |

Each decision is a verdict CloudEvent (D1104), and the job is recorded as something the agent left behind (D1089), so the console can show and roll it back. Untrusted code on a shared GPU is the open risk: §9.3 and Q1023.

## 7. The vendor-neutral model (D1229, D1243)

```rust
pub struct GpuRequest {
    pub vendor: GpuVendor,          // Nvidia | Amd | Intel | Any
    pub amount: GpuAmount,          // Whole(n) | Memory(bytes) | Fraction(f32 in (0,1)) | Mig(profile)
    pub product: Option<String>,    // "NVIDIA-L40S", "AMD-Instinct-MI300X"; a node-label match, optional
    pub placement: PlacementKind,   // Kubernetes (default) | Kai | Hami | Dra | Compose | External
    pub kai: Option<KaiOptions>,    // queue (auto | name), priority class; only with Kai
    pub node_selector: BTreeMap<String, String>,
    pub tolerations: Vec<Toleration>,
}
```

- **Vendors.** NVIDIA through `nvidia.com/gpu` (device plugin or GPU Operator); AMD through `amd.com/gpu` (ROCm's device plugin, Apache-2.0); Intel through `gpu.intel.com/i915` or `xe` (verify). Each vendor's resource names live in one table in `loams-gpu`.
- **Sharing.** KAI schedules whole AMD GPUs (`amd.com/gpu`), but every KAI sharing mode and the fractioning operator are NVIDIA-only (§2.2); KAI's `HamiCore` mode uses HAMi-core, which is NVIDIA too. Standalone HAMi (CNCF Incubating, Apache-2.0) lists device backends for NVIDIA, AMD and a dozen other vendors (verify AMD sharing in practice before the docs claim it). **DRA** (Kubernetes Dynamic Resource Allocation) is the long-term neutral path: NVIDIA's DRA driver now lives in `kubernetes-sigs/dra-driver-nvidia-gpu` (Apache-2.0), and a `dra` backend that writes `ResourceClaim`s works for any vendor with a DRA driver.
- **Model servers.** vLLM (Apache-2.0) runs on NVIDIA and AMD ROCm, so the GP0 default works on both.
- **Tests.** Every backend runs the same placement conformance cases (§12.1), and the `amd` cases run in CI with golden renders even before anyone tests on AMD hardware.

## 8. Self-hosted model serving: Dynamo and NIM (optional; D1250)

- Loams does not deploy Dynamo or NIM. The docs show how to point the gateway's `llm` module and `loams-llm` at them (OpenAI-compatible).
- **NIM** is free for development on up to 16 GPUs under NVIDIA's terms, and production needs NVIDIA AI Enterprise (research note §3). Loams never requires it, never defaults to it, and the docs say so next to every NIM example.
- **Dynamo** is open source; its GitHub licence field reads `NOASSERTION` on 2026-10-20 (verify the licence files before any example ships). Its Grove API is KAI-integrated upstream.

## 9. Isolation

### 9.1 Model servers

Platform-owned, from images the operator pins by digest. They run as non-root with the `restricted` profile where the image allows it (vLLM's image may need a writable cache directory; GP0 Task 0 checks).

### 9.2 GPU batch jobs from people

Ordinary tenant workloads in the namespace's Kubernetes namespace, with its NetworkPolicy, ResourceQuota and RuntimeClass.

### 9.3 Agent and other untrusted GPU code

A GPU is a shared device with its own memory and driver; a container boundary is weaker there than for CPU work. Until a stronger boundary is verified:
- agent GPU jobs run only on nodes tainted for the namespace (or for agent work), never on a node shared with another tenant's GPU work;
- **no fractional sharing for agent jobs** across tenants: a share is allowed only inside one namespace's dedicated nodes;
- gVisor's `nvproxy` and microVM GPU passthrough (Cloud Hypervisor with VFIO) are research for §56, not part of this design (Q1023). `SandboxSpec` gets no device field now.

## 10. Docker Compose (D1241)

Compose has no KAI and no fractions. GPUs reach a container through Compose's device reservation:

```yaml
# deploy/release/compose.gpu.yaml (GP0), used with: docker compose -f compose.yaml -f compose.gpu.yaml --profile gpu up -d
services:
  embed:
    profiles: [gpu]
    image: ${LOAMS_GPU_EMBED_IMAGE:?set an image pinned by digest}
    command: ${LOAMS_GPU_EMBED_ARGS}
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1                  # or device_ids: ["0"]
              capabilities: [gpu]
    networks: [loams]
    # no ports: reachable only on the compose network
```

- **AMD** uses plain device passthrough, `devices: [/dev/kfd, /dev/dri]`, instead of a reservation (AMD's ROCm Docker docs; Docker's GPU page documents NVIDIA only). It is a second file, `compose.gpu-amd.yaml`. Whether the container also needs the host's `video` or `render` group is (verify) per distribution.
- `count` and `device_ids` are exclusive, and `capabilities: [gpu]` is required.
- Two services on one GPU share it by time-slicing; there is no memory isolation. The docs say so.
- The engine reaches the server at `http://embed:8000/v1` through `loams-llm`'s provider config. Nothing binds a host port.
- `loams dev` (single binary) does not start model servers; it can point at one.

## 11. Licences and trademarks (D1242)

| Item | Licence | Rule |
|---|---|---|
| KAI Scheduler, GPU fractioning, `kai-resource-management`, Workload Map, Run:ai source | Apache-2.0 (§2.1) | Compatible. Loams links none of it; integration is Kubernetes objects. A vendored CRD schema or a Workload Map example keeps its header and gets a `NOTICE` line |
| Run:ai nested components | MIT and Apache-2.0 AND CC-BY-4.0 in parts (research note §4.5; verify per file) | Nothing vendored from them |
| NVIDIA images on `nvcr.io` and the NGC Community Product chart | NVIDIA Software License Agreement and product terms | **Reference only.** Never mirrored, re-tagged, pushed to `ghcr.io/ostrium-labs`, bundled in release tarballs or air-gap bundles, or set as a chart default. Users accept NVIDIA's terms themselves |
| KAI's and the fractioning operator's own charts and images (`ghcr.io/kai-scheduler/…`, §2.1) | Built from Apache-2.0 sources (verify the image licence labels) | Referenced by version in docs and CI; not mirrored |
| NVIDIA drivers, CUDA, container toolkit | NVIDIA EULAs | Never bundled; a prerequisite |
| GPU Operator, DCGM exporter, DRA driver | Apache-2.0 | Referenced |
| NIM | NVIDIA terms; production needs AI Enterprise | Optional, never required (§8) |
| Nemotron models | NVIDIA Open Model License | Allowed in examples, with the licence named |
| fake-gpu-operator (CI only) | MIT | Used in CI from its OCI chart |
| vLLM, TEI, HAMi, ROCm device plugin | Apache-2.0 | Referenced |

**Trademarks.** Apache-2.0 grants none, and both Run:ai's repository and Loams have a `TRADEMARKS.md`. Docs say "works with NVIDIA Run:ai" and "KAI Scheduler", never "Loams Run:ai" or anything that suggests NVIDIA endorses Loams. No NVIDIA logo.

**No BSL, GPL or other copyleft code** enters Loams through this work (`cargo deny` and the chart licence check, §12).

## 12. Testing

### 12.1 Without GPUs (every PR that touches the area)

- **Golden renders** of the chart and, from GP2, the operator's pods for each backend and vendor (`kubernetes`/`kai`/`hami` × `nvidia`/`amd`).
- **Schema validation** of the rendered objects with kubeconform, against the Kubernetes floor of §65 (Q997) and KAI's CRD schemas at the pinned release.
- **Policy checks**: no image from `nvcr.io` as a default, every image by digest, no public Service, no host port, sharing annotations only with `kai`.
- **kind with a fake GPU operator**: KAI's own end-to-end setup installs `run-ai/fake-gpu-operator` (MIT) on kind to advertise fake GPUs. Loams' job does the same, installs KAI from its chart with sharing on, and checks that the example pods are bound by KAI into the expected queue. It proves the wiring, not the GPU.

### 12.2 Loams Operator Conformance additions (GP2)

Loams-only requirements in the `GPU` area, outside LOC's operator-agnostic core:

| ID | Level | Requirement |
|---|---|---|
| LOC-GPU-001 | MUST | With `adoptPlatformPlacement`, a platform's mutation of `schedulerName`, the queue label or node affinity is not reverted, and no event says drift |
| LOC-GPU-002 | MUST | The operator never edits a KAI queue it did not create |
| LOC-GPU-003 | MUST | A namespace's child queue `limit` equals its GPU entitlement after each change, within the reconcile bound |
| LOC-GPU-004 | MUST | A pool with `scheduler: kai` on a cluster without KAI reports `GpuSchedulerAvailable=False` and creates no pods |

### 12.3 With GPUs (manual, before any support claim)

A real-GPU run (one node with driver r615+ and GPU Operator ≥ 26.7.0 for `nvFractions`, §2.2; whole-GPU and `nonMemoryEnforced` cases need less) of the GP0 example and, later, of GP1's re-embedding job, recorded in the docs with the hardware, driver, KAI and fractioning versions. CI runs on GitHub-hosted runners only; GPU runs need credits (Q1018).

## 13. The cuVS track (optional, separate; D1244)

Not part of the Run:ai integration, and decided separately (Q1024).

- **What exists** (research note §3, rechecked in §2): core Lance has CUDA k-means and PQ training (PR #2816, merged 2024). The cuVS path is an out-of-tree beta, `lance-format/lance-cuvs` (Apache-2.0), which trains IVF_PQ with cuVS and writes partition artifacts. CAGRA in Lance is [issue #6534](https://github.com/lance-format/lance/issues/6534), open, with maintainers preferring an accelerator built on `lance-cuvs`.
- **The proposal.** Ostrium contributes the **Rust** accelerator path upstream (Loams is Rust on Lance; the Python wheel does not fit), in `lance-cuvs` first, following the maintainers' direction. No Loams fork of Lance.
- **How Loams uses it.** Index builds (§06 §5.1) that cross a size threshold run as `GpuBatch` jobs on a GPU queue; the CPU build stays the default, and the on-disk format is Lance's unchanged, so any reader works.
- **Effort** 4–6 engineer-weeks (estimate), after GP1, one engineer, upstream review time on top.

## 14. What not to build (D1245)

- **No dependency on the Run:ai control plane**, its REST API, UI, CLI or MCP server; **no fork** of Run:ai, KAI or the fractioning operator; **no linking** of their Go code.
- **No Loams GPU scheduler** and no Loams Run on KAI. Loams Run (IS4) may borrow KAI's vocabulary (queues, fair share, reclaim) and places whole GPUs only (Q1029).
- **No mirroring** of NVIDIA images or charts; no NVIDIA default in any Loams chart.
- **No installer for KAI, the GPU Operator or drivers** in the operator or the chart.
- **No GPU for functions** (the `Runner` trait) and no GPU field in `SandboxSpec`.
- **No model hosting inside Loams processes**; model servers are separate pods.
- **No billing on utilisation.**
- **No Run:ai REST or MCP adapter** until a customer that runs the full Run:ai control plane asks (Q1025).
- **No House GPU engine** (Q1028).
- **No cross-tenant GPU sharing for agent jobs** (§9.3).

## 15. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Run:ai's open monorepo is a week old (3.0.x); APIs move | High | Low | Depend on KAI and Kubernetes fields only; pin KAI per release; golden tests catch drift |
| R2 | Fractioning needs very new GPU Operator, driver and CUDA versions; clouds lag | High | Medium | Whole GPUs, MIG and time-slicing as fallbacks; the docs carry the version matrix |
| R3 | Focus: 0.1.x launch, preview.3, SB1, OP1 come first | High | Medium | GP0 is docs and an example only; GP1+ wait for their gates |
| R4 | Vendor gravity: NVIDIA-only features leak into Loams APIs | Medium | Medium | `GpuRequest` with vendors; `kai` options in their own struct; AMD golden cases from GP0 |
| R5 | Overlap with NVIDIA's own platform (DSX OS, OpenShell, NemoClaw) | Medium | Low | Loams is the data and memory layer on the factory, not an orchestrator |
| R6 | Untrusted code on shared GPUs | Medium | High | §9.3: dedicated nodes, no cross-tenant shares, approval and budgets |
| R7 | No GPUs in CI | Certain | Medium | fake-gpu-operator on kind for wiring; manual real-GPU runs before claims (Q1018) |
| R8 | Someone republishes an `nvcr.io` image through a Loams channel | Low | High | Policy check in CI; release scripts refuse non-`ghcr.io/ostrium-labs` sources; §11 in the release checklist |
| R9 | Upstream declines the Workload Map definition | Medium | Low | Keep it in Loams (Q1021) |
| R10 | Quota drift between KAI queues and Meter entitlements | Medium | Medium | One-way sync, LOC-GPU-003, the gate decides for Loams work |

## 16. Decisions (D1228–D1250, approved by the owner on 2026-10-20)

| ID | Decision |
|---|---|
| D1228 | **Kubernetes-layer integration only:** Loams works with KAI Scheduler and NVIDIA's GPU sharing through Kubernetes objects (`schedulerName`, queue labels, sharing annotations, `Queue` objects); no dependency on the Run:ai control plane, no fork, no linked Run:ai code; docs say "works with NVIDIA Run:ai" |
| D1229 | **A vendor-neutral `GpuRequest`** (vendor, amount as whole, memory, fraction or MIG, placement backend, backend options) in a small crate `loams-gpu`, shared by Jobs and the operator, with placement backends `kubernetes` (default), `kai`, `hami`, `dra`, `compose`, `external` behind one `GpuPlacement` trait |
| D1230 | **GP0 ships no Loams code:** a docs page set under `docs/guides/gpu/`, the example chart `deploy/helm/loams-gpu-workers` (model servers, three placement modes, required image digests) and the opt-in `deploy/release/compose.gpu.yaml`, tested in GPU-free CI; the chart is an example until GP2's operator replaces it |
| D1231 | **Model servers are separate pods.** Embedding and rerank run in OpenAI-compatible servers (vLLM by default) called by `loams-llm` and the gateway's `llm` module; Loams processes never load models (keeps §09's rule) |
| D1232 | **Fractional sizing:** embed and rerank servers for models of 1B parameters or less default to a GPU-memory share (12 GiB, estimate) where sharing is available, and to one whole GPU (or a MIG slice) where it is not; shares across namespaces only with KAI's `nvFractions` (memory enforced), `nonMemoryEnforced` shares only inside one namespace's pool |
| D1233 | **`LoamsGpuPool`** joins `loams.dev/v1alpha1` with roles `modelServer` and `jobWorker` and a `gpu` block (`GpuSpec`); CEL rules for exclusive fields; a `GpuSchedulerAvailable` condition from CRD presence; Services ClusterIP only |
| D1234 | **The platform wins:** with `adoptPlatformPlacement`, the operator's field manager does not own scheduler, queue, node-pool or affinity fields, so Run:ai or platform admission mutations stay; the operator never edits a queue it did not create; `LOC-GPU-001`–`004` |
| D1235 | **Namespaces map to KAI queues:** a parent queue per `LoamsCluster` (or an adopted one), a child queue `loams-ns-<namespace>` per Loams namespace with a GPU entitlement, rendered one-way by the `LoamsNamespace` controller with `quota`, `limit` and over-quota weight from the entitlement; no Run:ai projects |
| D1236 | **GPU usage:** `io.loams.dev.usage.gpu.allocation.v1` from the node agent, billed on allocation (devices × share × time), never on utilisation; DCGM utilisation is a console gauge; the Jobs gate reserves `gpu_ms` on lease; model-server pools are metered to their owner namespace |
| D1237 | **A Workload Map definition for `LoamsGpuPool`** in `deploy/kai/workload-map/`, tested on kind, proposed upstream to `dsx-ai-factory/workload-map`; kept in Loams if declined |
| D1238 | **Jobs GPU work:** `QueueSpec.gpu` with GPU-aware leasing (a worker declares its GPU), and `EngineJob::GpuBatch` run by the Kubernetes engine runner through a `GpuPlacement` backend (`kai` for KAI clusters); Loams stays the queue of record; re-embedding (§21's phase D3) is the first user |
| D1239 | **Agent GPU jobs** go only through a `submit_gpu_job` tool → Jobs → the Jailer PDP: a `gpu:submit` Place grant, a digest-pinned image on the Place's allowlist, size and `gpu_ms` budget limits as hard stops, approval above a threshold, the Place's egress policy, a restricted pod shape; verdict events and left-behind records |
| D1240 | **No GPU in `loams-sandbox` now:** `SandboxSpec` gets no device field; untrusted GPU code runs only on dedicated, tainted nodes without cross-tenant sharing; gVisor `nvproxy` and microVM passthrough are §56 research |
| D1241 | **Compose:** GPUs through `deploy.resources.reservations.devices` (NVIDIA) or device nodes (AMD) in an opt-in `compose.gpu.yaml` with a `gpu` profile; no KAI, no fractions, no host ports |
| D1242 | **Licences:** Apache-2.0 sources referenced, never linked; NVIDIA-licensed images and charts referenced only, never mirrored, bundled or defaulted; drivers and CUDA never bundled; NIM optional; trademark wording "works with NVIDIA Run:ai" |
| D1243 | **Vendor neutrality:** NVIDIA, AMD and Intel resource names in one table; AMD golden cases from GP0; HAMi and DRA as the neutral sharing paths; no NVIDIA-specific field outside the `kai` and `nvidia` backends |
| D1244 | **cuVS track, separate and optional:** contribute a Rust cuVS accelerator for Lance upstream (`lance-cuvs`, CAGRA #6534); Loams runs it as a GPU job class for large index builds; CPU builds stay default; no Lance fork |
| D1245 | **Not built:** the list in §14 |
| D1246 | **Phases GP0–GP4** of §1.4 with the efforts of §19 |
| D1247 | **Plan [GP1](../plans/2026-10-20-gp1-gpu-runai.md)** (stub) specifies GP0 task by task; GP1–GP4 get full specifications when their gates are met |
| D1248 | **GPU-free CI:** golden renders, kubeconform, policy checks and a kind job with `run-ai/fake-gpu-operator` and KAI's chart; real-GPU runs are manual and recorded before any support claim |
| D1249 | **Pins:** KAI, the fractioning operator, Workload Map, fake-gpu-operator, vLLM and kubeconform are pinned by version (and images by digest) at releases at least 14 days old, bumped deliberately |
| D1250 | **Dynamo and NIM are optional serving backends:** documented behind the gateway's `llm` module, never deployed or required by Loams |

## 17. Open questions (Q1015–Q1029)

| ID | Question | Default (all answered by the owner on 2026-10-20: the default) |
|---|---|---|
| Q1015 | When does GP0 start? | After v0.1.1 ships; it is docs and an example, so it does not compete with the launch, and it is ready for the NVIDIA conversation before GTC DC (30 Nov – 3 Dec 2026) |
| Q1016 | The example's default model server? | vLLM (Apache-2.0, NVIDIA and AMD), with Hugging Face TEI as the documented alternative and NIM as an optional example with its licence terms |
| Q1017 | Ship `compose.gpu.yaml` in the release bundle, or only in the docs? | In `deploy/release/` as an opt-in override with a `gpu` profile; nothing starts without the profile |
| Q1018 | GPU hardware for testing? | No GPU in CI; a manual real-GPU run on credits (NVIDIA Inception or a cloud programme) before the docs claim support; the owner approves the budget |
| Q1019 | Queue depth: per namespace only, or also per pool and per Jobs queue? | Per namespace only in GP2; deeper levels when a user asks |
| Q1020 | Bill GPUs on allocation or utilisation? | Allocation; utilisation shown, not billed |
| Q1021 | When to send the Workload Map definition upstream? | After `LoamsGpuPool` exists at `v1alpha1` and the kind test passes (GP2); keep it in Loams if declined |
| Q1022 | Agent GPU jobs: on by default? | Off; a person grants `gpu:submit` per Place; every job asks for approval until the Place sets a threshold; 1 GPU and 1 GPU-hour per run by default |
| Q1023 | Untrusted GPU code under gVisor `nvproxy` or microVM passthrough? | Not in this design; dedicated tainted nodes now; a §56 research spike after SB1d |
| Q1024 | Approve the cuVS track? | Yes, as optional work after GP1, one engineer, upstream-first in `lance-cuvs`, no Lance fork |
| Q1025 | Build a Run:ai REST or MCP adapter? | No, until a customer that runs the full Run:ai control plane asks |
| Q1026 | ~~Offer "Loams with KAI on your cluster, with an SLA" as an Ostrium service?~~ Superseded by D1323: Ostrium has no services offering. Loams with KAI stays free and open | Not offered as a service |
| Q1027 | Should the operator install KAI? | No; KAI is a prerequisite, detected and reported by a condition |
| Q1028 | A House GPU analytics track? | No; revisit if chDB or ClickHouse gains a GPU engine, or users ask for cuDF-Polars over `scan_loams()` |
| Q1029 | GPUs in Loams Run (IS4) on VMs? | Whole GPUs only, through the `compose`-style device reservation; KAI's queue vocabulary borrowed, not its code |

## 18. Amendments to earlier decisions

| Decision | Change |
|---|---|
| §17 §7 ("GPU scheduling and model serving … on the lab's Ray or Spark cluster") | Narrowed: Loams can run embed and rerank servers and GPU batch jobs on Kubernetes with KAI or plain device plugins; training and large inference stay external |
| D1175 / §65 §5.2 (kinds) | Adds `LoamsGpuPool` (D1233), owned by `LoamsCluster` |
| D443 (`LoamsNamespace` renders quota objects) | Also renders the namespace's KAI child queue when the cluster's GPU scheduler is `kai` (D1235) |
| §63 §6.3 (event types) | Adds `io.loams.dev.usage.gpu.allocation.v1` (D1236) |
| §26 §5 (`QueueSpec`, `EngineJob`) | Adds `QueueSpec.gpu` and `EngineJob::GpuBatch` (D1238) |

## 19. Effort (estimate, engineer-weeks)

| Phase | Effort | Gate to start |
|---|---|---|
| GP0 | 1–2 | v0.1.1 shipped (Q1015) |
| GP1 | 4–6 | JB1's `TikvJobStore` and engine-runner registry merged |
| GP2 | 4–6 | OP1's `LoamsCluster`, `LoamsNamespace` and LOC harness |
| GP3 | 2–3 | JL1a and GP1 |
| GP4 (cuVS, optional) | 4–6 plus upstream review | Q1024 approved; GP1 |
| **Total** | **15–23** | |

## 20. Sources

Read on 2026-10-20 through the GitHub API (repository metadata, releases, and files) and the web:

- Research note `pitch/research/nvidia.md` (Ostrium Labs, 2026-10-20), §1, §3 and §4; not in this repository.
- NVIDIA Technical Blog, [NVIDIA Run:ai Is Now Open Source as Part of NVIDIA DSX AI Factory Platform](https://developer.nvidia.com/blog/nvidia-runai-is-now-open-source-as-part-of-nvidia-dsx-ai-factory-platform/) (2026-10-13).
- `dsx-ai-factory/run-ai` 3.0.20: `README.md`, `LICENSE`, `COPYRIGHT`, `TRADEMARKS.md`, `CONTRIBUTING.md`, `docs/getting-started/product-support-policy.md`; [NGC `runai-oss/application-controller`](https://catalog.ngc.nvidia.com/orgs/nvidia/runai-oss/containers/application-controller).
- `kai-scheduler/KAI-Scheduler` v0.18.6: `README.md`, `SUPPORT.md`, `docs/quickstart/README.md`, `docs/quickstart/pods/gpu-pod.yaml`, `docs/quickstart/default-queues.yaml`, `docs/queues/README.md`, `docs/gpu-sharing/README.md`, `docs/batch/batch-job-min-member.yaml`, `docs/dra/README.md`, `deployments/kai-scheduler/crds/scheduling.run.ai_queues.yaml`, `scheduling.run.ai_podgroups.yaml`, `deployments/kai-scheduler/templates/default-queue.yaml`, `hack/setup-e2e-cluster.sh`, `pkg/scheduler/api/resource_info/resource_requirment.go`, `pkg/podgroupcontroller/controllers/resources/fraction.go`, `pkg/podgrouper/podgrouper/plugins/karta/grouper.go`; [CNCF project page](https://www.cncf.io/projects/kai-scheduler/).
- `kai-scheduler/gpu-fractioning` v0.1.8: `README.md`, `LICENSE`, `operator/charts/Chart.yaml`.
- `kai-scheduler/kai-resource-management` v0.18.6: `README.md`, `docs/overview.md`, `deployments/kai-resource-management-chart/crds/`.
- `dsx-ai-factory/workload-map` v0.2.8: `README.md`, `CONTRIBUTING.md`, `charts/karta/crds/run.ai_kartas.yaml`, `docs/catalog/`, `karta/sdk/pkg/catalog/kartas/`.
- `run-ai/fake-gpu-operator` v0.2.1 (MIT).
- `Project-HAMi/HAMi` v2.10.0 (Apache-2.0, CNCF Incubating): `README.md` (device backends, including AMD).
- `kubernetes-sigs/dra-driver-nvidia-gpu` v0.5.1, `ROCm/k8s-device-plugin` v1.31.0.11, `NVIDIA/dcgm-exporter` 4.8.4 (all Apache-2.0); `vllm-project/vllm` (Apache-2.0; v0.31.0 of 2026-10-05, v0.32.0 of 2026-10-18); `huggingface/text-embeddings-inference` v1.9.4 (Apache-2.0); `ai-dynamo/dynamo` v1.5.1 (licence field `NOASSERTION`).
- `lance-format/lance-cuvs` (Apache-2.0, tags `v0.1.0b1`, `v0.1.0b2`, `README.md`); `lance-format/lance` issue [#6534](https://github.com/lance-format/lance/issues/6534) and closed PRs #2816 (merged), #6366, #6463, #6996, #8411.
- Docker, [Enable GPU support in Compose](https://docs.docker.com/compose/how-tos/gpu-support/); AMD, [ROCm in Docker](https://rocm.docs.amd.com/projects/install-on-linux/en/latest/how-to/docker.html).
- Loams at `dev` (`bee207385`): designs §06, §09, §17, §21, §24, §26, §38, §52, §56, §57, §60, §61, §63, §64, §65; plans JB1, MM1, OP1; `deploy/helm/loams-connectors/` and `.github/workflows/fabric.yml` (the chart test pattern); `deploy/release/` and `.github/workflows/release-compose.yml`.
