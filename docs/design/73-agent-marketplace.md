# 73 — The Loams Agent Marketplace: Any Framework, Mastra First

> **Amendment (approved 2026-10-23) by [§78](78-loams-cloud-on-tidb-x.md)** (Loams Cloud on TiDB X; D1700–D1735). D1461's rationale ("Ostrium hosts only the console and auth") reads "plus the Loams Cloud try-out (D1701)"; the index decision itself is unchanged.

> **Amendment (approved 2026-10-21) by [§76](76-identity-and-authorization.md)** (identity and authorization: Zitadel and OpenFGA; owner, all defaults). An installed marketplace agent becomes one Zitadel machine user (`agent:<id>`) in the installing org with no grants until someone grants them; inside a sandbox it still holds only agentd's 15-minute Loams token (D1453), whose `sub` is that machine-user id (D1607, D1618).

> **Amendment (approved 2026-10-23) by [§79](79-void-and-vite-plus.md)** (Void and Vite+; D1770–D1791 approved by the owner). JavaScript agent templates (D1458) are Vite+ projects (D1777). A listing that uses Void's Cloudflare-only APIs shows "Cloudflare only" (D1786).

> **Addendum (2026-10-25), [§18](#18-ms1-addendum-2026-10-25)** (the marketplace seed, plan [MS1](../plans/2026-10-25-ms1-marketplace-seed.md); D1806–D1814, Q1330–Q1332). `quickjs` becomes the default self-hosted agent runtime and Mastra runs on Node or Bun (both amend D1452); Goose is a first-party ACP harness; Walrus is a public read-only tier.

Status: **Approved** · 2026-10-21. The owner approved the design with every default on 2026-10-21 (and confirmed that "fx.sh" means Vercel Labs' fx, Q1142).

> "write the factory loop design and the agent marketplace will be based on our mastra integration, user can create agents publish to our marketplace etc, mastra uses loams postgres, live, engine, make mastra as our agent integration layer etc" (owner, 2026-10-21)

Approved the same day, from the coordinator's recommendations: the marketplace's first listings are **agents**, **harnesses** and **MCP servers**; the marketplace is **open source**, not private (D1119 removed open core); the primary call to action on loams.dev stays "Download Loams Desktop".

**Refined by the owner the same day:** the marketplace must support **any agent framework, in any language**, not only Mastra. The owner named Vercel's eve and its durable workflow toolkit, fx.sh, and Python frameworks such as Pydantic AI, LangGraph, CrewAI, the OpenAI Agents SDK, Google ADK and Microsoft Agent Framework. **Mastra stays the first-class, deepest integration and the reference implementation; the package format, the marketplace and the runtime are framework-neutral.**

Companion: [§72](72-software-factory-loop.md), the Software Factory loop. Builds on [§66](66-loams-mastra.md) and plan [MA1](../plans/2026-10-20-ma1-loams-mastra.md) (Loams × Mastra, D1202–D1227), [§53](53-studio-ade.md) (Studio and agentd, whose `harness.proto` is the harness catalog), [§60](60-places-and-harness-composition.md) (Places), [§61](61-loams-jailer.md) (the Jailer), [§56](56-loams-sandbox.md) (the sandbox), [§63](63-loams-meter.md) (the Meter), [§65](65-loams-operator.md) (the operator conformance philosophy this borrows), [§33](33-connectors.md) (the connector registry), [§44](44-unified-api-and-sdks.md) (the SDKs and the compatibility surfaces), the console plugin host (D423), §71 (app BYOC, open PR #979), and the old marketplace design, [platform doc 05](../platform/design/05-hosted-software-factory-and-marketplace.md), with the draft [publishing guide](../marketplace/publishing.md).

Decisions **D1445–D1479** and questions **Q1122–Q1144** are declared in the [decision log](13-decision-log.md). Plan stub: [MK1](../plans/2026-10-21-mk1-agent-marketplace.md), track MK. **No code is written by this document.**

Markers as in §72: **(verify)** means the plan's Task 0 checks it first; **(estimate)** means computed, not measured; status labels **Shipped**, **Built, unreleased**, **Designed**, **New**. Vendor and project facts were read on 2026-10-21 from the page linked beside each one.

---

## 1. Summary

### 1.1 The answer in one paragraph

**The marketplace is a signed, open, metadata-only index of listings, kept in a public Git repository and served as static files; the code stays on npm, PyPI, OCI registries and GitHub Releases, signed with provenance.** A listing is an **agent** (in any framework or language, packaged for a `node`, `python`, `oci` or `binary` runtime), a **harness** (a coding agent that agentd drives, usually over ACP) or an **MCP server** (an official-MCP-Registry `server.json` with a small Loams block). Every agent package declares the same things in a framework-neutral manifest: its runtime and entrypoint, the **protocol surfaces** it serves (ACP for interactive use in Studio, A2A for networked use, optionally MCP and Open Responses), its tools with their risk, the MCP servers and connectors it needs, the Place it asks for, the models it needs and its cost profile. **`loams-agentd` is the one runtime**: it installs a package, starts it in the sandbox with a fixed environment contract (Loams endpoints, short-lived credentials, a model proxy, its MCP servers), and shows it in Studio; Places, approvals and later the Jailer bound what it can do. **Mastra is the first-class path**: a Mastra agent on `@loams/mastra` gets Loams Postgres, the engine, Live, Streams, Resonate, House and Loams Memory wired in by the host, and is marked **Loams-native**. Agents in other frameworks reach Loams through the compatibility surfaces those frameworks already support (Postgres checkpointers and sessions on Loams Postgres, vector stores on the Qdrant API, Resonate SDKs, the 13 Loams SDKs, `loams-mcp`) and are marked **Loams-compatible**. A small **agent package conformance** suite, in the style of §65's operator conformance, proves that a package runs, declares its permissions honestly and exits cleanly. Ostrium serves only the official index, as static files beside loams.dev; anyone can host another. Everything is free; paid listings are a later owner decision, with only a hook into the Meter reserved.

### 1.2 Phase order

| Phase | Scope | Effort (estimate, engineer-weeks) |
|---|---|---|
| **MK0: Mastra first, first-party only** | The framework-neutral listing schema v1; the index repository, CI and signed static index; `MarketplaceService` in agentd; the `node` runtime with ACP; `@loams/agent-host` (the Mastra profile) and `@loams/mastra-acp`; installers for agents, harnesses and MCP servers; the conformance runner at the Core level; the console and Studio pages and CLI; first-party listings | **9** |
| MK1: any framework, third-party | The `python` runtime; the A2A surface through agentd's bridge; the model proxy; Loams-compatible recipes, templates and conformance cells for the first frameworks (Q1133); namespace verification and the scanning pipeline; tiers and revocation operations; third-party agents only inside SB1's sandbox | 12 |
| MK2: more targets and kinds | The `oci` runtime; BYOC server `agents` profile and app BYOC (§71); `loams-registry` with a publish API and MCP-Registry compatibility; curated MCP Registry import; templates, workflows, console plugins, connectors and factory apps as kinds | 9 |
| MK3 | Paid listings, only if the owner decides (Q1127) | — |

---

## 2. Any framework, Mastra first (D1445, D1447, D1449)

### 2.1 Principles

1. **The runtime is framework-neutral.** agentd runs a process (or a container) that speaks a declared protocol. It never imports a framework.
2. **The package format is framework-neutral.** One manifest for every agent; a `framework` field describes, it does not select code paths, except for the Mastra profile (§3.5).
3. **Mastra is the reference.** Loams builds and maintains the deepest integration for Mastra (owner, 2026-10-21) and uses it for its own agents (§72). Other frameworks get documented recipes, templates and conformance cells, not bespoke runtimes.
4. **Use what each framework already has.** A LangGraph agent keeps LangGraph's Postgres checkpointer; Loams supplies the Postgres. Loams writes adapters only where a framework has no open extension point that a Loams surface can satisfy.

### 2.2 Mastra: the reference integration (D1445, D1450)

| Mastra extension point | Loams | Package | Status |
|---|---|---|---|
| Storage (threads, messages, workflow snapshots, scores) | Loams Postgres | `@mastra/pg` unchanged | **Built** (gate 3316/3316) |
| | Engine collections | `@loams/mastra/storage` (`LoamsStore`, D1217) | Designed (MA2) |
| Vectors and hybrid search | The engine: the Qdrant API today; native hybrid vector + BM25 with RRF | `@mastra/qdrant` unchanged; `@loams/mastra/vector` (`LoamsVector`, D1216) | Built (224/235, 11 expected failures); Designed (MA2) |
| Durable workflows | Loams Durable (Resonate) | `@loams/mastra-resonate` | Built, npm publish pending |
| PubSub (workflow events, streaming) | Loams Streams | `@loams/mastra/pubsub` (D1218) | Designed (MA2) |
| **Reactive state** that UIs watch | **Loams Live** | `@loams/mastra/live` | **New** (D1450) |
| Cache | Engine collections | `@loams/mastra/cache` (D1219) | Designed (MA2) |
| Traces, scores, feedback | House through a stream | `@loams/mastra/observability` (D1220) | Designed (MA2) |
| Memory | Loams Memory as processors and tools | `@loams/mastra/memory` (D1221) | Designed (MA3, after MM1a) |
| Tools | `loams-mcp`, `loams-agentd-mcp`, app MCP servers | `@mastra/mcp`'s `MCPClient` unchanged (D1222) | `loams-agentd-mcp` shipped; `loams-mcp` missing (#847) |
| Running in Studio | agentd's ACP harnesses | `@loams/mastra-acp` | **New** (§72 D1426) |

**Live's role (D1450).** Live is Loams' reactive database: a client subscribes to a query and receives the new result when the data changes. MA2 gains a `live` subpath that mirrors an agent's run state (current step, waiting approvals, last output, spend) into Live tables keyed by run, so the console, Studio and phones show it without polling, and gives tools a typed Live client. `LoamsPubSub` stays on Streams (D1218), which gives Mastra's own events ordered, replayable delivery; Live is the UI-facing state. This amends D1214 by adding the subpath (Q1137).

No Mastra fork, no `ee/` or Elastic-2.0 code (D1203), and no Mastra deployer of ours (D1226 stands).

### 2.3 Other frameworks: how they use Loams (D1457)

The paths below use Loams' existing compatibility surfaces (Postgres wire, the Qdrant and Elasticsearch APIs, the Resonate HTTP protocol, MCP, §44 §1 item 3) and the SDKs in 13 languages (`sdks/`: C++, C#, Dart, Go, Java, Kotlin, Objective-C, PHP, Python, Ruby, Rust, Swift, TypeScript). Each "Loams path" becomes a recipe, a template where marked, and a conformance cell (§9) in MK1 or later; until its cell passes, it is **(verify)**.

| Framework | What it is (read 2026-10-21) | Loams path |
|---|---|---|
| **Mastra** | TypeScript; Apache-2.0 core, `ee/` source-available ([repo](https://github.com/mastra-ai/mastra)) | §2.2, Loams-native |
| **Pydantic AI** | Python, MIT, about 20.8k stars, `pydantic-ai` 2.59.0 on PyPI ([repo](https://github.com/pydantic/pydantic-ai)); durable execution through Temporal, DBOS, Prefect or Restate ([docs](https://pydantic.dev/docs/ai/capabilities/durable_execution/overview/)) | Message history in Loams Postgres (the application stores it, as the framework expects); DBOS on Loams Postgres for durability **(verify)**; vectors through the Qdrant client; MCP toolsets for `loams-mcp`; A2A surface. Template in MK1 |
| **LangGraph** | Python (and JS), MIT, about 43.6k stars, `langgraph` 1.2.16 ([repo](https://github.com/langchain-ai/langgraph)) | `PostgresSaver` and `PostgresStore` from `langgraph-checkpoint-postgres` 3.2.0 (2026-10-14, [PyPI](https://pypi.org/project/langgraph-checkpoint-postgres/)) on Loams Postgres; `langchain-qdrant` on the Qdrant API; MCP adapters. Template in MK1 |
| **CrewAI** | Python, MIT, about 59.9k stars, 1.15.x ([repo](https://github.com/crewAIInc/crewAI)); memory defaults to ChromaDB and SQLite, Qdrant supported since 1.12 ([changelog](https://docs.crewai.com/v1.12.0/en/changelog.md)) | Memory on the Qdrant API; no native Postgres backend found, so custom storage on Loams Postgres or Loams Memory's MCP tools **(verify)** |
| **OpenAI Agents SDK** | Python and JS, MIT ([Python](https://github.com/openai/openai-agents-python), [JS](https://github.com/openai/openai-agents-js)); sessions in SQLite, SQLAlchemy, Redis or MongoDB ([docs](https://openai.github.io/openai-agents-python/sessions/)); MCP built in | `SQLAlchemySession` on Loams Postgres (Python); MCP for `loams-mcp`; the model proxy (§3.4) speaks the Responses shape it expects |
| **Google ADK** | Apache-2.0; Python, TypeScript, Go, Java, Kotlin ([adk.dev](https://adk.dev)); `DatabaseSessionService` on any relational database including Postgres ([sessions](https://adk.dev/sessions/session/)); A2A in Python, Go and Java **(verify)** | `DatabaseSessionService` on Loams Postgres; A2A surface; MCP tools |
| **Microsoft Agent Framework** | MIT; .NET and Python stable, Go in preview; 1.0 GA 2026-04-03 **(verify)**; successor to Semantic Kernel and AutoGen; MCP and A2A native ([repo](https://github.com/microsoft/agent-framework)) | A2A surface; Loams' C# and Python SDKs; Postgres through its own providers **(verify)** |
| **Vercel eve** | TypeScript, Apache-2.0, an agent as a directory of files (model, instructions, tools, skills) with durable execution, sandboxing and approvals; launched 2026-06-17, public preview **(verify)** ([repo](https://github.com/vercel/eve), [The New Stack](https://thenewstack.io/vercel-launches-eve-an-open-source-framework-that-treats-agents-as-directories/)) | `node` runtime; its serving surface and durable backend are a MK1 Task 0 spike **(verify)**; an ACP or A2A wrapper if it serves neither |
| **Vercel Workflow DevKit** | TypeScript, Apache-2.0, `"use workflow"` / `"use step"` durable functions ([repo](https://github.com/vercel/workflow), [docs](https://workflow-sdk.dev)); `DurableAgent` is deprecated in favour of the AI SDK's `WorkflowAgent` ([docs](https://workflow-sdk.dev/docs/api-reference/workflow-ai/durable-agent)) | A durability library rather than an agent framework: agents built on it run in the `node` runtime; whether its storage backend can be pointed at Loams Postgres is a spike **(verify)** |
| **fx** (fx.sh) | The best match is **fx by Vercel Labs**: a small coding-agent harness in Zig, Apache-2.0, one binary with a shell CLI, a JSON mode, an **ACP server** and a WASM SDK; v0.0.14, experimental ([fx.sh](https://fx.sh)). It is a harness, not a general framework; the owner confirmed on 2026-10-21 that this is the fx meant (Q1142) | A **harness listing** (ACP, `binary` runtime) in MK1 |
| **Docker Agent** (formerly cagent) | YAML-defined agents, loadable from OCI references ([docs](https://docs.docker.com/ai/docker-agent/)) | `oci` runtime (MK2); its MCP mode as a surface |

**Durable execution outside Mastra.** Loams Durable speaks the Resonate HTTP protocol so unmodified Resonate SDKs work (D1). Resonate has SDKs for TypeScript, Python, Rust, Go and Java, all pre-1.0 ([resonatehq](https://github.com/resonatehq)). The TypeScript SDK is proven against Loams Durable by `loams-mastra`'s engine tests; the Python SDK is the next cell (Q1143), because Loams' newer server already differs from upstream v0.9.8 in ways the TypeScript work found (explicit ids, local-scope listeners). Frameworks with their own durability (LangGraph checkpoints, Pydantic AI on DBOS, ADK sessions) need only Loams Postgres.

### 2.4 Integration levels and badges (D1449)

| Level | Means | Proved by |
|---|---|---|
| **Loams-native** | Mastra on `@loams/mastra` and `@loams/mastra-resonate` through the Mastra profile (§3.5): Live state, House traces, Loams Memory, durable resume | Conformance Core + Compatible + Native |
| **Loams-compatible** | Any framework whose state lives in Loams through a declared capability (§2.3) | Conformance Core + Compatible |
| **Runs on Loams** | Any agent that passes Core; keeps no state in Loams or keeps it elsewhere with the user's consent | Conformance Core |

Listings show the level, the `framework` and the `language` as badges and filters.

---

## 3. The agent package format (D1451–D1456)

### 3.1 Choosing the invocation protocols (D1451)

| Protocol | What it gives | Adoption (read 2026-10-21) | Choice |
|---|---|---|---|
| **ACP** (Agent Client Protocol) | Local, stdio, interactive sessions with streaming updates, tool-call reports and **permission requests** | Zed's protocol; agentd already runs ACP harnesses; SDKs include TypeScript ([library](https://agentclientprotocol.com/libraries/typescript.md)); fx serves it | **Primary local surface.** A permission request becomes an agentd approval, which is what Places and the Jailer need |
| **A2A** 1.0 | Networked task delegation with a lifecycle, `input-required` interrupts, Agent Cards (optionally JWS-signed) at `/.well-known/agent-card.json` | Linux Foundation; native in Mastra, Google ADK, Microsoft Agent Framework; no standard registry API | **Primary network surface.** agentd bridges A2A to Studio (the stub client in `loams-agentd-link` becomes the bridge); the card is embedded in the listing |
| **MCP** (spec 2026-07-28, stateless) | Tools; an agent exposed as a tool | Universal for tools; "agent as MCP server" is a common pattern, not a standard | **Optional surface** (`mcp`), for offering an agent as a tool to other agents and harnesses |
| **Open Responses** | A vendor-neutral spec derived from the OpenAI Responses API (launched January 2026 with OpenAI, Hugging Face, Vercel, Ollama, vLLM and others; [InfoQ](https://infoq.com/news/2026/02/openai-open-responses/)) | Model-serving shape; agent semantics (approvals, sessions) outside it | **Optional surface** (`responses`) for chat clients; also the shape of agentd's **model proxy** (§3.4) |
| **Agent Protocol** (agentprotocol.ai) | An OpenAPI agent interface | Maintained by AGI, Inc.; no recent activity found; LangChain's separate Agent Protocol is LangGraph-centred | **Not adopted** |
| **OCI images** | Packaging and isolation for any language | Universal; Docker Agent loads agents from OCI references | **A runtime, not a protocol** (`oci`): the image's entrypoint must serve ACP or A2A |

**Rule:** an agent package MUST serve **ACP or A2A** (at least one) and MAY serve MCP and Open Responses. A package that serves only MCP is listed as an MCP server, not an agent.

### 3.2 Runtimes (D1452)

| Runtime | Install | Start | Phase |
|---|---|---|---|
| `node` | npm package by integrity hash, lockfile included, install scripts off, into `<agentd data>/agents/<id>/<version>`; the same for both engines | `node <entrypoint>` (ACP on stdio, or A2A on a loopback port agentd assigns); with `engine: bun` (D1808), `bun --no-install --no-env-file <entrypoint>` | MK0; `bun` MS1 |
| `python` | Wheels only (no sdist builds, so no build-time code) with hashes, locked, into a per-agent virtual environment created by a pinned `uv` | `python -m <module>` or a console script | MK1 |
| `oci` | Image by digest, signature verified | Through the sandbox's container path or the host's runtime | MK2 |
| `binary` | Archive by SHA-256, per OS and architecture | The binary | MK1 (harnesses such as fx) |
| `wasm` | — | — | Deferred: no agent framework ships WASM agents yet; fx's WASM SDK is for embedding (Q1144 notes it) |

Runtime versions (`node >= 22.13`, `python >= 3.11`, `bun >= 1.4.4`) are declared; agentd supplies them (Q1122). A `node` listing's `engine` is `node` (the default) or `bun`, at the Bun version D1791 pins; agentd refuses to install a Bun listing while Bun is not installed (plan MS1 Task 6).

### 3.3 The agent environment contract (D1453)

Every agent, in every runtime, receives the same environment and nothing else from the user's machine:

| Variable | Value |
|---|---|
| `LOAMS_ENDPOINT`, `LOAMS_NAMESPACE` | The Loams the install targets, and the namespace of its Place |
| `LOAMS_TOKEN_FILE` | A file agentd rewrites with a short-lived token (15 minutes) scoped to the Place; the Loams SDKs read it |
| `LOAMS_PG_URL` | A Loams Postgres connection string for a role created for this install, limited to the Place's databases (when the manifest declares `postgres`) |
| `LOAMS_QDRANT_URL`, `LOAMS_QDRANT_API_KEY` / `LOAMS_ES_URL` | The engine's Qdrant or Elasticsearch surface with an environment-bound key (when declared) |
| `LOAMS_DURABLE_URL` | Loams Durable's Resonate endpoint (when declared) |
| `LOAMS_MCP_CONFIG` | A JSON file listing the agent's MCP servers as local endpoints that agentd proxies, so their credentials stay in agentd |
| `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL` (and a dummy key) | agentd's model proxy (§3.4) |
| `HOME`, `TMPDIR`, `LOAMS_DATA_DIR` | A private directory per install; nothing else is writable |
| `HTTPS_PROXY`, `HTTP_PROXY` | The egress proxy that enforces the manifest's hosts |

An agent never reads the user's own credentials, dotfiles or keychain. The contract is the same for the Mastra profile, which builds its clients from it.

### 3.4 Models through agentd's model proxy (D1454)

Agents get model access only through a loopback proxy in agentd that speaks the Chat Completions, Responses (Open Responses) and Anthropic Messages shapes, injects the user's provider key, counts tokens and money per install and run, enforces the install's budgets, and refuses providers the user has not configured. It gives every framework the same budgets without per-framework code, and in conformance runs it serves scripted responses, so no test needs a real model. It shares its provider code with the gateway's planned `llm` module (§57) and `loams-llm` (§52) (Q1144). **MK0**'s first-party Mastra agents receive the user's keys from agentd's vault as environment variables; **from MK1 every third-party agent must use the proxy.**

### 3.5 The Mastra profile (D1456)

For `framework: mastra` on the `node` runtime, a package may instead default-export `createAgent(ctx: LoamsAgentContext)` returning a Mastra `Agent` or `Mastra` instance. **`@loams/agent-host`** (in `loams-mastra`) loads it, builds `ctx` from the environment contract (storage, vectors, Live, PubSub, the Resonate engine, Memory processors, an `MCPClient` already connected to the declared servers, the model router pointed at the proxy), registers only the declared tools behind the gate, and serves the agent over ACP through `@loams/mastra-acp` (and A2A or MCP if declared). This is the shortest path from `create-mastra` to a listing, and the one MK0 ships first.

### 3.6 The manifest (D1455)

Authors write it where their ecosystem keeps metadata (Q1125): a `loams.agent` block in `package.json` (as console plugins write `loams.plugin`, D423), `[tool.loams.agent]` in `pyproject.toml`, or `loams-agent.toml` for `oci` and `binary`. The index tooling turns it into `listing.json` (schema `https://loams.dev/schemas/listing/v1.json`; protobuf `loams.marketplace.v1.Listing`).

| Group | Fields |
|---|---|
| Identity | `id` (`<namespace>/<name>`), `kind` (`agent`, `harness`, `mcp-server`), `version`, `title`, `summary`, `description`, `icon`, `licence` (SPDX), `source` (repository and commit), `publisher`, `support`, `security_contact` |
| Framework | `framework { name, version }` (`mastra`, `pydantic-ai`, `langgraph`, `crewai`, `openai-agents`, `google-adk`, `microsoft-agent-framework`, `eve`, `other`), `language` |
| Artifact | `artifact { type: npm \| pypi \| oci \| archive, name, version, integrity }`; `provenance { type, ref }` (§6) |
| Runtime | `runtime { kind: node \| python \| oci \| binary, engine: node \| bun (node only, D1808), version }`, `entrypoint` (a module, command or image entrypoint), `targets` (`agentd`, `byoc-server`, `app-byoc`) |
| Surfaces | `surfaces` (`acp`; `a2a` with the Agent Card embedded; `mcp`; `responses`); at least one of `acp` and `a2a` |
| Capabilities | `loams { postgres, collections, qdrant, elasticsearch, streams, live, durable, memory, house }` used; `connectors` (registry ids); `mcp_servers` (listing ids with version ranges) |
| Tools | Each tool: `name`, `description`, `risk` (`read`, `write`, `egress`, `destructive`), and the MCP server it comes from if any. **Undeclared tools are refused at run time** (§7.4) |
| Place template | Data scopes (databases, collections, streams; read or write), egress hosts, approvals that always ask, roles |
| Models | Required capabilities (`tools`, `vision`, `context >= N`), suggested provider and model, the API shape it calls (`chat-completions`, `responses`, `anthropic-messages`) |
| Cost profile | Typical and maximum tokens per run, typical runs per day, proposed budgets; measured values replace them after use |
| Configuration | Inputs with types and defaults; which are secrets (stored in agentd's vault, never in the manifest or index) |
| Conformance | The signed report reference and level (§9) |
| Pricing | `pricing: "free"`, the only value v1 accepts (§11) |

### 3.7 Harness and MCP-server listings (D1463, D1464)

- **Harness:** `launch { protocol: acp \| native, command, args }`, `install` (npm, archive or binary with SHA-256), `auth` kinds, `models`. An ACP harness maps onto agentd's `AcpAgentSpec` (which already accepts an npm package or an archive) through `InstallHarness`. **Native** adapters need code in agentd, so the built-ins (Claude Code, Codex, opencode, Pi and others) are first-party descriptors. fx is a third-party ACP harness candidate (MK1).
- **MCP server:** the listing embeds the official `server.json` unchanged ([schema](https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/server-json/draft/server.schema.json)) plus a `_meta."dev.loams/listing"` block with tool risks, egress and the Place template.

---

## 4. Create (D1458)

- **Mastra (MK0):** `npx create-mastra@latest my-agent --template https://github.com/ostrium-labs/loams-mastra-template`. `create-mastra` takes only a whole-repository URL ([reference](https://mastra.ai/reference/cli/create-mastra)), so `loams-mastra/templates/loams-starter` is mirrored there (the repository exists; marking it a GitHub template is an owner action). MK0 adds the manifest block, a `publish` workflow with trusted publishing and the conformance run. Q1134: propose it as a partner template in Mastra's gallery ([mastra.ai/templates](https://mastra.ai/templates), 26 templates).
- **Other frameworks (MK1):** `ostrium-labs/loams-agent-templates` with a starter per framework in Q1133's order, each wired to the environment contract, with a manifest and a conformance job.
- **Studio:** "New agent" picks a template, creates a project, opens a thread with the user's harness to build it, and runs it against the local `loams dev`. The factory (§72) can produce an agent as its output like any repository.
- **CLI:** `loams agent new --framework <name>`; `loams agent conformance` runs §9 locally.

---

## 5. Publish: where the marketplace lives (D1460–D1462)

| Option | For | Against |
|---|---|---|
| **A. Signed static index in a public Git repository** (Homebrew taps, Krew, Claude Code plugin marketplaces) | No service to run; submissions are reviewed pull requests; mirrors and forks are trivial; fits "Ostrium hosts only the console and auth" because the index is static files beside loams.dev | Client-side search; no publish API; limits at tens of thousands of listings |
| B. A hosted registry API (the official MCP Registry's model) | Publish API, server-side search | A service Ostrium must run and secure; contradicts the hosting rule |
| C. Federation only | Nothing hosted | No place to send new users; no shared review |

**Recommendation: A now; B as open code later behind a demand gate; federation from the start.**

- **The index (D1460):** `ostrium-labs/loams-marketplace` (Apache-2.0; created by the owner, Q1124) with `listings/<namespace>/<name>/listing.json`, `versions/`, `publishers/`, `revocations.json`, `schemas/`. A submission is a pull request; CI validates, verifies provenance, runs the scans (§10.2) and conformance (§9) and posts a report. On merge, a GitHub-hosted workflow writes `index.json`, signs it with Sigstore keyless signing, and publishes it. **Metadata only**, as the official MCP Registry stores only metadata pointing at npm, PyPI and Docker Hub ([about](https://modelcontextprotocol.io/registry/about)).
- **Hosting (D1461):** the official index is static files at `https://loams.dev/marketplace/index.json` (built by `loams-site`), with GitHub raw as fallback: the only marketplace thing Ostrium serves. No accounts, no install telemetry, no artifacts.
- **Federation (D1462):** an install has ordered **sources**, each pinned to a signing identity (`loams marketplace source add <url> --identity <id>`); organisations run private indexes. **MK2:** `loams-registry`, an open Rust service with search and a publish API, compatible with the MCP Registry's v0.1 API for MCP servers so MCP clients can read it as a subregistry ([charter](https://modelcontextprotocol.io/community/registry/charter)); and a curated, allow-listed import from the official MCP Registry (no bulk mirror, Q1130).

---

## 6. Signing, provenance and namespaces (D1459)

| Ecosystem | Requirement |
|---|---|
| npm | **Trusted publishing** from GitHub Actions or GitLab CI with automatic provenance ([npm](https://docs.npmjs.com/trusted-publishers), edited 2026-10-06); the provenance's source repository must equal the listing's `source`. Classic tokens were revoked on 2025-12-09 ([changelog](https://github.blog/changelog/2025-12-09-npm-classic-tokens-revoked-session-based-auth-and-cli-token-management-now-available/)) |
| PyPI | Trusted publishing with digital attestations (PEP 740) from the same workflow identity **(verify current support)**; wheels only |
| OCI | cosign keyless signature by digest; cosign ≥ 3.0.4 or 2.6.2 (CVE-2026-22703, [NVD](https://nvd.nist.gov/vuln/detail/CVE-2026-22703)) |
| Archives | A Sigstore bundle per archive plus SHA-256 in the listing |

Namespaces are a GitHub organisation or user (proved by GitHub OIDC from the publishing workflow) or a domain (DNS TXT or an HTTP well-known file), as the official MCP Registry verifies ([repository](https://github.com/modelcontextprotocol/registry)). `ostrium` and `loams` are reserved. Before install, agentd checks the index signature against the source's identity, `revocations.json`, the artifact's integrity and its provenance, and stops with the reason on any failure.

---

## 7. Discover, install and run (D1465–D1468)

### 7.1 Discover

`@loams/plugin-marketplace` (first-party, `modes: ["console", "studio"]`): search and filters by kind, framework, language, level, tier and capability; a listing page that renders the manifest in plain words; installed agents with updates and revocation banners. Studio's harness picker gains "Add a harness…". The CLI: `loams marketplace search|info|install|uninstall|list|update|source`. loams.dev's `/products/marketplace` renders the same index.

### 7.2 Install is consent (D1466)

The install screen is generated from the manifest: the Place the agent will get (data scopes, tools by risk, egress hosts, approvals that always ask) as a "can / can't" list (§60 Q937); the MCP servers and connectors it will install or reuse and the secrets to supply; the models and proposed budgets, editable; the framework, level, licence, tier, provenance and the version's age. Nothing is created before the person confirms. Then agentd creates the Place, provisions dependencies (a Postgres role, keys, MCP servers), stores secrets in its vault, installs the artifact with no install-time code, and registers the agent. An update that widens any permission asks again.

### 7.3 Run: agentd is the runtime (D1467)

- agentd starts **one process (or container) per installed agent** on first use and stops it when idle, in the sandbox (§56): the `linux` adapter on desktops and servers from SB1, with gVisor or microVM adapters for multi-tenant servers.
- **ACP agents** are harnesses: `HarnessId` gains a `marketplace` variant carrying the listing id; Studio threads, approvals and the transcript work as for any harness.
- **A2A agents** run on a loopback port; agentd's **A2A bridge** (the stub client in `loams-agentd-link`, grown into a bridge) presents them in Studio: an A2A task becomes a thread turn, `input-required` becomes a question or an approval, artifacts become messages.
- **Targets:** MK0 and MK1, agentd on a desktop or a server (remote agentd). MK2, an `agents` profile in the BYOC Compose and Helm stacks (one container per agent), and app BYOC (§71) for agents whose manifest declares `app-byoc` and uses only §71's ports plus the `loams` binding.
- **Exposure** (D1476): offering an installed agent to others over A2A, MCP or Open Responses is off by default and turned on per agent by its owner.

### 7.4 Runtime enforcement (D1468)

- **Declared tools only.** For ACP agents, agentd refuses tool calls not in the manifest (the permission request names the tool); for MCP tools, agentd's MCP proxy hides undeclared ones; the Mastra profile registers only declared tools.
- **The gate.** Every risky call reaches agentd's approvals queue (ACP `request_permission`, A2A `input-required`, or the MCP proxy); with the Jailer (JL1a) the gate becomes its policy decision point, and with the gateway's `mcp` module MCP calls are brokered server side.
- **Egress.** The sandbox's network policy allows only the manifest's hosts and the model proxy (`set_net_policy`, D1096). Before SB1 only first-party agents run, behind a cooperative proxy (Q1136).
- **Budgets.** The model proxy enforces per-install and per-run budgets for every framework; with the Meter they become entitlements on the user's own Meter (§11).
- **Kill and disable** per agent from Studio, the console or the CLI; a revocation with `disable` severity does it automatically.

---

## 8. Harnesses and MCP servers (D1463, D1464)

- **Harnesses:** MK0 turns agentd's built-in harness list into first-party listings, so install recipes update through the index rather than a daemon release. MK1 opens third-party ACP harnesses (fx is a candidate) in the sandbox.
- **MCP servers, first-party in MK0** (Q1129): GitHub's official server; Plane's official server (`makeplane/plane-mcp-server`); Forgejo's and Sentry's servers if their licences and maintainers check out **(verify)**; `loams-mcp` when #847 ships. `loams-agentd-mcp` is built in and not listed.
- **Why now.** Custom GPTs are being retired in favour of plugins and MCP-based apps, Smithery ends hosted MCP on 2026-10-31, and the official registry leaves scanning to others (§13). A signed, reviewed index that installs into the user's own environment fills a gap.

---

## 9. Agent package conformance (D1469)

**Loams Agent Package Conformance (LAPC)** follows §65's operator conformance: a spec with IDs and MUST/SHOULD levels, a runner, signed reports and a badge. It is small on purpose: it proves a package runs, tells the truth about what it does, and stops when asked. It does not judge quality.

| ID | Level | Requirement |
|---|---|---|
| LAPC-C1 | Core, MUST | Ready within 30 s: ACP `initialize` answered, or the A2A card served on the assigned port |
| LAPC-C2 | Core, MUST | Serves every declared surface; ACP capabilities and the A2A card match the manifest |
| LAPC-C3 | Core, MUST | Completes a scripted turn using only the model proxy (scripted responses); no other model egress |
| LAPC-C4 | Core, MUST | Calls only declared tools; asks permission (ACP) or returns `input-required` (A2A) for every tool marked to ask |
| LAPC-C5 | Core, MUST | No network egress outside the declared hosts and the proxy (recorded by the sandbox) |
| LAPC-C6 | Core, MUST | Reads configuration only from the environment contract; writes only under its data directory |
| LAPC-C7 | Core, MUST | Stops cleanly on cancel and on SIGTERM within 10 s, leaving no child processes |
| LAPC-C8 | Core, MUST | No code runs at install (no npm install scripts, no sdist builds) |
| LAPC-C9 | Core, SHOULD | Declared cost profile within 2× of the measured tokens of the scripted run |
| LAPC-K1 | Compatible, MUST | State kept through declared Loams capabilities survives a kill and restart: the next turn sees the earlier one |
| LAPC-K2 | Compatible, MUST | Uses only the contract's scoped credentials (a wrong-namespace token fails) |
| LAPC-N1 | Native, MUST | A durable workflow resumes after `kill -9` without repeating a scripted model call |
| LAPC-N2 | Native, MUST | Run state appears in Live; traces reach House |

- **Runner:** `loams agent conformance <package|listing>` in agentd: it installs the package as a user would, in the sandbox, with the model proxy in scripted mode, a fake MCP server, a local Loams, and egress recording.
- **Reports** are JSON, signed with Sigstore by the index CI's identity, and linked from the listing; the badge shows the highest level passed.
- **Framework cells:** for each recipe in §2.3, a reference agent runs the suite in the index CI nightly, so a framework release that breaks a Loams path is seen before users see it (Q1143 for Resonate's Python SDK).

---

## 10. Trust and safety (D1470–D1473)

### 10.1 Tiers (D1470)

| Tier | Requirements | Phase |
|---|---|---|
| **First-party** (Ostrium Labs) | Reserved namespaces; our own release processes | MK0 |
| **Verified** | Verified namespace; provenance; automated checks and LAPC Core pass; human review of the manifest against the code; a security contact answering within 7 days | MK1 |
| **Community** | Verified namespace; provenance; automated checks and LAPC Core pass. Sandbox only | MK1 |

### 10.2 Review and scanning (D1471)

On every submission and nightly on listed versions: schema and licence policy (the monorepo's allow list; no `ee/` dependency for Mastra agents); provenance and signatures; install-time code refused; every version and new dependency at least **3 days old** (D1224's rule); OSV advisories; a malicious-package heuristic scanner (an open one such as Datadog's GuardDog, licence **(verify)**); **tool poisoning** checks on tool names and descriptions (from the manifest and a live `tools/list` or ACP capability listing in the sandbox) for instructions to the model, hidden text and mismatches with declared risk; LAPC; human review for the verified tier.

### 10.3 Revocation (D1472)

A signed `revocations.json` lists `listing@range`, a severity (`warn`, `disable`) and a reason; agentd checks it at start and every 6 hours (Q1135). `disable` stops the agent and blocks runs; `warn` shows a banner. Yanked versions stay where installed but cannot be newly installed. A publisher's namespace can be suspended. User data in the agent's Place is never deleted by revocation, and uninstall leaves it in the user's bucket.

### 10.4 Install-time supply chain (D1473)

Exact versions and the lockfile the index tested; integrity hashes; no install scripts and no source builds. On 2026-06-17 about 140 `@mastra/*` packages gained a malicious install-time dependency ([The Hacker News](https://thehackernews.com/2026/06/144-mastra-npm-packages-compromised-via.html), [Snyk](https://snyk.io/blog/a-forgotten-contributor-account-compromised-the-entire-mastra-npm-package-scope/)); scripts off, a minimum age and exact pins would each have stopped it.

---

## 11. Economics (D1474)

- **Free and open now.** Every listing is free; v1 accepts only `pricing: "free"`. The index, its tooling, the hosts, the runner and the installers are Apache-2.0. Ostrium sells nothing through the marketplace and has no services offering.
- **The Meter hook.** The model proxy and agentd emit usage events (`io.loams.dev.usage.agent.*`: runs, tokens, tool calls, wall time, per installed agent) to the **user's own** Meter when they run one, so cost profiles are measured and budgets become entitlements. The Meter's user meters (D1138) are where paid listings would attach.
- **Paid listings and revenue share are a future owner question** (Q1127). No billing, payouts or publisher entitlements are designed here.

---

## 12. Phases in detail (D1477)

| Phase | Scope | Gates |
|---|---|---|
| **MK0** | Plan MK1 specifies it task by task: schema v1 (framework-neutral, all fields, `node` runtime and ACP accepted); validator; index repository with CI, signing and publishing; `MarketplaceService` in `loams.agentd.v1`; installers for the `node` runtime, harness descriptors and MCP servers; `@loams/agent-host` and `@loams/mastra-acp`; LAPC Core runner; consent screen, plugin and CLI; first-party listings: Loams Bot and the factory's agents (§72), the Loams starter agent, the support-agent example (MA1 Task 43), the built-in harnesses, two or more MCP servers | `@loams/mastra-resonate` 0.1.0 published; the owner creates `ostrium-labs/loams-marketplace` |
| MK1 | `python` and `binary` runtimes; the A2A bridge; the model proxy; LAPC Compatible; templates and conformance cells for Q1133's frameworks; namespace verification; scanning; tiers; revocation operations; the publishing guide rewritten; third-party listings in SB1's sandbox | SB1 (`linux` adapter); MK0 in use for 4 weeks |
| MK2 | `oci` runtime; BYOC `agents` profile; app BYOC agents; `loams-registry`; curated MCP Registry import; new kinds; LAPC Native for non-Mastra durable paths where they exist; the Jailer and the gateway's `mcp` module as enforcement points | §71 AB0; JL1a; GW1 `mcp` |
| MK3 | Paid listings, only if the owner decides | Owner (Q1127); ME1f |

---

## 13. How others do it

| Marketplace | Model | What we take | What we avoid |
|---|---|---|---|
| **OpenAI GPT Store** | OpenAI says custom GPTs stop running on 2026-12-11, replaced by plugins and the MCP-based Apps in ChatGPT with review and organisation verification ([FAQ](https://help.openai.com/en/articles/20001519-custom-gpt-retirement-and-migration-faq), dates "subject to change", read through search results **(verify)**; [Apps SDK submission](https://developers.openai.com/apps-sdk/deploy/submission)) | Review; verified publishers | A closed store whose listings die with it; the 2024 builder revenue programme never broadly launched **(verify)** |
| **Official MCP Registry** | Preview; metadata only; `server.json`; GitHub or DNS/HTTP namespaces; subregistries; minimal moderation ([about](https://modelcontextprotocol.io/registry/about), [moderation](https://modelcontextprotocol.io/registry/moderation-policy)) | `server.json`, namespaces, metadata only, subregistry compatibility | Relying on it for safety |
| **Smithery** | Registry plus hosted servers; a 2025 path traversal exposed a Fly.io token over 3,000+ apps ([SC World](https://scworld.com/news/smithery-ai-fixes-path-traversal-flaw-that-exposed-3000-mcp-servers)); hosting ends 2026-10-31 ([smithery.ai/future](https://smithery.ai/future)) | — | Hosting others' code; fleet-wide tokens |
| **Glama** | About 108,000 MCP servers with licence, quality and maintenance grades and scanning ([glama.ai/mcp/servers](https://glama.ai/mcp/servers)) | Grades on the listing | Volume over review |
| **Docker MCP Catalog and Docker Agent** | Verified MCP servers as images on Docker Hub; custom catalogs by OCI reference; agents loadable from OCI ([catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog.md), [Docker Agent](https://docs.docker.com/ai/docker-agent/)) | OCI as a runtime; catalogs as references | Requiring Docker Desktop |
| **Vercel Marketplace** | AI agents and services billed through Vercel since October 2025 ([blog](https://vercel.com/blog/ai-agents-and-services-on-the-vercel-marketplace)) | — | Billing-first listings |
| **Cloudflare agent templates** | `--template cloudflare/agents-starter` on Durable Objects ([quick start](https://developers.cloudflare.com/agents/getting-started/quick-start)) | Templates as the way in | — |
| **Mastra templates and platform** | `create-mastra --template`; 26 templates; no marketplace; hosted platform with a free tier and a $250/month Team plan ([pricing](https://mastra.ai/blog/mastra-platform-pricing), 2026-07-17) | Templates as "create"; a partner listing | Competing with Mastra's hosting |
| **Google Cloud Agent Registry** | Registers A2A 0.3 and 1.0 agents in Google Cloud ([docs](https://docs.cloud.google.com/agent-registry/register-agents)) | A2A cards as listing metadata | A cloud-bound registry |
| **Claude Code plugin marketplaces** | A `marketplace.json` in any Git repository, pinned by ref or SHA ([docs](https://code.claude.com/docs/en/plugin-marketplaces)) | Git-hosted indexes and many sources | — |
| **Goose** | An open-source Rust agent, Apache-2.0, with MCP extensions and recipes; serves ACP over stdio (`goose acp`) and over HTTP or WebSocket (`goose serve`) ([repository](https://github.com/aaif-goose/goose), [ACP guide](https://goose-docs.ai/docs/gdk/acp), read 2026-10-25) | A first-party ACP harness, launched as a process under the environment contract (D1809) | Embedding its SDK; recipes as listings |
| **Walrus** (Mysten Labs) | Decentralised blob storage, Apache-2.0; public content-addressed blobs bought per epoch; HTTP aggregators read, publishers write; no public Mainnet publisher ([web API](https://docs.wal.app/usage/web-api.html), [network reference](https://docs.wal.app/docs/network-reference), read 2026-10-25) | A public read-only tier for artifacts, project knowledge and the build cache, every read checked against a signed SHA-256 (D1810–D1814) | Private data; a `Store` backend; wallets and payments |

None combines listings in any framework that **install into the user's own environment**, show their data access as a Place before consent, are proved by a conformance run, and execute under approvals, budgets and a sandbox.

---

## 14. Supersessions (D1479)

| Earlier | Now |
|---|---|
| D-SF-20 and platform doc 05 PD47–PD60 (a private, commercial marketplace; the hosted factory as listing #1; Stripe plans; entitlements; install sagas) | **Superseded** by D1446 and this document. PD56's licence display and PD58's abuse controls survive in §7.2 and §10 |
| `docs/marketplace/publishing.md` | Kept as a draft until MK1, with a note pointing here; rewritten for the three kinds in MK1 |
| §41 §2.2 and §15 risk 1; MT4's "What MT4 leaves to others" row | **Re-pointed** here: the marketplace is open, in this repository, `loams-mastra` and `loams-marketplace` |
| D1214 (`@loams/mastra` subpaths) | **Amended** by D1450: adds `live` |

---

## 15. Decisions (approved by the owner, 2026-10-21)

| # | Decision | § |
|---|---|---|
| D1445 | **Mastra is Loams' first-class agent integration and reference implementation**: Loams Postgres, the engine, Live, Streams, Durable (Resonate), House and Loams Memory through `@mastra/pg` and `@mastra/qdrant` today and `@loams/mastra` after MA2. **Approved (owner, 2026-10-21)** | 2.2 |
| D1446 | **The marketplace is open source** (Apache-2.0), superseding D-SF-20 and platform doc 05's private marketplace. **Approved (owner, 2026-10-21)** | 1, 14 |
| D1447 | **The marketplace and runtime are framework- and language-agnostic**; Mastra is first, not exclusive. **Approved (owner, 2026-10-21)** | 2.1 |
| D1448 | **Listing kinds**: `agent` (any framework), `harness`, `mcp-server` first; templates, workflows, console plugins, connectors and factory apps later. **First kinds approved (owner, 2026-10-21)** | 3 |
| D1449 | **Integration levels** Loams-native, Loams-compatible and Runs on Loams, shown with framework and language as badges and filters | 2.4 |
| D1450 | **`@loams/mastra/live`**: run state mirrored into Live and a typed Live client; `LoamsPubSub` stays on Streams; amends D1214 | 2.2 |
| D1451 | **Invocation surfaces**: an agent serves ACP (local, interactive, primary) or A2A (networked) or both, and may serve MCP and Open Responses; Agent Protocol is not adopted; OCI is a runtime | 3.1 |
| D1452 | **Runtimes** `node` (MK0), `python` and `binary` (MK1), `oci` (MK2); `wasm` deferred; no code runs at install | 3.2 |
| D1453 | **The agent environment contract**: Loams endpoints, a rotating Place-scoped token file, a per-install Postgres role, environment-bound engine keys, proxied MCP servers, the model proxy, a private data directory and an egress proxy; nothing else from the user's machine | 3.3 |
| D1454 | **Models only through agentd's model proxy** (Chat Completions, Responses, Anthropic Messages) for third-party agents from MK1: keys injected, usage metered, budgets enforced, scripted responses for conformance | 3.4 |
| D1455 | **One framework-neutral manifest** (`listing/v1`) written in `package.json`, `pyproject.toml` or `loams-agent.toml`: identity, framework, artifact and provenance, runtime and entrypoint, surfaces, capabilities, declared tools with risk, a Place template, models, a cost profile, configuration, conformance and `pricing: "free"` | 3.6 |
| D1456 | **The Mastra profile**: `createAgent(ctx)` loaded by `@loams/agent-host`, served over ACP by `@loams/mastra-acp` | 3.5 |
| D1457 | **Other frameworks use Loams through their own extension points** on Loams' compatibility surfaces and SDKs (§2.3), each a recipe, a template where listed and a conformance cell | 2.3 |
| D1458 | **Create** from `loams-mastra-template` (MK0) and `loams-agent-templates` (MK1), in Studio or with `loams agent new` | 4 |
| D1459 | **Signing and provenance**: npm and PyPI trusted publishing with provenance matching the listing's source; cosign for OCI; Sigstore bundles for archives; namespaces by GitHub OIDC or DNS/HTTP; agentd verifies before install | 6 |
| D1460 | **The registry is a signed static index** in `ostrium-labs/loams-marketplace`; pull requests are submissions; metadata only | 5 |
| D1461 | **Ostrium hosts only the official index**, as static files on loams.dev with a GitHub fallback; no accounts, telemetry or artifacts | 5 |
| D1462 | **Federated and self-hostable**: sources pinned to signing identities; private indexes; `loams-registry` (MCP-Registry-compatible) in MK2 behind a demand gate; curated MCP Registry import | 5 |
| D1463 | **MCP-server listings embed `server.json` unchanged** plus a `dev.loams/listing` block | 3.7, 8 |
| D1464 | **Harness listings** map onto `AcpAgentSpec` and `InstallHarness`; native adapters stay first-party; built-ins become first-party listings | 3.7, 8 |
| D1465 | **Discover** in `@loams/plugin-marketplace`, Studio's harness picker, `loams marketplace` and loams.dev, filtered by kind, framework, language, level and tier | 7.1 |
| D1466 | **Install is consent**, generated from the manifest; nothing created before confirmation; widening updates ask again | 7.2 |
| D1467 | **agentd is the runtime**: one sandboxed process or container per agent; ACP agents as harnesses; A2A agents through agentd's bridge; BYOC servers and app BYOC in MK2 | 7.3 |
| D1468 | **Runtime enforcement**: declared tools only; every risky call through agentd's approvals (then the Jailer); egress by the sandbox; budgets by the model proxy; kill and disable | 7.4 |
| D1469 | **Loams Agent Package Conformance** (Core, Compatible, Native levels; a runner in agentd; signed reports; badges; nightly framework cells) | 9 |
| D1470 | **Tiers**: first-party (MK0), verified and community (MK1); community only in the sandbox | 10.1 |
| D1471 | **Review and scanning** as §10.2 | 10.2 |
| D1472 | **Revocation** through a signed `revocations.json` checked at start and every 6 hours; yanking; namespace suspension; user data never deleted | 10.3 |
| D1473 | **Installs** use tested lockfiles, exact versions, integrity hashes, no install scripts and no source builds | 10.4 |
| D1474 | **Economics**: free; usage events to the user's own Meter as the only hook; paid listings are the owner's later decision | 11 |
| D1475 | **The factory dogfoods the marketplace** (§72 D1443): its agents are first-party listings, and any conformant agent can take a run role | 12 |
| D1476 | **Exposure** over A2A, MCP or Open Responses is off by default, per agent | 7.3 |
| D1477 | **Phases MK0–MK3**; plan [MK1](../plans/2026-10-21-mk1-agent-marketplace.md) specifies MK0 task by task | 12 |
| D1478 | **Not built**: payments, artifact hosting, hosted agent execution by Ostrium, engagement-ranked storefronts, framework-specific runtimes, a Loams agent framework, a Mastra deployer of ours, telemetry | 2.1, 5 |
| D1479 | **Supersessions** as §14; the name stays "Loams Marketplace" unless the owner rules otherwise (Q1138) | 14 |

## 16. Open questions (answered by the owner on 2026-10-21: every default)

| ID | Question | Default |
|---|---|---|
| Q1122 | Node and Python for the hosts on desktops and servers | agentd uses a suitable runtime on `PATH`, else installs a pinned Node archive or a `uv`-managed Python (SHA-256) into its data directory, as it installs archive harnesses; Electron's bundled Node is not used |
| Q1123 | Where the official index is served; when to build `loams-registry` | Static files on loams.dev (GitHub raw fallback); the API service only past about 5,000 listings or when an organisation needs a publish API |
| Q1124 | The index repository | `ostrium-labs/loams-marketplace`, public, Apache-2.0; an owner action |
| Q1125 | Where authors write the manifest | `package.json` `loams.agent`; `pyproject.toml` `[tool.loams.agent]`; `loams-agent.toml` otherwise |
| Q1126 | When third-party listings open | MK1, after 4 weeks of MK0 use and SB1's `linux` adapter |
| Q1127 | Paid listings and revenue share | Not now; revisit when publishers ask |
| Q1128 | One process per agent or a shared host | One per agent |
| Q1129 | The first MCP-server listings | GitHub's and Plane's official servers; Forgejo's and Sentry's if they check out; `loams-mcp` when #847 ships |
| Q1130 | Mirror the official MCP Registry? | No; curated import by allow-list |
| Q1131 | Minimum release age | 3 days (D1224); 0 for first-party security fixes |
| Q1132 | What "verified" requires | Namespace, provenance, automated checks, LAPC Core, human review, a security contact with a 7-day response |
| Q1133 | Which frameworks get templates and conformance cells in MK1, in order | Pydantic AI, LangGraph, the OpenAI Agents SDK, Google ADK; then CrewAI, Microsoft Agent Framework, eve |
| Q1134 | Propose the Loams starter to Mastra's template gallery? | Yes, after MK0 |
| Q1135 | Revocation check interval | At start and every 6 hours |
| Q1136 | Isolation before SB1 | First-party agents only, a scrubbed environment and a cooperative egress proxy; third-party waits for SB1 |
| Q1137 | Move `LoamsPubSub` from Streams to Live? | No; Streams for ordered events, Live for UI-facing state |
| Q1138 | The marketplace's name | Keep "Loams Marketplace" |
| Q1139 | What install verification requires from the official index | The index signature and per-artifact provenance; a private source may waive provenance for its own namespace only |
| Q1140 | Which surface non-Mastra agents should prefer | ACP for interactive agents used in Studio; A2A for agents that serve other agents or run as services |
| Q1141 | Adopt Open Responses as an agent surface? | Optional (`responses`), never required; used as the model proxy's shape |
| Q1142 | Does the owner's "fx.sh" mean fx by Vercel Labs (a Zig coding-agent harness with ACP)? | Yes: a harness listing (confirmed by the owner, 2026-10-21) |
| Q1143 | Test Resonate's Python (and Go, Java) SDKs against Loams Durable? | Yes, Python first, as an observed conformance cell in MK1 |
| Q1144 | Where the model proxy lives; WASM agents | A small proxy in agentd sharing provider code with §57's `llm` module and `loams-llm`; WASM deferred until a framework ships WASM agents |

## 17. Sources

All read 2026-10-21 unless a date is given.
- Frameworks: [Mastra](https://github.com/mastra-ai/mastra); [Pydantic AI](https://github.com/pydantic/pydantic-ai) and its [durable execution](https://pydantic.dev/docs/ai/capabilities/durable_execution/overview/); [LangGraph](https://github.com/langchain-ai/langgraph), [`langgraph-checkpoint-postgres`](https://pypi.org/project/langgraph-checkpoint-postgres/), [`langchain-qdrant`](https://pypi.org/project/langchain-qdrant/); [CrewAI](https://github.com/crewAIInc/crewAI) and its [changelog](https://docs.crewai.com/v1.12.0/en/changelog.md); OpenAI Agents SDK ([Python](https://github.com/openai/openai-agents-python), [JS](https://github.com/openai/openai-agents-js), [sessions](https://openai.github.io/openai-agents-python/sessions/)); [Google ADK](https://adk.dev) and its [sessions](https://adk.dev/sessions/session/); [Microsoft Agent Framework](https://github.com/microsoft/agent-framework); [Vercel eve](https://github.com/vercel/eve) ([The New Stack](https://thenewstack.io/vercel-launches-eve-an-open-source-framework-that-treats-agents-as-directories/)); [Vercel Workflow DevKit](https://github.com/vercel/workflow) ([docs](https://workflow-sdk.dev), [DurableAgent](https://workflow-sdk.dev/docs/api-reference/workflow-ai/durable-agent)); [fx](https://fx.sh); [Docker Agent](https://docs.docker.com/ai/docker-agent/); [Resonate](https://github.com/resonatehq).
- Protocols: [ACP TypeScript library](https://agentclientprotocol.com/libraries/typescript.md); A2A ([a2a-protocol.org](https://a2a-protocol.org)); [MCP 2026-07-28](https://blog.modelcontextprotocol.io/posts/2026-07-28/); Open Responses ([InfoQ](https://infoq.com/news/2026/02/openai-open-responses/)); [Agent Protocol](https://agentprotocol.ai), [LangChain Agent Protocol](https://github.com/langchain-ai/agent-protocol); Mastra [A2A](https://mastra.ai/docs/agents/a2a) and [ACP](https://mastra.ai/docs/connections/acp).
- Registries and stores: MCP Registry ([about](https://modelcontextprotocol.io/registry/about), [moderation](https://modelcontextprotocol.io/registry/moderation-policy), [charter](https://modelcontextprotocol.io/community/registry/charter), [repository](https://github.com/modelcontextprotocol/registry), [`server.json` schema](https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/server-json/draft/server.schema.json)); OpenAI ([FAQ](https://help.openai.com/en/articles/20001519-custom-gpt-retirement-and-migration-faq), [Apps SDK](https://developers.openai.com/apps-sdk/deploy/submission)); [Smithery](https://smithery.ai/future) and [SC World](https://scworld.com/news/smithery-ai-fixes-path-traversal-flaw-that-exposed-3000-mcp-servers); [Glama](https://glama.ai/mcp/servers); [Docker MCP Catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog.md); [Vercel Marketplace](https://vercel.com/blog/ai-agents-and-services-on-the-vercel-marketplace); [Cloudflare](https://developers.cloudflare.com/agents/getting-started/quick-start); Mastra [templates](https://mastra.ai/templates), [create-mastra](https://mastra.ai/reference/cli/create-mastra), [pricing](https://mastra.ai/blog/mastra-platform-pricing) (2026-07-17); [Google Cloud Agent Registry](https://docs.cloud.google.com/agent-registry/register-agents); [Claude Code plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces).
- Supply chain: [npm trusted publishers](https://docs.npmjs.com/trusted-publishers), [classic tokens revoked](https://github.blog/changelog/2025-12-09-npm-classic-tokens-revoked-session-based-auth-and-cli-token-management-now-available/) (2025-12-09), [CVE-2026-22703](https://nvd.nist.gov/vuln/detail/CVE-2026-22703), [CISA on Shai-Hulud](https://www.cisa.gov/news-events/alerts/2025/09/23/widespread-supply-chain-compromise-impacting-npm-ecosystem) (2025-09-23), the `@mastra/*` compromise ([The Hacker News](https://thehackernews.com/2026/06/144-mastra-npm-packages-compromised-via.html), [Snyk](https://snyk.io/blog/a-forgotten-contributor-account-compromised-the-entire-mastra-npm-package-scope/)).
- This repository at `origin/dev` `b281c2dde`; `ostrium-labs/loams-mastra` `main` `c1097d8`.

## 18. MS1 addendum (2026-10-25)

The owner set the direction for the marketplace's first part, the **seed**, on 2026-10-25, and said "start". Plan [MS1](../plans/2026-10-25-ms1-marketplace-seed.md) carries it out (track MS; D1806). Nothing above changes except where a decision below amends it.

| # | Decision | Amends | Status |
|---|---|---|---|
| D1806 | Track MS and plan MS1; the 2026-10-24 MK pause is lifted for MS1's scope only (with MK0 Task 4b) | — | Approved (owner, 2026-10-25) |
| D1807 | **`quickjs`** is the default agent runtime for self-hosted Loams: an `agent` profile on `loams-live-js`'s isolated QuickJS worker, every capability a host call through agentd, ACP over the worker's stdio; a spike decides go or no-go first | D1452 (§3.2) | Approved (owner, 2026-10-25) |
| D1808 | Mastra agents run on the `node` runtime with `engine: node \| bun` | D1452 (§3.2) | Approved (owner, 2026-10-25) |
| D1809 | **Goose** is a first-party ACP harness (`goose acp`), a process and not a library, under the environment contract | — (§3.7, D1464) | Approved (owner, 2026-10-25) |
| D1810 | **Walrus** is a public, read-only tier, off by default, for public data only: artifacts, project knowledge, the build cache. The bucket stays the truth (D1); never a `Store` backend | — | Approved (owner, 2026-10-25) |
| D1811 | The Walrus **fast path**: HTTP only, hedged reads, SHA-256 on every read, a local verified cache, no Sui; Testnet first | — | Approved (owner, 2026-10-25) |
| D1812 | A `walrus` artifact location beside the npm or archive one; Sigstore provenance still verified; the index stays metadata-only | D1455 (§3.6), within D1460 (§5) | Approved (owner, 2026-10-25) |
| D1813 | A read-only verifiable project knowledge cache: a quilt per commit, a signed manifest in the bucket, `knowledge.get` and `knowledge.search` in agentd's MCP shim | — | Approved (owner, 2026-10-25) |
| D1814 | A Walrus build cache tier after the bucket, with D398's trust classes and a signed key manifest in the bucket | D398 (§36 §8), additive | Approved (owner, 2026-10-25) |

Open questions: **Q1330** (Mainnet writes: an own publisher or the Upload Relay), **Q1331** (the knowledge cache's content set), **Q1332** (who pays for and renews storage epochs). The full rows are in the [decision log](13-decision-log.md).

What this does not do: no Walrus `Store` backend, no private data on Walrus, no Sui wallets or payments, no embedded `goose-sdk`, no goose recipes as listings. §3.2's runtime table gains `quickjs` (self-hosted default, MS1) when MS1 Task 2 merges; `node`'s `engine` choice is in it (MS1 Task 6).
