# Platform: metering, billing and hosted-cloud docs

Loams is open source end to end. On 2026-10-19 the owner ordered the former private repository `ostrium-labs/loams-platform` merged into this monorepo ("pull all code in loam-platform to the OSS repo"). It is imported as one snapshot of its tree at `0592cbf` (its history is not carried over), relicensed under Apache-2.0 by its sole author. Nothing here is closed any more.

On import, notes that were business strategy rather than technical design were taken out of the documents (notes on the company's financing, startup-programme eligibility and accelerator application, the product-direction rulings for real-time bidding, pricing tiers and margins, revenue-share and plan-mix questions). Billing mechanics, metering phases, credits and the technical design are kept.

## What came in, and where it lives

| Path | What |
|---|---|
| [`crates/loams-meter-agent`](../../crates/loams-meter-agent) | `loams-meter-agent` (was `loam-meter`, then `loams-meter` under the former `platform/` workspace), the node metering agent (MB0: cgroup v2 reader, delta tracker, static tenant resolver, CloudEvents meter events, the on-disk spool and the `_system/_meter` stream sink). It moved into the root workspace in plan ME1 Task 0b (design 63 D1135), and the `platform/` workspace was removed; the root `ci.yml` builds and tests it |
| [`design/`](design/README.md) | Platform design documents 01–07 (metering and billing, the protocol gateway and OpenRTB, the Cloudflare target, the hosted cloud on Cloudflare, the marketplace, the meter record and usage reporter, commercial APIs) |
| [`plans/`](plans) | Plans GW1–GW4 (protocol gateway, OpenRTB) and CF1 (the Cloudflare spike); none started |
| [`deploy/`](deploy) | Runbooks for Zitadel at `auth.loams.dev` ([`zitadel.md`](deploy/zitadel.md), planned, IA1), the tailnet ([`tailnet.md`](deploy/tailnet.md)) and Headscale ([`headscale.md`](deploy/headscale.md)) |
| [`../../deploy/tailnet`](../../deploy/tailnet) | The tailnet deploy files (policy as code, Tailscale and Headscale) those runbooks use. The Zitadel bundle for `auth.loams.dev` lands in [`../../deploy/release/auth`](../../deploy/release/auth) with IA1 |

The import renamed `loam-*` to `loams-*` in the crate (crate, binary, spool path, the `loams-meter://` event source, the `dev.loams.meter.v1` event type and the `loams.slice` cgroup prefix in docs and tests, matching the engine's [usage hooks](../design/27-usage-hooks.md) §3.2). It did not change behaviour; refactors belong to design 63.

## What no longer holds

The imported documents are kept as written. Read them with these corrections:

- **The open-core boundary is gone.** The platform README's rule (2026-10-02) that metering, billing and commercial APIs stay private, and every "this repository", "proprietary" or "private" in docs 01–07 and the plans, describe the former repository. The engine's [usage hooks](../design/27-usage-hooks.md) §3.3 and [design 41](../design/41-multitenant-byoc-control-plane.md) still point at `loams-platform` for the meter record and the reporter; those now resolve to [doc 06](design/06-meter-record-and-usage-reporter.md) here.
- **Integrity no longer comes from secrecy.** Doc 01, doc 06 and doc 07 argued that billing-grade producers and validators stay private so that tenants and agents cannot study and forge them. With the code public, the meter's integrity has to rest on where it runs (outside the tenant sandbox, as `loams-meter-agent` already does), on the idempotency keys and reconciliation against provider totals, and on server-side validation. Design 63 owns that argument.
- **The identity provider is Zitadel.** On 2026-10-21 the owner ruled a full cutover to Zitadel with no migration and no dual running ([design 76](../design/76-identity-and-authorization.md), D1600). Docs 04 and 05 and the deploy runbooks were updated for it; Zitadel at `auth.loams.dev` is planned (IA1), and until then installs use built-in single-org mode or any OIDC issuer.
- **Repository names.** `dina-kar/operon` is now `ostrium-labs/loams` (operon links in these docs were rewritten to relative links where the target exists); `dina-kar/loam-cloud` is `ostrium-labs/loams-site`.
- **The tailnet policy is not applied from here.** The private repository's CI applied `deploy/tailnet/policy/policy.hujson` to the hosted tailnet on push to `main`. The monorepo's `platform.yml` only lints it and runs Headscale's checker; applying it to the hosted tailnet belongs to a separate private deployment repository that holds the Tailscale credentials ([`tailnet.md`](deploy/tailnet.md), step 3).

## For design 63 (Loams Meter)

What the imported code gives design 63's migration section:

- **`crates/loams-meter-agent`** (formerly `platform/crates/loams-meter`; about 2,900 lines with tests): the node agent's loop (`Agent`, `TickReport`); `CgroupReader` over cgroup v2 `cpu.stat`, `memory.current`, `memory.peak` and `cgroup.events`, with suffix-glob targets (pod cgroups, `tenant-*.slice`, `wasm-host.scope`) and a lenient scan; `DeltaTracker` (counter resets, inode-keyed baselines, final reads); `StaticResolver` behind the `TenantResolver` trait (prefix rules from JSON); `MeterEvent` as a CloudEvent with the idempotency key `<node>/<source>/<seq>`; a size-bounded on-disk `Spool` whose baselines advance only after the write; `Sink` with `MemorySink` and `StreamSink` (HTTP produce to `_system/_meter`), and `forward`.
- **Not built** (doc 01's track MB): the Kubernetes pod-watch resolver and inotify final reads (MB2), Envoy ALS (MB3), the wasmtime host socket and the `loams.meter.v1` host report of doc 06 (MB4), the `loams-usage` aggregator, rollups, the ledger and billing (MB1 and later).
- **Specs:** [doc 01](design/01-metering-and-billing.md) (agent, aggregator, accuracy, billing phases, PD1–PD10, PD26–PD28) and [doc 06](design/06-meter-record-and-usage-reporter.md) (the meter record, host-report socket, delivery and acknowledgement, the reporter, PD61–PD68) are the inputs design 63 supersedes or amends.
- **Workspace:** the import kept `platform/` on reqwest 0.12 and base64 0.22; design 63 (approved 2026-10-19, D1135) kept the agent as `loams-meter-agent`, and [ME1](../plans/2026-10-19-me1-loams-meter.md) Task 0b moved it to `crates/loams-meter-agent` in the root workspace on reqwest 0.13 and base64 0.23 and removed `platform/`.
