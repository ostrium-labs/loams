# Loams platform design (imported)

Design documents written in the former private `ostrium-labs/loams-platform` repository for the hosted Loams Cloud: metering and billing, the meter record, commercial APIs, the protocol gateway and OpenRTB, the Cloudflare target, the hosted cloud and the marketplace. They are open source (Apache-2.0) since the owner's order of 2026-10-19 and are kept as written; [the platform docs index](../README.md) says what no longer holds. Their numbering (01–07, PD-numbers, Q-MB-n and so on) is their own and does not collide with the engine's [`docs/design`](../../design/README.md); where they say "operon §N" or give a bare D-number they mean the engine's design and decision log.

| # | Document | Scope | Status |
|---|---|---|---|
| 01 | [Metering and billing](01-metering-and-billing.md) | The `loam-meter` node agent (cgroup v2, `/proc`, wasmtime reports, Envoy ALS) and the `loam-usage` aggregator; the CloudEvents meter event and its idempotency key; the spool and exactly-once rules; accuracy; OpenMeter, Lago and Flexprice evaluated; phased billing (beta: Clerk with hard quotas; GA: Stripe Billing with meter events for overage); track MB (PD1–PD10, Q-MB-1..6); from 2026-10-02, runner usage reconciliation against provider invoices, the usage-event form as the meter event, and Knative and Cloudflare sources (PD26–PD28, §13) | **Proposed** |
| 02 | [The protocol gateway and OpenRTB](02-protocol-gateway-and-openrtb.md) | Moved from operon §34 on 2026-10-02 (operon D440). The `ProtocolAdapter`, the canonical `loam.rtb.v1`, per-partner version negotiation, OpenRTB 2.6/2.5/2.4 and Google's OpenRTB, the bid hot path, match/merge, the OpenRTB conformance suite, licensing; plans GW1–GW4 (PD11–PD19, PD45–PD46, Q-GW-1..7). The vendor-neutral charter, narrow waist, CloudEvents profile and `Runner` trait stay in operon | **Approved** (owner defaults, 2026-10-02) |
| 03 | [Cloudflare as a deployment target, and the startup credits](03-cloudflare-target-and-credits.md) | Moved from operon §35 (PR #179) on 2026-10-02. Workloads on Workers, Durable Objects, R2 and Containers; the `CloudflareRunner` (thin and native); Cloudflare `Fs` backends; tenancy on Cloudflare; the Tail Worker usage path; limits; the Cloudflare for Startups credits plan; plan CF1 (PD20–PD25, Q-CF-1..9) | **Approved** (owner defaults, 2026-10-02) |
| 04 | [Hosted Loams Cloud on Cloudflare](04-hosted-loams-cloud-on-cloudflare.md) | What "all on Cloudflare" can mean today (no managed Kubernetes, no persistent container disk, Enterprise-only custom TCP): Workers, WfP, Containers, R2 and Durable Objects at the front; a three-node k3s cluster behind Cloudflare Tunnel for TiKV, Iggy, Fluss, Neon and WeSQL; R2 as the bucket; Knative on k3s; customer BYOC into their own Cloudflare accounts; Authentik and Stripe instead of Clerk (owner decisions); Appendix Z, the interplanetary enterprise tier, for fun (PD29–PD44, Q-HC-1..12) | **Approved** (owner defaults, 2026-10-02) |
| 05 | [The hosted Loams Software Factory and the Loams Cloud marketplace](05-hosted-software-factory-and-marketplace.md) | The commercial half of operon §39 (PR #192): per-tenant hosted factory stacks, managed agents and model costs, the marketplace (listings, plans, versions, entitlements, the install saga, the Stripe billing hook), listing #1 Loams Software Factory with the page copy, install steps and screenshot shot list for `loam-cloud`; owner actions (PD47–PD60, Q-SF-1..9) | **Proposed** |
| 06 | [The meter record and the usage reporter](06-meter-record-and-usage-reporter.md) | **Authoritative** metering input spec, moved from Loams §27 §3.3 and §3.6 and RN1 Tasks 1, 2 and 5 on 2026-10-02 (Loams D540 to D559): `loams.meter.v1`, the host-report socket and delivery rules, the reporter, the Lambda usage rules, integrity rules (PD61–PD68, Q-OC-1..4) | **Proposed** |
| 07 | [Commercial APIs](07-commercial-apis.md) | **Authoritative** boundary of the commercial and private APIs: hosted paid APIs, the marketplace install and billing APIs, partner integrations; how they call the open control plane (PD69–PD72, Q-OC-5..6) | **Proposed** |

## Plans

| Plan | Scope | Depends on | Status |
|---|---|---|---|
| [GW1: Proto contract, CloudEvents profile, event Arrow schema](../plans/2026-10-01-gw1-proto-events-arrow.md) | `buf breaking`; `loam.rtb.v1`; the profile and Arrow mapping (the vendor-neutral parts return to operon as an open plan, operon Q450, answered 2026-10-02) | operon PRs #170/#171 | Planned (moved) |
| [GW2: Gateway skeleton, OpenRTB 2.6, golden corpus](../plans/2026-10-01-gw2-gateway-openrtb26.md) | `ProtocolAdapter`, 2.6 adapter, conformance, the gateway | GW1; after the hosted beta (Q-GW-1, PD45) | Planned (moved) |
| [GW3: Negotiation, OpenRTB 2.5 and 2.4](../plans/2026-10-01-gw3-negotiation-openrtb25-24.md) | Downgrade rules, per-partner negotiation | GW2 | Planned (moved) |
| [GW4: Google and OpenRTB protobuf, differential](../plans/2026-10-01-gw4-google-differential.md) | Vendored protos, Google adapter, cross-protocol differential | GW3 | Planned (moved) |
| [CF1: The Cloudflare spike](../plans/2026-10-01-cf1-cloudflare-spike.md) | Thin and native Workers, `Fs` backends, R2 races, gitoxide in a Durable Object, the credits decision | — | Planned (moved) |

## Owner actions pending (2026-10-02)

On 2026-10-02 the owner accepted the recommended or proposed default for every open question in docs 02–04 that has one ("do suggested for all"), and a chosen default where a doc gave none, except the owner actions below; the decisions PD11–PD42 are approved, and PD43–PD46 record the defaults chosen where a doc gave none. The questions below need an action only the owner can take (accounts, money, legal), so they stay open:

| # | Action | Needed by |
|---|---|---|
| Q-CF-5 | Confirm with Cloudflare, when applying, which products the startup credits cover | Before applying |
| Q-HC-3 | Choose, and rent or buy, the three k3s machines (provider and region) | Before the beta cluster |
| Q-GW-4 | Counsel sets the anonymisation rules, retention and storage of private partner corpora | Before any real-traffic fixture |
| CF1 | Subscribe to Workers Paid ($5/month; Containers need it) and issue the Cloudflare API tokens CF1 deploys with (doc 03 §8); Workers for Platforms ($25/month) when the hosted beta needs it | CF1 Task 1 |
