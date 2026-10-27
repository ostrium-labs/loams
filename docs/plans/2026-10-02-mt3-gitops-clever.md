# MT3 — GitOps with Clever Cloud's Open-Source Stack, Knative and Zitadel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Execute task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (paths, wave numbers, health expressions, defaults), use them verbatim. The code is not pre-written in this plan (M0.3 Ruling 1).

> **Status: Planned** (2026-10-02; the identity waves amended 2026-10-21 by [§76](../design/76-identity-and-authorization.md) §12.2 and D1600: Zitadel and OpenFGA). **Track MT** (design [§38](../design/38-knative-authentik-gitops.md) §6, D453–D456; amends [§25](../design/25-clever-cloud-stack.md) §6). Builds on §25's layout (`deploy/gitops/`, the umbrella chart `deploy/helm/loams-stack`, D186) and on IA1 (Zitadel's values and configuration; it supersedes MT1 Task 1) and MT2 Task 1 (the Knative chart). If §25's layout has not been built yet, Task 1 creates the minimum of it that the waves need and records that in "Rulings made during execution". Branches `mt3-t<N>`, stacked; PRs target `main`. MT3 is YAML, Lua and shell; it adds no Rust crate.

**Goal:**
- §25's Argo CD app-of-apps gains D455's waves: CloudNativePG and the Knative Operator, Zitadel's Postgres, Zitadel with Loams’ configuration and OpenFGA (§76 §12.2), `KnativeServing` and `KnativeEventing`, and `loams-knative-source`.
- Lua health checks for the new resources.
- A Flux layout with the same order (D454).
- The k3s small profile (D456) and the Clever Kubernetes Engine profile with Clever's Terraform and Karpenter providers (D453), both tested.

**Architecture:**
- **Argo CD is the engine** (D186, D453). Clever Cloud supplies `loams-operator`'s skeleton (D185) and, on CKE, the Terraform provider below GitOps and the Karpenter provider in wave −1.
- **Upstream charts by reference.** Zitadel's chart (`zitadel/zitadel-charts`, Apache-2.0; Zitadel itself AGPL-3.0, run unmodified, D1602), OpenFGA's chart and Knative's operator manifests are pulled by Argo CD from their upstream repositories with pinned versions; nothing upstream is copied into `deploy/` (D452).
- **Every component is a toggle.** `envs/<profile>/values.yaml` turns Knative and identity (Zitadel and OpenFGA) on or off; with both off, the waves are §25's.

**Tech Stack:** Argo CD v3.5.x (pinned), Flux v2.9.x (pinned), Helm 3, k3d and k3s v1.37.1+k3s1 with `--disable traefik`, CloudNativePG v1.30.x, Knative Operator knative-v1.23.1, Zitadel chart `zitadel-10.4.0`, OpenFGA chart `openfga-0.3.16` (§76 §12.2), `terraform-provider-clevercloud` v2.3.0, `karpenter-provider-clever-cloud` v0.13.0. Test tools: `kubeconform`, `helm template`, `argocd app wait` in CI, `flux check`, `shellcheck`. Task 0 checks every version and licence.

**Spec:**
- [§38](../design/38-knative-authentik-gitops.md) §4.4, §6 (all), §9, §12 Q447, Q449, Q453.
- [§25](../design/25-clever-cloud-stack.md) §5 (the operator fork), §6 (layout, waves, health), Q-RT-11.
- [IA1](2026-10-21-ia1-identity-authz.md) (it supersedes MT1 Task 1); [MT2](2026-10-02-mt2-knative.md) Task 1.

## Global Constraints

Same as the M1 overview §8, plus:
- **No vendored upstream charts** with copyleft licences (D452, D457). A CI check fails if `deploy/` contains a `Chart.yaml` whose `name` is `zitadel` or `openfga`.
- **No secrets in Git.** Secrets are `ExternalSecret`s or Dapr secret-store references (D189); a `gitleaks` pass runs on `deploy/`.
- **CKE tests cost money.** The CKE profile is validated by `terraform validate` and `helm template` in CI; a real CKE apply is manual and needs the owner's account decision.
- **The build machine.** No cargo build in this plan. k3d jobs run in CI.
- **Commit areas:** `deploy`, `gitops`, `ci`, `docs`.

## Rulings made while writing this plan

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | **Waves** exactly as §38 §6.3: −2 CRDs (+ Knative Operator and CNPG CRDs); −1 operators (+ Knative Operator, CNPG operator); 1 `zitadel-db`; 2 `zitadel` and `openfga`; 3 `knative` (`KnativeServing`, `KnativeEventing`); 5 `knative-sources` | D455, §76 §12.2 | Reordering later is a values change |
| 2 | **Health checks** (Argo CD `resource.customizations.health`, Lua): `operator.knative.dev/KnativeServing` and `KnativeEventing` → Healthy when `status.conditions` has `Ready=True`, Progressing otherwise, Degraded when `Ready=False` with a reason; `postgresql.cnpg.io/Cluster` → Healthy when `status.phase == "Cluster in healthy state"`; `sources.loams.dev/LoamsSource` → `Ready=True`. Zitadel's `Application` is Healthy when its `Deployment` is available and a `Job` hook `zitadel-config-check` (a curl of Zitadel's readiness endpoint and a check through its management API that the Git configuration is applied; verify both in Task 0) succeeded | Waves wait only for healthy resources (§25 §6.1) | A Lua error marks the resource Unknown; tested in Task 2 |
| 3 | **Zitadel's `Application`** sources the chart from `zitadel/zitadel-charts`, `targetRevision: 10.4.0`, with `deploy/zitadel/values.yaml` from this repository as a second source (`$values`); the configuration (orgs, projects, applications) comes from Git as IA1 lays it out, since Zitadel starts empty and is configured from Git (§76 §13) | Argo CD multi-source keeps the chart upstream and the values here | Multi-source is Argo CD ≥ 2.6; pinned v3.5 has it |
| 4 | **Profiles**: `envs/dev-k3d` (one replica everywhere, RustFS single node, one PD and TiKV, Zitadel and OpenFGA one replica each on one CNPG instance, Knative on); `envs/selfhosted` (three TiKV, CNPG with two replicas, Knative off by default); `envs/clever-cke` (Cellar or RustFS, Karpenter on, Knative off by default); `envs/k3s-single` (as `dev-k3d`, plus the Flux variant) | D454, D456 | New profiles are new values files |
| 5 | **Flux layout** in `deploy/gitops/flux/`: one `Kustomization` per wave with `dependsOn` on the previous wave and `wait: true`; `HelmRelease`s for upstream charts with `spec.chart.spec.sourceRef` pointing at `HelmRepository` sources (Flux's `chartRef` accepts only `OCIRepository`, `HelmChart` or `ExternalArtifact`); the same values files | D454: same order, no second source of truth | Flux and Argo CD drift if someone edits one; a CI test renders both and diffs the resource sets |

## Carried in

From §25: the umbrella chart, the root `Application`, the existing health customizations. From IA1: `deploy/zitadel/values.yaml` and the Zitadel configuration. From MT2: `deploy/helm/loams-stack/charts/knative/`.

## Review Focus

1. **Order is enforced by health, not by luck.** Tests: Task 2 (`zitadel_waits_for_db`, `knative_waits_for_operator`).
2. **Nothing copyleft or secret is in `deploy/`.** Tests: Task 1 (`no_vendored_identity_chart`, `gitleaks_clean`).
3. **Flux and Argo CD apply the same resources.** Tests: Task 5 (`flux_and_argo_render_same_set`).

## Tasks

### Task 0: Reconcile and check

**Produces:** which parts of §25's layout exist on `main`; pinned versions and licences for every component above; Argo CD's and Flux's measured RSS on k3d (for Q449 and Q-RT-11).

**Tests:** none. **Commit:** `docs: MT3 task 0 findings`.

### Task 1: Waves and Applications

**Files:** `deploy/gitops/apps/{cnpg,knative-operator,zitadel-db,zitadel,openfga,knative,knative-sources}.yaml`, `deploy/gitops/envs/*/values.yaml`, `.github/workflows/gitops.yml`.

**Produces:** Ruling 1's `Application`s with `argocd.argoproj.io/sync-wave` annotations; Ruling 3's multi-source Zitadel `Application`; Ruling 4's profiles; the CI checks of the Global Constraints.

**Tests:** `kubeconform_all_profiles`; `waves_match_design` (a script reads every `Application`'s wave and compares to Ruling 1); `no_vendored_identity_chart`; `gitleaks_clean`; `disabled_components_render_nothing`.

**Commit:** `gitops: waves for CloudNativePG, Zitadel, OpenFGA and Knative`.

### Task 2: Health checks

**Files:** `deploy/gitops/bootstrap/argocd/argocd-cm.yaml` (`resource.customizations.health.*`), `deploy/gitops/hooks/zitadel-config-check.yaml`.

**Produces:** Ruling 2's Lua checks and the hook `Job`.

**Tests:** Lua unit tests with Argo CD's `argocd admin settings resource-overrides health` against fixture resources (Ready, not Ready, missing status) for each kind; `zitadel_waits_for_db` and `knative_waits_for_operator` on k3d (stop the dependency, assert the dependent wave stays OutOfSync/Progressing).

**Commit:** `gitops: health checks for Knative, CloudNativePG, Zitadel and LoamsSource`.

### Task 3: Zitadel's database and secrets

**Files:** `deploy/gitops/apps/zitadel-db.yaml` (a CNPG `Cluster` `zitadel-db`, database `zitadel`, owner `zitadel`; OpenFGA gets its own database, D1617), the secret references in `deploy/zitadel/values.yaml`.

**Produces:** the CNPG cluster per profile (one instance in `dev-k3d`, two in `selfhosted`); Zitadel's database settings from the CNPG-generated secret; its masterkey and the first admin's credentials from the secret path (D189; Task 0 checks the chart's value names).

**Tests:** `zitadel_connects_to_cnpg`; `zitadel_survives_db_failover` (`selfhosted` profile on k3d: kill the primary, Zitadel's readiness endpoint recovers within 2 minutes).

**Commit:** `gitops: Zitadel on CloudNativePG with secrets from the secret store`.

### Task 4: The Clever Kubernetes Engine profile

**Files:** `deploy/infra/clever-cke/` (Terraform: cluster, node groups, Cellar or none), `deploy/gitops/envs/clever-cke/values.yaml` (Karpenter's `CleverNodeClass`, storage classes).

**Produces:** a CKE profile that applies Terraform outside Argo CD, then bootstraps Argo CD with the same root `Application`.

**Tests:** `terraform_validate_clever_cke`; `helm_template_clever_cke`; a manual runbook `docs/guides/gitops-clever-cke.md` with the apply steps (no CI apply).

**Commit:** `gitops: Clever Kubernetes Engine profile with Clever's Terraform and Karpenter providers`.

### Task 5: The Flux layout

**Files:** `deploy/gitops/flux/`.

**Produces:** Ruling 5's layout.

**Tests:** `flux_and_argo_render_same_set` (render both for `k3s-single` and diff the set of `(kind, namespace, name)`); `flux_bootstrap_on_k3d_reaches_ready`.

**Commit:** `gitops: a Flux layout with the same order`.

### Task 6: The small profile, measured

**Files:** `scripts/k3s/single-node.sh`, `docs/guides/gitops-small.md`.

**Produces:** a single-node k3s install script (`--disable traefik`) and a measured table (RSS per component: Argo CD or Flux, RustFS, PD, TiKV, Zitadel and OpenFGA with CNPG, Knative Serving with Kourier, the engine) for §38 §6.4 and Q-RT-11.

**Tests:** `single_node_profile_reaches_ready` (CI on k3d with the same values); the measurements recorded, not gated.

**Commit:** `gitops: the single-node k3s profile, with measured footprints`.

### Task 7: Docs and close

**Files:** §38 §6 (as built), §25 §6.3, the guides, `CHANGELOG.md`.

**Tests:** `gitops` job green.

**Commit:** `docs: record MT3 as built`.

## What MT3 leaves to others

| Item | Where |
|---|---|
| Tenant onboarding through Git, upgrade rings, hub-and-spoke or per-cluster Argo CD, the BYOC agent | [MT4](2026-10-02-mt4-byoc-control-plane.md) (open source since 2026-10-02, D540; this row said `loams-platform`) |
| A real CKE apply | Manual, after the owner's account decision |

## PR sizes

| Task | Expected size |
|---|---|
| 1 | ~500 lines (YAML, scripts) |
| 2 | ~400 lines (Lua, YAML) |
| 3 | ~250 lines |
| 4 | ~400 lines (Terraform, YAML) |
| 5 | ~500 lines |
| 6 | ~200 lines plus docs |
| 7 | docs |

## Rulings made during execution

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| — | (Task 0 fills this table) | | |
