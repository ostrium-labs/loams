# MK1 — The Loams Agent Marketplace

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, fields or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved stub** (owner, 2026-10-21, all defaults). Track MK, design [§73](../design/73-agent-marketplace.md) (D1445–D1479, Q1122–Q1144), with [§72](../design/72-software-factory-loop.md) for the factory agents that are its first listings. This plan specifies **MK0** (Mastra first, first-party only) task by task. MK1 (any framework, third-party), MK2 (more targets and kinds) and MK3 (paid listings, only if the owner decides) are listed at milestone level.

**Goal (MK0):** a Loams Desktop user opens the Marketplace page, finds a first-party Mastra agent, an ACP or built-in harness, or an MCP server, sees exactly what it will be allowed to do, installs it, and uses it in Studio; the listing came from a signed static index whose entries were checked for provenance and passed the Core level of the agent package conformance. The schema, the runtime contract and the conformance runner are framework-neutral from the first commit, even though MK0 lists only Mastra agents.

**Architecture** (§73 §3–§9):
- **`ostrium-labs/loams-marketplace`** (new public repository, created by the owner): listings, schemas, CI, the index build and signing.
- **`schemas/marketplace/listing.v1.json`** and **`proto/loams/marketplace/v1/listing.proto`** in this repository: the manifest's two forms, kept equal by a test.
- **`agents/loams-listing`** (TypeScript CLI, a member of the `agents/*` workspace that FT0 Task 1 adds): `lint`, `from-package` (turn a `loams.agent` block into `listing.json`), `index build`.
- **`crates/loams-agentd-marketplace`** (new): sources, index verification, revocations, install planning, installers for the `node` runtime, harness descriptors and MCP servers, the environment contract (§73 §3.3), and the conformance runner.
- **`proto/loams/agentd/v1/marketplace.proto`**: `MarketplaceService`.
- **`ostrium-labs/loams-mastra`**: `packages/agent-host` (`@loams/agent-host`) and `packages/acp` (`@loams/mastra-acp`, shared with FT0 Task 2).
- **`web/plugins/marketplace`** (`@loams/plugin-marketplace`, modes console and studio) and **`loams marketplace`** in the CLI.

**Tech stack:** Rust 1.97 (workspace dependencies only; `sigstore` verification through the crate the release pipeline already uses, else a ruling in Task 0); TypeScript and Node ≥ 22.13; `@mastra/*` at `loams-mastra`'s pinned set; GitHub Actions on GitHub-hosted runners for the index.

**Spec:** [§73](../design/73-agent-marketplace.md) (all of it); [§53](../design/53-studio-ade.md) §6; [§60](../design/60-places-and-harness-composition.md); [§65](../design/65-loams-operator.md) for the conformance shape; the draft [publishing guide](../marketplace/publishing.md).

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/mk0-<task>`; one PR per task into `dev`, or into `loams-mastra` / `loams-marketplace` `main` where the task says. `git commit -s`.
- **Light Rust builds.** `cargo build -p loams-agentd-marketplace`; never `--workspace` locally; never build in `/tmp`.
- **Metadata only.** The index never contains package bytes.
- **No code at install.** npm installs run with scripts off; anything that needs a script is refused.
- **Nothing before consent.** No Place, role, key, file or process is created before the person confirms the generated install plan.
- **First-party only in MK0.** The index CI refuses namespaces other than `ostrium` and `loams` until MK1.
- **No telemetry.** Neither agentd nor the index reports installs.

## Review Focus

1. **An unsigned or wrongly signed index is accepted.** Expected: refused. Tests: `index_signature_required`, `index_identity_pinned`.
2. **An artifact whose integrity or provenance does not match is installed.** Expected: refused. Tests: `integrity_mismatch_refused`, `provenance_repo_mismatch_refused`.
3. **An agent gets anything outside the environment contract.** Expected: never. Test: `agent_env_is_exactly_the_contract`.
4. **An undeclared tool runs.** Expected: refused. Tests: `undeclared_acp_tool_refused`, `undeclared_mcp_tool_hidden`.
5. **A revoked version keeps running.** Expected: stopped within one check. Test: `disable_revocation_stops_agent`.
6. **Something is created before consent.** Expected: never. Test: `plan_install_creates_nothing`.

## MK0 — Mastra first, first-party only (Tasks 0–11)

### Task 0 — Rulings

Check and record: the owner has created `ostrium-labs/loams-marketplace` (Q1124); `@loams/mastra-resonate` 0.1.0 is published with provenance; the Rust Sigstore verification library and its licence; how npm provenance is verified offline from a package's attestations; ACP's permission request as agentd's ACP harness maps it today; the GitHub and Plane MCP servers' licences, transports and `server.json` (Q1129); how agentd's `AcpAgentSpec` can be registered at run time (today it is a static table).

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — Listing schema v1

Files: `schemas/marketplace/listing.v1.json` (every field of §73 §3.6, with `runtime.kind` enumerating `node`, `python`, `oci`, `binary`; `surfaces` requiring `acp` or `a2a` for agents; `pricing` const `free`); `proto/loams/marketplace/v1/listing.proto`; fixtures under `schemas/marketplace/testdata/` (valid agent, harness and MCP-server listings; invalid ones for each rule).

Tests: `listing_schema_accepts_fixtures`, `listing_schema_rejects_each_invalid_fixture`, `proto_and_schema_agree`.

Commit: `marketplace: listing schema v1 (MK0 Task 1, D1455)`.

### Task 2 — `loams-listing` CLI

Files: `agents/loams-listing/` with `lint <listing.json>`, `from-package <dir>` (reads `package.json` `loams.agent`), `index build <listings-dir> --out <dir>`.

Tests: `from_package_round_trips`, `lint_reports_each_rule`, `index_build_is_deterministic`.

Commit: `marketplace: the loams-listing CLI (MK0 Task 2)`.

### Task 3 — The index repository (in `loams-marketplace`)

Files: layout of §73 §5; `.github/workflows/validate.yml` (on pull requests: lint, namespace check (first-party only), artifact resolution, integrity and provenance check, LAPC Core run from Task 8, a report comment); `.github/workflows/publish.yml` (on `main`: `index build`, Sigstore keyless signing of `index.json` and `revocations.json`, publish to GitHub Pages; a release artifact for `loams-site`).

Tests: workflow self-tests on fixture listings, `unsigned_publish_fails`.

Commit (in `loams-marketplace`): `index: validate and publish a signed static index (MK0 Task 3, D1460)`.

### Task 4 — `MarketplaceService` and sources

Files: `proto/loams/agentd/v1/marketplace.proto` (`ListSources`, `AddSource`, `RemoveSource`, `Search`, `GetListing`, `PlanInstall`, `Install`, `Uninstall`, `ListInstalled`, `CheckUpdates`, `Update`); `crates/loams-agentd-marketplace/src/{sources,index,verify,revocations}.rs`; the official source pinned to the publish workflow's identity by default.

Tests: `index_signature_required`, `index_identity_pinned`, `search_filters_by_kind_framework_level`, `revocations_checked_on_start`.

Commit: `agentd: MarketplaceService, sources and signed index verification (MK0 Task 4, D1462)`.

### Task 5 — Install planning and the environment contract

Files: `crates/loams-agentd-marketplace/src/{plan,contract}.rs`: `PlanInstall` returns the consent plan (Place preview, tools by risk, egress, dependencies, secrets, models, budgets, licence, tier, provenance, version age) and creates nothing; `contract.rs` builds the §73 §3.3 environment (token file rotation every 10 minutes for 15-minute tokens; a Postgres role per install when declared; proxied MCP config).

Tests: `plan_install_creates_nothing`, `plan_lists_every_declared_permission`, `agent_env_is_exactly_the_contract`, `token_file_rotates`.

Commit: `agentd: install plans and the agent environment contract (MK0 Task 5, D1453, D1466)`.

### Task 6 — Installers: `node` agents, harness descriptors, MCP servers

Files: `crates/loams-agentd-marketplace/src/install/{node,harness,mcp}.rs`: npm tarball fetched by integrity, lockfile install with scripts off, into `<data>/agents/<id>/<version>`; provenance verified; harness descriptors registered as `AcpAgentSpec` entries at run time (`HarnessId::Marketplace(listing_id)`); MCP servers registered for harness runs and agents with secrets in the vault; uninstall keeps user data.

Tests: `integrity_mismatch_refused`, `provenance_repo_mismatch_refused`, `install_scripts_refused`, `harness_descriptor_appears_in_list_harnesses`, `mcp_server_available_to_threads`, `uninstall_keeps_data`.

Commit: `agentd: installers for node agents, harnesses and MCP servers (MK0 Task 6, D1464, D1473)`.

### Task 7 — `@loams/agent-host` (in `loams-mastra`)

Files: `packages/agent-host/`: loads `createAgent(ctx)`; builds `ctx` from the environment contract (`PostgresStore` and `PgVector` on `LOAMS_PG_URL`, `@mastra/qdrant` on `LOAMS_QDRANT_URL`, the Resonate engine on `LOAMS_DURABLE_URL`, `MCPClient` from `LOAMS_MCP_CONFIG`); registers only declared tools; serves ACP through `@loams/mastra-acp`.

Tests: `host_loads_starter_agent`, `host_registers_only_declared_tools`, `host_reads_only_contract_env`, `host_exits_on_sigterm`.

Commit (in `loams-mastra`): `agent-host: run a packaged Mastra agent under the Loams contract (MK0 Task 7, D1456)`.

### Task 8 — LAPC Core runner

Files: `crates/loams-agentd-marketplace/src/conformance/`: `loams agent conformance <package|listing>` runs LAPC-C1 to C9 (§73 §9) in a sandboxed process (process isolation and an egress-recording proxy in MK0), with a scripted model endpoint and a fake MCP server; writes a JSON report; `--sign` signs it with the CI identity.

Tests: one passing and one failing fixture agent per requirement (`lapc_c4_undeclared_tool_fails`, `lapc_c5_egress_fails`, `lapc_c7_hang_fails`, …).

Commit: `agentd: the agent package conformance runner, Core level (MK0 Task 8, D1469)`.

### Task 9 — Console and Studio

Files: `web/plugins/marketplace/` (`modes: ["console","studio"]`): browse with filters, the listing page, the consent screen from `PlanInstall`, installed agents with updates and revocation banners; Studio's harness picker "Add a harness…".

Tests: component tests for the consent screen (every permission shown), a Playwright check in `studio-e2e`: install the fixture agent and open a thread with it.

Commit: `console: the marketplace plugin and consent screen (MK0 Task 9, D1465)`.

### Task 10 — CLI

Files: `loams marketplace search|info|install|uninstall|list|update|source` through agentd.

Tests: CLI integration tests against a fixture index.

Commit: `cli: loams marketplace (MK0 Task 10)`.

### Task 11 — First-party listings and docs

Listings in `loams-marketplace`: `ostrium/loams-bot` and the factory's agents (after FT0 Task 6), `ostrium/loams-starter`, `ostrium/support-agent` (MA1 Task 43), the built-in harness descriptors, and the MCP servers of Task 0's ruling. Docs: `docs/marketplace/README.md` (status **experimental**, first-party only); a note at the top of `docs/marketplace/publishing.md` pointing to §73; the plans README row.

Tests: every listing passes `validate.yml` and LAPC Core.

Commit: `marketplace: first-party listings and docs (MK0 Task 11)`.

## MK1 — Any framework, third-party (milestone level)

The `python` (wheels only, `uv`, hashes) and `binary` runtimes; agentd's A2A bridge from `loams-agentd-link`; the model proxy (§73 §3.4) required for third-party agents; LAPC Compatible; `ostrium-labs/loams-agent-templates` and conformance cells for Pydantic AI, LangGraph, the OpenAI Agents SDK and Google ADK (Q1133); Resonate's Python SDK cell against Loams Durable (Q1143); namespace verification (GitHub OIDC, DNS, HTTP); the scanning pipeline (§73 §10.2); verified and community tiers; revocation runbook; third-party ACP harnesses such as fx; everything third-party in SB1's `linux` sandbox; the publishing guide rewritten. Estimate: 12 engineer-weeks.

## MK2 — More targets and kinds (milestone level)

The `oci` runtime; the BYOC `agents` profile; app BYOC agents on §71's contract; `loams-registry` with a publish API and MCP-Registry v0.1 compatibility; a curated MCP Registry import; templates, workflows, console plugins, connectors and factory apps as kinds; the Jailer and the gateway's `mcp` module as enforcement points. Estimate: 9 engineer-weeks.

## MK3 — Paid listings (only if the owner decides)

Through the Meter's user meters (D1138). Not designed.

## Rulings made during execution

### Task 0 (2026-10-23)

Read on 2026-10-23 at `origin/dev` `710f47078`.

1. **The index repository exists** (Q1124). `ostrium-labs/loams-marketplace` is public; the owner created it on 2026-10-21, and it holds only `LICENSE` (Apache-2.0). Task 3 starts from an empty `main`.
2. **`@loams/mastra-resonate` is not on npm** (the registry answers 404 for the whole package; MA1 Task 21 is still pending). This is the MK0 gate of §73 §12, so Task 7's end-to-end check and the Task 11 listings that depend on it wait for the publish. Tasks 1–6 and 8–10 do not need it: their fixtures are local packages. The plan carries on in order and records the wait at each affected task.
3. **Sigstore verification in Rust: `sigstore-verify`** from `sigstore/sigstore-rust` (Apache-2.0; the modular successor of `sigstore-rs`, whose `sigstore` crate is still marked experimental). It verifies a Sigstore bundle offline against a trusted root (`sigstore-trust-root`), including DSSE envelopes, Rekor inclusion and TSA timestamps. The repository's 14-day rule for new dependencies makes **0.14.0** (published 2026-09-29) the version; 0.15.0 (2026-10-13) and 0.15.1 (2026-10-23) are too new. The release pipeline uses the `cosign` CLI (`nf1-pins.yml`, cosign v3.0.6), not a Rust crate, so there was nothing to reuse. The production trusted root is the Sigstore public-good root embedded at build time and refreshed through TUF later. Tests use their own test CA and trusted root behind the same verifier interface, so `index_signature_required` and `index_identity_pinned` run without the network.
4. **The index signature** is a Sigstore bundle written by `cosign sign-blob --bundle` with keyless signing in `publish.yml` (`index.json.sigstore.json`, `revocations.json.sigstore.json`). The official source pins the certificate identity to `https://github.com/ostrium-labs/loams-marketplace/.github/workflows/publish.yml@refs/heads/main` with issuer `https://token.actions.githubusercontent.com`. A source added with `--identity` pins its own pair.
5. **npm provenance, verified without trusting the registry.** `dist.attestations.url` in the version document points at `/-/npm/v1/attestations/<name>@<version>`. That returns two Sigstore bundles (media type `application/vnd.dev.sigstore.bundle+json;version=0.2`): npm's publish attestation, signed by npm's key, and the SLSA provenance (`https://slsa.dev/provenance/v1`), whose DSSE payload is an in-toto statement. It has the subject `pkg:npm/<name>@<version>` with a `sha512` digest, and `predicate.buildDefinition.externalParameters.workflow` with `repository`, `path` and `ref` (checked on `sigstore@3.1.0`). agentd verifies the SLSA bundle with `sigstore-verify`. It then requires:
   - the subject digest to equal the tarball's `integrity`, which equals the listing's `artifact.integrity`;
   - `workflow.repository` to equal the listing's `source.repository`;
   - the certificate's issuer to be GitHub Actions or GitLab CI.

   The index CI saves both bundles into `versions/<version>/provenance/`, so an install can verify against the index's copy when the registry is unreachable. The publish attestation is recorded but not required in MK0.
6. **ACP permission requests today** (`crates/loams-agentd-harness/src/acp/{mod,approvals}.rs`). In a supervised thread, `session/request_permission` becomes an `ApprovalAsk`:
   - the tool is the call's `kind`, else its `title`;
   - the call id is `toolCallId`;
   - the arguments are `title`, `kind`, `rawInput` and `locations`;
   - the risk comes from `approvals::tool_risk`.

   The decision maps to the agent's `allow_once` or `reject_once` option. In a full-access thread the harness auto-accepts the preferred allow option. **For marketplace agents** (§73 §7.4) the request is checked against the manifest before that mapping: a tool that is not declared is rejected with the reject option and recorded, whatever the thread's mode (`undeclared_acp_tool_refused`, Task 6). A declared tool's manifest `risk` replaces the heuristic `tool_risk`. Matching uses the declared `name` against the call's `title` first, then `kind`, because ACP has no stable tool name field.
7. **`AcpAgentSpec` is static today.** It is a private struct of `&'static str` fields and `fn` pointers, built by `grok_spec()`, `devin_spec()`, `hermes_spec()` and `antigravity_spec()`. `HarnessId` is a `Copy` kebab-case enum with `#[serde(other)] Unsupported`, used across sessions, the registry and the TypeScript bindings. Run-time registration (Task 6) therefore:
   - adds an owned `AcpLaunch` (command, args, environment, install root, models) that a marketplace harness is built from, beside the static specs;
   - adds `HarnessId::Marketplace`, a unit variant so `HarnessId` stays `Copy`, with the listing id carried next to it in the descriptor and the session record (`marketplace_listing`). This replaces the plan's `HarnessId::Marketplace(listing_id)`: a `String` payload would remove `Copy` from a type used in hundreds of places. The wire name is `marketplace`, and pickers key marketplace entries by listing id.
8. **The first MCP-server listings** (Q1129).
   - **GitHub's** `github/github-mcp-server` is MIT, written in Go, and v2.0.3 (2026-10-21) ships per-OS archives with a `checksums.txt`. Its repository's `server.json` (schema `2025-12-11`, name `io.github.github/github-mcp-server`) lists only the OCI package over stdio. The remote server is `https://api.githubcopilot.com/mcp/` over streamable HTTP. MK0 lists it as the archive runtime with SHA-256 per platform, launched as `github-mcp-server stdio`, with `GITHUB_PERSONAL_ACCESS_TOKEN` as a vault secret. The listing embeds the `server.json` unchanged plus our `dev.loams/listing` block. The OCI package waits for the `oci` runtime (MK2).
   - **Plane's** `makeplane/plane-mcp-server` is MIT and now Python (FastMCP, `plane-mcp-server` 0.3.5 on PyPI, run with `uvx`). The old npm package `@makeplane/plane-mcp-server` is deprecated and must not be listed. Because `python` is an MK1 runtime, MK0 lists Plane **only as its remote** streamable-HTTP server (`https://mcp.plane.so`, personal access token from the vault, egress `mcp.plane.so`), with no local install. The stdio package follows in MK1. Neither repository's licence or transport blocks a listing.
   - Forgejo's and Sentry's servers are not checked here and stay out of MK0.
9. **Node for the `node` runtime** (Q1122). agentd already resolves `node` for npm harness adapters (`adapter_install.rs`, `executable::find_on_paths` with the node-version-manager scan). The `node` installer reuses that resolution and refuses a Node older than the listing's `runtime.version` (default `>=22.13`). Installing a pinned Node archive when none is found is left to Task 6's ruling.
10. **The `agents/*` workspace** (FT0 Task 1) is not on `dev` yet. If it has not merged when Task 2 starts, Task 2 adds the one `agents/*` line to `pnpm-workspace.yaml` itself, so the FT1 work only has to rebase.

### Task 1 (2026-10-23)

1. **Kind-specific blocks.** A listing keeps the fields shared by every kind at the top level: identity, `artifact`, `provenance`, `runtime`, `entrypoint`, `tools`, `place` and `config`. Two nested blocks carry what only one kind has. `harness` holds `launch { protocol, command, args }`, `auth`, `models` and, for native harnesses, `native_adapter`, which is agentd's harness wire name. `mcp_server` holds the embedded `server.json`, `mode` (`local` or `remote`), `args`, `remote_url` and `transport`. The schema forbids the other kinds' blocks.
2. **The MCP server's Loams block is the listing itself** (refines §73 §3.7). The embedded `server.json` is kept byte-for-byte; the tool risks, egress and Place template are the listing's own top-level `tools` and `place`. When the index is served as an MCP-Registry subregistry (MK2), the build writes them into `_meta."dev.loams/listing"`. This keeps one reader for every kind in agentd.
3. **Rules the schema enforces:**
   - an agent serves `acp` or `a2a`;
   - each runtime takes one artifact type (`node`/`npm`, `python`/`pypi`, `oci`/`oci`, `binary`/`archive`);
   - every artifact needs `provenance`;
   - npm integrity is `sha512-…`, and pypi and oci integrity is `sha256:<hex>`;
   - archives carry a SHA-256 per platform and no top-level integrity;
   - a native harness is first-party only (`ostrium/` or `loams/`) and has no artifact, while an ACP harness has one;
   - a remote MCP server has `remote_url`, speaks `streamable-http` and installs nothing;
   - a secret config input has no `default`;
   - only `enum` inputs list `values`;
   - `pricing` is `free`.
4. **Fields added beyond §73 §3.6:** `source.path` (a package inside a monorepo), `config[].env` (the variable the value is passed in), `conformance.report_sha256`, and a `bucket` data scope beside databases, collections and streams. Budgets are `cost.budget_usd_per_run` and `cost.budget_usd_per_day`.
5. **Not in the listing:** `tier` and `yanked`. The index sets them (Task 2), because an author cannot claim them.
6. **`pricing` is a one-value enum** (`"free"`), not a `const`, so the proto has an equal `Pricing` enum.
7. **The proto–schema check is structural.** Each object schema names its message in `x-proto-message` and each enum names its proto enum in `x-proto-enum`. `proto_and_schema_agree` checks:
   - field names in both directions;
   - that arrays are `repeated`;
   - the field types (message, enum, scalar, `Struct` for embedded documents, `Value` for `default`);
   - the enum values (`"mcp-server"` is `KIND_MCP_SERVER`).
8. **`github-attestation` provenance** is added beside §73 §6's types. Release archives attested with GitHub artifact attestations, such as `github/github-mcp-server` v2.0.3, are looked up by each file's SHA-256 at `https://api.github.com/repos/<owner>/<repo>/attestations/sha256:<digest>`, which needs no token. The lookup returns Sigstore bundles (v0.3) whose SLSA v1 predicate names `workflow.repository`, so they are checked exactly like npm provenance (Task 0 ruling 5). The GitHub MCP fixture carries v2.0.3's real checksums and commit.
9. **Where the checks live.** The schema tests are in the new crate `crates/loams-agentd-marketplace`, which compiles the schema in (`listing::validate`), on the workspace's new `jsonschema` 0.58 dependency (MIT, the line `fabric/` already uses). CI's `rust` path filter gains `schemas/marketplace/**`. The invalid fixtures are one manifest, `testdata/invalid.json`: each case is a valid fixture plus a JSON-pointer patch that breaks one rule. The same manifest serves Task 2's lint tests.
### Task 2 (2026-10-23)

1. **Package.** The package is `@loams/listing` (private, Apache-2.0) at `agents/loams-listing`, with the bin `loams-listing` and one runtime dependency, `ajv` 8.20.0 (MIT, Draft 2020-12). It reads `schemas/marketplace/*.json` from the repository at run time, so the CLI and agentd always check the same schema. FT0 Task 1 had not merged, so this task adds `agents/*` to `pnpm-workspace.yaml` (ruling 10 of Task 0). It also adds `@loams/listing` to the npm licence gate's default projects, and `agents/**` and `schemas/marketplace/**` to the `web` path filter, whose Nx job runs the package's `typecheck` and `test`.
2. **`lint` runs the schema first**, then the rules a schema cannot express. These are listed by `loams-listing rules`:
   - `namespace-first-party`, only with `--first-party-only`, which is MK0's index policy;
   - unique tool names, config names and environment variables, archive platforms and data scopes;
   - `always_ask` names declared tools only;
   - a tool's `mcp_server` is one of the listing's `mcp_servers`;
   - a remote URL is one of its `server.json` remotes;
   - the `server.json` and npm versions equal the listing's version;
   - config never sets a variable of the environment contract (`LOAMS_*`, `HOME`, `TMPDIR`, the proxies, the model base URLs and keys, `PATH`, `NODE_OPTIONS`, `PYTHONPATH`);
   - the Mastra profile is `node` with TypeScript or JavaScript;
   - a first tool-poisoning check: instruction-like phrases, HTML comments, zero-width and bidirectional characters (§10.2).

   Each issue has a rule, a severity and a JSON Pointer. Only `egress-declared` (egress tools but no hosts) is a warning.
3. **`from-package` derives** these fields from `package.json`, and the `loams.agent` block may not set them:
   - `version`, and `licence` (from `license`);
   - `source` (`repository.url` normalised, `repository.directory`, the commit from `--commit` or `git rev-parse HEAD`);
   - `artifact` (npm, with the integrity from `--tarball`, hashed, or `--integrity`; there is no default, because the listing pins exact bytes);
   - `provenance` (the registry's attestations URL);
   - `runtime` (`node` with `engines.node`, default `>=22.13`);
   - `pricing`.

   `title`, `summary`, `entrypoint` and the framework version fall back to `name`, `description`, `main` and the framework's dependency (for example `@mastra/core`). A remote MCP server has no package, so its listing is written by hand.
4. **Index layout and format.** The input is `listings/<namespace>/<name>/versions/<version>/listing.json`, with an optional `yanked.json` (`{"reason"}`) beside it, and an optional `revocations.json` at the root. The output is two files, `index.json` and `revocations.json`, with schemas `schemas/marketplace/index.v1.json` and `revocations.v1.json`.
   - **The index embeds every listing version in full.** One signature over `index.json` then covers every listing, and a client needs one fetch.
   - Each version records `listing_sha256`, the SHA-256 of its canonical JSON.
   - The index sets `tier`: `first-party` for `ostrium` and `loams`, `community` otherwise, until MK1 adds verification.
   - It sets `latest`: the highest version that is neither yanked nor a prerelease, else the highest that is not yanked.
   - The output is canonical (keys sorted at every level, ids and versions sorted, no timestamps), so the same tree always gives the same bytes. `--source-commit` records the index repository's commit.
5. **Version ranges** in `mcp_servers[].version` and revocations use Cargo's `VersionReq` syntax (comma-separated comparators: `^2.0.0`, `>=0.1.0, <0.1.1`, `=2.0.3`, `*`), so agentd parses them with the `semver` crate unchanged.

### Task 3 (2026-10-24)

1. **Two repositories.** The checks that need code are `loams-listing verify`, here in `agents/loams-listing`. `ostrium-labs/loams-marketplace` holds:
   - the layout, `revocations.json` and the README;
   - `validate.yml`, `publish.yml` and `self-test.yml`;
   - the scripts: `setup-cli.sh`, `changed-listings.sh`, `check-signed.sh` and `report.mjs`;
   - `tests/self-test.sh` with its fixtures.

   It builds `loams-listing` from this repository at the commit pinned in `.github/loams-ref`, a sparse, shallow fetch of `agents/loams-listing`, `schemas/marketplace` and the root lockfile, installed with `pnpm install --frozen-lockfile --filter @loams/listing --ignore-scripts`. A schema or tooling change reaches the index only when that pin moves. Nothing was cloned locally: the files went to `loams-marketplace` through the GitHub API.
2. **`verify`** does the following:
   - **npm:** it resolves `/<name>/<version>` on the registry, requires `dist.integrity` to equal the listing's, downloads the tarball and hashes it again, then reads the attestations.
   - **Archives:** it downloads each platform file and checks its SHA-256. With `github-attestation` it looks the digest up in the repository's attestations API; with `sigstore-bundle` it reads the bundle URL.
   - **Provenance:** a SLSA v1 bundle must cover the bytes (`sha512` for npm, `sha256` for archives) and name the listing's `source.repository` in `workflow.repository`. It must verify cryptographically through `sigstore` 5.0.0 (sigstore-js, Apache-2.0, the public-good trusted root through TUF). It must be signed by a GitHub Actions or GitLab issuer, with a certificate SAN that is a workflow of that same repository.
   - **No artifact:** remote MCP servers and native harnesses pass.
   - **Not yet verified:** `pypi` and `oci` fail until MK1 and MK2.

   It was checked live on 2026-10-24: `sigstore@3.1.0` passes, and the same package listed under another source repository fails `provenance`. The network fetcher refuses non-https URLs and bodies over 512 MiB. The Node engine range follows `sigstore` 5: `^22.22.2 || ^24.15.0 || >=26`; the index workflows use Node 24.
3. **Published versions are immutable.** `changed-listings.sh` passes only added `listing.json` files; a change or deletion fails, with a pointer to `yanked.json` or a new version.
4. **Amends Task 0 ruling 5: no stored provenance copies in MK0.** The verified bundles are uploaded as the pull request's `listing-report` artifact, not committed, and agentd fetches provenance from the registry at install time (Task 6). Copies beside each listing, for installs without registry access, are deferred.
5. **The report** is a Markdown table per added version, written to the job summary and posted as one updated pull-request comment. The comment is posted only for pull requests from this repository, which are all of them in MK0. **LAPC Core is not in `validate.yml` yet**; the report says so, and Task 8 adds the step.
6. **Publishing.** `publish.yml` runs on `main` and does the following:
   - builds with `--first-party-only --source-commit $GITHUB_SHA`;
   - signs both files with `cosign sign-blob --bundle` (cosign v3.0.6, keyless);
   - runs `check-signed.sh`, which verifies both bundles against the pinned identity and refuses a missing or wrong signature (`unsigned_publish_fails` and `wrongly_signed_publish_fails` in `self-test.sh`);
   - uploads the `marketplace-index` artifact (90 days) for `loams-site`;
   - deploys GitHub Pages. GitHub Pages must be enabled with the "GitHub Actions" source.

### Task 4 (2026-10-24)

1. **The official source** is `https://ostrium-labs.github.io/loams-marketplace/index.json`, the GitHub Pages site of `loams-marketplace`, enabled on 2026-10-24 with the Actions source. It is pinned to `…/loams-marketplace/.github/workflows/publish.yml@refs/heads/main` with the GitHub Actions issuer. D1461's `https://loams.dev/marketplace/index.json` replaces the URL when `loams-site` serves the index. The identity does not change.
2. **Files of a source.** `index.json`, `index.json.sigstore.json`, `revocations.json` and `revocations.json.sigstore.json` sit side by side. A missing signature is a refusal (`index_signature_required`), as is a bundle by any other identity or issuer (`index_identity_pinned`). Nothing is parsed before both bundles verify.
3. **The cache** lives at `<agentd data>/marketplace/cache/<source>/`, with the sources in `<agentd data>/marketplace/sources.json`.
   - The cache is verified again on every start, so a file edited on disk is refused.
   - A failed refresh keeps the last verified index and reports the error on the source.
   - The official source is always first and always carries its built-in pin, whatever `sources.json` says.
   - A new source is kept only after its index verifies.
4. **Verification in Rust** uses `sigstore-verify` `=0.14.0` with no default features, so there is no TUF client. It checks against the public-good trusted root embedded in `sigstore-trust-root`, offline, and matches the certificate SAN exactly, whatever its kind. Its tests use a real bundle: cosign v3.0.6's own release bundle for `cosign-linux-amd64`, checked by digest. That bundle verifies for its signer and fails for the marketplace's pinned identity, for the wrong issuer, and for other bytes.
5. **Index parsing** validates every listing against schema v1, and checks each `listing_sha256` against the Rust canonical form, so Rust and `loams-listing` must agree byte for byte. The fixture `testdata/index` was built by `loams-listing index build`. Revocations accept Cargo `VersionReq` syntax. A range covers prereleases inside it (`>=0.1.0, <0.2.0` covers `0.2.0-rc.1`), and a version that is not semantic is treated as revoked (fail closed).
6. **Search** matches every word of the query against the latest version's id, title, summary and tool names. It filters by kind, framework, language, minimum LAPC level, tier and source, and leaves out listings whose versions are all yanked.
7. **Scope of this PR.** It adds:
   - `proto/loams/agentd/v1/marketplace.proto`, with all eleven RPCs of the plan (the install-side messages are used by Tasks 5 and 6);
   - the `loams-agentd-marketplace` core.

   Wiring the service into the daemon is the follow-up, Task 4b. That means ndjson methods in `loams-agentd-proto`, handlers in `loams-agentd-sessions`, the Connect service, `twins.rs` rows and the renderer allowlist, plus an HTTPS `Fetcher` and the 6-hour refresh. Until then `marketplace.proto` is not in `loams-agentd-connect`'s `FILES`, so nothing serves it.

### Task 4b (2026-10-26)

Brought into MS1's scope by D1806 and confirmed by the owner on 2026-10-25. Read at `origin/dev` `bc902971d`.

1. **Where each part lives.**
   - `loams-agentd-marketplace` gains `service.rs`: `MarketplaceHost`, the daemon's marketplace. It owns the sources, the installs under `<agentd data>/agents`, the vault, the refresh and the registrations. It knows neither wire.
   - The crate also gains `fetch.rs` (`HttpsFetcher`) and, behind the `testkit` feature, a signed fake world that the sessions and Connect tests share.
   - The ndjson types are `loams-agentd-proto::marketplace`, and the handlers are `loams-agentd-sessions`' `rpc/marketplace_rpc.rs`.
   - The Connect service is `loams-agentd-connect`'s `services/marketplace.rs` and `map/marketplace.rs`.
   - `listing.proto` and `marketplace.proto` join `FILES`.
   - The daemon opens the host under its own directory (`loams-agentd/src/marketplace.rs`) and advertises the feature `marketplace`.
2. **The ndjson names.** The ndjson method table is flat, so the twins carry the noun: `ListMarketplaceSources`, `AddMarketplaceSource`, `RemoveMarketplaceSource`, `SearchListings`, `GetListing`, `PlanInstall`, `InstallListing`, `UninstallListing`, `ListInstalledListings`, `CheckListingUpdates` and `UpdateListing`. All are `Owner` and `renderer: true`. None is open to the `mcp` role (`marketplace_not_in_mcp_role`): only the person installs.
3. **Two wire forms, one value.**
   - The ndjson wire keeps the listing schema's strings (`mcp-server`, `mastra`, `core`, `destructive`) and carries listing parts (the listing, its Place, models, cost, provenance) as JSON.
   - Connect maps them to the `loams.marketplace.v1` messages through proto3 JSON. Each enum string is renamed by the schema's `x-proto-enum` annotation, and `$`-keys are dropped.
   - The mapping test round-trips a fixture of every new type.
4. **The fetcher.** `HttpsFetcher` uses reqwest 0.12 on rustls, as the other agentd crates fetch. It is `https_only`, with a 120-second timeout and a 512 MiB body cap. It refuses a plain-http URL before any request and treats any non-2xx status as an error. Every byte goes through the `Fetcher` trait, so MS1 Task 5's Walrus location is a second implementation, not a change here. Its tests run against a local TLS server with a test CA.
5. **Refresh and revocations.**
   - The host refreshes every source at start and every 6 hours (`REFRESH_INTERVAL`), and checks the installs against the new revocation lists after each refresh.
   - It also checks them at open, from the verified cache, with no network.
   - A `disable` finding removes that install's harness and MCP server registrations. The install and its `data/` stay (D1472).
   - Tested with paused Tokio time.
6. **Install.**
   - `InstallListing` makes the plan again. The digest must equal the confirmed one, or the call answers `plan_changed`.
   - A required secret must be given, or be in the vault already. Secrets go to the OS keyring (service `dev.loams.agentd`, account `marketplace/<id>/<input>`), never to a file. Non-secret configuration is `config.json` beside the install, and the listing JSON is kept as `listing.json` so registrations survive a restart.
   - `UninstallListing` deletes the install's secrets: they are credentials, not the agent's data. `data/` stays.
   - `UpdateListing` without the new plan's digest answers `consent_required` when the update widens anything (`plan::widens`). It also does when the installed version is no longer in its source.
   - New error reasons: `plan_changed`, `consent_required`, `listing_revoked` and `untrusted_listing`, each `failed_precondition` on Connect.
7. **What installs in Task 4b.**
   - The `node` runtime with an npm artifact: Task 6's installer, with the tarball from `registry.npmjs.org` and the attestations from `/-/npm/v1/attestations/`.
   - Listings with no artifact: remote MCP servers and native harnesses.
   - A `binary` listing with an archive (the index's `vercel-labs/fx` and `ostrium/github-mcp-server`) answers `Unsupported`. The archive installer, which can reuse MS1 Task 3's tar.gz support in `archive_install`, is a follow-up.
   - agentd's npm is `ProcessNpm`: the `npm` agentd finds for its adapters, run with `npm_args` (scripts off), a cleared environment and a cache beside the install. With no npm, an npm install fails with a plain message.
8. **Harness registration** (Task 0 ruling 7, as built).
   - `HarnessId::Marketplace` has wire name `marketplace` and proto value `HARNESS_ID_MARKETPLACE = 13` (after `HARNESS_ID_GOOSE = 12`, MS1 Task 3). `HarnessDescriptor.marketplace_listing` (proto field 9) names the listing.
   - The registry keeps marketplace harnesses beside its id-keyed slots, by listing id. Each is an `AcpHarness::marketplace(AcpLaunch)`, with the owned launch line beside the static specs. A sync keeps an unchanged harness's instance.
   - A marketplace harness's process runs in its install's `data/` directory, not in the project: Bun reads `bunfig.toml` from its working directory (MK0 Task 6, PR #1254), and a project's file must never configure an installed agent's runtime. The project still reaches the agent as ACP `cwd`. Installed MCP servers are spawned by the harness that runs them, so their working directory is that harness's choice until `McpServer` carries one (follow-up).
   - Marketplace harnesses are listed `enabled` (the person installed each by consent). Harness updates skip them, because they update through the marketplace.
   - `harness_descriptor_appears_in_list_harnesses` passes.
   - **Not in this PR:** running a thread on one. That needs the listing id on the chat's config and session record, so dispatch can call `HarnessRegistry::resolve_marketplace`. MS1 Task 2 needs it, and it is the first follow-up.
9. **MCP servers for runs.**
   - `RunRequest` gains `mcp_servers`, which is additive and serde-defaulted. The engine adds every installed local MCP server to every run, with its declared secret variables read from the vault when the run starts. A `node` server runs as `node <entry> <args>`.
   - ACP harnesses pass the servers in `session/new` `mcpServers`. Claude Code, Codex, OpenCode and Pi take only the one injected server today and ignore the list (follow-up).
   - Remote MCP servers wait for agentd's MCP proxy (D1463): they are recorded as `remote_mcp` and not passed.
   - `mcp_server_available_to_threads` passes.
10. **The plan's `version_age_days`** is unset: the index carries no publish date yet.
11. **Generated files.** The TypeScript bindings under `web/packages/agentd-client/src/` and the renderer allowlist `apps/desktop-electron/src/main/agentd/allowlist.gen.ts` were regenerated by their generators (`export_bindings`, `LOAMS_UPDATE_ALLOWLIST=1`), not edited by hand. The Marketplace page itself is MK0 Task 9.

### Task 10 (2026-10-26)

Built on Task 4b's branch (`mk0-t4b-daemon-wiring`).

1. **Where it lives.** The command is `loams marketplace`, as planned, but its code is a new crate, `loams-agentd-cli`. The `loams` binary only adds the subcommand and calls `loams_agentd_cli::marketplace::run`, the way `loams upgrade` and `loams cloud` call their own client crates.
   - The crate links `loams-agentd-proto` and `loams-agentd-rpc`, plus `rpassword`, and no daemon crate. In `loams`' normal tree that adds about 17 small crates (tungstenite 0.24, ts-rs, pulldown-cmark 0.12 and their dependencies) and no engine, harness, store or Sigstore crate.
   - The agentd-deps guard (`scripts/ci/agentd-deps.sh`) checks only that `loams-agentd` stays headless, so this is outside its scope.
   - The `loams-agentd-` prefix puts the crate in CI's `agentd` job, which runs its clippy and tests.
   - Desktop already bundles `loams` beside `loams-agentd` (design 50 §5.2).
2. **Reaching agentd.** The CLI looks for the daemon directory in this order:
   1. `--agentd-dir`;
   2. the directory of `LOAMS_AGENTD_CONFIG`;
   3. `<LOAMS_DESKTOP_USER_DATA>/agentd`;
   4. Loams Desktop's default `<userData>/agentd` (`~/.config/Loams Desktop/agentd`, or under `XDG_CONFIG_HOME`; `~/Library/Application Support/…` on macOS; `%APPDATA%\…` on Windows).

   It reads `agentd.json` and `agentd.token`, dials `ws://127.0.0.1:<port>` with the owner token and says `Hello` as `cli`, all within 5 seconds. It needs the `marketplace` feature in the reply.
   - It does not check the lock holder as `loams-agentd status` does, because that check lives in the daemon crates. `Hello` with the token is the check: a stale file names a port that does not answer.
   - Only the local daemon is supported. A remote agentd (`serve --remote`, OIDC over Connect) is not.
3. **Consent.** `install` prints the `PlanInstall` plan in plain words, then asks "with exactly these permissions? [y/N]". The plan covers:
   - tools by risk, most dangerous first, with "always asks you first";
   - data scopes, egress hosts, roles and MCP servers (reused or installed);
   - models, proposed budgets and the "can't" list;
   - secrets, licence, tier, provenance, the source repository and the version's age;
   - the digest.

   Only `y` or `yes` confirms. `InstallListing` is then called with that digest, pinned to the plan's version and source.
   - Scripts use `--dry-run` (text or `--json`) to see the plan and its digest, then pass `--yes --plan-digest <digest>`. clap refuses `--yes` without a digest, and a digest that is no longer the plan's is refused before any call, as `plan_changed`.
   - `--json` installs only with `--yes`, so no prompt is mixed into JSON.
   - With no terminal and no `--yes`, the command refuses.
   - A declined plan creates nothing, and no secret is asked for before the person confirms.
4. **Secrets** are asked for after the person confirms, without echo, through `rpassword` 7.5 (Apache-2.0, a new dependency). The only other source is `--secret NAME=@FILE` (or `@-` for stdin, once). A literal `--secret NAME=value` is refused because it would sit in shell history and `ps`, and so is a name the plan does not ask for. An empty answer sends nothing, so a secret already in the vault is kept.
   - Required non-secret settings with no default are read from `GetListing`'s listing. They are asked for interactively, or required as `--config NAME=VALUE` with `--yes`.
5. **Update.**
   - `update` with no id lists `CheckListingUpdates`.
   - `update <id>` calls `UpdateListing`. On `consent_required` it prints the daemon's widening list and the new plan from `PlanInstall` (at the installed source), asks, and retries with that plan's digest.
   - Without a terminal, or with `--json`, it refuses and points to `update <id> --dry-run`, then `--yes --plan-digest`.
   - An update that does not widen anything goes ahead without asking (D1466).
   - `uninstall` asks, or takes `--yes`.
6. **Output.** Text by default. With `--json`, the ndjson replies are printed as they are (`ListingSearchResults`, `ListingDetail`, `PlanInstallReply`, `InstalledListingReply`, `InstalledListingList`, `AvailableUpdateList`, `MarketplaceSourceList`). Errors go to stderr with their daemon reason prefix (`plan_changed`, `consent_required`, `listing_revoked`, `untrusted_listing`, `not_found`), and the exit code is 1.
7. **Tests.** `crates/loams-agentd-cli/tests/marketplace.rs` starts the sessions engine with a `MarketplaceHost` on Task 4b's testkit world and serves it on a real loopback `WsServer` with an owner token. It writes `agentd.json` and `agentd.token`, then runs parsed `loams marketplace …` command lines through discovery, `Hello` and the ndjson methods, with a scripted terminal. The tests cover:
   - search and info;
   - a dry run that creates nothing;
   - an interactive install, a declined one, and refusal without a terminal;
   - a stale digest and `--yes` without a digest;
   - secrets from files, and refusal of literal secrets;
   - a widening update, interactive and scripted;
   - uninstall and sources.

   `loams`' own `marketplace_parses` checks the subcommand's wiring.
8. **Not in this task (follow-ups).**
   - Proposed budgets are shown but not editable, because `InstallListingParams` has no budget field. The console (Task 9) or a later wire change edits them.
   - `UpdateListing` takes no secrets, so an update that adds a required secret fails with `bad_params` until the wire carries them.
### Task 9 (2026-10-26)

Built on Task 4b's branch (`mk0-t4b-daemon-wiring`), stacked until it merges.

1. **Where it lives.** `web/plugins/marketplace` (`@loams/plugin-marketplace`, `modes: ["console", "studio"]`, first-party, `agentd:use`, every edition), in the console's base catalog. The same pages answer at `/marketplace…` in Console (nav group Integrate) and `/studio/marketplace…` in Studio (nav group Studio): browse, `listing`, `install` (the consent screen), `installed` and `sources`. A listing id has a `/`, so ids travel in the query (`?id=ostrium/github-mcp-server&source=official`). Headings say "The Seed Exchange" with an "Experimental" badge; the body says first-party listings only.
2. **No thread on a marketplace harness yet.** Task 4b ruling 8 leaves running a thread on one to a follow-up. Studio's harness picker lists installed Seed Exchange harnesses as disabled entries ("threads on it come later") and ends with "Add a harness…", which opens `/studio/marketplace?kind=harness`. The Playwright check (`web/apps/console/test/e2e/marketplace.spec.ts`, `marketplace_install_harness_appears_in_picker`) covers the picker link, the consent screen, that nothing is installed before Install, the install, and the harness in the picker. Opening a thread with it joins the spec with Task 4b's first follow-up.
3. **One value, two wires.** The pages are written against the ndjson types in `@loams/agentd-client`; the console reaches agentd over Connect, and `src/wire.ts` maps `MarketplaceService`'s messages to them by the rule of `map/marketplace.rs` (enum prefix stripped, lower-cased, `_` as `-`; listing parts as proto3 JSON under proto field names). A test maps a Connect plan back to the `PlanInstallReply` fixture. An unspecified tier shows as Community, never First-party. `@loams/proto/agentd` now exports `MarketplaceService`, and `@loams/proto/marketplace` the listing types.
4. **Stale generated TypeScript.** The Task 4b branch had not regenerated `@loams/proto`'s `studio_pb.ts`, `harness_pb.ts` and `threads_pb.ts` (`HARNESS_ID_MARKETPLACE`, `marketplace_listing`). They are regenerated here with `pnpm --filter @loams/proto generate`.
5. **Which agentd on the web.** The console's `agentd` reports no host in a browser (only Studio's host registry knows the host and its pasted token). A new single slot, `studio.agentd.scope` (`@loams/slots`), is registered by Studio and renders its children under Studio's `AgentdScope`; the Seed Exchange renders through it, and falls back to the console's `agentd` (the desktop's daemon) while Studio has not loaded. When the web host registry becomes `agentd.endpoint` (SD1 Task 7's gap), the slot can go.
6. **The consent screen** shows every part of the plan: data scopes, tools by risk (most dangerous first, with "Asks you every time" and the MCP server), egress hosts and roles as "It can"; `cannot` plus "Call any tool not listed here" as "It can't"; the always-ask approvals; dependencies (reused or installed with it); secrets to supply (password fields; they go to agentd's vault); models; budgets; licence, tier, provenance, source repository and the version's age ("The index does not say…" while it is unset, Task 4b ruling 10). Install sends `planDigest: plan.digest` and nothing else that agentd could re-plan. `plan_changed`, `consent_required`, `listing_revoked` and `untrusted_listing` are said in plain words; the first two offer "Read the new plan". A `disable` revocation disables Install.
7. **Budgets are read-only.** §7.2 says the proposed budgets are editable, but `InstallListing` carries no budget. The screen shows them as proposed; editing waits for agentd's per-install budgets in the model proxy (D1468). Non-secret configuration is sent empty: `PlanInstall` carries only secrets.
8. **Updates.** An update that does not widen is sent from the Installed page without a digest. One that widens opens the consent screen in update mode (`?update=1`), which sends `UpdateListing` with the new plan's digest. After an install, update or uninstall the page also invalidates Studio's `ListHarnesses`, so the picker shows the change.
9. **Sources.** The official source cannot be removed from the page. Adding a source needs a name, the index URL and the signing identity; the issuer defaults to GitHub Actions'.
10. **The e2e daemon.** A debug `loams-agentd` built with the new `e2e-marketplace` feature, with `LOAMS_AGENTD_TEST_MARKETPLACE` set, opens the testkit's world (the signed fake official index with `testkit/echo-acp`, stand-in signatures, a fake npm, a memory vault) instead of the network. This is the same pattern `LOAMS_AGENTD_TEST_ISSUER` uses for tokens. The feature enables `loams-agentd-marketplace/testkit`, and no release build enables it, so the testkit is never in a shipped daemon. The code is also behind `cfg!(debug_assertions)`. `studio-e2e.yml`'s web job builds `cargo build -p loams-agentd --features e2e-marketplace`; with any other daemon the spec skips itself. The workflow also runs on changes to `web/plugins/marketplace` and `web/packages/slots`.
### Task 8 (2026-10-26)

Read at `origin/dev` `817d13814`. Brought into scope beside Task 4b; it does not need `@loams/mastra-resonate` (its fixtures are local packages).

1. **Where it lives.** The runner is `crates/loams-agentd-marketplace/src/conformance/` and the command is `loams-agentd agent conformance <package-dir|listing.json> [--package <dir|tgz>] [--out <report.json>] [--sign]`, in the daemon's own binary (§73 §9 says "in agentd"; the plan's `loams agent conformance` becomes `loams marketplace` in Task 10). It exits 0 when the package reaches Core, 1 when it does not, and 2 when the run could not happen (an invalid listing, no Node, a package that does not install). A summary goes to stderr and the report to `--out`, else stdout.
2. **What it runs.** It runs agents that serve ACP and ACP harnesses, on the `node` runtime (MK0), on Linux and macOS. A2A agents, other runtimes and Windows answer "cannot run" (exit 2) until MK1. The listing must pass schema v1 first, and the Node it finds must satisfy `runtime.version`.
3. **How it installs.**
   - A directory holds `listing.json` and is the package.
   - A listing file uses `--package` (a directory, or a tarball checked against `artifact.integrity`), else `npm pack <name>@<version>` from the registry, also checked.
   - Tarballs install with Task 6's `npm_args` (scripts off) into `<work>/install`. A directory without dependencies is copied, leaving out `node_modules`, `.git` and symbolic links, so no npm is needed. One with dependencies is packed with `--ignore-scripts` first.
   - Provenance is not checked here. `loams-listing verify` (Task 3) does that before the index CI runs LAPC.
4. **Reuse.** The ACP client is agentd's own: `loams-agentd-harness::jsonrpc` (`RpcClient`, `Incoming`) is now public, and `loams-agentd-marketplace` depends on `loams-agentd-harness`. The harness does not depend on the marketplace, so there is no cycle. Task 0 ruling 6's matching (the declared name against the call's `title`, then its `kind`) is `conformance::declared_tool`, for Task 6's `undeclared_acp_tool_refused` to reuse. C8 reuses `install.rs`'s install-script and `hasInstallScript` refusal.
5. **Isolation in MK0** is the process only, with no sandbox until SB1:
   - the environment is exactly `contract::environment` (cleared, then only the contract's variables);
   - `HOME`, `TMPDIR` and `LOAMS_DATA_DIR` are a fresh data directory, which is also the working directory;
   - the agent leads its own process group.
   - Loams services are not provided in a Core run, so their variables are absent. `--pg-url` and `--durable-url` pass a local Loams's when the CI has one.
   - Secret configuration inputs get a dummy value, and other inputs their default.
6. **The agent's world** is one loopback HTTP server:
   - **The scripted model** speaks OpenAI Chat Completions only (`/openai/v1/chat/completions`, plain or `stream: true`). A call to another API shape (Responses, Anthropic Messages) is answered 501 and fails C3. The other shapes come with the model proxy (§3.4).
   - **The script.** The model calls each declared tool the agent offers, always-ask tools first, at most four, with arguments built from each tool's JSON Schema. It then ends with a fixed reply.
   - **Tokens** are estimated at four characters a token, the same figure the model reports in `usage`.
   - **Fake MCP servers.** There is one per `mcp_servers` dependency, at `/mcp/<name>` in `LOAMS_MCP_CONFIG`, and each lists exactly the tools the listing declares from it.
   - **The egress proxy** (`HTTPS_PROXY`, `HTTP_PROXY`) records every host asked for and forwards none, declared hosts included, so runs never touch the network. `LOAMS_ENDPOINT` answers 503.
7. **The checks** (a skipped MUST is not a pass; Core means every Core MUST passed):
   - **C1:** `initialize` answered within 30 s.
   - **C2:** the agent's `protocolVersion` equals `surfaces.acp.protocol_version` (default 1), and `session/new` opens a session. A listing that also declares `a2a`, `mcp` or `responses` gets C2 `skip` in MK0.
   - **C3:** the turn ends with `end_turn`; at least one request reached the scripted model; its final reply reached the client; nothing went to an unsupported model API or, through the proxy, to a model provider's host; and `models.api` declares `chat-completions` when the agent called it.
   - **C4:** every permission request, `tool_call` update, fake-MCP call and tool offered to the model names a declared tool. Undeclared permission requests are rejected. Every always-ask tool the script ran was asked about before its result reached the model and before the MCP call that ran it.
   - **C5:** every host through the proxy is declared (`*.` covers subdomains). On Linux the agent's TCP sockets are also sampled every 50 ms through `/proc`, and any non-loopback peer, which went around the proxy, fails. macOS has the proxy record only; the report's `runner.egress_recording` says which.
   - **C6:** nothing changed under the run's directory outside `data/`, which includes the installed package. On Linux, open files are sampled: one held for writing outside the data directory fails, as does one read under the user's real home (outside the run and the Node installation). Descriptors the agent inherited from the runner (same number, same file) are not counted. Short-lived opens can be missed; the sandbox makes this complete (SB1).
   - **C7, cancel:** a second turn's model call is held, and `session/cancel` is sent once the agent waits on it, or after 2 s. The turn must answer `cancelled` within 10 s. A probe turn that ends before it calls the model passes with a note.
   - **C7, SIGTERM:** SIGTERM goes to the agent's process only. It must exit within 10 s, and within the same 10 s no process may remain in its group or, on Linux, among the descendants seen during the run.
   - **C8:** the installed tree has no `preinstall`, `install` or `postinstall`, no native build and no `hasInstallScript`.
   - **C9 (SHOULD):** `cost.typical_tokens_per_run` is within 2× of the scripted turn's tokens, and the turn does not exceed `max_tokens_per_run`. No cost profile is a C9 failure, which does not change the level.
8. **The report** follows `schemas/marketplace/conformance-report.v1.json` (`$id` `https://loams.dev/schemas/conformance-report/v1.json`; `conformance::validate_report`). It holds the runner, its isolation and egress recording; the subject with the listing's canonical SHA-256 (as the index records it); each check with its details; and the measured tokens, tool calls, egress and the tail of stderr.
9. **`--sign`** writes `<out>.sigstore.json` by running `cosign sign-blob --yes --bundle`, keyless, with the ambient OIDC identity of the CI. It needs `--out` and `cosign` on PATH. It was not run locally (there is no cosign or OIDC here); the index CI step exercises it. Adding the LAPC step to `loams-marketplace`'s `validate.yml` is a follow-up there, together with a move of its `.github/loams-ref` and a pinned `loams-agentd` build.
10. **Fixtures and tests.** `testdata/conformance/agent/` is one Node package with no dependencies. `lib.js` is a small ACP agent; `good.js` conforms; and `c1-no-initialize.js`, `c2-protocol-mismatch.js`, `c3-no-model.js`, `c4-undeclared-tool.js`, `c4-skips-ask.js`, `c5-egress.js`, `c6-writes-package.js` and `c7-hang.js` each break one requirement. A failing case is the good `listing.json` with that entry file. C8's fixture is `c8-install-script/package.json`, laid over a copy of the package (an install script needs its own `package.json`). C9's fixture is the good listing with an inflated `cost`.
    - The tests in `tests/conformance.rs` are one pass and one fail per requirement, among them `lapc_c4_undeclared_tool_fails`, `lapc_c5_egress_fails` and `lapc_c7_hang_fails`, plus `lapc_c4_unasked_tool_fails`.
    - Each failing case also asserts that every other Core MUST passes.
    - They skip when `node` is not on PATH.

### Owner approvals (2026-10-24)

The owner approved the pending decisions below on 2026-10-24 ("approve all with your recommendations").

- **The official index stays on GitHub Pages for now.** D1461's move to `loams.dev/marketplace` comes later. **Approved (owner, 2026-10-24).**

### Status (2026-10-24, paused by the owner)

| Task | State | Where |
|---|---|---|
| 0 | Merged | #1126 |
| 1 | This PR | `mk0-t1-listing-schema`. `loams.marketplace.v1` is a data-model package: added to design 44 §7.2 and to the facade generator's `NOT_SDK_PACKAGES`, so no SDK lists it |
| 2 | Built and tested (23 vitest tests), unmerged | branch `mk0-t2-listing-cli`, stacked on Task 1 |
| 3 | Built and tested, unmerged | branch `mk0-t3-index-ci` (`loams-listing verify`, 35 vitest tests, checked live against npm), and `ostrium-labs/loams-marketplace` PR #1 (workflows, scripts, self-tests; `self-test` green). Its `.github/loams-ref` must move to Task 3's merge commit before it merges. GitHub Pages is enabled (Actions source) |
| 4 | Built and tested (`cargo test -p loams-agentd-marketplace`), unmerged | branch `mk0-t4-marketplace-service`: `marketplace.proto`; sources, signed index (real Sigstore verification, `sigstore-verify` 0.14.0), revocations, search. Daemon wiring is the follow-up Task 4b (ruling 7 of Task 4) |
| 5 | Built and tested, unmerged | branch `mk0-t5-install-plan`: `plan.rs` (consent plan, digest, widening) and `contract.rs` (environment, token rotation, MCP config, Postgres role) |
| 6 | Built and tested (53 crate tests in all, real npm provenance), unmerged | branch `mk0-t6-installers`: `provenance.rs` and `install.rs` (npm installs with scripts off, script and lockfile refusal, uninstall keeps data, harness and MCP registrations). Registering them in agentd's `HarnessRegistry` and MCP runs (`harness_descriptor_appears_in_list_harnesses`, `mcp_server_available_to_threads`) waits for Task 4b's daemon wiring |
| 4b | This PR (2026-10-26) | branch `mk0-t4b-daemon-wiring`: the daemon wiring, rulings above |
| 7 | Blocked | waits for `@loams/mastra-resonate` on npm (Task 0 ruling 2), an owner step |
| 8 | Merged | #1284: the LAPC Core runner and `loams-agentd agent conformance`, rulings above. The `validate.yml` step in `loams-marketplace` is the follow-up (ruling 9) |
| 9 | In review | #1268, stacked on 4b |
| 10 | In review | #1277, stacked on 4b |
| 11 | Listings merged; docs in review | `ostrium-labs/loams-marketplace` #2 (native harnesses, GitHub and Plane MCP servers); docs #1272. The Mastra-agent listings wait for Task 7 |

Each branch is stacked on the one before. To resume, merge Task 1, rebase each branch onto `dev` in order, and open one PR per task. Label Tasks 3, 4 and 6 `review:greptile`.

### Task 11 (2026-10-26)

1. **Two pull requests.** The docs (`docs/marketplace/README.md`, the note at the top of `publishing.md`, and the plans README row) are in this repository. The listings are a pull request in `ostrium-labs/loams-marketplace`.
2. **What is listed now:** agentd's built-in harnesses as native harness listings, GitHub's MCP server (the archive runtime), and Plane's remote MCP server. The GitHub listing is valid and published, but agentd cannot install it until the archive installer lands (Task 4b ruling 7).
3. **Blocked:** `ostrium/loams-bot`, the factory's agents, `ostrium/loams-starter` and `ostrium/support-agent`. They are Mastra agents, so they need `@loams/agent-host` (Task 7), which needs `@loams/mastra-resonate` on npm (Task 0 ruling 2, an owner step). They are listed when Task 7 merges.
4. **The docs name the soil name** ("the Seed Exchange") in headings and the technical name in body text, as the site does. They describe each listing kind with its MK0 status rather than as shipped.
