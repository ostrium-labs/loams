# 64 — Loams Infra: the HashiCorp Stack, Loams-Native and in Rust

> **Amendment (approved 2026-10-23) by [§78](78-loams-cloud-on-tidb-x.md)** (Loams Cloud on TiDB X; D1700–D1735). HCP is reconciled (§78 §16, D1729): nothing in Loams depends on HCP; Ostrium's own Loams Cloud infrastructure uses OpenTofu with state in R2; HCP Terraform Free is at most an optional state-only backend; HCP Boundary and Vault are not used. §1's "HCP" row stays superseded by D1323.

> **Amendment (approved 2026-10-21) by [§76](76-identity-and-authorization.md)** (identity and authorization: Zitadel and OpenFGA; owner, all defaults). §2.3's "Loams keeps Authentik" becomes Zitadel (D1600; still one IdP and no new one besides it), and §10.2's "two `Authorizer` traits, two principal vocabularies" is resolved by one trait in `loams-authz` over §19's three principal kinds (D1610, D1605).

Status: **Approved** · 2026-10-19 (owner approved all decisions and questions, 2026-10-19). Source: the owner's request of 2026-10-19, **"I want all of the HashiCorp stack in Loams, in Rust."** Decisions **D1150–D1171** and questions **Q970–Q983** are in the [decision log](13-decision-log.md), all approved by the owner on 2026-10-19 with their recommended defaults. The numbers leave a gap: D1119 is the no-open-core ruling of the same day, and D1120–D1149 and Q960–Q969 are left for §63 (Loams Meter), which is being written in parallel. Plan stub: [IS1](../plans/2026-10-19-is1-infra-stack.md). **This document writes no code.**

Builds on:
- [§36](36-loams-git.md) §19.13: `env/` secrets as code, the `loams-vault` crate and its `SecretBackend` (D839–D847).
- [§46](46-loams-postgres-production.md) §8.6: `IssueConnectCredential` and `VALID UNTIL` login roles (D711).
- [§02](02-stream-engine.md), [§10](10-operations.md) §4: customer-managed keys through the `KeyProvider` trait (D96).
- [§18](18-metastore-backends-and-router.md), [§19](19-console-identity-and-agents.md), [§38](38-knative-authentik-gitops.md): principals, `ControlStore`, OpenFGA, Authentik and the RFC 8693 exchange (D65–D67, D447–D449).
- [§24](24-cpu-time-runtime.md) and [§26](26-jobs-api.md): Dapr secrets (D189), the `Runner` trait (D375), Loams Jobs on TiKV (D204–D216).
- [§41](41-multitenant-byoc-control-plane.md) and the [CP1](../plans/2026-10-10-cp1-control-plane-byoc.md) plan: the BYOC agent (D543), GitOps and the cluster CA (CP1 Task 18).
- [§43](43-private-networking.md): the tailnet (D580–D599).
- [§44](44-unified-api-and-sdks.md): one Connect API and compatibility surfaces (D600–D603).
- [§53](53-studio-ade.md), [§39](39-software-factory-and-loams-bot.md): Studio, the Software Factory and its deploy stage.
- [§56](56-loams-sandbox.md), [§60](60-places-and-harness-composition.md), [§61](61-loams-jailer.md): the sandbox, Places and the Jailer's PDP.
- [§57](57-loams-gateway.md), [§58](58-data-access-and-htap.md): the gateway and the Data API.

Markers, as in §62:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.

---

## 1. Summary

### 1.1 The request and what it means

The owner asked for "all of the HashiCorp stack in Loams, in Rust", with two rules from the same day: Loams is fully open source (Apache-2.0, no open core; D1119), and OpenTofu (MPL-2.0) is preferred over Terraform (BSL).

Read literally, the request is eight products and a cloud: Terraform, Vault, Consul, Nomad, Packer, Boundary, Waypoint, Vagrant and HCP. Cloning them would take a large company years, and HashiCorp itself has since archived two of them (Waypoint, Boundary Community Edition). This document reads the request the way Loams has read every "replace X" so far: **take over the role each product plays for Loams users, on Loams' own primitives, not its wire protocol or its feature list** (the D42 rule). Only where an ecosystem protocol *is* the value do we speak it: the OpenTofu provider protocol, OpenTofu's state backend, and later a small Vault/OpenBao client-compatible subset.

### 1.2 The recommendation in one paragraph

Build **Loams Infra** as a few small Rust services inside the `loams` binary, sharing one identity (Loams principals, OIDC exchange and workload identity), one policy layer (OpenFGA for who-may, the Jailer's PDP for agents), one storage model (small hot state in TiKV, bodies in the bucket under the namespace's KMS key) and one API style (Connect services, CloudEvents, durable workflows). The first slice has the highest user value and the least new surface: **an OpenTofu provider and state backend for Loams, a secrets service with dynamic credentials for Loams Postgres and Loams SQL, and a config and service catalog on TiKV.** Access brokering, deploy workflows and plan runs follow. A Nomad-style scheduler, image builder and dev-environment manager come only if users ask, and some parts of the HashiCorp stack are never built (§9).

### 1.3 The map

| HashiCorp product | Role for a Loams user | Loams component | Reuse | New | Phase |
|---|---|---|---|---|---|
| **Terraform** (BSL) | Declare Loams resources next to the rest of the infrastructure; keep state somewhere safe | **OpenTofu provider `ostrium-labs/loams`** and the **Loams state backend** (OpenTofu `http` protocol, `loams.infra.v1`) | The Connect API and proto annotations (`loams-facade-gen`), the bucket, D96, MetaStore leases | `loams-tfplugin` (a tfplugin6 server in Rust), the provider binary, the state service | IS1 |
| **Vault** (BSL) | Store secrets; never hand out long-lived DB passwords; let CI and workloads authenticate without static keys | **Loams Secrets**: `loams-vault` (GT1) grown into `loams.secrets.v1` with KV, leases, dynamic credentials, workload identity | `loams-vault`, `SecretBackend`, `KeyProvider`, `IssueConnectCredential` (D711), rotation providers (D843), Dapr `loams` store (D189) | Leases, the engines registry, trust policies for external OIDC issuers, the Loams workload issuer | IS1 (core), IS2 (transit, PKI, Vault-client subset) |
| **Consul** (BSL) | Config that changes without a redeploy; find services on BYOC VMs without Kubernetes | **Loams Config and Catalog**: `loams.config.v1` on TiKV, with Watch | `loams-kv` (embedded or TiKV), MetaStore leases, `env/` live sets (D842), the gateway route table (D1018) | Config entries, the service catalog, health by lease | IS1 |
| **Nomad** (BSL) | Run batch, cron and long-running workloads on own VMs | **Loams Run**: placement over the `Runner` trait and `loams-sandbox` | Jobs (D204–D216), Durable schedules, `SupervisorRunner`, `KnativeRunner`, the sandbox adapters | A small scheduler and node agent | IS4, gated on demand |
| **Packer** (BSL) | Images for sandboxes, workspaces and BYOC nodes | **`loams image`**: OCI → rootfs and microVM images | §15 §5.2 environment images, Containerfiles in CI, cloud-init | An OCI-to-rootfs converter and signing | IS4, gated on demand |
| **Boundary** (BSL; Community Edition archived 2026-10) | Just-in-time, audited access to databases and hosts for people and agents | **Loams Access**: grants with expiry, approvals and brokered credentials | The tailnet (D580s), `loams-gateway` `pg` (§57), D711, phone approvals (§37), Places (D1087), Jailer verdicts | `loams.access.v1`, an SSH certificate authority | IS3 |
| **Waypoint** (archived 2024) | Build, deploy, promote and roll back an app | **Deploy stage of the Software Factory**: `loams deploy` as a durable workflow | SF4's factory loop, D842 promotion on merge, runners, §45's versioned deploys with rollback | A deploy record and the CLI | IS3 |
| **Vagrant** (BSL) | Reproducible dev environments | **Studio workspaces with `devcontainer.json`** | Studio worktrees (D903), microVM workspaces (D1006), `loams dev` stacks (D285) | A devcontainer reader | IS4 |
| **HCP** | Someone else runs it | ~~**Ostrium services**: the same open code, operated under an SLA~~ (superseded by D1323: Ostrium offers no services and no SLA; the open code is run by the user or a third party) | BYOC agent (D543), the hosted console and auth | Nothing closed | Any phase |
| Sentinel (proprietary) | Policy on plans and changes | **No new language.** OpenFGA, the PDP's typed rules, OPA/conftest optional | §61 PDP, OpenFGA | Plan checks as PDP rules | IS3 |

## 2. The landscape in October 2026

### 2.1 Licences after the BSL change and IBM

- **The change.** On 2023-08-10 HashiCorp moved future releases of its products to the Business Source License 1.1. APIs, SDKs and almost all libraries stayed MPL-2.0 ([announcement](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license), [licence FAQ](https://www.hashicorp.com/en/license-faq)).
- **The terms today** (the `LICENSE` file on each repository's `main`, read 2026-10-19). All six name International Business Machines Corporation as licensor and carry the same Additional Use Grant: production use is allowed except offering the software to third parties, hosted or embedded, in competition with IBM's paid versions. Each release converts to MPL-2.0 four years after it is published. The first BSL version of each:

  | Product | BSL from | Licence file |
  |---|---|---|
  | Terraform | 1.6.0 | [LICENSE](https://raw.githubusercontent.com/hashicorp/terraform/main/LICENSE) |
  | Vault | 1.15.0 | [LICENSE](https://raw.githubusercontent.com/hashicorp/vault/main/LICENSE) |
  | Consul | 1.17.0 | [LICENSE](https://raw.githubusercontent.com/hashicorp/consul/main/LICENSE) |
  | Nomad | 1.7.0 | [LICENSE](https://raw.githubusercontent.com/hashicorp/nomad/main/LICENSE) |
  | Packer | 1.10.0 | [LICENSE](https://raw.githubusercontent.com/hashicorp/packer/main/LICENSE) |
  | Vagrant | 2.4.3 (as the file states) | [LICENSE](https://raw.githubusercontent.com/hashicorp/vagrant/main/LICENSE) |

- **IBM.** The acquisition closed on 2025-02-27 ([Consulting](https://www.consultingmag.com/2025/03/04/ibm-completes-acquisition-of-hashicorp)). No relicensing has followed; only the named licensor changed.
- **Products HashiCorp has wound down:**
  - Waypoint Community Edition: archived 2024-01-08 ([repo](https://github.com/hashicorp/waypoint)).
  - CDK for Terraform: archived 2025-12-10 ([repo](https://github.com/hashicorp/terraform-cdk)).
  - HCP Vault Secrets: end of sale 2025-06-30 ([notice](https://support.hashicorp.com/hc/en-us/articles/41802449287955-HCP-Vault-Secrets-End-Of-Life)).
  - HCP Consul Dedicated: retired 2025-11-12; the legacy HCP Terraform free plan ended 2026-03-31 ([notice](https://support.hashicorp.com/hc/en-us/articles/47520801288083-End-of-Life-Notice-for-HCP-Terraform-Legacy-Free-Plan-HCP-Consul-Dedicated)).
  - HCP Vagrant (the box registry): no new boxes or registries from 2026-10-01, end of support 2026-11-02, end of operations 2026-12-31. The blog (2026-09-17) and the docs page agree as of 2026-10-19 ([blog](https://hashicorp.com/blog/hcp-vagrant-deprecation-important-dates-and-migration-guidance), [docs](https://developer.hashicorp.com/hcp/docs/vagrant/hcp-vagrant-eol)).
  - **Boundary Community Edition: archived on 2026-10-08/09**, with no further releases or security fixes; HashiCorp points users to HCP Boundary and Boundary Enterprise ([repo](https://github.com/hashicorp/boundary)).
- **Still moving.** Vault 2.0 shipped around April 2026 with SPIFFE JWT-SVIDs and workload identity federation, still under BSL ([InfoQ](https://infoq.com/news/2026/04/vault-2-0-ibm-identity/)).

The pattern matters for Loams: the open user base of the HashiCorp tools is looking for homes, and the two products HashiCorp gave up on (deploy workflows, identity-aware access) are the ones closest to what Loams already does.

### 2.2 The open forks and alternatives

| Need | Open project | Licence and governance | What Loams does with it |
|---|---|---|---|
| IaC | **OpenTofu** | MPL-2.0; Linux Foundation; CNCF Sandbox since 2025-04-23 ([CNCF](https://www.cncf.io/projects/opentofu/)) | **The engine Loams supports.** Loams writes a provider and a backend for it; it never forks it |
| Secrets | **OpenBao** | MPL-2.0; OpenSSF Sandbox; forked from Vault 1.14, the last MPL release ([docs](https://openbao.org/docs/what-is-openbao/)) | An interop target: a `SecretBackend` (D840), a dynamic-credential consumer, and a client-compatible subset (IS2). Ideas only, never code (§11) |
| Secrets (Rust) | **RustyVault** (Tongsuo) | Apache-2.0; small, not clearly production-ready ([repo](https://github.com/Tongsuo-Project/RustyVault)) | Read for design ideas. Not adopted: its storage model and API scope are not Loams' |
| Discovery and mesh | No community fork of Consul or Nomad was found in a search on 2026-10-19 (unverified) | — | Nothing to adopt |
| Mesh data plane (Rust) | **linkerd2-proxy** | Apache-2.0; CNCF; open stable releases stopped in 2024 ([releases](https://linkerd.io/releases/)) | Not adopted: no sidecar mesh (D1010, §9) |
| Access | **Teleport** | AGPL-3.0 since 2023-12, Community Edition binaries under a commercial licence from v16 ([blog](https://goteleport.com/blog/teleport-oss-switches-to-agpl-v3/), [discussion](https://github.com/gravitational/teleport/discussions/39158)) | Excluded by licence (AGPL, D1023) |
| Access | **Pomerium**, **OpenZiti** | Apache-2.0, Go ([Pomerium](https://github.com/pomerium/pomerium), [OpenZiti](https://github.com/openziti/ziti)) | References. Loams' tailnet and gateway already cover their transport |
| Dev environments | **devcontainer.json** | Spec under CC-BY-4.0 ([spec](https://github.com/devcontainers/spec)) | The descriptor Loams workspaces read (IS4) |
| Dev environments | **DevPod** | MPL-2.0 ([repo](https://github.com/loft-sh/devpod)) | Reference; not linked |
| Workload identity | **SPIFFE** and the `spiffe` crate | Apache-2.0 ([rust-spiffe](https://github.com/maxlambrecht/rust-spiffe)) | Loams workload IDs are SPIFFE-shaped; the crate is optional |
| Policy (Rust) | **Cedar**; **regorus** (Rego) | Cedar Apache-2.0 ([repo](https://github.com/cedar-policy/cedar)); regorus `MIT AND Apache-2.0 AND BSD-3-Clause` per its `Cargo.toml` and crates.io 0.13.0 ([repo](https://github.com/microsoft/regorus)) | Not adopted now (Q979) |

### 2.3 Rust building blocks

- **TLS and certificates:** rustls (Apache-2.0/MIT/ISC) with aws-lc-rs, rcgen (MIT/Apache-2.0) already pinned by CP1, instant-acme (Apache-2.0) for ACME ([rustls](https://github.com/rustls/rustls), [rcgen](https://github.com/rustls/rcgen), [instant-acme](https://github.com/djc/instant-acme)).
- **Isolation:** Firecracker (Apache-2.0), already the `microvm` adapter's first VMM (§56).
- **Identity:** Kanidm (MPL-2.0) and Rauthy (Apache-2.0) are Rust identity providers. Loams keeps Authentik (D447) and adds no IdP ([Kanidm](https://github.com/kanidm/kanidm), [Rauthy](https://github.com/sebadob/rauthy)).
- **OpenTofu providers in Rust:** `tf-provider` (Apache-2.0, last release July 2024) and `terustform` (which vendors `tfplugin6.6.proto`) prove a non-Go provider works ([tf-provider](https://github.com/aneoconsulting/tf-provider), [terustform](https://git.noratrieb.dev/Noratrieb/terustform)). Neither covers ephemeral resources or write-only attributes, so Loams writes its own small server (§4.2).

## 3. Principles

1. **Roles, not clones.** Each component answers a Loams user's need. A HashiCorp feature that no Loams user needs is not built, however central it is to the original.
2. **Ecosystem protocols only where they are the value.** The OpenTofu provider protocol and state backend protocol are spoken exactly. A Vault/OpenBao client subset is a later, tested compatibility surface (D603's rule), never the native API.
3. **One identity, one policy, one storage model, one API style** (§5). No component brings its own user database, ACL language, Raft group or REST API.
4. **Reuse first.** Loams already has a vault (GT1), dynamic Postgres credentials (D711), a cluster CA (CP1), leases with fencing (MetaStore), durable workflows, a sandbox, a gateway and a scheduler-shaped jobs system. Most of this document wires them together.
5. **The default build does not change.** Each component is a cargo feature (`infra-state`, `secrets`, `config`) and enters the prebuilt variants (§30 §9), like `durable` and `graph`.
6. **Licence hygiene before features** (§11): no BSL code ever; MPL code only as interop, never ported.

## 4. Terraform → the OpenTofu provider and the Loams state backend

### 4.1 What users need

- **Declare Loams resources in the same plan as everything else.** A team that creates its AWS VPC with OpenTofu wants its Loams Postgres project, branches, roles, collections, streams, API keys and secrets in the same plan, reviewed in the same PR.
- **A safe place for state.** State files hold secrets and must be locked, versioned and encrypted. Loams already has a bucket, KMS envelope encryption and leases.
- **No new language.** HCL and OpenTofu's module ecosystem are what users know. A Loams IaC language would be a cost with no user value (§9).

### 4.2 The provider (D1151, D1152)

- **Address:** `registry.opentofu.org/ostrium-labs/loams`, from a mirror repository `ostrium-labs/terraform-provider-loams` written only by a release workflow, as D619 does for SDKs. The registry requires that repository name and a GitHub issue to list the provider and its signing key ([registry](https://github.com/opentofu/registry)). The same binary works with Terraform, since the protocol is shared; Loams documents and tests OpenTofu only (D1150).
- **Language:** Rust. A new crate `loams-tfplugin` implements the plugin side of protocol v6: the go-plugin handshake line on stdout, AutoMTLS with the client's certificate, the gRPC `Provider` service, and the stdio and controller services. `tfplugin6.proto` is vendored unmodified under its MPL-2.0 header in `proto/vendor/tfplugin6/` (§11). The crate is small (estimate: 2–3 kLoC) and has no other user.
- **Fallback:** if the Rust server fails the acceptance gate at the end of its spike (IS1 Task 2), the provider is written in Go on `terraform-plugin-framework` (MPL-2.0) against the generated Go SDK. Q970 records this.
- **Schemas from the protos.** Resources map one-to-one onto Connect RPCs. A new proto option `(loams.options.v1.resource)` marks create/read/update/delete RPCs, the identity fields, immutable fields and secret fields, and `loams-facade-gen` (D606) generates the provider's schema tables and CRUD glue. Hand-written code is limited to import IDs, plan modifiers and diff suppression.
- **First resources (IS1):** `loams_namespace`, `loams_api_key`, `loams_postgres_project`, `loams_postgres_branch`, `loams_postgres_endpoint`, `loams_postgres_role`, `loams_postgres_database`, `loams_sql_database`, `loams_collection`, `loams_stream`, `loams_secret`, `loams_config_entry`, `loams_identity_trust`. Data sources for each, plus `loams_postgres_connection_info`.
- **Secrets never land in state.** OpenTofu 1.11 added **ephemeral resources** and **write-only attributes** ([release](https://opentofu.org/blog/opentofu-1-11-0), [docs](https://opentofu.org/docs/language/ephemerality/)). The provider uses them:
  - `loams_secret.value_wo` with `value_wo_version` writes a value that state never holds;
  - `ephemeral "loams_postgres_credential"` issues a short-lived login role through the secrets service (§5.3) for a run, so the `postgresql` provider can apply grants without a stored password;
  - `ephemeral "loams_secret"` reads a value for another provider's write-only attribute.
- **Authentication:** `LOAMS_TOKEN`, an API key, or, preferred, an OIDC token from the CI system exchanged for a Loams token under a trust policy (§5.4). The provider never asks for a password.
- **Tests:** acceptance tests run the real `tofu` binary (pinned, two minor versions) against `loams dev` in CI, with a plan-apply-refresh-import-destroy cycle per resource and a "no secret in state" check that greps the state file and the plan file for every issued value.

### 4.3 The state backend (D1153)

OpenTofu has three backends that need nothing from Loams, and Loams documents all three:

| Backend | Works today against | Gap |
|---|---|---|
| `s3` with `use_lockfile = true` (conditional writes) ([docs](https://opentofu.org/docs/language/settings/backends/s3/)) | The deployment's own bucket (RustFS, S3, R2) | No per-workspace authorisation beyond bucket IAM; no audit; locks not visible in the console |
| `pg` ([docs](https://opentofu.org/docs/language/settings/backends/pg/)) | A Loams Postgres database | Advisory locks with no `force-unlock`; the state is a text column in the user's own database |
| `http` ([docs](https://opentofu.org/docs/language/settings/backends/http/)) | The Loams state service below | — |

The **Loams state backend** is OpenTofu's `http` protocol, served as a compatibility surface (D603) on the main port at `/tofu/v1/state/<org>/<namespace>/<workspace>`:
- `GET` returns the latest version; `POST` writes a new one; `LOCK` and `UNLOCK` (on the same address) take and release a lease, answering `423` with the holder's lock info when it is held. Updates carry the lock ID, and a write without the current lock is refused.
- **Storage:** each version is a create-only object `ns/<ns>/infra/state/<workspace>/v/<n:010>.tfstate`, sealed with a fresh data key wrapped by the namespace's `KeyProvider` key (D96). The workspace record (latest version, lock, serial, lineage) is a MetaStore record with CAS, and the lock is a MetaStore lease with a fencing epoch, so a stale writer cannot overwrite a newer serial.
- **Auth:** the backend takes a Loams bearer token through OpenTofu's `headers` setting or basic auth with an API key; OpenFGA checks `state_workspace#writer` or `#reader`.
- **`loams.infra.v1.StateService`** (Connect) gives the console and CLI what the `http` protocol cannot: list workspaces and versions, show a redacted diff between versions, roll back by writing an old version as new, break a lock (owner only, audited), and read declared outputs.
- **OpenTofu's client-side state encryption** ([docs](https://opentofu.org/docs/language/state/encryption/)) stays compatible: Loams then stores ciphertext and cannot show diffs or outputs. The docs recommend it for teams that do not want Loams to read state; the default is server-side encryption so the console works.
- **Events:** `io.loams.dev.infra.state.written.v1`, `…locked.v1`, `…unlocked.v1` on the namespace stream `_infra`, with workspace, serial, principal and resource counts, never values.

### 4.4 Plan runs (later, IS3; D1164)

The HCP Terraform and Atlantis role: `tofu plan` on a pull request, a reviewed apply after merge. Loams already has the parts: Loams Git events (§36), Jobs (§26), the sandbox (§56) and approvals. A **plan run** is a job in a `linux` sandbox with egress limited to the providers' endpoints and Loams, credentials from ephemeral resources and trust policies only, the plan JSON and redacted diff attached to the PR or Studio thread, and an apply gated by a person's approval and the PDP's plan rules (§8). Not before IS3, and only if IS1's users ask for it.

### 4.5 What is not built

No HCL engine, no fork of OpenTofu, no Loams IaC language, no CDK, no private provider registry (a provider network mirror is a static file tree any bucket can serve), no Terraform CLI in Loams' own CI (D1150).

## 5. Vault → Loams Secrets

### 5.1 What users need

1. A place for application secrets with versions, audit and environment scoping. **§36's `env/` and `loams-vault` already give this** for repositories.
2. **No long-lived database passwords**, for people, apps or agents. **D711's `IssueConnectCredential` already gives this** for Loams Postgres; Loams SQL and external databases need the same.
3. **No static cloud keys in CI or workloads.** CI should log in with its own OIDC token; a Loams job should reach AWS through federation, not an access key.
4. Secrets reaching runtimes the way they expect: environment variables, the Dapr API, Kubernetes, files.
5. Revocation that actually happens: when an agent run is quarantined (§61 §4.4) or a person leaves, every credential they were issued dies.

What Vault adds beyond this (transit encryption as a service, a PKI, dozens of engines, namespaces, Sentinel, replication) is valuable to some users, and §5.6 sets which parts come later.

### 5.2 Shape (D1154, D1155)

- **`loams-vault`** stays GT1's crate (§36 §19.13.3, GT1 Tasks 59–60): versions as create-only bucket objects, AES-256-GCM under fresh data keys wrapped by `KeyProvider`, and `SecretBackend` adapters for OpenBao/Vault KV v2, AWS, Azure and GCP. **It is built once,** by whichever of GT1 and IS1 reaches it first; the other reuses it (D1154).
- **`loams-secrets`** (new crate) is the service on top:
  - **Folders and secrets.** A secret lives at `<namespace>/<folder path>/<name>`, with versions and states from D840 (`draft`, `live`, `superseded`, `destroyed`). `env/` live sets (D841) become one kind of folder, so repository secrets and application secrets are the same objects.
  - **Engines.** An engine produces credentials: `kv` (stored values), `loams-postgres`, `loams-sql`, `postgres` and `mysql` (external, through an admin credential held as a secret), `random`, and `webhook` (D843's providers, reused). Later `aws`, `gcp`, `azure` (§5.4), `transit` and `pki` (§5.6).
  - **Leases.** Every dynamic credential has a lease record in TiKV (`loams_secrets` keyspace; redb in `loams dev`, as Jobs, D214): engine, target, principal, Place and run labels, issue time, TTL, max TTL, renewals, state. Revocation is a durable workflow (`loams-durable`), so a crash resumes it and an unreachable target is retried, never forgotten. `RevokeByPrincipal`, `RevokeByRun` and `RevokeByPrefix` exist from the start, because the Jailer's quarantine and offboarding need them.
  - **API:** `loams.secrets.v1` with `SecretService` (Put, Get, List, ListVersions, Destroy, Watch), `LeaseService` (Issue, Renew, Revoke, RevokeBy*, List) and `EngineService` (configure roles per engine). Values travel only in fields marked with a new field option `(loams.options.v1.secret) = true` (IS1 adds it beside `FacadeOptions`), which the SDKs' `Secret<T>` wraps and every log path redacts.
- **Where it runs:** the API in the `gateway` role of `loams`, revocation and rotation workflows in `worker`. It has no Raft and no unseal ceremony of its own: TiKV and the bucket are the storage, and the KMS behind `KeyProvider` is the root of trust (D1157).
- **Two secret paths, one rule** (D1156, resolves the D189/D840 overlap):
  - Loams-managed secrets live in `loams-vault`, whichever `SecretBackend` holds the bodies.
  - Applications read them through the SDK, `loams env run`, the Dapr `loams` component (§36 §19.13.5) or the Kubernetes path below.
  - Dapr's other secret-store components stay for tenant functions that read a store Loams does not manage. Loams' own services never read an external store through Dapr; they use `SecretBackend`.

### 5.3 Dynamic database credentials (IS1)

| Engine | How it issues | Revokes by | Built on |
|---|---|---|---|
| `loams-postgres` | `IssueConnectCredential` (D711): a `tok_<hash>` login role `VALID UNTIL` the lease end, granted the configured role, rendered into PgDog | `DROP ROLE` after terminating its sessions, and the `VALID UNTIL` as a backstop | `pg-control`, built (`crates/loams/src/api/connect_postgres.rs`) |
| `loams-sql` | A MySQL user with `PASSWORD EXPIRE INTERVAL` and the configured grants, through `loams.sqldb.v1` | `DROP USER` | §47's control plane. No `loams.sqldb.v1` proto or user-issuing RPC exists on `dev` at `034c8cb4` (checked 2026-10-19); IS1 Task 14 adds it |
| `postgres`, `mysql` (external) | SQL templates run with an admin credential stored as a secret, as OpenBao's database engine does | The revocation template | New; tested against stock Postgres 16–18 and MySQL 8.4 |

The lease is the record; D711's role naming and TTL limits (15 minutes default, 1 hour maximum for agents) carry over. The OpenTofu ephemeral resource (§4.2), `loams env run`, the Data API and agent sessions all issue through the same `LeaseService.Issue`.

### 5.4 Identity for machines (IS1; D1158)

- **Trust policies.** `IdentityService.CreateTrust` maps an external OIDC issuer (GitHub Actions, GitLab CI, Forgejo Actions, a Kubernetes service-account issuer, any issuer) plus required claims (`repository`, `ref`, `environment`, `sub` pattern) to a Loams service account and a maximum token lifetime. The token endpoint's RFC 8693 exchange (D449, D608) accepts the external token when a trust matches. This is how CI runs OpenTofu against Loams with no stored key.
- **The Loams workload issuer.** Loams signs short-lived JWTs for its own workloads (jobs, functions, sandboxes, agent runs) with the subject `spiffe://<trust-domain>/ns/<ns>/<kind>/<id>` and publishes its JWKS under `/.well-known/` (D602 keeps that path). AWS, GCP and Azure can federate to it, so a Loams job assumes a cloud role with no access key. Biscuit stays the in-runtime attenuation format (D188); the JWT is for parties outside Loams.
- **Cloud engines** (IS2): `aws` (STS `AssumeRoleWithWebIdentity` using the workload JWT), `gcp` (workload identity federation), `azure` (federated credentials). They issue nothing Loams has to store.

### 5.5 Delivery

- `loams env run` and the SDKs (IS1).
- The Dapr `loams` secret store (§36 §19.13.5), for functions (IS1).
- **Kubernetes:** first through the External Secrets Operator's generic webhook provider, which needs no Go code: Connect's unary JSON POST to `SecretService.Get` with a `jsonPath` ([ESO webhook](https://external-secrets.io/latest/provider/webhook/)). A native ESO provider and a Secrets Store CSI provider (a gRPC server on a unix socket, [CSI providers](https://secrets-store-csi-driver.sigs.k8s.io/providers)) are IS2.
- **Sandboxes and agents:** credentials are brokered, not copied. JL1c already routes model keys through the `llm` gateway (Q943); IS1 adds database credentials issued per run with the run's labels, so the Jailer can revoke them (§7.3).

### 5.6 Later engines (IS2)

- **A Vault/OpenBao client-compatible subset** (D1159): `sys/health`, `auth/jwt/login`, `auth/approle/login`, KV v2 reads and writes, `database/creds/<role>`, `sys/leases/renew` and `revoke`, implemented from the public API documentation and tested against the official clients (the MPL-2.0 Go `vault/api`, `hvac`, Spring Cloud Vault, Vault Agent, ESO's Vault provider, the Dapr `hashicorp.vault` component). It exists so those clients work unmodified; it is not the native API, and it ships only after counsel's review (Q974).
- **Transit:** encrypt, decrypt, sign, verify and rewrap with keys that never leave Loams, the keys themselves wrapped by `KeyProvider`. Also the backend for OpenTofu's state-encryption key provider through the compatible subset (OpenTofu's `openbao` key provider uses Transit).
- **PKI:** CP1 Task 18's cluster CA generalised into `loams-pki` (rcgen): an internal CA per deployment for mTLS between Loams services (the networking map's "short-lived internal mTLS certificates"), certificates for `loams-gateway`, SSH certificates for §7, and an ACME client (instant-acme) for public names. Not a general-purpose public CA.

### 5.7 Security bar

A secrets service is a high-value target, so IS1's secrets tasks carry extra gates (D1157):
- every value typed `Secret<T>`; a CI test greps logs, errors, events and metrics from the whole test suite for issued values;
- audit as CloudEvents on `_secrets` plus a hash-chained local log (D1104's pattern), with reads audited, not only writes;
- fuzzing of every parser that takes a token or a request before authorisation;
- an external security review before the secrets service leaves preview;
- documented failure modes: a lost KMS key loses the namespace's secrets (as §12 risk 30 says for data), and `loams dev`'s file key provider is for development only.

## 6. Consul → Loams Config and Catalog

### 6.1 What users need

- **Configuration that changes without a redeploy**, scoped by environment, with history, watched by running processes. On Kubernetes, ConfigMaps do part of this; on compose and BYOC VMs nothing does.
- **Finding services on BYOC VMs** without Kubernetes: a function or job runner on node A must reach a service on node B over the tailnet.
- **Not** a service mesh: the tailnet encrypts between hosts, the gateway terminates and routes, and mTLS between Loams services comes from `loams-pki` (§5.6).

### 6.2 Shape (D1160, D1161)

- **`loams.config.v1.ConfigService`:** typed entries (`string`, `int`, `bool`, `duration`, `json` with an optional JSON Schema) at `<namespace>/<environment>/<key>`, each write a revision with author and cause, CAS on the revision, `Watch` from a revision as a server stream, and `Render` to dotenv or JSON. Non-secret `# loams:plain` lines of an `env/` file (D839) promote into the same entries, so one environment has one live set of config and secret references, and `loams env run --watch` restarts the child on either.
- **`loams.config.v1.CatalogService`:** `Register` a service instance (name, address on the tailnet or cluster, port, tags, metadata) under a lease that the instance or its node agent renews; `Resolve` by name and tags; `Watch`. Health is the lease (a missed renewal removes the instance) plus an optional HTTP or TCP check run by the node agent. Loams' own consumers come first: `loams-gateway`'s `http` module reads upstreams from it, and runners register function and job endpoints in it.
- **Storage:** TiKV keyspace `loams_config` through `loams-kv` (embedded in `loams dev` and Desktop), watches on the store's change feed. No gossip, no Raft of its own, no agents on every host beyond the node agent that runners already have.
- **Not built:** a DNS server (Q977; MagicDNS, Docker DNS and CoreDNS already exist), a Consul-compatible HTTP API, sidecar proxies, intentions as a separate concept (OpenFGA and Places hold who may call whom), WAN federation.

## 7. Boundary → Loams Access

### 7.1 What users need

"Give Priya read access to the production database for two hours, with her manager's approval, and record it" — and the same for an agent. Boundary Community Edition is archived (§2.1), Teleport is AGPL with a commercial edition, and Loams already holds every piece: the identity, the tailnet route, the gateway in front of the database, short-lived roles, approvals on the phone, and Places for agents.

### 7.2 Shape (IS3; D1162)

- **`loams.access.v1.AccessService`:** `RequestAccess(target, role, duration, reason)` creates a durable approval promise (§15 §9.2) decided in the console, Studio or the phone app (§37's biometric-bound approvals). An approval writes an OpenFGA tuple with an expiry and returns a grant.
- **Brokered credentials per target kind:**
  - **Loams Postgres:** a lease from the `loams-postgres` engine (§5.3), connected through `loams-gateway`'s `pg` module.
  - **Loams SQL:** a lease from `loams-sql`, through `loams-sqlgate`.
  - **SSH to BYOC nodes:** an SSH user certificate from `loams-pki`, valid for the grant's duration, with principals from the grant. Hosts trust the Loams SSH CA. Tailscale SSH stays an option for tailnet-only operators.
  - **HTTP admin UIs** (Argo CD, Grafana): the gateway's `http` module with OIDC in front, once it exists.
- **Agents** request access with the same RPC; the Place's effective capability (D1087) bounds what can be granted, and the Jailer can revoke a grant as an actuator (§61 §4.4).
- **Audit:** `io.loams.dev.access.*.v1` events for request, decision, grant, connection and expiry. Postgres statement logging per grant and SSH session recording (asciicast) are later, opt-in.
- **Not built:** a worker network or client of our own (the tailnet does transport), RDP and Windows desktops, Kubernetes API proxying in IS3.

## 8. Policy

Loams adds **no policy language** (D1163).
- **Who may** (read a secret, write a state workspace, approve access) is OpenFGA, with new types `secret_folder`, `state_workspace`, `config_env`, `access_target` that follow the namespace and project hierarchy (D66, D67).
- **What an agent may do now** is the Jailer's PDP (§61): leases carry Place and run labels, the PDP is consulted on issue, and quarantine revokes by run.
- **What a plan may change** (IS3) is a set of typed PDP rules over the plan JSON: protected resource types that may not be destroyed, attributes that need an approval, maximum resource counts. Teams that already use OPA can run `conftest` (Apache-2.0) as a step in the plan job. Cedar and regorus are recorded alternatives (Q979).

## 9. Nomad, Packer, Waypoint and Vagrant

These four have the weakest case for new code. Each section says what Loams already has, the smallest useful addition, and the gate.

### 9.1 Nomad → Loams Run (IS4, gated; D1165)

- **Already there:** Jobs with leases and fencing (§26), Durable schedules and flows, the `Runner` trait with the node supervisor, process, Lambda and Knative runners (D375, D441), and `loams-sandbox` for isolation.
- **Missing:** placement of long-running services across a handful of VMs, restart on failure, rolling updates, and health. That is the gap Nomad fills for teams without Kubernetes.
- **Smallest useful addition:** a scheduler in the `worker` role that places `RunSpec`s (task groups of `SandboxSpec`s plus an OCI image or a binary) onto node agents by spread or bin-pack, holds each placement as a lease, and restarts on lease loss. OCI images are unpacked to a rootfs with `oci-client` (Apache-2.0) and run under the `linux` adapter; youki (Rust OCI runtime, Apache-2.0, [repo](https://github.com/youki-dev/youki)) is the recorded alternative.
- **Gate:** built only when at least three BYOC users run long-lived services on Loams VMs without Kubernetes (Q980). On Kubernetes, Loams uses Kubernetes.
- **Never:** a general cluster scheduler at Nomad's or Kubernetes' scale, CSI or CNI plugins, Windows drivers, multi-region federation.

### 9.2 Packer → `loams image` (IS4, gated; D1166)

- **Already there:** §15 §5.2's environment images (built in a builder sandbox, keyed by lockfiles and toolchain, published as nydus RAFS), Containerfiles built in CI, and stock cloud images.
- **Smallest useful addition:** convert an OCI image into the rootfs and kernel pair the `microvm` adapter boots, store it content-addressed in the bucket, and sign it (Sigstore, as releases are signed). BYOC nodes use stock distribution images plus cloud-init rendered by the OpenTofu provider; **no golden VM images**.
- **Never:** a multi-cloud machine-image builder or a Packer template language.

### 9.3 Waypoint → the deploy stage of the Software Factory (IS3; D1167)

- **Already there:** the factory loop's deploy stage behind an approval (SF4's when this was written; SF4 is retired and the stage is now [§72](72-software-factory-loop.md) stage 6, D1434), D842's promotion on merge into environment branches, the runners, and §45's versioned function deploys with rollback.
- **Smallest useful addition:** a deploy record (`loams.deploy.v1`: app, environment, build artifact digest, runner, config and secret revisions, state) and `loams deploy` / `loams rollback`, run as a durable workflow that builds, releases, deploys, health-checks and promotes. A deploy pins the config and secret revisions it used, so a rollback restores code and configuration together.
- **Never:** Waypoint's plugin system or a separate "platform" abstraction.

### 9.4 Vagrant → devcontainer workspaces (IS4; D1168)

- **Already there:** Studio's worktree per thread, microVM workspaces with snapshot and fork (D1006), `loams dev` stacks (D285).
- **Smallest useful addition:** read `.devcontainer/devcontainer.json` (image or Dockerfile, features, `postCreateCommand`, forwarded ports) when Studio or a cloud workspace creates an environment, so the same file works in VS Code, DevPod and Loams.
- **Never:** a Vagrantfile reader, VM provider plugins, or a box registry.

## 10. Architecture

### 10.1 One picture

```
 clients:  tofu (provider + http backend) │ loams CLI / SDKs │ console / Studio │ CI (OIDC) │ ESO / Dapr
                                   │
                ┌──────────────────┴──────────── loams (gateway role) ─────────────────────────┐
                │  token endpoint: OIDC exchange (D449) + trust policies (§5.4) → Loams token  │
                │  OpenFGA check ──── Jailer PDP for agent principals (§61)                    │
                │                                                                              │
                │  loams.infra.v1      loams.secrets.v1          loams.config.v1   (IS3)       │
                │  StateService        Secret/Lease/Engine/      Config/Catalog    access.v1   │
                │  + /tofu/v1 (http)   IdentityService                             deploy.v1   │
                └───────┬───────────────────┬───────────────────────┬──────────────────────────┘
                        │                   │                       │
        worker role: revocation, rotation, renewal and deploy workflows (loams-durable)
                        │                   │                       │
          ┌─────────────▼───────────────────▼───────────────────────▼─────────────┐
          │ TiKV (loams-kv; redb in loams dev): locks, workspace records, leases,  │
          │ config entries and revisions, catalog instances, trust policies        │
          └────────────────────────────────────────────────────────────────────────┘
          ┌────────────────────────────────────────────────────────────────────────┐
          │ bucket: state versions, secret versions, audit segments, images        │
          │ — each body under a data key wrapped by the namespace KeyProvider (D96)│
          └────────────────────────────────────────────────────────────────────────┘
          engines reach: pg-control (D711) · loams.sqldb · external DBs · STS/WIF · loams-pki
```

### 10.2 Shared pieces (D1155)

| Concern | Mechanism | Owner |
|---|---|---|
| People | Authentik or any OIDC issuer, exchanged for Loams tokens (D447–D449); tailnet identity on self-hosted stacks | §38, §43 |
| Machines | API keys (D65), trust policies for external OIDC (§5.4), the Loams workload issuer, Biscuit inside the runtime (D188) | This document, §24 |
| Agents | Agent principals with ≤ 1 h tokens (§19 §5), Places (D1087), the Jailer PDP | §19, §60, §61 |
| Authorisation | OpenFGA through the `Authorizer` trait (D66), new object types per §8 | §18 |
| Hot state | `loams-kv` over TiKV, embedded store in `loams dev`; MetaStore leases with fencing | §18, §26 |
| Bodies | Create-only objects in the bucket, envelope-encrypted under `KeyProvider` | §36, D96 |
| Workflows | `loams-durable` (Resonate) for revocation, rotation, renewal, plan runs, deploys | §21 |
| Events and audit | CloudEvents `io.loams.dev.{infra,secrets,config,access,deploy}.*.v1` on namespace streams; hash-chained local audit | D364, D1104 |
| API | Connect services `loams.<service>.v1` (D600); compatibility surfaces only for unmodified third-party clients (D603) | §44 |
| CLI and console | `loams secrets`, `loams config`, `loams state`, `loams access`; console pages per service | §30, §59 |

The CP1 discrepancies the research found (two `Authorizer` traits, two principal vocabularies) are not resolved here; IS1 Task 0 uses §18's trait and §19's principal kinds and records the choice.

## 11. Licence notes (D1169)

- **No BSL code, ever.** No file, snippet, test fixture or translated function from any HashiCorp repository released after 2023-08-10, in any language. Contributors who have worked on HashiCorp's BSL source write specs and tests for these components, not their implementation (the same clean-room rule as Grafeo's and §36's ideas-only rules).
- **MPL-2.0 (OpenTofu, OpenBao, Vault ≤ 1.14, the HashiCorp API and SDK libraries):**
  - interop is fine: Loams speaks their protocols and runs their unmodified binaries in tests;
  - a protocol file needed verbatim (`tfplugin6.proto`) is vendored unmodified, keeps its MPL-2.0 header, sits in its own directory with a NOTICE entry, and is listed in `deny.toml`'s accepted licences (MPL-2.0 is already there);
  - **no porting.** A Rust translation of OpenBao's Go code would be a derivative of MPL files; Loams implements from documentation and observed behaviour instead.
- **The Vault HTTP API.** No primary source settles whether reimplementing it raises issues beyond copyright of the code; OpenBao does it under MPL, and the HashiCorp client libraries remain MPL-2.0 ([licenses.dev](https://licenses.dev/go/github.com%2Fhashicorp%2Fvault%2Fapi/v1.9.0)). Counsel reviews the compatible subset before it ships (Q974).
- **Trademarks.** Product names are Loams' own ("Loams Secrets", never "Loams Vault" in user-facing text; the crate name `loams-vault` stays internal). "Terraform", "Vault", "Consul" and "Nomad" appear only nominatively ("works with Vault clients"). The provider repository name `terraform-provider-loams` is the registry's required convention (verify with counsel, Q974).
- **AGPL** (Teleport, vaultwarden) and **GPL** libraries are never linked (D1023). eBPF stays under D1102.
- **Everything Loams writes here is Apache-2.0** (D1119).

## 12. Minimum viable slice and phases

### 12.1 Why this slice

| Slice | User value | New surface | Reuse |
|---|---|---|---|
| OpenTofu provider + state backend | Every team with IaC can adopt Loams without click-ops; secrets stay out of state | Medium (tfplugin6 server, generated resources) | High (protos, bucket, leases) |
| Secrets with dynamic Loams Postgres/SQL credentials and CI OIDC trust | No long-lived DB passwords or CI keys; agents get revocable credentials | Medium (leases, engines, trust) | Very high (`loams-vault`, D711, D843, Durable) |
| Config and catalog on TiKV | Config without redeploys on compose and BYOC; the gateway and runners get a registry | Small | High (`loams-kv`, leases, env live sets) |

Everything else either depends on these (access needs leases; plan runs need the provider and trust; deploys need config and secrets revisions) or serves fewer users.

### 12.2 Phases and effort (estimate, engineer-weeks)

| Phase | Content | Effort | Gate to start |
|---|---|---|---|
| **IS1a** | `loams-tfplugin` spike and server; provider with the 13 resources, ephemeral and write-only support; state backend and `StateService`; acceptance tests with `tofu`; registry listing | 10–12 | v0.1.0 shipped |
| **IS1b** | `loams-vault` core (if GT1 has not built it); `loams.secrets.v1`; leases and revocation workflows; `loams-postgres`, `loams-sql`, external `postgres`/`mysql` engines; trust policies and the workload issuer; CLI, SDK modules, Dapr and ESO-webhook docs; audit; security tests | 14–18 | IS1a Task 0 (shared rulings) |
| **IS1c** | `loams.config.v1` config and catalog; env live-set integration; gateway upstreams from the catalog | 4–6 | — |
| **IS1 total** | | **28–36** (two engineers, about four months) | |
| **IS2** | Vault/OpenBao client subset (after counsel); transit; `loams-pki` (CA generalised from CP1, ACME); cloud engines through federation; native ESO and CSI providers; external security review | 16–20 | IS1b in use by at least one deployment |
| **IS3** | Loams Access (JIT grants, SSH CA, approvals); plan runs; deploy records and `loams deploy` | 18–22 | IS2's PKI for SSH; SF4 shipped for deploys |
| **IS4** | Loams Run scheduler; `loams image`; devcontainer workspaces | 22–34 | Each item behind its own demand gate (Q980–Q982) |

These numbers assume the reuse in §1.3 holds; if GT1's vault, D711 or the sandbox slip, IS1b and IS3 grow by their size.

## 13. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | **Scope.** "All of HashiCorp" absorbs the team for a year and the engine's own roadmap stalls | High | High | IS1 only, then demand gates; §14's list is binding; every phase has its own owner sign-off |
| R2 | **A secrets service with a vulnerability** | Medium | Very high | §5.7's gates; external review before leaving preview; minimal engines in IS1; Ostrium's security services cover incident response |
| R3 | **Lost or revoked KMS key** loses secrets and state | Low | High | Documented as data loss (as §12 risk 30); key-health checks in the CLI and console; a recovery export for the file provider |
| R4 | **Rust tfplugin6 server misses protocol details** (AutoMTLS, deferred actions, ephemeral resources) | Medium | Medium | A one-week spike with a pass/fail gate; Go fallback (Q970); acceptance against two OpenTofu versions |
| R5 | **OpenTofu and Terraform protocols diverge** | Low | Medium | Test OpenTofu only; Terraform works as long as the protocol is shared, stated as best effort |
| R6 | **Users prefer OpenBao** | Medium | Low | That is fine: OpenBao is a `SecretBackend` and an engine target; Loams Secrets' value is dynamic Loams credentials and agent revocation, which work with either |
| R7 | **The Vault-compatible subset invites a long tail** of engines and endpoints | High | Medium | A fixed, versioned list (like `chsurface-1`); anything else answers a typed `unsupported` |
| R8 | **Licence contamination** from reading BSL code | Low | High | §11's clean-room rule; CodeRabbit and a reviewer check provenance on these crates |

## 14. What not to build

Said plainly, so later plans do not drift:
- **No new IaC language, no HCL engine, no fork of OpenTofu or Terraform, no CDK.**
- **No Terraform CLI** in Loams' CI, docs or images; OpenTofu only (the CKE profile switches from `terraform` to `tofu`; D1150).
- **No cluster scheduler at Kubernetes scale.** On Kubernetes, use Kubernetes.
- **No service mesh, no sidecars, no Consul API, no DNS server.**
- **No Shamir unseal ceremony and no own Raft** in the secrets service; the KMS and TiKV are the roots.
- **No general public CA,** no HSM product, no PKCS#11 in IS1–IS2.
- **No golden VM image pipeline,** no Packer templates, no Vagrantfile, no box registry.
- **No Boundary-style worker network,** RDP or Windows access.
- **No proprietary or new policy language** (no Sentinel equivalent).
- **No Vault namespaces, replication or performance standbys**; Loams namespaces and TiKV already partition and replicate.
- **No closed "HCP" edition.** Hosted operation is a service on the same open code.
- **No copying of HashiCorp code,** BSL or otherwise (§11).

## 15. Amendments to earlier decisions

| Decision | Change |
|---|---|
| D840 (`loams-vault`) | Extended: the vault serves all Loams secrets, not only `env/`; built once by GT1 or IS1 (D1154) |
| D711 (credential exchange) | Wrapped: `IssueConnectCredential` becomes the `loams-postgres` engine behind `LeaseService.Issue`; the RPC stays as is |
| D189 (Dapr secrets) | Narrowed: Loams' own services read external stores through `SecretBackend`, not Dapr (D1156) |
| D843 (rotation providers) | Reused as engines (`random`, `loams-postgres`, `loams-sql`, `webhook`) |
| D453, §25 §8, MT3 | The CKE profile uses `tofu` instead of `terraform` (D1150) |
| D608 (SDK auth) | `WorkloadIdentity` gains the Loams workload issuer and trust policies (D1158) |
| D1022 (gateway modules) | The `http` module reads upstreams from the catalog (D1161) |
| D1096, D1103 (Jailer actuators) | `RevokeByRun` is the "revoke token" actuator for database and cloud credentials |

## 16. Decisions (D1150–D1171, approved by the owner on 2026-10-19)

| ID | Decision |
|---|---|
| D1150 | **OpenTofu is the IaC engine Loams supports, documents and tests.** No Terraform CLI in Loams' CI, docs or images; the CKE profile moves to `tofu`. The provider works with Terraform on a best-effort basis |
| D1151 | **The OpenTofu provider `ostrium-labs/loams`** is written in Rust on a new crate `loams-tfplugin` (protocol v6, vendored MPL `tfplugin6.proto`), published from a mirror repository `ostrium-labs/terraform-provider-loams` written by a release workflow |
| D1152 | **Provider schemas are generated from proto annotations** (`(loams.options.v1.resource)`, secret field markers) by `loams-facade-gen`; secrets use write-only attributes and ephemeral resources only, and a test proves no issued value reaches state or plan files |
| D1153 | **The Loams state backend** is OpenTofu's `http` protocol at `/tofu/v1/state/<org>/<ns>/<workspace>` (a D603 compatibility surface), with versions as KMS-wrapped create-only objects, locks as fenced MetaStore leases, OpenFGA per workspace, and `loams.infra.v1.StateService` for history, diff, rollback, lock break and outputs. The `s3` and `pg` backends are documented as zero-code alternatives |
| D1154 | **`loams-vault` is built once,** by whichever of GT1 (Tasks 59–60) and IS1 reaches it first; the other reuses it |
| D1155 | **Loams Secrets** is `loams-secrets` over `loams-vault`: folders and versioned secrets (env live sets included), engines, TiKV leases with durable revocation, and `loams.secrets.v1` (`SecretService`, `LeaseService`, `EngineService`, `IdentityService`). Shared identity, OpenFGA, `loams-kv` storage, bucket bodies and CloudEvents audit, as §10.2 |
| D1156 | **One rule for secret paths:** Loams-managed secrets live in `loams-vault`; Loams services read external stores through `SecretBackend`; Dapr's other components serve tenant functions only (narrows D189) |
| D1157 | **No unseal ceremony and no own Raft:** the KMS behind `KeyProvider` is the root of trust; §5.7's gates (redaction tests, read audit, fuzzing, external review before leaving preview) apply |
| D1158 | **Machine identity:** trust policies map external OIDC issuers and claims to Loams service accounts through the RFC 8693 exchange; Loams signs SPIFFE-shaped workload JWTs with a published JWKS so clouds can federate to it |
| D1159 | **A Vault/OpenBao client-compatible subset** (a fixed, versioned list of endpoints) in IS2, implemented from public documentation, behind counsel's review (Q974) |
| D1160 | **Loams Config:** `loams.config.v1.ConfigService` with typed, environment-scoped entries, revisions, CAS, `Watch` and `Render`; `# loams:plain` env lines promote into it |
| D1161 | **Loams Catalog:** `CatalogService` with lease-based registration and health, `Resolve` and `Watch` on TiKV; first consumers are `loams-gateway`'s `http` module and the runners. No DNS, mesh or Consul API |
| D1162 | **Loams Access** (IS3): `loams.access.v1` just-in-time grants with durable approvals, OpenFGA tuples with expiry, brokered credentials per target (Postgres and SQL leases, SSH certificates from `loams-pki`), Places bounding agent requests, and access events |
| D1163 | **No new policy language:** OpenFGA for who-may, the Jailer PDP for agents, typed PDP rules for plans; `conftest` as an optional step |
| D1164 | **Plan runs** (IS3): `tofu plan` and `apply` as sandboxed jobs on Loams Git events, credentials from ephemeral resources and trust policies only, apply after a person's approval and the plan rules |
| D1165 | **Loams Run** (IS4) is a small placement scheduler over the `Runner` trait and `loams-sandbox`, built only after its demand gate (Q980); never a Kubernetes-scale scheduler |
| D1166 | **`loams image`** (IS4) converts OCI images into signed rootfs and microVM images in the bucket; no golden VM images, no template language |
| D1167 | **Deploys** (IS3) are the Software Factory's deploy stage ([§72](72-software-factory-loop.md) stage 6 since SF4's retirement, D1420): `loams.deploy.v1` records pinning artifact, config and secret revisions; `loams deploy` and `loams rollback` as durable workflows |
| D1168 | **Dev environments** (IS4): Studio and cloud workspaces read `devcontainer.json`; no Vagrantfile or box registry |
| D1169 | **Licence rules** of §11: no BSL code; MPL only as unmodified vendored protocol files and interop; no ports; clean-room for contributors exposed to BSL source; product names avoid HashiCorp marks |
| D1170 | **Hosted operation is a service, not an edition:** Ostrium may operate state, secrets and config for customers under SLAs, on the same Apache-2.0 code (D1119) |
| D1171 | **Plan IS1** (stub) covers IS1a–IS1c; IS2–IS4 get their own plans when their gates are met |

## 17. Open questions (Q970–Q983)

| ID | Question | Default (all answered by the owner on 2026-10-19: the default) |
|---|---|---|
| Q970 | Provider in Rust (`loams-tfplugin`) or Go (`terraform-plugin-framework`)? | Rust, with a one-week spike gate; Go if the spike fails |
| Q971 | Default state encryption: server-side (console can show diffs and outputs) or OpenTofu client-side? | Server-side by default; client-side documented and supported |
| Q972 | Does IS1's provider cover Live, House, Graph, jobs and functions too? | No; IS1 covers the 13 resources in §4.2, the rest follows the generated schema as their protos gain annotations |
| Q973 | Leases for external `postgres`/`mysql` in IS1, or Loams engines only? | Include them; teams with RDS are a large share of the value |
| Q974 | Counsel review: the Vault-compatible subset, nominative use of marks, the `terraform-provider-` repository name | Before IS2's subset ships and before the registry listing (owner action) |
| Q975 | Is the workload issuer's trust domain per deployment or per org? | Per deployment (`spiffe://<deployment-id>.loams`), with the org in the path |
| Q976 | Read audit for every secret read, or sampled? | Every read; rate-limited events with counts for hot paths |
| Q977 | A DNS interface for the catalog? | No; revisit if BYOC users without MagicDNS ask |
| Q978 | Who holds the provider's GPG signing key? | The release environment's reviewers, like the Linux packaging GPG key |
| Q979 | Adopt Cedar or regorus for plan rules? | Neither now; typed PDP rules plus optional `conftest` |
| Q980 | The demand gate for Loams Run | At least three BYOC deployments running long-lived services on VMs without Kubernetes |
| Q981 | The demand gate for `loams image` | The `microvm` adapter shipped (SB1f) and workspaces needing custom images |
| Q982 | The demand gate for devcontainer workspaces | Cloud workspaces (§15) shipped; Studio local first if cheap |
| Q983 | Should the OpenTofu provider and state backend ship before v0.1.0? | No; after v0.1.0, as the first IS1a deliverable |

## 18. Sources

- HashiCorp: [BSL announcement](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license); [licence FAQ](https://www.hashicorp.com/en/license-faq); [Vault LICENSE](https://raw.githubusercontent.com/hashicorp/vault/main/LICENSE); [Nomad LICENSE](https://raw.githubusercontent.com/hashicorp/nomad/main/LICENSE); [Boundary repository](https://github.com/hashicorp/boundary) (archived 2026-10); [Waypoint repository](https://github.com/hashicorp/waypoint); [CDKTF repository](https://github.com/hashicorp/terraform-cdk); [HCP Vault Secrets EOL](https://support.hashicorp.com/hc/en-us/articles/41802449287955-HCP-Vault-Secrets-End-Of-Life); [HCP Consul Dedicated and Terraform free plan EOL](https://support.hashicorp.com/hc/en-us/articles/47520801288083-End-of-Life-Notice-for-HCP-Terraform-Legacy-Free-Plan-HCP-Consul-Dedicated); [HCP Vagrant deprecation (blog)](https://hashicorp.com/blog/hcp-vagrant-deprecation-important-dates-and-migration-guidance) and [(docs)](https://developer.hashicorp.com/hcp/docs/vagrant/hcp-vagrant-eol); [plugin protocol](https://developer.hashicorp.com/terraform/plugin/terraform-plugin-protocol).
- IBM: [acquisition completed](https://www.consultingmag.com/2025/03/04/ibm-completes-acquisition-of-hashicorp); [Vault 2.0 (InfoQ)](https://infoq.com/news/2026/04/vault-2-0-ibm-identity/).
- OpenTofu: [CNCF project page](https://www.cncf.io/projects/opentofu/); [repository](https://github.com/opentofu/opentofu); [registry](https://github.com/opentofu/registry); [1.11 release](https://opentofu.org/blog/opentofu-1-11-0); [ephemerality](https://opentofu.org/docs/language/ephemerality/); [state encryption](https://opentofu.org/docs/language/state/encryption/); backends [http](https://opentofu.org/docs/language/settings/backends/http/), [s3](https://opentofu.org/docs/language/settings/backends/s3/), [pg](https://opentofu.org/docs/language/settings/backends/pg/).
- OpenBao: [repository](https://github.com/openbao/openbao); [what is OpenBao](https://openbao.org/docs/what-is-openbao/).
- Kubernetes plug-in points: [External Secrets Operator](https://github.com/external-secrets/external-secrets), [ESO webhook provider](https://external-secrets.io/latest/provider/webhook/); [Secrets Store CSI providers](https://secrets-store-csi-driver.sigs.k8s.io/providers).
- Alternatives: [Teleport AGPL](https://goteleport.com/blog/teleport-oss-switches-to-agpl-v3/), [Teleport v16 licence](https://github.com/gravitational/teleport/discussions/39158); [Pomerium](https://github.com/pomerium/pomerium); [OpenZiti](https://github.com/openziti/ziti); [DevPod](https://github.com/loft-sh/devpod); [devcontainer spec](https://github.com/devcontainers/spec); [linkerd releases](https://linkerd.io/releases/).
- Rust: [Firecracker](https://github.com/firecracker-microvm/firecracker); [rustls](https://github.com/rustls/rustls); [rcgen](https://github.com/rustls/rcgen); [instant-acme](https://github.com/djc/instant-acme); [aws-lc-rs](https://github.com/aws/aws-lc-rs); [linkerd2-proxy](https://github.com/linkerd/linkerd2-proxy); [Kanidm](https://github.com/kanidm/kanidm); [Rauthy](https://github.com/sebadob/rauthy); [RustyVault](https://github.com/Tongsuo-Project/RustyVault); [rust-spiffe](https://github.com/maxlambrecht/rust-spiffe); [Cedar](https://github.com/cedar-policy/cedar); [regorus](https://github.com/microsoft/regorus); [tf-provider](https://github.com/aneoconsulting/tf-provider); [terustform](https://git.noratrieb.dev/Noratrieb/terustform); [Krustlet (unmaintained)](https://github.com/krustlet/krustlet).
- Licence of the Vault client library: [licenses.dev, `vault/api` v1.9.0](https://licenses.dev/go/github.com%2Fhashicorp%2Fvault%2Fapi/v1.9.0).
- Loams: the designs and plans linked at the top; `crates/loams/src/api/connect_postgres.rs` (`IssueConnectCredential`); `crates/loams-pg-control/src/secrets.rs` (`SecretStore`); `deny.toml` (MPL-2.0 accepted).
