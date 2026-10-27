# The Seed Exchange

The Seed Exchange is the Loams Marketplace: a signed, open index of agents, harnesses and MCP servers that you install into your own Loams Desktop or agentd. Its design is [§73](../design/73-agent-marketplace.md), and the build plan is [MK1](../plans/2026-10-21-mk1-agent-marketplace.md).

Status: **experimental**, first-party listings only. Listings come only from the `ostrium` and `loams` namespaces until MK1 opens the index to third parties. Nothing here is a hosted service: the index is a static file, and every install runs on ground you own.

## How it works

1. **The index.** Listings live in the public repository [`ostrium-labs/loams-marketplace`](https://github.com/ostrium-labs/loams-marketplace). A pull request runs the index CI: schema lint, the first-party namespace check, and an integrity and provenance check of the artifact. On `main`, the CI builds `index.json` and `revocations.json`, signs both with Sigstore keyless signing, and publishes them to GitHub Pages at `https://ostrium-labs.github.io/loams-marketplace/index.json`. The index holds metadata only, never package bytes.
2. **Sources.** agentd fetches each source's index and revocation list. It uses them only after both signatures verify against the identity the source is pinned to. The official source is pinned to the index repository's `publish.yml` workflow on `main`. agentd keeps the last verified copy, checks it again at every start, and refreshes every 6 hours.
3. **The consent plan.** Before anything is installed, agentd shows what the listing will be allowed to do: its Place (data scopes), its tools by risk, the tools that always ask, its egress hosts, its dependencies, the secrets you supply, its models and budgets, its licence, tier and provenance. Nothing is created before you confirm (D1466).
4. **Install.** agentd installs exactly the plan you confirmed, identified by its digest. If the listing changed in between, the install is refused (`plan_changed`). npm packages install with scripts off, and a package that needs install-time code is refused. Secrets go to the OS keyring, never to a file. An update that asks for more than the installed version needs your consent again (`consent_required`).
5. **Uninstall** removes the package, its registrations and its keyring secrets. The agent's own data directory stays.
6. **Revocation.** A version whose revocation has `disable` severity has its harness and MCP server registrations removed at the next check. Its data stays.

## Listing kinds

| Kind | What installing it does | MK0 status |
|---|---|---|
| Harness, native | Enables one of agentd's built-in adapters (Claude Code, Codex, OpenCode, Pi and the others). The vendor's CLI is installed separately. | Installs |
| Harness, ACP | Installs an ACP agent package and registers it in the harness list. | Installs and appears in the harness list. Running a thread on it is a follow-up. |
| MCP server, local | Installs the server and adds it to every harness run that takes MCP servers (ACP harnesses today). | `node` packages install. Release archives (such as GitHub's server) wait for the archive installer. |
| MCP server, remote | Records the server. agentd reaches it through its MCP proxy. | Recorded. Passing it to runs waits for the MCP proxy (D1463). |
| Agent | Installs an agent package that runs under the agent environment contract (§73 §3.3). | Mastra agents wait for `@loams/mastra-resonate` on npm and `@loams/agent-host` (MK0 Task 7). |

## Where to use it

- **Loams Desktop and the console:** the Seed Exchange page (`@loams/plugin-marketplace`, MK0 Task 9) lets you browse, open the consent screen, install, and review updates and revocations.
- **Studio:** the harness picker's "Add a harness…" opens the Seed Exchange filtered to harnesses.
- **The command line:** `loams marketplace search|info|install|uninstall|list|update|source` (MK0 Task 10).
- **agentd's API:** `MarketplaceService` over Connect (`proto/loams/agentd/v1/marketplace.proto`), and the same methods over agentd's local ndjson wire. Only the owner's connection can call them; a harness run's MCP token cannot.

## Publishing

Publishing is first-party only in MK0. The listing format is [`schemas/marketplace/listing.v1.json`](../../schemas/marketplace/listing.v1.json), and the `loams-listing` CLI (`agents/loams-listing`) lints a listing, makes one from a package's `loams.agent` block, verifies its artifact, and builds the index. The [publishing guide](publishing.md) predates §73 and will be rewritten for third-party publishing in MK1.

## Not yet

These are not built yet: third-party listings, verified and community tiers, the `python`, `oci` and archive runtimes, A2A agents, the model proxy, the conformance badge on listings, sandboxed runs (SB1), and paid listings. Paid listings are not planned.
