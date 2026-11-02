# OP2-C: One-Click Deploy (DEP1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One command (`loams deploy`), one Desktop deep link or one copy-paste OpenTofu root takes a signed marketplace listing to a Ready `LoamsInstall` on a new k3s VM, managed Kubernetes cluster or bare server in the user's own account. It shows a cost estimate and asks for confirmation before anything is created. It tears everything down again on the same state.

**Architecture:**
- **The home.** New OpenTofu modules (`aws-k3s`, `gcp-k3s`, `azure-k3s`, `aws-eks`, `gcp-gke`, `azure-aks`, `server-ssh`) provision a home. On k3s homes, cloud-init or `install.sh --k3s` installs k3s and the OP2-A operator chart, with `home.create=true` and `home.spec` taken from `deploy/operator/home.yaml`. On managed Kubernetes, a `helm_release` does the same.
- **The CLI.** A new crate, `loams-deploy`, embeds those modules and their example roots. It renders one root per deployment under `~/.loams/deploy/<name>/`, runs `tofu plan`, and prices the plan's `cost_items` output and the listing's `cost_hint` against `deploy/tofu/prices.toml`. After confirmation it applies, waits for `loamscluster/home` to be Ready, and creates the `LoamsInstall` through the Kubernetes API with the kubeconfig from the module output. It walks the user through OP2-A's cluster-scoped confirmation when the install asks for it.
- **Desktop** parses `loams://deploy?…`, shows a deploy page and drives the same CLI.

**Tech Stack:**
- OpenTofu ≥ 1.8 (CI pins 1.13.2), with providers `hashicorp/{aws ~> 6.0, google, azurerm, random, tls, helm, kubernetes}`.
- k3s and Tailscale, from their official releases.
- Rust: `clap`, `serde`, `toml`, `kube =3.1.0`, `k8s-openapi =0.27.1`, `jsonschema`, `loams-cost`, and `loams-agentd-marketplace` (`default-features = false, features = ["fetch"]`).
- Electron and TypeScript, with `vite-plus` tests.
- bash, with cloud-init 26.2 static checks.

**Spec:** `docs/design/86-open-platform-operator-v2.md` §6 (D2611–D2616, D2627, D2614), with §4.1 (the home, D2623; package limits, D2626).
- Background: `docs/design/70-byoc-clouds.md` §9.3 and §12.3, `docs/design/71-app-byoc-free-tiers.md` §9, and `docs/design/84-cost-attribution.md` §6.4 and §10 (D2370).
- Sibling plans: `docs/plans/2026-11-02-op2a-operator-v2-core.md` and `docs/plans/2026-11-02-op2b-open-marketplace.md`.

## Interfaces for sibling plans

### Consumed exactly (defined by the sibling plans; do not redefine)

**From OP2-A ("Interfaces for sibling plans" table):**
- **Chart:** `oci://ghcr.io/ostrium-labs/charts/loams-operator`, version = the Loams release.
  - Values: `image.repository`, `image.digest` (the release fills it), `replicas`, `home.create`, `home.spec`, `marketplace.index`, `marketplace.publicKey`, `marketplace.configMap`.
  - It installs only into `loams-system`.
  - This plan sets `home.create=true` and `home.spec`. Every other value passes through from `operator_values` unchanged.
- **Home:** `LoamsCluster` `home` in `loams-system`. The default `home.spec` is the `spec` of `deploy/operator/home.yaml` (k3s single-node, `exposure.mode: internal`). The Secrets named in its comments are created by the target (Task 4).
- **Readiness:**
  - the home: `loamscluster/home` condition `Ready`, waited up to 15 minutes;
  - an install: `loamsinstall/<name>` condition `Ready`.
- **`LoamsInstall`:** `loams.dev/v1alpha1`, in `loams-system`, with `spec.cluster: home`, `spec.listing.{id, version}` and `spec.values`. The operator places its workloads in `loams-i-<install name>`.
- **Cluster-scoped confirmation:**
  - The install reports condition `AwaitingConfirmation=True`, `status.clusterScoped.objects[] {apiVersion, kind, name}` and `status.clusterScoped.digest`.
  - ConfigMap `cluster-scoped` (key `manifest.yaml`) sits in `loams-i-<name>`.
  - The client applies that manifest with its own credentials (server-side apply, field manager `loams-approver`), then sets `spec.approvedClusterScoped` to the digest.
- **Workload identity:** the `install` service account in each `loams-i-*` namespace.

**From OP2-B ("Interfaces for sibling plans" block),** in `crates/loams-agentd-marketplace` (dev: `"testkit"`):
- **Types:**
  - `ListingV2` (`id`, `kind: ListingKind`, `version`, `package: Option<Package>`, `targets: Vec<Target>`, `requires: Requires { storage, cpu_millis, memory_mib, gpu }`, `cost_hint: Option<CostHint { cpu_millis, memory_mib, storage_gib, egress_gib_per_month, managed }>`, `values_schema`, `validate_values`);
  - `IndexFiles`, `TrustAnchor { Keyless, PublicKey { pem } }` with `TrustAnchor::official()`, `SignedIndex::verify(&IndexFiles, &TrustAnchor)`;
  - `VerifiedListing { listing, tier, listing_sha256, warning }` and `VerifyError`;
  - `testkit::signed_v2_index`.
- **Function:**

  ```rust
  fn verify_listing(index: &SignedIndex, id: &str, version: &semver::Version) -> Result<VerifiedListing, VerifyError>
  ```

- **Index files:** `index.v2.json`, `index.v2.json.sigstore.json`, `revocations.json` and `revocations.json.sigstore.json`, from `https://ostrium-labs.github.io/loams-marketplace/`, fetched with OP2-B's `HttpsFetcher`.

### Produced (this plan owns them)

- **Target kinds:**
  - operator targets: `aws-k3s`, `gcp-k3s`, `azure-k3s`, `aws-eks`, `gcp-gke`, `azure-aks`, `server-ssh`;
  - listed but not deployable by `loams deploy`: `aws-vm`, `gcp-vm`, `azure-vm` (compose); `aws-free`, `gcp-free`, `azure-free` (apps, `loams app deploy`); `laptop` (EDGE1).
- **`deploy/tofu/targets.toml`.** One `[targets.<kind>]` table each, with:
  - `module`, `cloud` (`aws|gcp|azure|server|none`), `class` (`k3s|managed-k8s|server|compose|app-free|laptop`), `operator`, `available`, `free_tier`, `default_region`;
  - `sizes.{small,medium} = {cpu_millis, memory_mib, nodes}` and `reserve = {cpu_millis, memory_mib}` (the platform base; estimate);
  - `disk_sku`, `egress_sku`, `required_inputs`, `keep_on_destroy` (state addresses), `credentials` (`aws|gcloud|az|ssh`), `needs_tailnet`.
- **The operator-target module contract.** Every operator module and example root:
  - takes `name`, `release`, `size`, `home_spec` (any; the CLI passes `home.yaml`'s spec), `operator_values` (any, default `{}`), `access_db` (`"local"` or `"external"`) and `tags`/`labels`;
  - outputs `kubeconfig` (sensitive), `api_url`, `cost_items` (`list(object({sku=string, quantity=number, unit=string, note=string}))`), `bucket_url` (null for `server-ssh`), `cloud` and `region`;
  - tags every resource `loams-deploy=<name>`.
- **ConfigMap `loams-system/loams-target`:** the keys `deployment`, `target`, `cloud`, `region`, `bucket_url` and `workload_identity`. It holds facts for DEP2 only.
- **The deep link** `loams://deploy?listing=<namespace>/<name>@<version>&target=<kind>`. Exactly these two keys. The version is SemVer without build metadata. It opens the Desktop route `/deploy/new/<namespace>/<name>/<version>/<target>`.
- **The CLI** `loams deploy`, with `loams_deploy::cli::DeployArgs` (Task 17) and `pub fn run(args: &DeployArgs) -> ExitCode`.
  - Exit codes: 0 ok, 2 usage or preflight, 3 refused by the cost guard or declined, 4 listing refused, 5 `tofu` failed, 6 not Ready in time.
  - JSON shapes `DeployPlanJson` and `DeployEvent` (Task 17).
- **The cost estimator** (Task 2):

  ```rust
  loams_deploy::estimate::estimate_monthly_cost(target: &DeployTarget, region: &str, items: &[CostItem], hint: Option<&CostHint>, prices: &PriceTable) -> CostEstimate
  ```

  (`DeployTarget`, so the name does not clash with OP2-B's `Target` enum.)

## Decisions made in this plan (spec gaps)

- **C1. The Access database.** The home needs a Postgres DSN Secret (OP2-A R8).
  - The default is `access_db = "local"`: one official `postgres` image (pinned by digest in `deploy/byoc/k3s.env`, mirrored as a module default) as StatefulSet `loams-access-db` in `loams-system` with a 10 GiB PVC. The password is `random_password`, kept in the encrypted state.
  - `--access-db-dsn-file` uses managed Postgres instead (D2631's production default).
  - Either way the target creates the Secrets that `home.yaml` names.
- **C2. Chart integrity.** k3s VMs pull the chart through GHCR's registry API by tag, and record the manifest digest in `loams-target` as `chart_digest`. Signature verification of the chart waits on the release signing it. This is a gap reported to OP2-A, not built here.
- **C3. Workload identity.**
  - EKS gets an IRSA role trusting `system:serviceaccount:loams-i-*:install` (`StringLike`).
  - GKE and AKS get the identity (a GSA, or a UAMI) and bucket access only. Their per-install bindings come with DEP2's `StorageBinding`s.
  - On k3s VMs, pods are kept off the metadata service (Review Focus 5).
- **C4. Listings.** DEP1 refuses a listing before any cloud call (exit 4) when:
  - its kind is not `app` or `engine` (OP2-A R5);
  - its package is `kustomize`, `wasm` or `binary`;
  - `requires.storage` is non-empty (bindings are DEP2);
  - `requires.gpu` is set;
  - it does not fit the largest size.
- **C5. Budgets** (D1381) are opt-in in DEP1 (`--budget-email`), because a budget needs an alert address, and on Google Cloud a billing-account permission. The D2615 confirmation is always on.

## Global Constraints

- **OpenTofu only.** No Terraform and no BSL HashiCorp tool sources (the existing `run.sh` check). The CLI needs `tofu` ≥ 1.8 on `PATH` or in `LOAMS_TOFU`; CI pins 1.13.2 as `byoc-static.yml` does.
- **Everything runs on the user's machine with the user's credentials** (D2614): their cloud CLI sessions, and SSH for `server-ssh`. Ostrium holds no state and no credentials.
  - A deploy calls only GitHub (releases, GHCR) and the marketplace index on GitHub Pages.
  - It sends no telemetry.
- **State.**
  - It is local by default (`~/.loams/deploy/<name>/root/terraform.tfstate`), or a bucket the user names (`--state s3://…|gs://…|azurerm://…`).
  - State and plan encryption are always enforced on operator targets, because the CA keys, the Access database password and the setup token are in state (D1827's rule).
- **Cost guard** (D2615):
  - The resources and an estimated monthly cost are shown before every apply. Confirmation is required.
  - `--yes` is refused on an interactive terminal; it is for CI, and for Desktop after its own native confirmation.
  - Cluster-scoped objects need their own confirmation (D2626).
  - Every estimate is labelled `estimated`, with its price-table read date (D2370). Free-tier lines and targets say "free tier".
- **No inbound rule** on any VM, as in the `*-vm` modules. Managed Kubernetes APIs authenticate with cloud IAM only (no local accounts, no client certificates).
- **The `*-vm` modules keep their interface and behaviour.** `deploy/byoc/gen-cloud-init.sh --check` and `tests/byoc/static/run.sh` stay green. A first-apply bug fix in a `*-vm` module is allowed, with a test, and changes no variable or output.
- **Official upstream only** (D2600): k3s from `k3s-io/k3s` releases with pinned SHA-256s, Tailscale from `pkgs.tailscale.com` with a pinned key fingerprint, the official `postgres` image by digest, and official providers.
- **VMs are x86-64 only** (D1384). `server-ssh` accepts amd64 and arm64 Ubuntu/Debian hosts.
- **Real-cloud runs never run on pull requests,** only from `workflow_dispatch` or locally on the owner's machine.
  - Budget caps, shared with CB1's ledger (`bench/cloud/rows.json`): **AWS $60, GCP $100, Azure $25**.
  - `bench/cloud/ledger.py check` gates every run: at most 25% of the cap per run, at most 60% total, 06:00–20:00 IST, no hold file.
  - Destroy always runs.
- **Rust builds.** Use the global target dir (never set `CARGO_TARGET_DIR`, never build in `/tmp`), one heavy build at a time, `-j 4`.
- **CI stays offline.** `tofu validate` and `tofu test` (mock providers) run in `byoc-static.yml`. Rust tests use a fake `tofu` runner, a fake kube and `testkit::signed_v2_index`.

## Review Focus

1. **A region outside the price table** (for example `eu-west-1`). Every line is flagged `RegionNotInTable`, with the priced region shown, and the confirmation says so. It must never show $0, and never show us-east-1 silently. (Task 2.)
2. **An interrupted apply** (Ctrl-C, a crash, a lost network). `deployment.toml` is written with status `applying` before `tofu apply` starts, and `loams deploy destroy <name>` works on the partial state. (Task 17.)
3. **A crafted deep link**: traversal, extra or duplicate keys, a missing version, build metadata, an odd target. It is rejected. A valid link only opens the page.
   - Nothing is planned without a click.
   - Nothing is applied, and no cluster-scoped object approved, without a main-process native confirmation built from the main process's own plan or status.
   (Tasks 18–19.)
4. **Teardown of a deployment whose bucket holds data.**
   - By default the bucket and its key are removed from state, kept, and printed.
   - `--delete-bucket` on a non-empty bucket fails with a message that names it.
   (Task 17.)
5. **A pod on a k3s VM reaching the cloud metadata service** (169.254.169.254) and taking the VM's role. The VM role reads the install's secrets and bucket, so a Community listing must not get it.
   - AWS k3s VMs use IMDSv2 hop limit 1.
   - The bootstrap drops pod-CIDR traffic to 169.254.169.254 on every cloud, and the rule is re-applied at boot.
   (Tasks 3, 5.)

---

## File structure

```
deploy/tofu/targets.toml, deploy/tofu/prices.toml        (Task 1)
deploy/byoc/k3s.env                                      pins: k3s, Tailscale key, postgres digest (Task 3)
deploy/byoc/cloud-init-k3s.tmpl.yaml, {aws,gcp,azure}/cloud-init-k3s.yaml (generated)
deploy/byoc/lib/{loams-k3s-bootstrap,loams-k3s.service,loams-k3s-imds-block.service}
deploy/byoc/k3s/access-db.yaml.tmpl                      the C1 manifest (k3s path)
deploy/tofu/modules/shared/main.tf                       + `template` and k3s placeholders (Task 4)
deploy/tofu/modules/k3s-home/                            (Task 4)
deploy/tofu/modules/{aws,gcp,azure}-k3s/                 (Tasks 5–7)
deploy/tofu/modules/k8s-operator/                        (Task 8)
deploy/tofu/modules/{aws-eks,gcp-gke,azure-aks}/         (Tasks 9–11)
deploy/tofu/modules/server-ssh/                          (Task 12)
deploy/tofu/examples/<operator target>/                  copy-paste roots; also the CLI's templates
crates/loams-deploy/                                     targets, prices, estimate, bundle, render, record, tofu, listing, install, cli, flow
crates/loams/src/main.rs                                 `Deploy(loams_deploy::cli::DeployArgs)` (Task 17)
apps/desktop-electron/src/{shared/deeplink.ts,shared/contracts.ts,main/deploy/} (Task 18)
web/plugins/desktop-deploy/                              (Task 19)
tests/deploy/real/, .github/workflows/deploy-real.yml    (Task 20)
```

---

### Task 1: Target registry and price table

**Files:**
- Create: `deploy/tofu/targets.toml`, `deploy/tofu/prices.toml`
- Create: `crates/loams-deploy/{Cargo.toml,BUCK,src/lib.rs,src/targets.rs,src/prices.rs}`
- Modify: root `Cargo.toml` (workspace member)
- Test: `crates/loams-deploy/tests/registry.rs`

**Interfaces:**
- Produces:
  - `DeployTargets::embedded() -> &'static DeployTargets` and `DeployTargets::get(&self, kind: &str) -> Option<&DeployTarget>`.
  - `DeployTarget { kind, module, cloud, class: TargetClass, operator, available, free_tier, default_region, sizes: BTreeMap<String, Size { cpu_millis: u32, memory_mib: u32, nodes: u32 }>, reserve: Capacity { cpu_millis, memory_mib }, disk_sku, egress_sku, required_inputs: Vec<String>, keep_on_destroy: Vec<String>, credentials, needs_tailnet }`.
  - `PriceTable::embedded() -> &'static PriceTable` and `PriceTable::get(&self, sku) -> Option<&PriceEntry>`.
  - `PriceEntry { unit: PriceUnit, usd: loams_meter_core::Amount, regions: Vec<String>, source: PriceSource /* list | estimate */, url: String, verified: bool, free: Option<FreeAllowance { quantity: u64, note: String }> }`.
  - `PriceUnit { Hour, GibMonth, Gib, Month }`; `PriceTable { read: time::Date, .. }`.
- **SKU naming:** `<cloud>/<service>/<name>`, for example `aws/ec2/t3.large`, `aws/ebs/gp3`, `aws/vpc/public-ipv4`, `aws/eks/control-plane`, `aws/network/egress-internet`, `gcp/gke/zonal-management` and `azure/disk/premium-ssd-p6`.
- **Default sizes** (the same VM shapes as `*-vm`):
  - k3s VMs: AWS `t3.large`/`t3.xlarge`, Google Cloud `e2-standard-2`/`e2-standard-4`, Azure `Standard_D2s_v5`/`Standard_D4s_v5`; 60 GiB disk.
  - Managed Kubernetes: `small` = 1 node, `medium` = 3 nodes, of `t3.xlarge`, `e2-standard-4` or `Standard_D4s_v5`.
  - `reserve = {cpu_millis = 1500, memory_mib = 4096}`: the platform base plus the C1 Postgres (estimate).

- [ ] **Step 1: Write the failing tests.**
  - `every_target_module_exists`: an available target's `deploy/tofu/modules/<module>/main.tf` exists, and so does `deploy/tofu/examples/<kind>/main.tf` when it is an operator target.
  - `target_kinds_are_exactly`: the 14 kinds above.
  - `laptop_is_not_available` and `vm_and_free_targets_are_not_operator`.
  - `free_targets_are_labelled`.
  - `every_sku_in_modules_is_priced`: grep `"(aws|gcp|azure)/[a-z0-9._-]+/[A-Za-z0-9._-]+"` over `deploy/tofu/modules/*-{k3s,eks,gke,aks}/*.tf`; every match has an entry. It passes vacuously until Task 5.
  - `prices_carry_source_and_date`: a `source`, a non-empty `url`, and `read` ≥ 2026-11-02.
- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test registry`. Expected: a compile error (empty crate).
- [ ] **Step 3: Implement.**
  - Embed both TOML files with `include_str!("../../../deploy/tofu/…")` and parse them in a `OnceLock`.
  - Prices are list prices for `us-east-1`, `us-central1` and `eastus`, each with its pricing-page URL and `verified = false`.
  - `gcp/gke/zonal-management` carries `free = {quantity = 730, note = "one zonal or Autopilot cluster per billing account"}`. `azure/aks/control-plane-free` is $0.
- [ ] **Step 4: Run the tests and confirm they pass.** Run `cargo test -p loams-deploy -j 4 --test registry`. Expected: PASS.
- [ ] **Step 5: Commit.** `git add deploy/tofu/targets.toml deploy/tofu/prices.toml crates/loams-deploy Cargo.toml && git commit -m "feat(deploy): target registry and dated price table (DEP1)"`

### Task 2: Cost estimator

**Files:**
- Create: `crates/loams-deploy/src/estimate.rs`
- Test: `crates/loams-deploy/tests/estimate.rs`

**Interfaces:**
- Consumes:
  - Task 1;
  - OP2-B's `CostHint { cpu_millis, memory_mib, storage_gib, egress_gib_per_month, managed: Vec<StorageKind> }`;
  - `loams_cost::Basis` and `loams_meter_core::Amount`.
- Produces:
  - `CostItem { sku: String, quantity: u64, unit: PriceUnit, note: Option<String> }`, deserializable from the `cost_items` output;
  - `estimate_monthly_cost(target: &DeployTarget, region: &str, items: &[CostItem], hint: Option<&CostHint>, prices: &PriceTable) -> CostEstimate`;
  - `CostEstimate { lines: Vec<CostLine>, total_usd_month: Amount, complete: bool, basis: Basis /* always Estimated */, prices_read: Date, free_tier_target: bool, all_verified: bool }`;
  - `CostLine { sku, quantity, unit, usd_month: Amount, flags: Vec<CostFlag> }`;
  - `CostFlag { FreeTier { note }, RegionNotInTable { priced_region }, Unpriced { reason }, FromListingHint }`;
  - `format_usd(a: Amount) -> String` (cents, half up).
- **The hint mapping:**
  - `storage_gib` → `target.disk_sku` × GiB;
  - `egress_gib_per_month` → `target.egress_sku` × GiB;
  - each `managed` kind → an `Unpriced { reason: "billed by the managed service (DEP2)" }` line;
  - `cpu_millis` and `memory_mib` are not priced (Task 15 checks fit).

- [ ] **Step 1: Write the failing tests.**
  - `aws_k3s_small_us_east_1`: the items `[{aws/ec2/t3.large,730,Hour}, {aws/ebs/gp3,60,GibMonth}, {aws/vpc/public-ipv4,730,Hour}, {aws/kms/key,1,Month}, {aws/secretsmanager/secret,4,Month}]` sum exactly to Σ quantity × price from `prices.toml`. `complete`, and the basis is `Estimated`.
  - `region_not_in_table_is_flagged`: `eu-west-1` puts `RegionNotInTable{"us-east-1"}` on every line, and the total is non-zero.
  - `unknown_sku_is_unpriced_not_zeroed`: `complete == false`.
  - `gke_management_fee_is_free_tier`: $0.00, flagged `FreeTier`.
  - `hint_storage_and_egress_priced`: a hint `{storage_gib: 20, egress_gib_per_month: 50}` adds two `FromListingHint` lines.
  - `hint_managed_kafka_unpriced`.
  - `format_usd_rounds_half_up`: 1,234,567,890 nano-USD is `"1.23"`, and 1,235,000,000 is `"1.24"`.
- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test estimate`. Expected: FAIL (`estimate_monthly_cost` not found).
- [ ] **Step 3: Implement.** Use exact `Amount` arithmetic. A free allowance subtracts up to `free.quantity` units. A price whose `regions` lack `region` is used and flagged.
- [ ] **Step 4: Run the tests and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git add crates/loams-deploy && git commit -m "feat(deploy): monthly cost estimator over plan cost_items and listing cost_hint"`

### Task 3: k3s home bootstrap and cloud-init

**Files:**
- Create:
  - `deploy/byoc/k3s.env`: `K3S_VERSION`, `K3S_SHA256_AMD64`, `K3S_SHA256_ARM64`, `K3S_INSTALL_SHA256`, `TAILSCALE_KEY_FPR`, `POSTGRES_IMAGE` (`docker.io/library/postgres@sha256:…`);
  - `deploy/byoc/cloud-init-k3s.tmpl.yaml`, `deploy/byoc/k3s/access-db.yaml.tmpl`;
  - `deploy/byoc/lib/{loams-k3s-bootstrap,loams-k3s.service,loams-k3s-imds-block.service}`.
- Modify:
  - `deploy/byoc/gen-cloud-init.sh`: also build `<cloud>/cloud-init-k3s.yaml`, with the `@@K3S_PINS@@`, `@@K3S_BOOTSTRAP@@`, `@@K3S_UNIT@@`, `@@K3S_IMDS_UNIT@@` and `@@K3S_ACCESS_DB@@` lines.
  - `deploy/byoc/lib/loams-byoc-secrets`: an additive `--get NAME` that prints one secret's value and nothing else.
- Generate: `deploy/byoc/{aws,gcp,azure}/cloud-init-k3s.yaml`
- Test: `tests/byoc/static/test_k3s_bootstrap.sh`, wired into `tests/byoc/static/run.sh`

**Interfaces:**
- Consumes: the OP2-A chart at `ghcr.io/ostrium-labs/charts/loams-operator:<release>`, and `deploy/operator/home.yaml` (only through the values the module passes).
- Produces:
  - **Placeholders**, beyond the compose ones: `@@TAILNET_DOMAIN@@`, `@@DEPLOYMENT@@`, `@@ACCESS_DB@@` (`local|external`), `@@HOME_VALUES_B64@@` (base64 JSON helm values) and `@@TARGET_FACTS_B64@@`.
  - **Secrets the VM reads:**
    - `<prefix>/k3s-ca`: JSON `{server_ca_crt, server_ca_key, client_ca_crt, client_ca_key}`;
    - `<prefix>/access-db`: JSON `{password}` when `local`, `{dsn}` when `external`.
    - On Google Cloud and Azure, `-` replaces `/`.
  - **For `server-ssh`:** with `LOAMS_K3S_CLOUD=server`, the bootstrap reads `/etc/loams-k3s/secrets/{tailnet-authkey,k3s-ca.json,access-db.json}`.

**The bootstrap's order:**
1. Install Tailscale from the official apt repository (fingerprint checked), then run `tailscale up --authkey … --hostname $TS_HOSTNAME`.
2. Write `server-ca` and `client-ca` into `/var/lib/rancher/k3s/server/tls/` before the first start. k3s generates the others (verify at Task 21's first boot).
3. Fetch `k3s` (or `k3s-arm64`) and `install.sh` at the pinned tag, and check both SHA-256s.
4. Run `INSTALL_K3S_SKIP_DOWNLOAD=true INSTALL_K3S_EXEC="server --disable traefik --secrets-encryption --write-kubeconfig-mode 0600 --tls-san $TS_HOSTNAME --tls-san $TS_HOSTNAME.$TAILNET_DOMAIN"`.
5. Install and start `loams-k3s-imds-block.service`. It inserts `-s 10.42.0.0/16 -d 169.254.169.254 -j DROP` into a `LOAMS-IMDS` chain jumped to from `FORWARD`, after `k3s.service`, at every boot.
6. Pull the chart from GHCR's registry API with curl:
   - anonymous token, `GET /v2/ostrium-labs/charts/loams-operator/manifests/<release>`, then the `application/vnd.cncf.helm.chart.content.v1.tar+gzip` layer, checked against the layer digest;
   - store it in `/var/lib/rancher/k3s/server/static/charts/loams-operator-<release>.tgz`;
   - record the manifest digest as the `chart_digest` fact.
7. Write `/var/lib/rancher/k3s/server/manifests/loams-home.yaml` containing:
   - Namespace `loams-system`;
   - ConfigMap `loams-target` (facts + `chart_digest`);
   - the Secrets `home.yaml` names (C1);
   - for `local`: `access-db.yaml.tmpl` rendered (StatefulSet `loams-access-db`, `POSTGRES_IMAGE`, 10 GiB `local-path` PVC, Service);
   - `HelmChart` `loams-operator` in `kube-system`: `chart: https://%{KUBERNETES_API}%/static/charts/loams-operator-<release>.tgz`, `targetNamespace: loams-system`, `valuesContent` = the decoded home values.
8. Touch `/var/lib/loams-k3s/done`. A rerun after that is a no-op.

- [ ] **Step 1: Write the failing static tests** in `test_k3s_bootstrap.sh`, with `mock_cloud.py` plus fake `curl`, `tailscale`, `k3s` and `iptables` on `PATH`:
  - `k3s_cloud_init_files_are_generated_and_current`;
  - `cloud_init_k3s_validates_<cloud>` (template and rendered);
  - `k3s_scripts_in_<cloud>_match_lib`;
  - `k3s_binary_hash_mismatch_aborts`;
  - `chart_layer_digest_mismatch_aborts`;
  - `manifest_has_home_create_true_and_spec`;
  - `access_db_local_renders_statefulset_with_pinned_digest`;
  - `access_db_external_renders_no_statefulset`;
  - `imds_block_rule_inserted`;
  - `rerun_is_noop`;
  - `secrets_get_prints_only_value`;
  - `compose_cloud_init_unchanged_except_secrets_get` (diff against `git show HEAD:` limited to the `loams-byoc-secrets` block).
- [ ] **Step 2: Run them and confirm they fail.** Run `tests/byoc/static/run.sh`. Expected: `FAIL` on the new names.
- [ ] **Step 3: Implement**, then run `deploy/byoc/gen-cloud-init.sh`.
  - The unit is `Type=oneshot`, `Restart=on-failure`, `RestartSec=30` (Azure secrets may arrive after boot).
  - Pin k3s to the latest stable release at implementation time, with hashes from its `sha256sum-*.txt`.
- [ ] **Step 4: Run the tests and confirm they pass.** Run `SHELLCHECK=$(command -v shellcheck) tests/byoc/static/run.sh`. Expected: every line `ok`.
- [ ] **Step 5: Commit.** `git add deploy/byoc tests/byoc/static && git commit -m "feat(byoc): k3s home cloud-init and bootstrap (operator chart, home, access db)"`

### Task 4: `k3s-home` shared module

**Files:**
- Modify: `deploy/tofu/modules/shared/main.tf`. Add `template` (default `"cloud-init.yaml"`, validated to `cloud-init.yaml|cloud-init-k3s.yaml`) and the Task 3 placeholders, each a validated variable with a default of `""`. Base64 values must match `^[A-Za-z0-9+/=]*$`.
- Create: `deploy/tofu/modules/k3s-home/{main.tf,variables.tf,outputs.tf,versions.tf,tests/plan.tftest.hcl}`
- Modify: `tests/byoc/tofu/run.sh` (the module loop)

**Interfaces:**
- Produces:
  - **Variables:** `cloud` (`aws|gcp|azure|server`), the `shared` pass-through values, `release`, `hostname`, `tailnet_domain`, `deployment`, `home_spec`, `operator_values`, `access_db`, `access_db_dsn` (sensitive, null) and `target_facts`.
  - **Outputs:**
    - `user_data` and `user_data_base64gz` (null for `server`);
    - `ca_bundle_json` and `access_db_json` (sensitive);
    - `helm_values_json`, which is `jsonencode(merge(var.operator_values, {home = {create = true, spec = var.home_spec}}))`;
    - `kubeconfig` (sensitive; `https://<hostname>.<tailnet_domain>:6443`, an admin client cert with `O=system:masters, CN=loams-admin`, ECDSA P-256, valid one year);
    - `api_url`.
- **Before writing the module,** read `deploy/operator/home.yaml` and copy the Secret names and keys its comments give into `locals.home_secrets`. The bootstrap and `k8s-operator` both create exactly those.

- [ ] **Step 1: Write the failing `tofu test` runs:**
  - `vm_modules_render_unchanged`: the default template with `aws-vm`'s test inputs gives the file with the same replacements;
  - `kubeconfig_points_at_tailnet`;
  - `helm_values_set_home_create`: `.home.create == true`, and `operator_values` keys are kept;
  - `access_db_local_has_password_external_has_dsn`;
  - `server_has_no_user_data`;
  - `no_unfilled_placeholders`.
- [ ] **Step 2: Run them and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/k3s-home init -backend=false && tofu -chdir=deploy/tofu/modules/k3s-home test`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use `tls_private_key` ×3, `tls_self_signed_cert` ×2 (CAs, 10 years), `tls_locally_signed_cert` ×1 and `random_password` (32 characters, no specials).
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`, the existing modules included.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): k3s-home module (CAs, admin kubeconfig, home values, access db)"`

### Task 5: `aws-k3s` module and example

**Files:**
- Create: `deploy/tofu/modules/aws-k3s/{main.tf,variables.tf,outputs.tf,versions.tf,tests/plan.tftest.hcl}`
- Create: `deploy/tofu/examples/aws-k3s/{main.tf,encryption.tf}`
- Modify:
  - `tests/byoc/tofu/run.sh` (the loops);
  - `tests/byoc/tofu/check_encryption.py`: also require enforced encryption in every example named after an operator target in `targets.toml`.

**Interfaces:**
- Consumes: `k3s-home`.
- Produces:
  - **Inputs:** the contract, plus `region`, `tailnet_domain`, `secret_prefix`, `bucket`, `kms_key_arn`, `subnet_id`, `instance_type`, `disk_size_gb` and `ssm` (the same meanings as `aws-vm`).
  - **Outputs:** the contract, plus `instance_id`, `kms_key_arn` and `secret_prefix`.
  - **`cost_items`:** `aws/ec2/<type>` ×730, `aws/ebs/gp3` × disk GiB, `aws/vpc/public-ipv4` ×730, `aws/kms/key` ×1 when created, `aws/secretsmanager/secret` ×4 (tailnet key, setup token, k3s CA, access db).
  - **The example:**
    - Variables: `name`, `region`, `release`, `size`, `tailnet_domain`, `home_spec`, `operator_values`, `access_db`, `access_db_dsn` (sensitive) and `state_passphrase` (sensitive).
    - `encryption.tf` uses `key_provider "pbkdf2"` with `passphrase = var.state_passphrase`, and state and plan `enforced = true`.
    - It outputs everything in the contract.
    - `source = "../../modules/aws-k3s"` is its only `source` line.
    - A header comment gives the `git::https://github.com/ostrium-labs/loams.git//deploy/tofu/modules/aws-k3s?ref=v<release>` form.

- [ ] **Step 1: Write the failing tests.**
  - Copy `aws-vm`'s `defaults_never_public` and `bucket_is_private_versioned_and_encrypted` unchanged, except that the IMDS assert becomes `http_tokens == "required" && http_put_response_hop_limit == 1` (Review Focus 5).
  - `user_data_is_k3s_not_compose`: the gunzipped user data contains `/usr/local/bin/loams-k3s-bootstrap` and not `docker-ce`.
  - `k3s_ca_and_access_db_secrets_created`.
  - `cost_items_small` (`aws/ec2/t3.large` ×730).
  - `every_resource_tagged` (`loams-deploy == "t"`).
- [ ] **Step 2: Run them and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/aws-k3s init -backend=false && tofu -chdir=deploy/tofu/modules/aws-k3s test`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Start from a copy of `modules/aws-vm`. Keep the network, bucket, key, role and secret data source as they are.
  - Replace the `shared` call with `k3s-home` (`template = "cloud-init-k3s.yaml"`).
  - Add the two secrets and their versions.
  - Set `target_facts.workload_identity` to the instance role ARN.
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh && cargo test -p loams-deploy -j 4 --test registry`. Expected: all `ok` and PASS (`every_sku_in_modules_is_priced`, `example_with_keys_enforces_encryption`).
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): aws-k3s home target and copy-paste root"`

### Task 6: `gcp-k3s` module and example

**Files:** as Task 5, under `gcp-k3s`, starting from a copy of `modules/gcp-vm`.

**Interfaces:**
- Produces: as Task 5, with `project`, `region`, `zone`, `allow_iap_ssh`, `subnet_cidr` and `labels`.
- The secrets are `<prefix>-k3s-ca` and `<prefix>-access-db`.
- `cost_items`: `gcp/gce/<machine>`, `gcp/pd/balanced`, `gcp/network/external-ipv4`, `gcp/kms/key-version`, `gcp/secretmanager/secret-version` ×4.
- `workload_identity` is the VM service account's email.

- [ ] **Step 1: Write the failing tests.** Carry over `gcp-vm`'s hardening tests. Add `user_data_is_k3s_not_compose` (metadata `user-data`), `secrets_created`, `cost_items_small` and `every_resource_labelled`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/gcp-k3s test`. Expected: FAIL.
- [ ] **Step 3: Implement**, as Task 5 step 3.
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): gcp-k3s home target and copy-paste root"`

### Task 7: `azure-k3s` module and example

**Files:** as Task 5, under `azure-k3s`, starting from a copy of `modules/azure-vm`.

**Interfaces:**
- Produces: as Task 5, with `location`, `resource_group_name`, `vm_size`, `key_vault_name`, `purge_protection`, `address_space` and `tags`.
- The secrets are Key Vault secrets `<prefix>-k3s-ca` and `<prefix>-access-db`.
- `cost_items`: `azure/vm/<size>`, `azure/disk/premium-ssd-p6`, `azure/network/public-ipv4`, `azure/keyvault/key`.
- `workload_identity` is the managed identity's client ID.

- [ ] **Step 1: Write the failing tests.** Carry over `azure-vm`'s (the NSG has no rule; shared keys are off). Add `custom_data_is_k3s_not_compose`, `secrets_created`, `cost_items_small` and `every_resource_tagged`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/azure-k3s test`. Expected: FAIL.
- [ ] **Step 3: Implement**, as Task 5 step 3.
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): azure-k3s home target and copy-paste root"`

### Task 8: `k8s-operator` module (managed Kubernetes homes)

**Files:**
- Create: `deploy/tofu/modules/k8s-operator/{main.tf,variables.tf,outputs.tf,versions.tf,tests/plan.tftest.hcl}`
- Modify: `tests/byoc/tofu/run.sh`

**Interfaces:**
- Consumes: providers `helm` (~> 3.0) and `kubernetes`, configured by the root.
- Produces:
  - **Variables:** `release`, `home_spec`, `operator_values`, `access_db`, `access_db_dsn` (sensitive), `postgres_image` (default = `k3s.env`'s `POSTGRES_IMAGE`; a test keeps them equal) and `target_facts`.
  - **Resources:**
    - `kubernetes_namespace_v1.loams_system`;
    - `kubernetes_config_map_v1.loams_target`;
    - `random_password.access_db`, the home Secrets, and (for `local`) `kubernetes_stateful_set_v1.access_db` with its `kubernetes_service_v1`, matching `access-db.yaml.tmpl` (the same names, image and 10 GiB request);
    - `helm_release.operator`: `repository = "oci://ghcr.io/ostrium-labs/charts"`, `chart = "loams-operator"`, `version = var.release`, `namespace = "loams-system"`, `atomic = true`, `wait = true`, `timeout = 900`, `values = [jsonencode(merge(var.operator_values, {home = {create = true, spec = var.home_spec}}))]`, with `depends_on` on the Secrets and the database.
  - **Outputs:** `namespace` and `release_name`.

- [ ] **Step 1: Write the failing tests** (mock `helm`, `kubernetes` and `random`):
  - `home_create_true`;
  - `chart_is_op2a_oci_ref`;
  - `operator_values_passed_through`;
  - `namespace_is_loams_system`;
  - `access_db_local_statefulset`;
  - `access_db_external_none`;
  - `postgres_image_matches_k3s_env` (`file("…/byoc/k3s.env")` holds `var.postgres_image`'s default);
  - `target_facts_in_configmap`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/k8s-operator test`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): k8s-operator module for managed Kubernetes homes"`

### Task 9: `aws-eks` module and example

**Files:** `deploy/tofu/modules/aws-eks/…` and `deploy/tofu/examples/aws-eks/{main.tf,encryption.tf}`. The root configures `helm` and `kubernetes` from module outputs, with the exec `aws eks get-token`, and calls `k8s-operator`. Modify: `tests/byoc/tofu/run.sh`.

**Interfaces:**
- Produces: the contract, plus `kubernetes_version` (the newest EKS standard-support version at implementation time) and `authorized_cidrs` (default `["0.0.0.0/0"]`; shown in the plan summary).
- Resources:
  - `aws_eks_cluster`: `access_config.authentication_mode = "API"`, `bootstrap_cluster_creator_admin_permissions = true`, the default VPC's subnets in at least 2 AZs;
  - a managed node group (`t3.xlarge` ×1 or ×3, 20 GiB gp3, no remote access);
  - an EBS CSI addon for the C1 PVC;
  - `aws_iam_openid_connect_provider`;
  - the IRSA role `loams-<name>-install`, whose trust has `StringLike` `system:serviceaccount:loams-i-*:install` (C3), with read and write on the bucket;
  - the bucket and KMS key, as `aws-vm`.
- `kubeconfig` holds no secret (an exec credential).
- `cost_items`: `aws/eks/control-plane` ×730, `aws/ec2/t3.xlarge` ×730×n, `aws/ebs/gp3` × (20×n + 10), `aws/vpc/public-ipv4` ×730×n, `aws/kms/key`.

- [ ] **Step 1: Write the failing tests:** `api_auth_is_iam_only`, `node_count_small_is_1`, `irsa_trust_is_install_wildcard`, `bucket_is_private_versioned_and_encrypted`, `cost_items_have_control_plane` and `every_resource_tagged`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/aws-eks test`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): aws-eks home target"`

### Task 10: `gcp-gke` module and example

**Files:** as Task 9, under `gcp-gke`. The root uses the `gke-gcloud-auth-plugin` exec.

**Interfaces:**
- A zonal Standard cluster: `remove_default_node_pool`, `workload_identity_config`, `master_authorized_networks_config` from `authorized_cidrs`, legacy ABAC off, client certificate off.
- A node pool of `e2-standard-4` ×1 or ×3.
- A GSA `loams-<name>-install` with bucket access, and no KSA binding (C3).
- A GCS bucket and KMS key.
- `cost_items`: `gcp/gke/zonal-management` ×730, `gcp/gce/e2-standard-4` ×730×n, `gcp/pd/balanced` × (100×n + 10).

- [ ] **Step 1: Write the failing tests:** `zonal_cluster`, `no_client_certificate`, `workload_identity_on`, `node_count_small_is_1`, `cost_items_mark_management` and `every_resource_labelled`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/gcp-gke test`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): gcp-gke home target"`

### Task 11: `azure-aks` module and example

**Files:** as Task 9, under `azure-aks`. The root uses a `kubelogin` exec in `azurecli` mode.

**Interfaces:**
- The cluster: `sku_tier = "Free"`, `local_account_disabled = true`, Entra ID with `azure_rbac_enabled = true`, `oidc_issuer_enabled` and `workload_identity_enabled`.
- A node pool of `Standard_D4s_v5` ×1 or ×3.
- "Azure Kubernetes Service RBAC Cluster Admin" for the caller's object ID.
- A UAMI `loams-<name>-install` with bucket access, and no federated credential yet (C3).
- A storage account (shared keys off) and a container.
- `cost_items`: `azure/aks/control-plane-free`, `azure/vm/Standard_D4s_v5` ×730×n, `azure/disk/premium-ssd-p10` ×n, `azure/disk/premium-ssd-p3` ×1 (the C1 PVC), `azure/lb/standard` ×730, `azure/network/public-ipv4` ×730. AKS creates the load balancer and IP, and they are not free.

- [ ] **Step 1: Write the failing tests:** `local_accounts_disabled`, `free_tier`, `workload_identity_on`, `cost_items_include_load_balancer` and `every_resource_tagged`.
- [ ] **Step 2: Run and confirm they fail.** Run `tofu -chdir=deploy/tofu/modules/azure-aks test`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Run `tests/byoc/tofu/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(tofu): azure-aks home target"`

### Task 12: `server-ssh` target

**Files:**
- Modify:
  - `deploy/release/byoc/install.sh`: `--k3s`. It refuses `--cloud`, `--tikv`, `--rustfs`, `--kafka` and the profile flags, and refuses non-Ubuntu/Debian hosts.
  - `deploy/release/byoc/uninstall.sh`: `--k3s` runs `k3s-uninstall.sh` and `tailscale logout`.
- Create: `deploy/tofu/modules/server-ssh/…`, `deploy/tofu/examples/server-ssh/{main.tf,encryption.tf}`
- Test: `tests/byoc/static/test_install_k3s.sh`, `tests/byoc/tofu/run.sh`

**Interfaces:**
- Consumes: Task 3 (`LOAMS_K3S_CLOUD=server`) and Task 4 (`cloud = "server"`).
- Produces:
  - **Inputs:** `host` (`user@host`), `ssh_port` (22), `ssh_identity_file` (null), `release`, `hostname`, `tailnet_domain`, `home_spec`, `operator_values`, `access_db` and `access_db_dsn`.
  - **`terraform_data.install`:**
    - `input = {host, ssh_port, ssh_identity_file, release}` and `triggers_replace = [var.release, var.host]`;
    - `local-exec`: `install.sh --k3s --version <release> --hostname <hostname> [-p] [-i] <host>`, with `environment = {LOAMS_K3S_CA_BUNDLE, LOAMS_K3S_ACCESS_DB, LOAMS_K3S_VALUES_B64, LOAMS_K3S_FACTS_B64, TAILNET_DOMAIN}`. `TS_AUTHKEY` is inherited from the caller and is never a variable;
    - a destroy-time `local-exec`: `uninstall.sh --k3s` from `self.input`.
  - **`cost_items = []`.** The target is `cloud = "server"`, and the estimate says "your own server: no cloud cost".

- [ ] **Step 1: Write the failing tests.**
  - Static, with fake `ssh`/`scp` logging their argv:
    - `k3s_install_copies_bootstrap_and_secrets` (mode 0600 on the host);
    - `k3s_install_never_puts_secrets_in_argv`;
    - `k3s_refuses_compose_flags`;
    - `k3s_refuses_non_apt_host`.
  - `tofu test`:
    - `install_input_has_host`;
    - `release_change_replaces`;
    - `no_cost_items`.
- [ ] **Step 2: Run and confirm they fail.** Run `tests/byoc/static/run.sh && tests/byoc/tofu/run.sh`. Expected: FAIL on the new names.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Same commands. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "feat(byoc): server-ssh target (install.sh --k3s) and module"`

### Task 13: Module bundle, root rendering and deployment records

**Files:**
- Create: `crates/loams-deploy/build.rs`, `src/bundle.rs`, `src/render.rs`, `src/record.rs`
- Test: `crates/loams-deploy/tests/render.rs`

**Interfaces:**
- **Bundle.** `build.rs` emits `static FILES: &[(&str, &[u8])]` with `include_bytes!`, plus `BUNDLE_SHA256`. It covers:
  - `deploy/tofu/modules/{shared,k3s-home,k8s-operator,<operator target modules>}/**` (without `tests/`);
  - `deploy/tofu/examples/<operator target>/*.tf`;
  - `deploy/byoc/{aws,gcp,azure}/cloud-init-k3s.yaml`;
  - `deploy/byoc/{k3s.env,k3s/access-db.yaml.tmpl,lib/loams-k3s-*}`;
  - `deploy/release/byoc/{install.sh,uninstall.sh}`;
  - `deploy/operator/home.yaml`.

  It uses no new dependency.
- **Functions:**
  - `extract_bundle(dest: &Path) -> io::Result<()>` keeps repo-relative paths and `+x` bits.
  - `home_spec_default() -> serde_json::Value`: the `spec` of the embedded `home.yaml`.
  - `render_root(target: &DeployTarget, dir: &DeploymentDir, inputs: &RootInputs, state: &StateLocation) -> Result<(), RenderError>`:
    - copies the example into `root/`;
    - rewrites its single `source = "../../modules/` to `"../bundle/deploy/tofu/modules/`;
    - writes `backend.tf` for a non-local state;
    - writes `terraform.tfvars.json`, never with the passphrase or the DSN, which go through `TF_VAR_*`.
- **State locations.** `StateLocation { Local, S3 { bucket, key, region }, Gcs { bucket, prefix }, Azurerm { account, container, key } }`, from `--state`:
  - `s3://b/k?region=r` gives `use_lockfile = true`;
  - `gs://b/p`;
  - `azurerm://acct/container/key` gives `use_azuread_auth = true`.
- **Deployment directory.** `DeploymentDir::new(name)` resolves `${LOAMS_DEPLOY_HOME:-~/.loams/deploy}/<name>/`, with `bundle/`, `root/`, `deployment.toml` and `state.key` (32 random bytes, hex, mode 0600).
- **The record.** `Record { name, listing: Option<String>, version: Option<String>, install: Option<String>, target, region, size, state: StateLocation, bundle_sha256, status: Status { Planned, Applying, Applied, AwaitingConfirmation, Destroying, Destroyed, Failed }, created, updated }`.
  - `Record::save` writes atomically.
  - The record holds no secrets.

- [ ] **Step 1: Write the failing tests:**
  - `every_operator_example_has_one_module_source`;
  - `render_rewrites_source_to_bundle`;
  - `render_s3_backend`;
  - `tfvars_never_contain_secrets`;
  - `state_key_is_0600`;
  - `record_roundtrip_and_atomic`;
  - `bundle_contains_cloud_init_k3s_and_home_yaml`;
  - `home_spec_default_is_home_yaml_spec`;
  - `rendered_root_validates`: when `tofu` is on `PATH`, run `init -backend=false` and `validate` for `aws-k3s`; otherwise print `skip` and pass.
- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test render`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(deploy): embedded module bundle, root rendering, deployment records"`

### Task 14: The `tofu` runner and plan summary

**Files:**
- Create: `crates/loams-deploy/src/tofu.rs`
- Test: `crates/loams-deploy/tests/tofu.rs`, with a scripted `FakeRunner` and the fixture `tests/fixtures/plan-aws-k3s.json` (a real `tofu show -json` of the mocked module, captured once)

**Interfaces:**
- **The runner trait:**

  ```rust
  pub trait Runner {
      fn run(&self, cwd: &Path, args: &[&str], env: &[(&str, String)], log: &dyn Fn(&str)) -> io::Result<RunOutput>;
  }
  ```

  `RunOutput { status: i32, stdout: Vec<u8>, stderr: Vec<u8> }`. `SystemRunner` uses `std::process::Command` with no shell.
- **`Tofu<R: Runner> { bin, runner }`:**
  - `find(env_override) -> Result<PathBuf, TofuError>` checks ≥ 1.8.
  - `init`.
  - `plan(root, out) -> Result<PlanSummary>` runs `plan -out` and then `show -json`.
  - `apply_plan(root, plan)`, `destroy(root)` and `output_json(root, name) -> Result<Value>`.
  - `state_list(root)` and `state_rm(root, addrs)`.
- **Every call:**
  - sets `TF_IN_AUTOMATION=1`, `TF_INPUT=0`, `TF_VAR_state_passphrase` (from `state.key`) and `TF_VAR_access_db_dsn` when one is given;
  - never logs a value;
  - names the deployment in errors.
- **`PlanSummary { changes: Vec<PlannedChange { address, r#type, action: Create|Update|Delete|Replace }>, cost_items: Vec<CostItem>, plan_sha256 }`.**

- [ ] **Step 1: Write the failing tests:** `plan_parses_changes_and_cost_items`, `plan_digest_is_sha256_of_plan_file`, `secrets_in_env_not_args`, `version_below_1_8_rejected` and `nonzero_exit_maps_to_tofu_failed_with_stderr_tail`.
- [ ] **Step 2: Run and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test tofu`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(deploy): tofu runner and plan summary"`

### Task 15: Listing resolution and fit check

**Files:**
- Modify: `crates/loams-deploy/Cargo.toml`: `loams-agentd-marketplace = { path = "../loams-agentd-marketplace", default-features = false, features = ["fetch"] }`, with dev `features = ["fetch", "testkit"]`.
- Create: `crates/loams-deploy/src/listing.rs`
- Test: `crates/loams-deploy/tests/listing.rs`

**Interfaces:**
- Consumes: OP2-B's `HttpsFetcher`, `IndexFiles`, `TrustAnchor`, `SignedIndex::verify`, `verify_listing`, `ListingV2`, `ListingKind`, `Package`, `Target`, and `testkit::signed_v2_index`.
- Produces:
  - `IndexSource { base_url: Url, anchor: TrustAnchor }`.
    - The default is `https://ostrium-labs.github.io/loams-marketplace/` with `TrustAnchor::official()`.
    - `--index <url>` and `--index-public-key <pem file>` select `TrustAnchor::PublicKey`.
  - `fetch_index(src: &IndexSource) -> Result<SignedIndex, ListingError>` fetches the four v2 files and verifies them.
  - `ListingRef::parse(s) -> Result<ListingRef, ListingError>`: `<ns>/<name>[@<semver>]`, with no build metadata.
  - `resolve(index, r) -> Result<VerifiedListing, ListingError>`. A missing version means the index's latest non-yanked version.
  - `choose_size(target: &DeployTarget, l: &ListingV2, requested: Option<&str>) -> Result<String, FitError>`. The need is `max(requires, cost_hint)` for `cpu_millis` and `memory_mib`. The smallest size where `size × nodes − reserve ≥ need` wins.
  - **The C4 refusals,** all exit 4 and all before any `tofu` call:
    - `targets` lacks `Target::Cluster`;
    - the kind is not `App` or `Engine`;
    - the package is `Kustomize`, `Wasm` or `Binary`;
    - `requires.storage` is non-empty;
    - `requires.gpu` is set;
    - the listing does not fit;
    - the target has `operator = false` or `available = false`;
    - any `VerifyError` (named in the message).
  - A `VerifiedListing.warning` (a `warn` revocation) becomes a plan warning, not a refusal.

- [ ] **Step 1: Write the failing tests,** on a `signed_v2_index`:
  - `parse_rejects_build_metadata`;
  - `parse_rejects_traversal`;
  - `latest_when_version_omitted`;
  - `disabled_revocation_refused`;
  - `warn_revocation_is_warning`;
  - `bad_signature_refused`;
  - `agent_kind_refused`;
  - `kustomize_refused`;
  - `node_only_listing_refused`;
  - `gpu_refused`;
  - `storage_requirement_refused_until_dep2`;
  - `memory_6000_mib_picks_medium_on_aws_k3s` (small 8192 − 4096 < 6000);
  - `too_big_for_medium_refused`;
  - `vm_target_refused_with_hint` (the message names `loams app deploy` or the compose path).
- [ ] **Step 2: Run and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test listing`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(deploy): signed v2 index, listing resolution and fit check"`

### Task 16: Home readiness, `LoamsInstall`, and cluster-scoped approval

**Files:**
- Create: `crates/loams-deploy/src/install.rs`
- Test: `crates/loams-deploy/tests/install.rs`, with `kube`'s `Client::new` over a `tower-test` mock, as `crates/loams-spark` does

**Interfaces:**
- Consumes: OP2-A's CRDs in `deploy/helm/loams-operator/crds/` (tests only) and the `kubeconfig` output.
- Produces:
  - `install_name(listing_id: &str) -> String`: `<ns>-<name>`, DNS-1123. Over 63 characters it is cut to 54 and given `-<8 hex of sha256>`.
  - `install_object(deployment: &str, l: &VerifiedListing, values: Value) -> DynamicObject`:
    - `LoamsInstall` in `loams-system`, labelled `loams.dev/deployment=<deployment>`;
    - `spec.cluster = "home"`, `spec.listing = {id, version}` and `spec.values`, checked first with `ListingV2::validate_values`.
  - `wait_api(kc, timeout)` (15 minutes) and `wait_home_ready(kc, timeout)` (`loamscluster/home` `Ready`, 15 minutes).
  - `apply_install(kc, obj)`: server-side apply, field manager `loams-deploy`.
  - `wait_install(kc, name, timeout) -> Result<InstallState, InstallError>`, which returns `Ready` or `AwaitingConfirmation(ClusterScoped { objects: Vec<ObjectId { api_version, kind, name }>, digest })`, with a default timeout of 20 minutes.
  - `approve_cluster_scoped(kc, name, digest) -> Result<(), InstallError>`:
    - reads ConfigMap `cluster-scoped` in `loams-i-<name>` (OP2-A's namespace rule; `manifest.yaml`);
    - refuses unless its object list equals `status.clusterScoped.objects` and the status digest equals `digest`;
    - server-side applies it with field manager `loams-approver`;
    - patches `spec.approvedClusterScoped = digest`.
  - `remove_install(kc, name, timeout)` waits until 404.

- [ ] **Step 1: Write the failing tests:**
  - `object_validates_against_op2a_crd_schema` (`jsonschema` against `openAPIV3Schema`);
  - `long_names_are_dns1123`;
  - `install_namespace_matches_op2a_rule`;
  - `wait_home_ready_true` and `wait_home_ready_times_out`;
  - `awaiting_confirmation_surfaces_objects_and_digest`;
  - `approve_refuses_digest_mismatch`;
  - `approve_uses_loams_approver_field_manager_then_patches_digest`;
  - `remove_waits_until_404`;
  - `invalid_values_refused_before_apply`.
- [ ] **Step 2: Run and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --test install`. Expected: FAIL.
- [ ] **Step 3: Implement** with `kube::Config::from_custom_kubeconfig` (client-cert and exec auth).
- [ ] **Step 4: Run and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(deploy): home readiness, LoamsInstall, cluster-scoped approval"`

### Task 17: `loams deploy` CLI

**Files:**
- Create: `crates/loams-deploy/src/cli.rs` (feature `cli`), `src/flow.rs`
- Modify:
  - `crates/loams/Cargo.toml`: `loams-deploy = { path = "../loams-deploy", features = ["cli"] }`;
  - `crates/loams/src/main.rs`: the variant, the `unreachable!` arm and the early dispatch, as for `Cost`.
- Test: `crates/loams-deploy/tests/cli.rs` (a fake runner, a fake index, a fake kube and an injected TTY flag), plus `deploy_parses` in `main.rs`'s tests

**Interfaces:**
- Produces:

```rust
#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DeployArgs {
    #[command(subcommand)] pub command: Option<DeployCommand>,
    #[command(flatten)] pub new: NewDeployArgs,
}
#[derive(Debug, Clone, clap::Args)]
pub struct NewDeployArgs {
    pub listing: Option<String>,                       // <ns>/<name>[@<version>]
    #[arg(long)] pub home_only: bool,
    #[arg(long)] pub target: Option<String>,
    #[arg(long)] pub name: Option<String>,             // default <listing name>-<target>, or home-<target>
    #[arg(long)] pub region: Option<String>,
    #[arg(long)] pub project: Option<String>,
    #[arg(long)] pub size: Option<String>,
    #[arg(long)] pub host: Option<String>,
    #[arg(long)] pub ssh_port: Option<u16>,
    #[arg(long)] pub ssh_identity: Option<PathBuf>,
    #[arg(long, default_value = "local")] pub state: String,
    #[arg(long)] pub values: Option<PathBuf>,
    #[arg(long)] pub home_spec: Option<PathBuf>,       // overrides home.yaml's spec
    #[arg(long)] pub access_db_dsn_file: Option<PathBuf>,
    #[arg(long)] pub index: Option<String>,
    #[arg(long)] pub index_public_key: Option<PathBuf>,
    #[arg(long)] pub budget_email: Option<String>,
    #[arg(long)] pub max_monthly_usd: Option<String>,
    #[arg(long)] pub yes: bool,
    #[arg(long)] pub approve_cluster_scoped: bool,     // non-interactive only, with --yes
    #[arg(long, value_enum, default_value_t = Output::Text)] pub output: Output,
}
#[derive(Debug, clap::Subcommand)]
pub enum DeployCommand {
    Plan(NewDeployArgs),
    Apply { name: String, #[arg(long)] plan_digest: Option<String>, #[arg(long)] yes: bool,
            #[arg(long)] approve_cluster_scoped: bool, #[arg(long, value_enum, default_value_t = Output::Text)] output: Output },
    Approve { name: String, #[arg(long)] digest: String, #[arg(long)] yes: bool },
    Destroy { name: String, #[arg(long)] install_only: bool, #[arg(long)] delete_bucket: bool, #[arg(long)] yes: bool,
              #[arg(long, value_enum, default_value_t = Output::Text)] output: Output },
    List { #[arg(long, value_enum, default_value_t = Output::Text)] output: Output },
    Targets { #[arg(long, value_enum, default_value_t = Output::Text)] output: Output },
    Estimate { #[arg(long)] plan: PathBuf, #[arg(long)] target: String, #[arg(long)] region: Option<String> },
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)] pub enum Output { Text, Json, JsonLines }
pub fn run(args: &DeployArgs) -> std::process::ExitCode;
```

- **`DeployPlanJson`:**

  ```
  { name, listing, version, tier, target, region, size, free_tier_target,
    resources: [{address, type, action}],
    estimate: { lines: [{sku, quantity, unit, usd_month, flags}], total_usd_month, complete, basis: "estimated", prices_read, all_verified },
    plan_digest, warnings: [string] }
  ```

- **`DeployEvent`** (JSON lines):
  - `{"event":"phase","phase":"init|plan|apply|wait-api|wait-home|install|wait-install|destroy|done"}`;
  - `{"event":"confirm-cluster-scoped","objects":[…],"digest":"sha256:…"}`;
  - `{"event":"log","line":…}`;
  - `{"event":"error","code":…,"message":…}`.

**The default flow** (no subcommand) is plan, then confirm, then apply:
1. Preflight. Check `tofu`. Check the credentials tool on `PATH`. For `needs_tailnet` targets, read `.MagicDNSSuffix` from `tailscale status --json`; if it is absent, exit 2.
2. Fetch the index, then resolve and fit the listing (Task 15).
3. Extract the bundle, render the root and save the record as `Planned`.
4. Run `init`, `plan` and then `estimate_monthly_cost`.
5. Print the resources, the estimate and the warnings. Ask `Type the deployment name to create these resources:`.
   - On a TTY, `--yes` is refused (exit 2).
   - Off a TTY, `--yes` is required (exit 3 without it).
   - `--max-monthly-usd` refuses an estimate above it, and any incomplete estimate (exit 3).
6. Save the record as `Applying`, then apply the saved plan.
7. Wait for the API, then for the home to be Ready. With `--home-only`, stop: the record is `Applied`.
8. Apply the install and wait.
9. If the install is `AwaitingConfirmation`, list the objects and their digest and ask separately.
   - Off a TTY, approval needs both `--yes` and `--approve-cluster-scoped`. Otherwise the record becomes `AwaitingConfirmation` and the CLI exits 3 with `loams deploy approve <name> --digest …` printed.
10. Wait until Ready, then save the record as `Applied`.

- **`approve`** runs step 9 for a recorded deployment, which is Desktop's path.
- **`apply <name>`** refuses unless the plan file's SHA-256 equals `--plan-digest` when one is given.
- **`destroy <name>`:**
  - `--install-only` runs `remove_install`.
  - Otherwise the record becomes `Destroying`. Unless `--delete-bucket` is given, the `keep_on_destroy` addresses present in state are `state rm`'d and printed as kept. Then `tofu destroy` runs, and the record becomes `Destroyed`.
  - The same confirmation rules apply.

- [ ] **Step 1: Write the failing tests:**
  - `yes_refused_on_tty`;
  - `no_yes_off_tty_refused_exit_3`;
  - `max_monthly_usd_refuses_over_and_incomplete`;
  - `record_is_applying_before_apply_starts`;
  - `destroy_works_from_applying`;
  - `destroy_keeps_bucket_by_default`;
  - `delete_bucket_nonempty_message_names_bucket`;
  - `apply_refuses_stale_digest`;
  - `k3s_target_without_tailnet_refused_before_tofu`;
  - `install_waits_for_home_ready_first`;
  - `cluster_scoped_needs_separate_flag_off_tty`;
  - `awaiting_confirmation_prints_approve_command`;
  - `plan_json_shape` (a snapshot);
  - `listing_or_home_only_required`;
  - `deploy_parses`, in `main.rs`, for `loams deploy ostrium/house@1.0.0 --target aws-k3s` and `loams deploy destroy x`.
- [ ] **Step 2: Run and confirm they fail.** Run `cargo test -p loams-deploy -j 4 --features cli --test cli`. Expected: FAIL.
- [ ] **Step 3: Implement** `cli.rs` (arguments and output) and `flow.rs` (the steps, over injected runner, index, kube and TTY dependencies).
- [ ] **Step 4: Run and confirm they pass.** Run `cargo test -p loams-deploy -j 4 --features cli`, then, once nothing else is building, `cargo test -p loams -j 4 deploy_parses`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(cli): loams deploy (plan, cost guard, apply, install, approve, destroy)"`

### Task 18: Desktop deep link and deploy bridge

**Files:**
- Modify:
  - `apps/desktop-electron/src/shared/deeplink.ts`;
  - `apps/desktop-electron/src/shared/contracts.ts`: the optional `deploy?` and the `CH.deploy*` channels;
  - `apps/desktop-electron/src/preload/index.ts` and `src/main/index.ts`.
- Modify: move the `PATH` lookup in `src/main/stacks/ipc.electron.ts` to `src/main/which.ts`.
- Create: `apps/desktop-electron/src/main/deploy/{service.ts,ipc.electron.ts}`
- Test: `test/deeplink.test.ts`, `test/deploy-service.test.ts`

**Interfaces:**
- **The link.** `parseDeepLink("loams://deploy?listing=a/b@1.2.3&target=aws-k3s")` returns `{path: "/deploy/new/a/b/1.2.3/aws-k3s"}`.
  - The regex is `^loams:\/\/deploy\?([^#]*)$`.
  - Only `listing` and `target` are allowed, each exactly once.
  - The listing must match `^[a-z0-9][a-z0-9-]{0,38}/[a-z0-9][a-z0-9-]{0,63}@\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$`, and the target `^[a-z0-9-]{1,32}$`.
  - Each segment must also pass `SEG`.
- **The bridge:**

```ts
deploy?: {
  targets(): Promise<IpcResult<DeployTarget[]>>;
  plan(req: DeployPlanRequest): Promise<IpcResult<DeployPlan>>;
  apply(name: string, planDigest: string): Promise<IpcResult<void>>;   // native confirm from main's own plan, then `apply --yes --plan-digest`
  approve(name: string): Promise<IpcResult<void>>;                     // native confirm listing objects from main's own status read, then `approve --yes --digest`
  destroy(name: string, opts: { installOnly: boolean; deleteBucket: boolean }): Promise<IpcResult<void>>;
  list(): Promise<IpcResult<DeploymentSummary[]>>;
  onEvent(cb: (name: string, e: DeployEvent) => void): () => void;
}
```

- **The types:**
  - `DeployPlanRequest { listing?: string; homeOnly?: boolean; target: string; name?: string; region?: string; project?: string; size?: "small" | "medium"; host?: string }`.
  - `DeployPlan` mirrors `DeployPlanJson`.
- **`DeployService` (main):**
  - Spawns `paths.engineBin` with an argv array (no shell) and stdin set to `ignore`.
  - Validates every field against the link regexes first.
  - Keeps the last plan per name. `apply`'s native dialog shows that plan's total, the "estimated" label, the free-tier label and the resource count.
  - `approve`'s dialog shows the objects from the latest `confirm-cluster-scoped` event the main process itself parsed.
  - Its preflight finds `tofu` (or `LOAMS_TOFU`) and the target's credentials tool. A missing one returns `{ok:false, code:"missing_tool", message:"<tool> not found on PATH (Desktop started from the Dock may not see your shell PATH; set LOAMS_TOFU or start Desktop from a terminal)"}`.

- [ ] **Step 1: Write the failing tests.**
  - `deeplink.test.ts` accepts `loams://deploy?listing=ostrium/house@0.2.0&target=aws-k3s`.
  - It rejects:
    - a duplicate `target`;
    - an extra key;
    - a missing version;
    - `@1.0.0+b`;
    - `listing=../x@1.0.0`;
    - a 33-character target;
    - a `#frag`;
    - `loams://deploy/x?…`.
  - `deploy-service.test.ts`:
    - `argv_has_no_shell_and_validated_fields`;
    - `apply_confirm_uses_main_side_plan`;
    - `approve_confirm_lists_objects_from_main_side_event`;
    - `cancelled_confirm_spawns_nothing`;
    - `missing_tofu_returns_missing_tool`.
- [ ] **Step 2: Run and confirm they fail.** Run `pnpm --filter @loams/desktop test -- deeplink deploy-service`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Same command. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(desktop): loams://deploy deep link and deploy bridge"`

### Task 19: Desktop Deploy page and the marketplace Deploy button

**Files:**
- Create: `web/plugins/desktop-deploy/{package.json,tsconfig.json,vite.config.ts,src/index.tsx,src/DeployPage.tsx,src/DeploymentsPage.tsx,test/deploy.test.tsx}`
- Modify: `web/apps/console/src/cordis/desktop.ts` (register the plugin), `web/plugins/marketplace/src/pages.tsx` (the button)

**Interfaces:**
- Consumes: `desktop.deploy` (Task 18). Without it, the plugin registers nothing.
- Produces these routes:
  - **`/deploy/new/:ns/:name/:version/:target?`**:
    - a target select, with free-tier and "not available" labels;
    - the inputs named by `required_inputs`;
    - **Plan and estimate**, then the resources table, the estimate lines with flags, and the total labelled "estimated · list prices read <date>";
    - **Deploy**, then a live event log;
    - on `confirm-cluster-scoped`, an **Approve cluster-wide objects** panel listing them, which calls `approve`.
  - **`/deploy`**: the deployments list, with status and the buttons **Remove install** and **Tear down** (a `delete bucket` checkbox, off by default).
- The listing page's Deploy button navigates to `/deploy/new/<ns>/<name>/<version>`. It is shown only when `desktop.deploy` exists and the listing's `targets` include `cluster`.

- [ ] **Step 1: Write the failing tests:**
  - `nothing_planned_until_click`;
  - `estimate_shows_region_flag_and_estimated_label`;
  - `incomplete_estimate_shows_at_least`;
  - `free_tier_label_shown`;
  - `deploy_disabled_until_plan`;
  - `cluster_scoped_panel_lists_objects`;
  - `teardown_bucket_checkbox_default_off`;
  - `marketplace_button_hidden_without_bridge`.
- [ ] **Step 2: Run and confirm they fail.** Run `pnpm --filter @loams/plugin-desktop-deploy test`. Expected: FAIL.
- [ ] **Step 3: Implement** with `@loams/ui`, as `desktop-transfers` does.
- [ ] **Step 4: Run and confirm they pass.** Run `pnpm --filter @loams/plugin-desktop-deploy test && pnpm --filter @loams/plugin-marketplace test`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(console): desktop Deploy page and marketplace Deploy button"`

### Task 20: Real-cloud run harness (manual only)

**Files:**
- Create: `tests/deploy/real/{run.sh,sweep.sh}`, `.github/workflows/deploy-real.yml`
- Modify: `.github/workflows/byoc-static.yml` (the paths `tests/deploy/**` and `crates/loams-deploy/**`)
- Test: `tests/byoc/static/test_deploy_real_guard.sh`

**Interfaces:**
- **`run.sh <target> [--listing <ref>] [--max-minutes 60]`.** `LOAMS_BIN` must point to a locally built `loams` or a release download.
  - `<target>` is any operator target, or `aws-vm`, `gcp-vm` or `azure-vm`. Those apply `deploy/tofu/examples/<target>` with `tofu` directly, and use `estimated_monthly_usd`.
  - Operator targets run `$LOAMS_BIN deploy plan … --output json` (`--home-only` unless `--listing` is given).
  - The run estimate is `total × max_minutes / 60 / 730`.
  - Then `python3 -I bench/cloud/ledger.py check --cloud <c> --row dep1-<target> --estimate <run>`, then `start`, then `apply <name> --yes --plan-digest <d> [--approve-cluster-scoped]`.
  - Checks:
    - `kubectl wait loamscluster/home --for=condition=Ready` (and the install's);
    - `tests/byoc/exposure.sh` for VMs;
    - from a pod, `curl -m 5 http://169.254.169.254/` must fail on k3s targets (Review Focus 5).
  - A `trap` on EXIT plus a `timeout` both lead to `destroy --yes --delete-bucket`, then `sweep.sh` (tag or label `loams-deploy=<name>`, through `aws resourcegroupstaggingapi`, `gcloud asset search-all-resources` and `az resource list --tag`), then `ledger.py destroyed --sweep clean|dirty`.
- **`deploy-real.yml`:**
  - `on: workflow_dispatch` only. The inputs are `target` (a choice), `loams_version` and `max_minutes` (at most 60).
  - Environments `deploy-aws`, `deploy-gcp` and `deploy-azure` use OIDC. Each is a no-op that says so when its variables are unset.
  - `tailscale/github-action` runs for the k3s targets.
  - `BENCH_LEDGER=$RUNNER_TEMP/ledger.md`, so the per-run 25% stop line and the IST window still apply.
  - It has no `schedule` and no `pull_request` trigger.

- [ ] **Step 1: Write the failing static tests:**
  - `real_run_refuses_without_ledger_ok` (`ledger.py` faked to exit 3, so the fake `loams` never sees `apply`);
  - `real_run_destroys_on_failure`;
  - `deploy_real_workflow_is_dispatch_only`;
  - `max_minutes_capped_at_60`.
- [ ] **Step 2: Run and confirm they fail.** Run `tests/byoc/static/run.sh`. Expected: FAIL on the new names.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** Run `SHELLCHECK=$(command -v shellcheck) tests/byoc/static/run.sh`. Expected: all `ok`.
- [ ] **Step 5: Commit.** `git commit -m "test(deploy): manual real-cloud run harness with the CB1 cost guard"`

### Task 21: First real applies, docs and status

**Files:**
- Modify:
  - `deploy/tofu/README.md`: the module list, plus a status line per module with the date and result of its first apply;
  - `deploy/tofu/prices.toml`: `verified = true` where a billed amount confirmed a price.
- Create: `docs/guides/deploy.md`, covering `loams deploy`, the deep link, the copy-paste roots, state, the cost guard, cluster-scoped approval, the Access database (C1) and teardown.
- Fixes: each finding gets its own commit, with a `tofu test` or static test that pins it.

**Interfaces:**
- Consumes: Task 20's harness, run by the owner, or by an agent on the owner's machine with the credentials in `~/aws.env`, `~/gcp-sa.json` and `~/azure.env` (never printed). Runs are 06:00–20:00 IST, one cloud at a time.
- **The order** (the cheapest first, per design 86 §11):
  1. `aws-vm`, `gcp-vm`, `azure-vm` (the existing modules' first applies);
  2. `aws-k3s`, `gcp-k3s`, `azure-k3s`, each `--home-only` first and then with one first-party `app` listing from OP2-B's seed;
  3. `gcp-gke`, `azure-aks`, `aws-eks`;
  4. `server-ssh` on a k3s-free spare VM.
- **Within the caps** (AWS $60, GCP $100, Azure $25, shared with CB1), each run is at most 60 minutes.

- [ ] **Step 1: Write the failing check** `readme_status_matches_targets` in `tests/byoc/tofu/run.sh`: every module in `targets.toml` has a README status line.
- [ ] **Step 2: Run it and confirm it fails.** Run `tests/byoc/tofu/run.sh`. Expected: `FAIL readme_status_matches_targets`.
- [ ] **Step 3: Run the real applies in order,** with `tests/deploy/real/run.sh <target>`. Record each ledger line, sweep result and finding, and fix the findings.
  - Verify on the first k3s boot that partial custom CAs (server and client only) are accepted. If not, extend `k3s-home` to generate every CA k3s expects, with a test.
  - Verify that the IMDS block holds.
- [ ] **Step 4: Update the README status and write the guide,** then run `tests/byoc/tofu/run.sh && tests/byoc/static/run.sh && cargo test -p loams-deploy -j 4 --features cli`. Expected: all pass. The ledger shows every run destroyed with a clean sweep.
- [ ] **Step 5: Commit.** `git commit -m "docs(deploy): one-click deploy guide; first real applies recorded"`

---

## Self-review notes

- **Spec coverage:**

  | Requirement | Tasks |
  |---|---|
  | D2611: deep link | 18–19 |
  | D2611: CLI | 17 |
  | D2611: copy-paste roots | 5–12 |
  | D2612: k3s targets | 3–7 |
  | D2612: managed Kubernetes targets | 8–11 |
  | D2612: `server-ssh` | 12 |
  | D2612: `*-vm` and `*-free` registered, not converted | 1 |
  | D2612: `laptop` listed as unavailable | 1 |
  | D2614: state | 13, Global Constraints |
  | D2615: cost guard | 2, 17–19 |
  | D2616: teardown | 17, 19 |
  | D2623: home | 3, 4, 8, 16 |
  | D2626: cluster-scoped confirmation | 16–19 |
  | D2627: k3s home default | 3–7 |
  | First real applies within the caps | 20–21 |

  D2613 (`tidbcloud`, `StorageBinding`) is DEP2. C4 refuses storage requirements until then.
- **Sibling contracts:**
  - every OP2-A name (chart ref, values, `home`, `loams-system`, readiness, `loams-i-<name>`, `loams-approver`, `approvedClusterScoped`, the `install` service account) and every OP2-B name (`IndexFiles`, `TrustAnchor`, `SignedIndex::verify`, `verify_listing`, numeric `Requires` and `CostHint`, `index.v2.json`, the feature flags) is used verbatim;
  - `DeployTarget` avoids OP2-B's `Target`.
- **Types:** `CostItem`, `DeployTarget`, `PriceTable`, `PlanSummary`, `Record`, `InstallState`, `DeployArgs` and `DeployPlanJson` keep the same names in Tasks 1–19. The contract outputs (`kubeconfig`, `cost_items`, `api_url`) are the same in every module task.
- **Proportion:** no task carries a function body. Only the bootstrap order, the CLI flow and the refusals are spelled out, because neither the spec nor the siblings fix them.
