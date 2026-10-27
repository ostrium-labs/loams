# 07 — Commercial APIs (authoritative boundary)

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Status: **Proposed** · 2026-10-02. **This document is the authoritative statement of which APIs are commercial and private.** It follows the owner's open-core ruling of 2026-10-02 (Loams D540 to D559): "move the commercial API and metering to private, because they may be used to abuse by agents. Integrity is the security principle of Loams." Metering is specified in doc 06 and doc 01; this document covers the APIs that spend money.

Numbering: decisions **PD69–PD72**; questions **Q-OC-5..6**.

## 1. What is a commercial API

An API is commercial, and lives only in this repository, if calling it **creates a charge, changes what a tenant pays, grants a paid entitlement, or reads or writes billing-grade usage**. Scripted calls to such an API by an agent or a tenant must not be able to cost the owner or another customer money, so neither its handlers nor its validators are open.

| API family | Examples | Where |
|---|---|---|
| Hosted Loams Cloud paid APIs | signup and org creation with a plan, plan changes, entitlements, credits and invoices, `GET /v1/orgs/{org}/usage` (billing-grade rollups, doc 01 PD5), the Stripe and Clerk billing webhooks | here (doc 01 §9) |
| Marketplace install and billing APIs | listings, plans and versions, entitlements, the install saga, the billing hook (doc 05 §4) | here |
| Partner and commercial integrations | the protocol gateway and OpenRTB adapters (doc 02), the Cloudflare target and its Tail Worker usage path (doc 03), the hosted-on-Cloudflare control (doc 04) | here |
| Plan-to-limits mapping | the table that turns a plan into quota values | here; it calls the open limits API (below) |

## 2. What is not commercial

The open **Loams Multitenant BYOC Control Plane with GitOps** (Loams §41) has an **operations API**: tenants, namespaces, clusters, BYOC enrolment, release channels, quota limits, and the GitOps writer. It has no endpoint that creates a charge, changes a plan or reads billing-grade usage. A self-hoster runs it for their own tenants and clusters.

## 3. How they connect (PD69 to PD72)

| # | Decision | Status |
|---|---|---|
| PD69 | **The hosted Loams Cloud is the open control plane plus a private commercial layer.** Every tenant-facing request first reaches a commercial API here, which checks identity, plan and entitlement, then calls the open operations API with a service credential. The open operations API is never exposed to tenants on the hosted cloud | Proposed |
| PD70 | **Quota limits flow one way.** This repository computes a plan's limits and writes them to the open limits API (`PUT /v1/orgs/{org}/limits`); the open engine and operator enforce them (Loams §41 §9). A failed write leaves the previous limits in force, so an outage here never removes enforcement | Proposed |
| PD71 | **Commercial APIs are rate limited and abuse-scored here** (doc 01 and the abuse tooling of this repository's README), with per-principal spend caps; agent principals have a hard daily spend cap and cannot raise it | Proposed |
| PD72 | **This repository consumes only open extension points**: the operations API, the limits API, the tenants Git repository, the `InvocationObserver` trait, OTel, the cgroup layout and pod labels. Nothing in the open repository depends on this one | Proposed |

## 4. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-OC-5 | Which agent principals, if any, may call a commercial API at all (default: none; a human approves each spend) | Founder | Before the first paid API ships |
| Q-OC-6 | Publish the commercial API's OpenAPI document to customers (documentation only, never the validators) | Founder | Before GA |

## 5. Charging agents with the Monetization Gateway (PD80 to PD83; added 2026-10-02)

**Monetization Gateway** ([blog 2026-09-30](https://blog.cloudflare.com/monetization-gateway-beta/), read 2026-10-02). **Closed beta** for eligible US-based sellers and buyers; access is requested in the Cloudflare dashboard or at x402@cloudflare.com (owner action). The seller names which requests need payment and the price; a buyer (an AI agent) receives HTTP 402 payment instructions under the **x402** protocol, authorizes, and gets the resource after settlement in USDC on Base through Coinbase's facilitator. Cloudflare's own fee is not stated.

| # | Decision | Status |
|---|---|---|
| PD80 | **The Monetization Gateway is the candidate way to charge AI agents for commercial Loams Cloud APIs** (the paid-API families of §1, and marketplace listings' APIs, doc 05). It is private because it creates a charge, so by §1 neither its configuration nor its settlement handlers are open | Proposed |
| PD81 | **Stripe remains the billing system for humans and organisations** (PD38). 402 payments are a second rail for **agent principals without an account**; each settled payment is matched to a metered call (doc 06 §4a) and shown as separate revenue | Proposed |
| PD82 | **An accountless agent is bound to a stable payer identity, and 402 traffic stays under PD71.** The principal is `payer:<address>`, the wallet address that signs the x402 authorization (the only identity the protocol gives; confirm in Q-OC-9). The gateway verifies the signature before the commercial API serves anything, so the principal cannot be spoofed. PD71's per-principal rate limits, abuse scores and daily caps key on that principal and also on the network source; a new payer starts with a low rate and concurrency tier that rises with settled history. **Settle before serving** for any priced endpoint with a nontrivial cost, so the sellers' exposure is bounded by the price of one call; metered streaming endpoints are prepaid in fixed units. Only metered, idempotent read and compute endpoints may be priced for 402; nothing that changes a plan, an entitlement or an org (§1) is payable by an anonymous agent. Q-OC-5 stays at default none for account-bound agent principals | Proposed |
| PD83 | **Adoption is gated on three facts**: closed-beta access is granted; the fee, payout and tax treatment are in writing (not stated in the post); counsel confirms USDC settlement on Base is acceptable for the company. Until then, no 402 endpoint ships | Proposed |

**Owner actions:** request closed-beta access (dashboard onboarding application or x402@cloudflare.com; eligibility is US-based sellers and buyers).

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-OC-7 | Cloudflare's fee and payout terms for the Monetization Gateway | Founder | Before PD83 clears |
| Q-OC-9 | Is the signing wallet the only payer identity x402 provides, and can the facilitator or Cloudflare supply a stronger one (for example the buyer's Cloudflare account)? | Eng | Before PD83 clears |
| Q-OC-8 | Which endpoints are priced for 402 first (candidates: search and embed calls on a public demo dataset) | Founder | After access |
