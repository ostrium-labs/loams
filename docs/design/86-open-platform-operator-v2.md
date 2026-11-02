# 86 — Loams Open Platform: Operator v2, an Open Marketplace, and One-Click Deploy Anywhere

Status: **Approved** · 2026-11-02 (owner: "all ok, do the best", then "go ahead with the plan"). The owner set the direction on 2026-11-02 in conversation:
- "make Loams an open cloud marketplace with one click deploy on AWS, Azure, GCP, TiDB Cloud, Confluent Cloud etc"
- "our loams engines, dbs etc will be deployed on customer desired cloud, server, laptops, instruments, sensors etc"
- "I want to build a single v2 kube operator in rust for whole loams open platform, anybody can submit their agents, apps etc and deploy on any cloud etc as they need"
- "laptop can join using wifi or bluetooth, search about new standard protocol like zigbee etc"
- "yes storage also in cloud only"
- "they can open PR for submission"
- "all our forks are superseded by the original repo … we can adopt it directly"

The owner approved the direction and asked for the best design ("all ok, do the best").

Decisions **D2600–D2634** and questions **Q1600–Q1609** use a reserved block. They go into the [decision log](13-decision-log.md) when this design is approved. Plan stub: OP2, written after approval. **This document writes no code.**

This design amends:
- [§65](65-loams-operator.md), the Loams Operator. It becomes operator v2.
- [§73](73-agent-marketplace.md), the agent marketplace. It becomes the open marketplace for every listing kind.
- [§70](70-byoc-clouds.md) and [§71](71-app-byoc-free-tiers.md), BYOC. They become the one-click deploy targets.
- Every fork decision, superseded by D2600.

It builds on:
- [§69](69-edge-reach.md), the edge listener
- [§76](76-identity-and-authorization.md), identity
- [§78](78-loams-cloud-on-tidb-x.md), TiDB Cloud
- [§80](80-confluent-cloud-byoc.md), Confluent
- [§83](83-cluster-transport-http3-mtls.md), mTLS over QUIC

Markers: **(verify)** means not yet checked against a primary source; **(estimate)** means judged, not measured.

---

## 1. Summary

### 1.1 What it is

Loams becomes an **open platform with an open marketplace**. Anyone can submit an agent, an app, an MCP server or a Loams engine as a listing, by pull request to the public `ostrium-labs/loams-marketplace` index. Anyone can then deploy any listing **with one click** onto the target they choose:
- their own AWS, Azure or GCP account
- a bare server
- a laptop
- an instrument or sensor

The data always lives in **cloud storage the customer owns**: S3/GCS/Azure Blob, TiDB Cloud and Confluent Cloud.

One Rust Kubernetes operator, **`loams-operator` v2**, runs everything that lands in a cluster. A small agent, **`loams-node`**, runs on laptops and devices that cannot run Kubernetes. Devices join the way the Matter standard does: **set up over Bluetooth LE from a QR code, then run over Wi-Fi or Thread**.

### 1.2 Why

With no forks (D2600), Loams' value is not patched engines. It is **packaging official engines, managed services and community work so they deploy anywhere with one click**, and wiring them to identity, storage, metering and the gateway the same way every time. An open marketplace makes that the work of everyone, not just Ostrium.

### 1.3 What changes

| Before | After |
|---|---|
| Forks and patch queues: Neon, TiDB, Grafeo, Resonate, client-rust, iceberg-rust, Iggy, StreamFusion, Buck2, RisingWave and others (D231, D241, D820, D821, D2243, D2244, §47 §5.4) | **No forks** (D2450). Official upstream releases, images and managed services only. Behaviour that existed only in a fork is dropped or rebuilt in Loams' own crates (D2600) |
| §65 operator for Loams components only; PD and TiKV through tidb-operator v2, then native | **Operator v2** also installs marketplace listings (`LoamsInstall`), binds them to cloud storage (`StorageBinding`) and manages devices (`LoamsDevice`). It drives official upstream operators where they exist (D2601–D2604) |
| Seed Exchange (§73): agents, harnesses and MCP servers; `ostrium` and `loams` namespaces only (MK0) | **Open marketplace**: listing kinds `agent`, `harness`, `mcp-server`, `app` and `engine`; any GitHub user or org namespace; submission by PR (D2605–D2610) |
| BYOC modules validated offline, never applied (§70, §71) | **One-click deploy**: the Desktop app or `loams deploy` runs the OpenTofu modules with the user's own credentials, installs the operator, then installs the listing (D2611–D2616) |
| No device story | **Devices**: `loams-node` on laptops, instruments and sensors; BLE + QR setup; Wi-Fi or Thread for normal use; Zigbee through a gateway; storage in the cloud only (D2617–D2622) |

D1580 ("no cloud marketplace listings for now") is about selling Loams through the AWS, Azure or GCP Marketplaces. **It stands**: this design builds Loams' own marketplace, not vendor marketplace listings.

## 2. Goals, non-goals, anti-goals

**Goals**
1. One click (or one command) from a listing to a running deployment on AWS, Azure, GCP, a bare server, a laptop or a device.
2. Anyone can publish a listing by pull request. It is validated, signed and given provenance automatically.
3. One Rust operator for everything in a cluster, built on §65's model and its conformance suite (LOC).
4. Customer data only in the customer's own cloud storage. Ostrium hosts no customer data and holds no customer cloud credentials.
5. Official upstream software only.

**Non-goals (v1 of this design)**
- Paid listings. Every listing stays `pricing: "free"`, as §73 §11 says.
- Vendor marketplace listings (D1580 stands).
- Running Kubernetes on sensors.
- Storing data on devices beyond a bounded send buffer.
- Becoming a certified Matter device or controller. Loams uses the same setup pattern, not the certification (D2619).

**Anti-goals** (failures even if it works):
- A deploy that needs an Ostrium account, an Ostrium-hosted control plane or Ostrium-held cloud credentials.
- A fork, vendored patch or patch queue of any upstream project.
- Running unsigned or unreviewed third-party code on a customer's devices.
- A "one click" that silently creates paid cloud resources. Every deploy shows a cost estimate and needs confirmation (D2615).

## 3. Upstream adoption (D2600)

Every component comes from its official source. The adoption work is under way, one PR per dependency.

| Component | Official source | Fork-only behaviour that is dropped or rebuilt |
|---|---|---|
| Graph | Grafeo, official crates | 10 correctness patches. Upstream is 832 commits ahead and may already fix them; if not, they are worked around in `loams-graph` |
| Durable | Resonate, official crates | FW poll groups, `Client::poll`, trace fields. Rebuilt in `loams-durable` if still needed |
| TiKV client | tikv-client, official | Read-past-pushed-locks (upstream PR tikv/client-rust#578 is open). Until it merges, the conflicting read waits as stock upstream does |
| Iceberg | iceberg-rust, official | RowDelta / merge-on-read writes until upstream ships them |
| Postgres | neondatabase/neon, official | 3 dependency-slimming commits; heavier dependencies are accepted |
| MySQL (Loams SQL) | `pingcap/tidb` official image | pingcap/tidb#60403 until upstream releases it |
| Kafka-compatible stream | apache/iggy, official | The Kafka-wire gateway written in our fork. Offered as a Loams crate on top of the official server, if kept |
| Stream engine (§82) | datafusion-contrib/StreamFusion, official | `loams-stream` additions move into Loams crates |
| Build | meta/buck2, official release | None |

A component may also be **the managed service itself**: TiDB Cloud for SQL and KV, Confluent Cloud for Kafka, and S3/GCS/Azure Blob for objects. This is the storage-in-the-cloud rule (D2621).

## 4. Operator v2 (D2601–D2604)

§65 stands: one `kube-rs` binary, API group `loams.dev`, `LoamsCluster` and `LoamsInstance`, a journal in status, fenced disruptive actions, never on the data path, and LOC. v2 changes four things.

1. **Official operators as drivers (D2601).** Where an upstream project ships an official operator, v2 drives it as the default:
   - PingCAP tidb-operator v2 for PD, TiKV and TiDB
   - Strimzi only for self-hosted Kafka, if a listing asks for it (verify licence and fit)
   - the CloudNativePG or Neon guidance for Postgres (verify)

   §65's "native reconcilers" become optional. They are our own code, not forks, and replace a driver only after passing the same LOC profile.

2. **`LoamsInstall` (D2602).** One object per installed listing. It holds:
   - `listing` (id and version from the signed index)
   - `values` (validated against the listing's schema)
   - `storage` (references to `StorageBinding`s)
   - `identity` (the Zitadel machine user that §73 already creates per agent)
   - `exposure` (none, tailnet or gateway edge, per §69)

   The operator checks the listing's signature and provenance before anything runs. It then renders the listing's package (OCI image, Helm chart or Kustomize), applies it into a namespace per install, and reports health and cost labels to the Meter (§84).

3. **`StorageBinding` (D2603).** It connects an install to customer-owned cloud storage. Kinds:
   - `object` (S3, GCS, Azure Blob, R2)
   - `sql-mysql` (TiDB Cloud, or self-hosted TiDB)
   - `sql-postgres` (Neon or any Postgres)
   - `kafka` (Confluent Cloud, or self-hosted)
   - `kv` (TiDB Cloud's TiKV, where exposed (verify); otherwise self-hosted TiKV)

   Credentials come from the cloud's workload identity (IRSA, Workload Identity, Managed Identity) or from a Kubernetes Secret the user created. Never from Ostrium.

4. **`LoamsDevice` and `LoamsFleet` (D2604).** The operator's view of laptops and devices that run `loams-node` (§7). They are not Kubernetes nodes. The operator stores their identity, desired listings and last-seen status. Devices pull their desired state from the gateway; the operator never connects to a device directly.

The operator is distributed as one Helm chart and one OCI image, amd64 and arm64. It is installed by the one-click flow or by `helm install`. It works on EKS, AKS, GKE, k3s on a VM or server, and kind for development.

### 4.1 The platform base and listings (D2623–D2626, builder-check calls)

- **The home cluster (D2623).** Every customer deployment has at least one cluster target, called the **home**. Its `LoamsCluster` carries only the **platform base**:
  - the gateway (§57, with §69's edge listener)
  - Loams Access: identity, permissions and usage (§4.3)
  - agentd (§73)
  - the Meter (§63)
  - the operator

  Laptops and devices always belong to a home. A laptop is never the home. The cheapest home is a free-tier or small VM with k3s (`*-k3s` targets, §6).

  **Engines are `engine` listings.** TiKV/PD, TiDB, Postgres, House, Live, Graph and Stream are installed through `LoamsInstall` into a home, not hard-wired into `LoamsCluster`. §65's PD/TiKV reconcilers, HA journal, fencing and backup logic serve those installs.
- **Dependencies (D2624).** `listing/v2` adds `depends_on`: a list of listing ids and version ranges. Platform-base parts are implicit. The operator installs dependencies first, in order.
- **Agents (D2625).** `agent`, `harness` and `mcp-server` listings keep §73's packaging (npm, PyPI, OCI, release binaries) and run inside **agentd**. A `LoamsInstall` of an agent hands it to the home's agentd, which applies §73's consent, budgets and egress proxy. Only `app` and `engine` listings must ship `oci`, `helm` or `kustomize`.
  On laptops, `loams-node` **is** agentd plus the device-sync loop. Desktop runs one process, not two. `loams-node` is therefore a build of agentd with the §7 additions.
- **What third-party packages may create (D2626).**
  - **Community** listings: OCI images only, namespace-scoped resources only, no CRDs, no cluster roles, no webhooks, no privileged or host-network pods. They run under the §56 gVisor RuntimeClass where the cluster has it, and otherwise refuse to install unless the user overrides.
  - **Verified** listings: Helm and Kustomize too. Charts are rendered client-side by the operator with hooks disabled and checked against an allow-list of kinds. Cluster-scoped objects need a separate user confirmation listing them.
  - The operator's RBAC stays least-privilege (§65 §11). Each install gets a namespace-scoped service account.

### 4.2 Identity and Postgres without forks or Ostrium accounts (D2630–D2631, the author's calls)

- **Identity lives in each home (D2630).** Each home runs its own identity provider as part of Loams Access (D2632). Install and device identities, machine users and the device mTLS trust root (§83) all come from the customer's own home. `auth.loams.dev`, which Ostrium hosts, is used only to sign in to loams.dev itself, for example to submit a listing or star one. A deploy never needs it.
  This amends §76 where it assumed one shared instance, and the preview model's "Ostrium hosts console and auth" for deployments.
- **Postgres without the Neon fork (D2631).**
  - The default is a **`sql-postgres` binding to managed Postgres**: Neon's cloud, RDS, Cloud SQL or Azure Database. This fits storage in the cloud.
  - Self-hosted Postgres in a home is the official **CloudNativePG** operator, driven by operator v2 as an `engine` listing. It is plain Postgres, without branching.
  - Running official Neon self-hosted (pageserver and safekeepers, no `loams-wal`) through our own reconciler is a later, optional listing.
  - `crates/loams-postgres` and `loams-wal`'s Postgres role are retired once their users move. Loams Postgres branching survives only through Neon's managed branching.

### 4.3 Loams Access: identity, permissions and usage as one product (D2632–D2634, owner 2026-11-02)

The owner asked to "combine Loams Meter, our Zitadel Rust rewrite and OpenFGA to provide a unified experience".

- **One product, three engines (D2632).** **Loams Access** is part of every home's platform base:
  - **identity:** `loams-identity`, our clean-room Rust identity provider (repo `ostrium-labs/loams-identity`). It is not a fork, so D2450 allows it.
  - **permissions:** official **OpenFGA** (§76).
  - **usage, budgets and entitlements:** the **Loams Meter** (§63, §84).

  Until `loams-identity` passes its OIDC conformance gate, the identity part is **official Zitadel** behind the same port (`loams-zitadel`'s client and `loams-identity-sync`). Moving to `loams-identity` is then a configuration change with a data migration, not a redesign. `loams-identity`'s Phase B is no longer deferred: it runs in parallel with OP2 (its open PRs #3 and #4 are the start).
- **One model (D2633).** All three engines share one tenant tree, `org → project → {install, device, user, agent}`, and one id scheme. Creating any subject creates all three records together, from one operator reconcile or one API call:
  - a `LoamsInstall` or `LoamsDevice` gets a machine identity
  - OpenFGA tuples (`owner`, `operator`, `viewer` on its project)
  - a Meter subject with the project's budgets as entitlements

  Deleting the subject removes all three. One audit stream (`io.loams.dev.access.*` CloudEvents) records identity, permission and usage changes together.
- **One experience (D2634).** One console area, **Access**, with members and machines, roles, usage and budgets, and the audit log on one page per project. One API, `loams.access.v1`, which wraps the three engines. One CLI, `loams access`. A budget can block or warn on an action through the same OpenFGA check path: the Meter's gate (`loams-meter-gate`) answers as a condition in the permission check, so "allowed" already includes "within budget".

## 5. The open marketplace (D2605–D2610)

The index stays the public GitHub repo `ostrium-labs/loams-marketplace`, signed with Sigstore and served from GitHub Pages (§73, MK0).

- **Listing format `listing/v2` (D2605).** `listing/v1` plus:
  - `kind` (adds `app` and `engine` to `agent`, `harness` and `mcp-server`)
  - `package` (one of `oci`, `helm` or `kustomize`, by digest)
  - `requires` (storage kinds, minimum CPU and memory, GPU)
  - `targets` (any of `cluster`, `node-laptop`, `node-device`)
  - `package` also allows `wasm` (a WASI component) and `binary` (a signed static binary per target triple), but only for `node-device` and `node-laptop` targets
  - `depends_on` (D2624)
  - `values_schema` (JSON Schema)
  - `cost_hint` (expected cloud resources)

  Version 1 listings stay valid as agents.
- **Submission by PR (D2606).** A submitter forks the index repo on GitHub (an ordinary contribution fork, not a project fork) and opens a PR that adds `listings/<namespace>/<name>/versions/<version>/listing.json`. Automated checks on the PR:
  - schema validation
  - namespace ownership (the GitHub user or org that owns the namespace must open or approve the PR)
  - the package digest resolves
  - the image signature or provenance is present
  - a dry-run install on kind against LOC Core
- **Namespaces (D2607).** Any GitHub user or org namespace, opened at MK1, which §73 already planned. `ostrium` and `loams` stay reserved.
- **Review and trust tiers (D2608).** §73 §10's tiers stand.
  - Third-party listings are installable as soon as checks pass and a maintainer merges.
  - They show as **Community** until they earn **Verified**: provenance plus a named security contact plus the suite for their kind:
    - **LAPC** (§73) for `agent`, `harness` and `mcp-server`
    - **Listing Install Conformance (LIC)** for `app` and `engine`: a new, small suite that installs on kind, checks health, upgrades from the previous version, uninstalls cleanly and leaves no cluster-scoped objects; it reuses LOC's harness
    - LIC on the emulated device image (QEMU arm64) for device listings
  - Devices only run listings marked `node-device` that are Verified (D2620).
- **Revocation (D2609).** `revocations.json` (already present) is honoured by the operator and `loams-node` within one sync interval.
- **No paid listings yet (D2610)**, as §73 §11.

## 6. One-click deploy (D2611–D2616)

"Deploy" means three steps, all run **on the user's machine with the user's credentials**:
1. Provision the target.
2. Install the operator.
3. Create the `LoamsInstall`.

- **Entry points (D2611).**
  - A **Deploy** button on loams.dev listing pages and in Desktop. The button is a deep link, `loams://deploy?listing=<id>@<version>&target=<kind>`, opened by Desktop.
  - `loams deploy <listing> --target <kind>` in the CLI.
  - A copy-paste OpenTofu root module for people who use neither.
- **Targets (D2612).** Each target is an OpenTofu module in `deploy/tofu/modules/`:
  - `aws-vm`, `gcp-vm` and `azure-vm`: existing. They install Docker and the signed **compose bundle**. They stay as the non-operator path, so 0.1.x installs keep their upgrade promises (§54).
  - `aws-k3s`, `gcp-k3s` and `azure-k3s`: new. The same VMs, but cloud-init installs k3s, the operator and a home `LoamsCluster`. This is the default one-click home (D2627).
  - `aws-eks`, `gcp-gke` and `azure-aks`: new; managed Kubernetes.
  - `server-ssh`: wraps the existing `install.sh`, plus k3s.
  - `laptop`: no Kubernetes; Desktop starts `loams-node`.
  - `*-free`: existing, for apps that fit the free tiers.
- **Managed-service bindings (D2613).** `confluent` already exists. A `tidbcloud` module creates a TiDB Cloud cluster and user and emits a `StorageBinding`; it uses TiDB Cloud's official provider (verify that it works with OpenTofu). Object-store bindings come with each cloud module.
- **State (D2614).** OpenTofu state stays with the user: a local file by default, or a bucket they choose. Ostrium never holds state or credentials, as §70 says.
- **Cost guard (D2615).** Before applying, Desktop and the CLI show the resources and an estimated monthly cost from the listing's `cost_hint` and the cloud price tables (§84 inputs), and ask for confirmation. Free-tier targets say so.
- **Teardown (D2616).** One click removes the install, and optionally the target, through `tofu destroy` on the same state.

## 7. Laptops, instruments and sensors (D2617–D2622)

Research summary (2026-11-02, vendor and community sources; see Q1603):
- **Matter** is an application layer that runs over **Wi-Fi, Thread or Ethernet**.
- Almost every Matter device is **set up over Bluetooth LE first**, from a QR code or an 11-digit code, with certificate-based device attestation.
- **Thread** is an IPv6 802.15.4 mesh, so devices are reachable over IP.
- **Zigbee** uses the same radio but needs a coordinator or gateway to reach IP.
- Common chips (Nordic nRF52/53, ESP32-H2, Silicon Labs) do BLE and 802.15.4 at the same time.
- Sources disagree on whether Matter 1.4 or 1.6 is current (verify).

Design:
- **`loams-node` (D2617).** A small Rust agent, built from Loams' own crates and official libraries only. It:
  - holds the device identity
  - pulls desired state from the gateway
  - runs listings marked for its target (OCI containers on laptops; WASM components or a native binary on devices)
  - streams data to the bound cloud storage

  Build targets: Linux, macOS and Windows on x86-64 and arm64 for laptops; Linux on armv7 and arm64 for gateway-class devices. Microcontroller targets are Q1601.
- **Laptops (D2618).** Desktop (Electron, already shipping) installs and supervises `loams-node`. A laptop joins over its normal Wi-Fi or Ethernet network by signing in through Desktop; no Bluetooth is needed. Bluetooth setup is available for headless machines.
- **Devices: setup (D2619).** Matter-style, but Loams' own:
  1. The device shows or carries a QR code (a setup code plus device key fingerprint).
  2. The Loams mobile or desktop app connects over **BLE**, checks the device's key, hands it Wi-Fi or Thread credentials and a one-time join token, and the device enrols with the gateway.
  3. The gateway returns a short-lived mTLS certificate (§83) bound to a Zitadel machine identity (§76).

  Loams does not claim Matter certification. A future Matter bridge listing could expose Loams data to Matter ecosystems (Q1602).
- **Networks (D2620).**
  - **Wi-Fi** and **Ethernet** first.
  - **Thread** through any Thread border router, since devices get IPv6.
  - **Zigbee** devices join through a Zigbee coordinator attached to a `loams-node` gateway device. That coordinator runs an official open-source Zigbee stack as a listing (verify which: Q1604).
  - **LoRaWAN** is out of scope for v1.

  Devices reach Loams only through the gateway edge listener (§69). They are never directly on the public internet.
- **How a device reaches its home (D2628).** Through the customer's **tailnet**, the way the release bundle already works (the Headscale or Tailscale profile, §43). `loams-node` embeds a tailnet client. Gateway-class Linux devices and laptops use the official Tailscale client. The one-time join token from setup includes a pre-auth key for the customer's Headscale or Tailscale, scoped to a `tag:loams-device` ACL. §69's public profile (D1312) stays deferred. A customer who already uses Cloudflare can choose Cloudflare Tunnel with an Access service token instead.
- **Who does the Bluetooth setup (D2629).** Desktop first. `loams-node`'s native BLE uses the official `btleplug` crate, which is not a fork. Electron's Web Bluetooth is not used. Desktop already ships on Linux, macOS and Windows. The mobile apps follow when plans AP2 and AP3 publish them.
- **Storage (D2621).** Cloud only. Devices and laptops keep at most a bounded, encrypted send buffer, so data survives an outage up to its limit (default 256 MiB (estimate)). After that, the oldest data is dropped and the drop is counted and reported. No Loams database runs on a device.
- **What runs on devices (D2622).** Only Verified listings marked `node-device`, signed and checked against `revocations.json`. Each runs in the strongest sandbox the device supports (§56: Linux namespaces and seccomp; WASM where there is no OS isolation).

## 8. Phases

| Phase | Scope | Depends on |
|---|---|---|
| **UP1** (in progress) | Adopt every official upstream and remove the forks (§3) | none |
| **OP2-0** | Operator v2 core: §65 OP1 (`LoamsCluster`, journal, fencing, LOC Core) plus `LoamsInstall` for `oci` and `helm` packages | UP1 |
| **MK1** | `listing/v2`, open namespaces, PR checks with a kind dry-run, Community and Verified tiers | OP2-0 for the dry-run |
| **DEP1** | One-click on AWS, GCP and Azure (VM + k3s, then EKS/GKE/AKS), Deploy deep link, cost guard, teardown; first real applies of the existing modules | OP2-0 |
| **DEP2** | `StorageBinding` with Confluent (existing module), TiDB Cloud (new module) and object stores | DEP1 |
| **EDGE1** | `loams-node` on laptops through Desktop; `LoamsDevice`; pull-based desired state | OP2-0 |
| **EDGE2** | BLE + QR setup; Wi-Fi and Thread devices on Linux arm | EDGE1 |
| **EDGE3** | Zigbee through a coordinator listing; optional Matter bridge listing | EDGE2 |

## 9. Decisions (D2600–D2634)

| ID | Decision |
|---|---|
| D2600 | No forks, hard forks or patch queues of any project: this design's restatement of **D2450** (recorded by the UP1 PRs, 2026-11-02). Official upstream releases, images and managed services only; fork-only behaviour is dropped or rebuilt in Loams' crates. Supersedes D231, D241, D820, D821, D2243, D2244 and §47 §5.4 |
| D2601 | Operator v2 drives official upstream operators where they exist; native reconcilers are optional and LOC-gated |
| D2602 | `LoamsInstall`: one object per installed listing, signature-checked, one namespace per install |
| D2603 | `StorageBinding` to customer-owned cloud storage, with workload identity first |
| D2604 | `LoamsDevice` and `LoamsFleet`; pull-based; the operator never dials devices |
| D2605 | `listing/v2` with kinds `agent`, `harness`, `mcp-server`, `app` and `engine` |
| D2606 | Submission by PR to `loams-marketplace`, with automated checks and a kind dry-run |
| D2607 | Open GitHub user and org namespaces at MK1 |
| D2608 | Community and Verified tiers |
| D2609 | Revocation honoured within one sync interval |
| D2610 | Free listings only |
| D2611 | Deploy deep link, `loams deploy`, and a copy-paste OpenTofu root module |
| D2612 | Targets are OpenTofu modules: VM + k3s, managed Kubernetes, `server-ssh`, `laptop`, free tiers |
| D2613 | Managed-service bindings: Confluent (existing) and TiDB Cloud (new module) |
| D2614 | OpenTofu state and credentials stay with the user |
| D2615 | Cost estimate and confirmation before every apply |
| D2616 | One-click teardown |
| D2617 | `loams-node` agent for laptops and devices |
| D2618 | Laptops join through Desktop over their normal network |
| D2619 | Matter-style BLE + QR setup; not Matter-certified |
| D2620 | Wi-Fi, Ethernet and Thread directly; Zigbee through a coordinator; LoRaWAN out of scope |
| D2621 | Storage in the cloud only; bounded, encrypted send buffer on devices |
| D2622 | Devices run only Verified, signed `node-device` listings, sandboxed |
| D2623 | Every deployment has a home cluster with the platform base (gateway, identity, agentd, Meter, operator); engines are `engine` listings installed by `LoamsInstall` |
| D2624 | `listing/v2` `depends_on`; the operator installs dependencies first |
| D2625 | Agent, harness and MCP-server listings run in agentd (§73 packaging); `loams-node` is agentd plus device sync |
| D2626 | Community listings: OCI only, namespace-scoped, gVisor RuntimeClass; Verified may use Helm/Kustomize rendered client-side, no hooks, allow-listed kinds |
| D2627 | New `aws-k3s`, `gcp-k3s`, `azure-k3s` home targets; the compose VM modules stay as the non-operator path |
| D2628 | Devices reach their home over the customer's tailnet (official Tailscale/Headscale), or Cloudflare Tunnel + Access; D1312 stays deferred |
| D2629 | Desktop does BLE setup first, through `btleplug`; mobile follows AP2/AP3 |
| D2630 | Identity lives in each home, as part of Loams Access; `auth.loams.dev` only for loams.dev itself |
| D2632 | Loams Access = `loams-identity` (Rust; official Zitadel behind the same port until it passes OIDC conformance) + official OpenFGA + Loams Meter, in every home's platform base; `loams-identity` Phase B runs in parallel with OP2 |
| D2633 | One tenant tree (`org → project → install/device/user/agent`) and id scheme; one reconcile creates and removes identity, OpenFGA tuples and Meter subject together; one `io.loams.dev.access.*` audit stream |
| D2634 | One Access console area, `loams.access.v1` API and `loams access` CLI; budgets enforced as a condition in the OpenFGA check |
| D2631 | Postgres: managed by default (`sql-postgres` binding); self-hosted via official CloudNativePG; official Neon self-hosted later; `crates/loams-postgres` retired as users move |

## 10. Open questions (Q1600–Q1609)

| ID | Question | Default |
|---|---|---|
| Q1600 | Does `loams-node` on a laptop also run full Loams engines locally for offline development? This would conflict with "storage in the cloud only" | No. The cloud only; local development keeps using the existing compose bundle |
| Q1601 | Microcontroller devices (no Linux), such as ESP32 or nRF | Later phase: a `no_std` data-push client, without running listings |
| Q1602 | Matter bridge listing | EDGE3, optional |
| Q1603 | Matter and Thread versions to target | Check the CSA and Thread Group specifications before EDGE2 |
| Q1604 | Which official Zigbee stack the coordinator listing uses | Choose at EDGE3 (zigbee2mqtt or the Zigbee stack of the chosen coordinator chip (verify)) |
| Q1605 | Who reviews third-party listing PRs | Maintainers in `CODEOWNERS` for the index repo; automated checks first |
| Q1606 | Is TiDB Cloud a deploy target (Loams engines run inside it) or only a storage binding? | Storage binding only, as the owner chose "storage in the cloud" |
| Q1607 | Keep the Iggy Kafka gateway as a Loams crate on official Iggy, or use Confluent and Kafka directly? | Use Kafka or Confluent directly; drop the gateway unless a listing needs it |
| Q1608 | Self-hosted Kubernetes distribution on VMs and servers | k3s |
| Q1609 | Where the Deploy button runs for users without Desktop | The CLI and the copy-paste OpenTofu module; no hosted deploy service |

## 11. Risks

- **Lost fork behaviour.** Some fixes (read-past locks, Grafeo correctness, RowDelta, the TiDB patch) disappear until upstream ships them. Mitigation: UP1 lists every removal; tests for removed behaviour are marked, not silently deleted.
- **Third-party code.** An open marketplace is a supply-chain target. Mitigation: signatures, provenance, revocation, sandboxing, the Verified tier for devices, and the kind dry-run.
- **Real cloud spend.** The VM modules have never been applied. DEP1 starts with the free-tier rows and the cost guard, within the existing benchmark budgets.
- **Scope.** Operator, marketplace, deploy and devices together are large (estimate: several engineer-months). The phases ship value in order: UP1, then OP2-0, then MK1 and DEP1 in parallel, then DEP2, then EDGE.

## 12. Plan scope

The first plan, **OP2**, covers OP2-0, MK1 and DEP1, and starts **after UP1 merges**. It assumes the official upstream crates and images, with no fork code in the tree. DEP2 and EDGE1–EDGE3 get their own plans later.

Decisions added after the builder check, all the author's calls under the owner's "do the best": D2623 (home cluster, engines as listings), D2624 (`depends_on`), D2625 (agents run in agentd; `loams-node` is agentd plus device sync), D2626 (third-party package limits), D2627 (`*-k3s` targets beside the compose VMs), D2628 (devices reach their home over the customer's tailnet), D2629 (Desktop does the Bluetooth setup first), D2630 (identity in each home), D2631 (Postgres: managed by default, CloudNativePG self-hosted), plus LIC and the `wasm` and `binary` package kinds.

## 13. Left to the builder

The following are left to the builder:
- CRD field names beyond those named here
- reconcile intervals
- the exact PR-check workflow
- the cost estimator's price sources (reusing §84's tables)
- the OpenTofu layout for the new modules
- `loams-node`'s internal structure
- the BLE GATT service layout and QR payload encoding (follow Matter's setup-payload ideas without claiming compatibility)
- the sizes of the device send buffer per class
