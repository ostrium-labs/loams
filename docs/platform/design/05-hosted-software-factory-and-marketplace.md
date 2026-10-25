# 05 — The Hosted Loams Software Factory and the Loams Cloud Marketplace (Listing #1)

> **Superseded 2026-10-21 ([D1446](../../design/13-decision-log.md)).** The marketplace is open source and is designed in [§73](../../design/73-agent-marketplace.md): a signed static index of agents (any framework, Mastra first), harnesses and MCP servers, installed through `loams-agentd`, free, with no plans, Stripe subscriptions or install sagas. The hosted factory as listing #1 is retired with SF2–SF5 (D1420); the factory loop is [§72](../../design/72-software-factory-loop.md). PD56's licence display and PD58's abuse controls survive in §73 §7.2 and §10. Kept for history.

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Status: **Proposed** · 2026-10-02. Proprietary. Source: the owner's direction of 2026-10-02, "make it available as first in our Cloud marketplace page", on top of the open design `operon §39` (Loams Software Factory and Loams Bot, D460–D479, in dina-kar/operon PR #192). This document designs only what is **commercial** under operon D220: the hosted multi-tenant factory, managed agents and model costs, the marketplace (listings, install flow, billing hook) and the first listing. The single-organisation factory, the A2A agents, the plugins and the chat are open and are **not** redesigned here; this platform installs and operates them unmodified.

Names: the owner ruled on 2026-10-02 that the products are **Loams Bot** and **Loams Software Factory** (this document and the listing use them). The desktop app the listing refers to is the native Loams Desktop, a fork of zeron (operon §37 as amended for a native desktop, D440-series pending), not a Tauri app; nothing here depends on how the desktop is built.

Numbering: decisions **PD47–PD60**, questions **Q-SF-1–Q-SF-9** (Q-SF-2 withdrawn). Bare D-numbers and "operon §NN" point into operon's `docs/design`. Markers as in operon: **(verify)** means not checked against a primary source; **(estimate)** means computed, not measured. All prices are **placeholders** (`$XX`) until the owner sets them.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| PD47 | **The marketplace is a catalogue of installable packages**, each a **listing** with plans, versions, an entitlement model and an install saga. It lives in the control plane (this repository); the public pages live in `loam-cloud`. **Beta: first-party listings only**; third-party publishers are a later decision (Q-SF-5) | Proposed |
| PD48 | **Listing #1 is Loams Software Factory.** Its package is operon's `deploy/factory/` chart and catalog patch (operon D-SF-20), installed unmodified into the tenant; the platform adds only an overlay of values (domain, model route, budgets, quotas) | Proposed |
| PD49 | **Install flow** (§4): sign in (Zitadel, operon §76) → choose organisation and plan → consent screen → Stripe subscription (14-day trial, card required, Q-SF-1) → entitlement → **install saga** on operon §21 → health check → onboarding checklist. Uninstall is a saga with a 30-day grace and an export | Proposed |
| PD50 | **Hosted tenancy for the factory is one stack per tenant** (§3): a Kubernetes namespace `tenant-<org>-factory` on the k3s cluster (PD29, PD31) running the open chart (Zulip, Plane CE, Forgejo, the factory service, the five agents, Loams Bot's host), because Zulip, Plane and Forgejo are single-tenant apps and AGPL and GPL forbid us nothing but patching. A shared CloudNativePG cluster holds one database and role per app per tenant; one RustFS bucket per tenant (R2 on the Cloudflare path, operon D381). No shared app database rows across tenants | Proposed |
| PD51 | **Observability apps are per tenant where light, shared where heavy** (§3.3): GlitchTip and OpenObserve (open edition, which has no RBAC and so cannot be shared safely) run **per tenant**; **Langfuse 4** (ClickHouse, Redis, Postgres, S3) runs **once per region** with one Langfuse organisation per tenant and Zitadel OIDC, and OpenPanel (needs ClickHouse) is an **add-on** (Q-SF-4). Content capture follows the tenant's policy (operon Q476) | Proposed; OpenObserve replaced by SigNoz on Loams House (D1666, owner, 2026-10-22) |
| PD52 | **Managed agents and model costs are the product.** The platform runs each tenant's five agents and Loams Bot's harness host (Knative, scale-to-zero between runs) and routes model calls through the AI gateway. Plans include a **token allowance**; overage is metered (§4.4); **bring your own model key** is an option on every plan and waives the model charge | Proposed |
| PD53 | **Billing hook**: Stripe Billing (PD38). A listing plan is a Stripe Price (flat, monthly or annual); usage items are Stripe meter events fed from `loam-usage` (doc 01), which consumes operon's business CloudEvents (`io.loams.dev.factory.run.completed.v1` with counts) and the AI gateway's token reports; **operon contains no metering** (D403, D444) | Proposed |
| PD54 | **Entitlements set limits; the engine enforces them.** The platform writes the plan's quotas into the tenant's factory policy defaults (runs per day, concurrent runs, open PRs, budgets, projects, seats) through operon's admin API; the open guard (operon SF4) enforces. A downgrade lowers the defaults, never deletes data | Proposed |
| PD55 | **The kill switch, audit and approvals are the open ones** (operon D-SF-9, D-SF-14, D-SF-15). The hosted offering adds: a **platform kill** (suspend a tenant's factory for abuse or non-payment, PD58), long-retention audit and a hosted audit UI (D220), and per-tenant **spend alarms** | Proposed |
| PD56 | **Licences are shown at install and in the listing**: the apps run unmodified under Apache-2.0, AGPL-3.0, GPL-3.0-or-later, MIT; the listing links each app's source and licence (operon §4). **Counsel clears the hosted offering before the beta** (§8 owner action, Q-SF-3) | Proposed |
| PD57 | **BYOC factory**: the same package installs into a customer's Kubernetes through the BYOC management agent (operon §18 §8, PD36); the listing's "Self-managed" plan is the open chart with support, not a different product | Proposed |
| PD58 | **Abuse and trust**: an automated agent that writes code and opens PRs is abusable (spam PRs, credential stuffing through app APIs, crypto-mining CI). Controls: Forgejo Actions runner disabled until the tenant passes verification (payment method, email domain, a cap of CI minutes), egress allowlists for the coding sandbox, per-tenant outbound rate limits, and the platform kill | Proposed |
| PD59 | **Screenshots and copy are produced from a seeded demo tenant** (§6.4), not mocked: the same seed is the product's demo mode (operon Q435) and the marketplace's "see it run" video | Proposed |
| PD60 | **The marketing page is built by `loam-cloud` from §6** (routes, copy, features, the pricing slot, install steps, screenshot shot list, events). This document is its source of truth until the page exists | Proposed |

## 2. Goals and non-goals

### 2.1 Goals

1. A paying customer installs Loams Software Factory in minutes, without running Plane, Zulip, Forgejo or any agent.
2. The same package is what a self-hoster installs (D220): the hosted product is operation, not withheld code.
3. A listing is data: adding a second listing needs no new platform code beyond its package.
4. Billing is by plan plus usage, with the model cost visible and capped.
5. A tenant can leave with its data.

### 2.2 Non-goals

- **No marketplace for third parties in the beta** (Q-SF-5): no publisher payouts, no review pipeline, no sandboxing of foreign packages yet.
- **No selling inside the mobile apps.** The apps are clients; purchases happen on the web (store rules, §8).
- **No fork of any app, no paid-edition features** (operon D-SF-17). If Plane's paid edition is wanted, that is a commercial agreement with Plane, not a code change.
- **No new factory logic here.** The loop, agents and chat are operon's.

## 3. The hosted factory

### 3.1 Shape

```
 customer browser / desktop / phone
        │  Cloudflare edge (Workers front, PD32)
        ▼
  api.loams.dev · console.loams.dev · <tenant>.factory.loams.dev (apps, per tenant, behind the edge of operon §39 §3.3)
        │  Tunnel / Workers VPC binding
        ▼
 ┌─────────────── k3s (PD29, PD31) ───────────────────────────────────────────────────────────────┐
 │ control plane: marketplace service, install sagas (operon §21), entitlements, loam-usage        │
 │ per tenant namespace  tenant-<org>-factory:                                                     │
 │   Zulip · Plane CE · Forgejo (+ Runner, gated, PD58) · GlitchTip · SigNoz                       │
 │   factory service · 5 agents (Knative, scale-to-zero) · Loams Bot host (Knative)                 │
 │ per region: shared CNPG (db+role per app per tenant) · Langfuse 4 (org per tenant) ·            │
 │             AI gateway (aisix) → model providers (DeepSeek default) · OTel collector            │
 │ identity: Zitadel per cluster, one org per tenant, its apps (operon §76)                        │
 └─────────────────────────────────────────────────────────────────────────────────────────────────┘
        │ S3                         │ metrics, usage events (no content)
        ▼                            ▼
   R2 (PD30) bucket per tenant     loam-meter, loam-usage → Stripe meter events (PD53)
```

## 3.5 Birthday Week 2026 betas in the hosted factory (added 2026-10-02)

Details, sources and status in doc 03 §12a and doc 04 §2a; open adapters in operon §42. All three are **beta**, so each has the non-Cloudflare exit named.

- **Coding sandboxes pause and resume (PD74).** `forgejo.propose_patch` (operon §39 SF4 step 4) runs a coding agent in a per-run workspace. On the Cloudflare target the workspace is a Container whose filesystem is saved with `snapshotContainer` between turns and restored with `start({ containerSnapshot })` (beta); after the snapshot the pause flow **stops the container** (destroys the instance, or sets a short sleep timeout), because compute billing stops only once the instance has slept; snapshot storage is billed separately and its price is unconfirmed (doc 03 Q-CF-10). Exit: a branch plus a volume snapshot on k3s.
- **Generated apps (PD76).** When the factory produces an app for a tenant to try, it runs as a Dynamic Worker with its own database from a Durable Object Facet (both open beta), inside a supervisor Durable Object that meters and logs before forwarding. Exit: workerd under gVisor on k3s.
- **Hosted repositories (PD78).** The tenant's Forgejo stays the repository of record. Artifacts holds per-run workspace repositories and mirrors for agents that need a Cloudflare-native Git remote, within its 1 GB and 32 MB limits.
- **Agents that pay (PD80).** A marketplace listing's *commercial API* calls can be charged to an agent through the Monetization Gateway (doc 07 §5). The gateway is closed beta, so the Stripe path of §4.4 remains the only billing path until access is granted.

### 3.2 Resource and cost estimate (estimate; SF1 and SF5 exit reports replace it)

| Component | Idle RAM per tenant | Notes |
|---|---|---|
| Zulip (web, workers, memcached, rabbitmq, redis) | about 2 GB | Not scale-to-zero friendly: it holds queues and sessions |
| Plane CE (web, space, api, worker, beat, live, redis, mq) | about 2–3 GB | Same |
| Forgejo (+ Runner when enabled) | 0.3–0.5 GB | |
| GlitchTip (web, worker) | 0.3–0.5 GB | |
| SigNoz (the MIT build and its OTel collector; telemetry stored in Loams House; preview) | About 85 MiB idle (SigNoz 38 MiB, collector 45 MiB; measured 2026-10-22, §39 §9.3), plus its data's share of the tenant's House | |
| Factory service, agents, Loams Bot host | 0.5 GB idle | Knative scale-to-zero between runs |
| **Total** | **about 6–8 GB** | A dedicated stack is the main fixed cost per tenant |

So a dedicated stack costs real money before the first run. Consequences: no free tier for the hosted factory (a **trial** only, PD49); **hibernation** (snapshot the databases to R2 and scale the namespace to zero after 14 days without a login, restore on demand in under 10 minutes) is a cost control, not a promise (Q-SF-7); plan limits must account for the fixed floor (§6.3).

### 3.3 Where shared and per-tenant meets

| Component | Hosted model | Why |
|---|---|---|
| Zulip, Plane, Forgejo, GlitchTip, SigNoz | Per tenant | Single-tenant apps or no SSO (SigNoz's MIT build) |
| Postgres | Shared CloudNativePG cluster, one database and role per app per tenant, connection limits per role | Cost; row-level mixing is never used |
| Langfuse 4 | Shared per region, one **Langfuse organisation per tenant**, SSO through Zitadel (`AUTH_CUSTOM_*`), `ee/` never enabled | ClickHouse, Redis and S3 are heavy; Langfuse organisations isolate projects and keys. A cross-tenant leak test is a launch gate (§8 risk 2) |
| OpenPanel | Add-on, per tenant (Q-SF-4) | Needs ClickHouse; AGPL |
| AI gateway | Shared, per-principal budgets and keys | Operon's gateway is already multi-principal |
| Zitadel | One per cluster, one org and application set per tenant; teams are Loams' own | operon §76 (D1600, D1605) |

### 3.4 Operating it

Provisioning, upgrades and rollbacks are the install saga's steps replayed (PD49); app upgrades follow operon's digest pins and the edge assertions in CI (operon §39 §15 risk 2), rolled to tenants in waves with the platform's fleet automation (D220). Backups: CNPG PITR to R2; Forgejo repositories on the tenant bucket; the factory's run records on Live (TiKV) with the platform's backups. Support tooling reads the run record and the audit log; it cannot read model content.

## 4. The marketplace

### 4.1 Concepts

| Concept | Definition |
|---|---|
| **Listing** | A product in the catalogue: `slug`, name, tagline, description, category, publisher, licence list, screenshots, links, status (`draft`, `beta`, `ga`, `retired`) |
| **Plan** | A priced way to buy a listing: flat price, included quantities, usage items, limits, trial length, `requires` (for example cloud region) |
| **Version** | An immutable package: an OCI reference to a Helm chart by digest, the operon release it targets, a manifest, an install schema (JSON Schema), and release notes. Installs track a channel (`stable`, `beta`) |
| **Entitlement** | The right of an organisation to a listing under a plan: status (`trialing`, `active`, `past_due`, `canceled`), limits, the Stripe subscription id |
| **Install** | An instance of a version in a tenant: a Helm release in the tenant namespace, its saga run, its health, its values overlay |
| **Install saga** | A durable workflow (operon §21 §6.3) with compensation |

The **manifest** (`marketplace.yaml` in the package, shipped by operon's `deploy/factory/`; operon D-SF-20 reserves it): `id`, `name`, `version`, `requires` (operon release range, apps), `permissions` (the principals and scopes the install creates, the apps provisioned, egress), `licences` (each app, licence, source URL), `values_schema`, `healthchecks`, `uninstall` (data export steps), and `telemetry` (the business events it emits). The platform renders the consent screen from it, so what the user agrees to is what the package declares.

### 4.2 The control-plane service

`loam.marketplace.v1` (Connect, in this repository): `ListListings`, `GetListing`, `ListPlans`, `StartCheckout`, `GetEntitlement`, `Install`, `GetInstall`, `ListInstalls`, `Upgrade`, `Uninstall`, `ExportData`. Shared caching applies only to the catalogue reads (`ListListings`, `GetListing`, `ListPlans`), which feed the marketing pages; `GetEntitlement`, `GetInstall` and `ListInstalls` are `Cache-Control: no-store` (or cached only under organisation-partitioned keys); writes need an organisation admin's session with step-up for `Uninstall`. State in the control store (TiKV); Stripe is the source of truth for payment state through webhooks (idempotent, signed).

### 4.3 The install flow (PD49)

1. **Discover**: `loams.dev/cloud/marketplace` and the listing page (built by `loam-cloud`, §6). "Start free trial" goes to `console.loams.dev/marketplace/install/loams-software-factory`.
2. **Sign in**: Zitadel; a new user creates an organisation (a Loam tenant, operon §19).
3. **Choose**: organisation, plan, region (one region in the beta, PD40), custom subdomain, and the model option (managed or bring-your-own-key).
4. **Consent**: rendered from the manifest: the apps that will be created and their licences, the agents (names, what each may do, which actions need approval), the budgets and default policy (conservative: gates on, auto-fix off), what is sent to the model provider, and the 14-day trial terms.
5. **Pay**: Stripe Checkout for the subscription, card required for the trial (Q-SF-1); the webhook creates the entitlement `trialing`.
6. **Install saga** (each step idempotent, compensated on failure): create the namespace and quotas; create databases, roles and the bucket; install the Helm release with the overlay values; create the tenant's Zitadel org and applications; run operon's webhook-provisioning saga (operon SF4 Task 10) so apps, agents and principals exist; create the AI gateway route and budgets; seed the demo project (optional, Q-SF-8); health checks of every app and agent; mark `ready`.
7. **Onboard**: a checklist in the console: connect a repository (mirror from GitHub or push), invite people (Zitadel org members, Loams teams), choose the watched projects and metrics, review the default policy, run the demo signal. The Loams Bot desktop and phone apps are offered with a QR pairing (operon §37 D434).
8. **Operate**: upgrades by channel; plan changes (the platform rewrites quota defaults, PD54); cancellation: the entitlement ends at period end, the stack hibernates, data is exportable for 30 days, then deleted by a saga that needs the admin's approval (operon D435) unless the org has been closed.

### 4.4 Billing hook (PD53)

| Item | Stripe object | Source |
|---|---|---|
| Plan | Price (flat, monthly or annual), trial of 14 days | The listing's plan |
| Factory runs above the plan's allowance | Meter `factory_runs` | `io.loams.dev.factory.run.completed.v1` (counts only) from the tenant's factory, through `loam-usage` (doc 01) |
| Model tokens above the allowance | Meter `model_tokens` (input and output separately) | The AI gateway's per-principal reports; zero when the tenant brings its own key |
| Extra seats, extra projects | Quantity on a Price | The entitlement's limits, checked at invite and at project creation |
| Add-ons (OpenPanel, extended retention) | Prices | Entitlement items |
| Spend alarm and hard cap | Customer-set cap | Enforced by the AI gateway budget and the factory guard; the platform emails at 80 percent and 100 percent |

Rules: metering is idempotent on the event id (doc 01); usage never includes model content; the invoice shows runs and tokens by day; credits and refunds are Stripe-side. Entitlement drift (Stripe says `past_due`) triggers a **grace** of 7 days, then the platform kill (PD55) with data kept.

## 5. Security, trust and compliance

| Topic | Decision |
|---|---|
| Tenant isolation | Namespaces with network policies deny-all by default; per-tenant Postgres roles; per-tenant buckets with vended prefix credentials (PD30); Langfuse isolated by organisation and tested (§8 risk 2); gVisor for the coding sandbox and Forgejo Runner (operon D444 makes it optional for trusted single-org code; **mandatory hosted**) |
| Egress | The coding sandbox reaches only the model route and a package mirror; agents reach only their app and the gateway; tenant apps can reach the internet only as the apps need (webhooks, SMTP through a relay with per-tenant limits) |
| Data | Chat transcripts, runs and app data stay in the tenant's region; model prompts may contain code and personal data, so the consent screen says so and the model route is configurable per tenant; a **DPA** and subprocessors list (§8 owner action); erasure through operon D68 and the uninstall saga |
| Secrets | App credentials in the Dapr secret store (operon D189, PD41) behind the credential broker; none in a Helm values file in Git |
| Abuse | PD58 |
| Audit | The open audit events plus hosted long retention and UI (D220) |
| Compliance | SOC 2 is a later programme (not claimed); the listing says what is true |

## 6. Listing #1: Loams Software Factory (the page builder's source of truth)

### 6.1 Pages and routes (for `loam-cloud`)

| Route | Content |
|---|---|
| `/cloud/marketplace` | The marketplace index. Heading, one-line explainer, the featured listing card (Loams Software Factory), and a quiet "More listings are coming" card (no fabricated listings). Filters are not needed with one listing |
| `/cloud/marketplace/loams-software-factory` | The listing page (§6.2) |
| Console `…/marketplace/install/loams-software-factory` | Out of scope for the page builder: the install flow of §4.3 |

### 6.2 Listing page, section by section

**Meta.** Title: `Loams Software Factory: AI agents that close the loop from error to deploy | Loams Cloud`. Description: `Chat with Loams Bot, and a team of agents triages errors in Zulip, files issues in Plane, opens pull requests in Forgejo, deploys behind your approval, and watches the result. Hosted by Loams.` Open Graph image: the loop diagram (shot 1).

**Hero.**
- Eyebrow: `Loams Cloud marketplace · Listing #1`
- Headline: `From error to fix to deploy, in one loop you can stop.`
- Subhead: `Loams Software Factory connects Zulip, Plane, Forgejo, GlitchTip and your product analytics with AI agents, driven from one chat called Loams Bot on desktop and mobile. Agents do the legwork. You approve the merge.`
- Primary CTA: `Start 14-day free trial` (to the install route). Secondary: `Run it yourself, it's open source` (to the GitHub repository's factory docs).
- Hero visual: shot 1 (the loop) beside shot 2 (Loams Bot chat).

**The loop** (a horizontal 8-step strip, each step an icon, a title and one line):
1. Signal: `An error spikes in GlitchTip, or a metric dips in your product analytics.`
2. Triage: `A thread opens in Zulip with the evidence. You reply "fix", "ignore" or "escalate".`
3. Plan: `An issue appears in Plane, linked to the thread and the error.`
4. Fix: `An agent drafts a patch, opens a pull request in Forgejo, and runs CI. If CI fails, it tries again, up to three times.`
5. Review: `You review the diff and approve the merge on your phone, with your fingerprint or face.`
6. Deploy: `The change rolls out through your GitOps pipeline.`
7. Observe: `Agents watch errors, metrics, traces and LLM evaluations for the next 30 minutes.`
8. Close: `Fixed: the issue closes with a summary and what it cost. Regressed: a revert pull request opens, ready for your approval.`

**Loams Bot** (two columns, copy plus shot 2 and shot 3 on a phone frame):
- Heading: `One chat for every tool.`
- Body: `Ask in plain words: "Why did checkout errors spike after the 3 pm deploy?" or "Open an issue for it and draft a fix." Loams Bot asks the right agent in Zulip, Plane, Forgejo, GlitchTip or analytics, shows you each step as it happens, and hands you the approval when something needs a human. It runs on the Loams desktop app and on iOS and Android.`
- Bullets: `Mention an agent with @plane, @zulip, @forgejo, @glitchtip or @analytics.` · `Every answer links to the real issue, thread, pull request or error.` · `Notifications open the exact thread or approval.`

**Features** (a grid of 8 cards; title and one sentence):
1. `Your tools, unmodified`: `Real Zulip, Plane and Forgejo, with single sign-on. Open the full app inside Loams or in your browser.`
2. `Agents with their own identity`: `Each agent has its own account, scoped permissions and a kill switch. No agent ever holds your passwords.`
3. `Human approval where it matters`: `Merging and deploying always wait for you. Approvals are signed on your device.`
4. `Budgets and limits`: `Set a budget per run and per day. Runs pause when they hit it.`
5. `A kill switch that always works`: `Stop one run, one agent or everything, from your desktop or phone.`
6. `See every step`: `Open any agent step in Langfuse to see what the model saw and did, and in SigNoz for the full trace.`
7. `A complete record`: `Every run keeps its thread, issue, pull request, deploy, cost and audit trail in one place.`
8. `Open standards`: `Agents talk over A2A, the Linux Foundation agent protocol. Bring your own model, or use ours.`

**Agents** (six small cards): Plane agent (issues and cycles), Zulip agent (streams and threads), Forgejo agent (repositories, pull requests and CI), GlitchTip agent (errors), Analytics agent (product metrics), Loams Bot (your chat). Each with three example skills from operon §39 §5.5.

**Safety** (a band): `Built to be supervised.` Three columns: `Gates` (`Merge and deploy need your approval. Reads are free; writes are logged.`), `Limits` (`Budgets, attempt caps, cooldowns and loop limits.`), `Control` (`Pause, kill, audit. Your data stays in your tenant and is never used to train models.` *only if true; counsel and model-provider terms confirm, Q-SF-3, otherwise delete that sentence*).

**Under the hood** (short, honest): `Loams Software Factory is open source. The apps run unmodified: Zulip (Apache-2.0), Plane (AGPL-3.0), Forgejo (GPL-3.0-or-later), GlitchTip (MIT), Langfuse (MIT), SigNoz (MIT community build; its collector is AGPL-3.0). Run the same package yourself, or let Loams run it for you.` Links to each source and licence.

**Pricing**: set by the owner, outside this document. The page reads each plan's price and allowances from one config, so changing them is one edit; the plan structure is §6.3.

**Install steps** (a 5-step list, with a screenshot each, shots 8 and 9): 1 `Start your trial and sign in`. 2 `Choose your plan, region and model option`. 3 `Review what will be created and what each agent may do`. 4 `Wait about X minutes while Loams sets up your stack` (the page builder shows "a few minutes" until SF exit reports give a number). 5 `Connect a repository, invite your team and run the demo`.

**FAQ** (8 entries): `Does it merge or deploy by itself?` No, never by default; you approve in Loams Bot, the console or your phone. · `Which model does it use?` DeepSeek through Loams' gateway by default; you can bring your own key. · `Can I use my own Zulip, Plane or Forgejo?` The self-hosted open-source package connects your own; the hosted listing runs its own copies (Q-SF-6). · `What happens to my data if I cancel?` You can export for 30 days; then it is deleted. · `Is it open source?` Yes, the package, agents, apps' integration and chat are open; the hosted service, billing and managed models are Loams Cloud. · `What does it cost to run a fix?` Each run shows its cost; set a budget. · `Do you train on my code?` *Answer after Q-SF-3.* · `Which apps do I need to use?` None; the factory ships with its own.

**Footer legal line:** the licence table's names and links; `Plane, Zulip, Forgejo, GlitchTip, OpenPanel, Langfuse and SigNoz are trademarks of their owners; Loams is not affiliated with them.`

### 6.3 Plans (data for the control plane)

`starter`, `team`, `business`, `self-managed` (support for the open chart, no hosting; `contact us`). Limits per plan are placeholders: projects, seats, runs per day, concurrent runs, open PRs, per-run and per-day budgets, token allowance, audit retention (30 days, 90 days, 1 year). The defaults written into the tenant's factory policy follow PD54.

### 6.4 Screenshots and media to produce (from the seeded demo tenant)

All at 2x, light and dark, the desktop app shell without OS chrome unless stated, no real names (seeded persona `Priya`, project `shop`), no secrets. A seed script (`demo/seed-factory.ts`) creates the tenant with a failing checkout bug, one run at each stage, one approval waiting, one regression and revert.

| # | Shot | Content |
|---|---|---|
| 1 | The loop | The Run detail page: stage graph with the 8 stages, one run completed, with artifact links (thread, issue, PR, deploy, trace) |
| 2 | Loams Bot, desktop | The chat: `@plane open an issue…`, a streaming subagent card, an issue card, a PR card, an approval card |
| 3 | Loams Bot, phone | iOS and Android side by side: the thread, and the approval screen with the biometric prompt |
| 4 | Embedded apps | The console with Plane's board embedded; a second frame with Forgejo's diff embedded; a third with the Zulip triage thread |
| 5 | Agents and safety | Agents page (five agent cards, health) and Policy and budgets with the conservative defaults |
| 6 | Kill switch | The kill dialog naming the in-flight tasks, step-up prompt visible |
| 7 | Traces | Langfuse's trace for one agent step in a pane, and SigNoz's pre-filtered search in another |
| 8 | Install: consent | The consent screen: apps, licences, agents and what each may do |
| 9 | Install: progress | The install saga progress with steps and ticks, then the onboarding checklist |
| 10 | Notifications | A phone lock-screen notification (`Review the merge for run 42`) and the approvals list |
| V1 | 60-second video | Signal to closed: an error appears, the Zulip thread opens, the issue, the PR, the phone approval, deploy, observe, close (screen capture of the seeded demo, no voice, captions) |

### 6.5 Analytics events (PostHog, as `loam-cloud` uses)

`marketplace_viewed`, `marketplace_listing_viewed {slug}`, `marketplace_cta_clicked {slug, cta: trial|source}`, `marketplace_install_started {slug, plan}` (server-side, from the console), `marketplace_install_completed`, `marketplace_install_failed {step}`. No personal data in properties.

## 7. Cost and plan sanity (estimate)

The fixed per-tenant floor of §3.2 (6–8 GB) is the cost driver; with hibernation (Q-SF-7) idle tenants cost storage only. Two measurements size the plans' allowances: the idle footprint after SF1 and SF5, and the median model cost per run after SF4's e2e.

## 8. Risks and owner actions

| # | Risk | Mitigation |
|---|---|---|
| 1 | AGPL and GPL apps offered as a paid hosted service, and trademarks | Unmodified images, source links on the listing, counsel review before the beta (owner action 1) |
| 2 | Cross-tenant leak in shared Langfuse | Org-per-tenant, an automated leak test as a launch gate, per-tenant projects and keys; fall back to per-tenant Langfuse and raise the floor |
| 3 | The per-tenant stack is expensive | No free tier, hibernation, plan allowances from measurements |
| 4 | Abuse of agents and CI | PD58 |
| 5 | Model provider terms (data use, retention) for a hosted model route | Read and sign before the beta (owner action 3); the consent screen states them |
| 6 | App store rules on selling digital services | Purchases on the web only; the apps are clients of an account (owner action 4) |
| 7 | A hosted agent leaks one tenant's content to another through a shared cache | No shared model-side cache across tenants; per-tenant gateway keys; tests |
| 8 | Plane CE's limits (no OIDC, API gaps) frustrate hosted users | Chained SSO (operon §22 §5) |

**Owner actions** (only the owner can do these):
1. **Counsel**: confirm hosting unmodified AGPL-3.0, GPL-3.0-or-later and Apache-2.0 apps as a paid service, the marks, and the listing's wording (PD56).
2. **Stripe**: create the products and prices for the placeholders when the numbers are set; the meters `factory_runs` and `model_tokens`; the tax settings.
3. **Model provider**: accept the commercial terms and the data-processing agreement for the model route (DeepSeek by default) and decide whether "never used to train" can be promised.
4. **Apple and Google**: confirm no purchase flow in the apps; an in-app link to the web account page only, in the form each store allows.
5. **Publish a DPA and a subprocessors list**, and the privacy-policy paragraph about prompts.
6. **Domain**: `*.factory.loams.dev` wildcard DNS and certificates.

## 9. Open questions

| # | Question | Owner | Gate |
|---|---|---|---|
| Q-SF-1 | Trial: card required (proposed) or not, and the abuse trade-off (PD49, PD58) | Founder | Before the beta |
| Q-SF-3 | Counsel's findings on hosting the apps, the "never used to train" claim and the FAQ answers (§6.2) | Founder | Before the page ships |
| Q-SF-4 | OpenPanel as a paid add-on (proposed) or out of the hosted product until the ClickHouse cost is known | Founder | After SF5 |
| Q-SF-5 | Third-party publishers: when, review process, package sandboxing | Founder | After beta |
| Q-SF-6 | Hosted copies only (proposed) or also "connect your own Plane, Zulip and Forgejo" in the hosted plan | Founder | Beta feedback |
| Q-SF-7 | Hibernation after 14 days idle: acceptable restore time and what it promises | Eng | After SF1 measurements |
| Q-SF-8 | Seed the demo project into every new tenant (proposed) or only on request | Founder | Install flow build |
| Q-SF-9 | Langfuse per tenant instead of shared, if the leak test or the floor says so | Eng | Before the beta |

## 10. Cross-references

- operon `docs/design/39-software-factory-and-loams-bot.md` (PR #192): the product, the agents, the loop, the open package; D460–D479.
- operon `docs/open-core.md` (D220) and D403, D444: the split and no metering in the open repository.
- This repository: doc 01 (metering and billing; `loam-usage`, Stripe), doc 04 (the hosted cloud on k3s and Cloudflare: PD29–PD41, PD36 BYOC, PD37 identity (Zitadel since operon D1600), PD38 Stripe).
- `loam-cloud`: the marketing pages (§6), PostHog events (§6.5), the console routes.

## 11. Contradictions, and how they are resolved

| Earlier text | This document | Resolution |
|---|---|---|
| operon D444 makes gVisor optional for trusted single-org code | Mandatory for hosted multi-tenant (§5) | Hosted tenants are untrusted; the open default is unchanged |
| PD10's beta phase used Clerk Billing | Stripe from the start | PD38 already moved billing to Stripe |
| operon §22 §2.2: the showcase suite is a separate project | The factory installs the §22 apps | The package in operon `deploy/factory/` is the supported install; `loam-commons` stays the showcase |
