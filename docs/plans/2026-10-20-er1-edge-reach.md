# ER1 — Edge Reach: Serverless Runtimes to a Private Loams

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, flags, ports or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved** (2026-10-21). Track ER, design [§69](../design/69-edge-reach.md) (D1300–D1322, Q1060–Q1075). The owner approved the direction on 2026-10-20 and the design with every default on 2026-10-21. Part A of the never-public amendment (D1311) is approved and applied to the networking map. **D1312, the public edge profile, is deferred** (per Q1060) and is not part of this plan until it is revisited after ER2. This plan specifies **ER0** task by task. ER1 (operator and Postgres), ER2 (workload identity) and ER3 (deferred) are listed at milestone level and get full specifications when their gates are met.

**Goal (ER0):** a Cloudflare Worker reaches a self-hosted compose stack through a Workers VPC binding, with an environment-bound `lk_` key, and nothing in the stack is published. So does any runtime with `fetch`, through an Access-protected tunnel hostname with a service token. In both cases:
- the request enters through `loams-gateway`'s edge listener only;
- only allow-listed data-plane routes work;
- every admin, setup and key route is refused at the gateway and again at the engine;
- operator, org and session credentials are refused on the edge;
- all of it is tested on GitHub-hosted runners without a Cloudflare account.

**Architecture** (§69 §6):
- **`proto/loams/options/v1/options.proto`** gains `bool edge = 50003` on `MethodOptions`. `scripts/api/gen-edge-allowlist` writes `api/edge-allowlist.txt`.
- **`crates/loams`** gains the edge upstream listener (`--edge-upstream-listen`), its credential policy and the audit fields.
- **`crates/loams-gateway`** gains the module `edge/` with these parts:
  - `config`, `admission` (`access`, `network`), `allowlist`, `hygiene`, `limits`, `proxy` (hyper client to the engine, streaming passthrough), `metrics`;
  - the `EdgeProxy` trait.
- **`deploy/release/compose.edge.yaml`**: the `edge` profile with `loams-gateway` and `cloudflared`.
- **`sdks/typescript/packages/client`**: `accessServiceToken()`.
- **`docs/guides/edge/`**: one page per runtime.

**Tech stack:**
- Rust 1.97, edition 2024, workspace lints.
- Workspace dependencies only:
  - `hyper` 1 with the `client`, `http1` and `http2` features added to the gateway, and `hyper-util`;
  - `jsonwebtoken` (`=10.4.0`, as the engine uses it) and `reqwest` (`rustls-no-provider`, as the engine uses it) for the Access JWKS;
  - `tokio`, `rustls`, `serde`, `toml`, `tracing`; `rcgen` in tests.

  **No Pingora** (D1303, Q1062).
- `cargo fuzz` as the existing fuzz jobs use it.
- `workerd` from npm, pinned exactly, at a release at least 14 days old.
- Docker Compose v2.
- `cloudflared`'s image, pinned by digest, at a release at least 14 days old.

**Spec:** [§69](../design/69-edge-reach.md) (all of it); [§57](../design/57-loams-gateway.md) §5, §9.3, §10; [§19](../design/19-console-identity-and-agents.md) §5.5; [§44](../design/44-unified-api-and-sdks.md) §7.4.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/er0-<task>`, one branch and one PR per task into `dev`. CodeRabbit reviews; merge on green. Commit with `git commit -s`.
- **Light Rust builds.** `cargo build -p loams-gateway` or `-p loams` with the configured jobs; never `--workspace` locally. Build output goes to the shared target in `~/Documents/.cargo/config.toml`; never build in `/tmp`.
- **Never public.** No `ports:` in `compose.edge.yaml`, no `0.0.0.0` host bind, no Funnel. `cloudflared` is never on `edge-link` and never in the engine's network namespace.
- **Never weaken the existing listeners.** The engine's main port, Tailscale listener and compatibility listeners behave exactly as before. Every engine change is gated on `--edge-upstream-listen` being set.
- **Allow-list by default off.** A method is on the edge only if it carries `(loams.options.v1.edge) = true`. No wildcard rules in `api/edge-allowlist.txt`.
- **No secrets in the repository or logs.** Tests make their own keys. The link key, tunnel token and service token come from files under `deploy/release/secrets/`, which are git-ignored.
- **GitHub-hosted runners only.** ER0 has no job that needs a Cloudflare account; the nightly real-Cloudflare job is ER1.

## Review Focus

1. **A route that is not on the list is reachable through the edge.** Expected: never, at either enforcement point. Tests: `gateway_refuses_every_unlisted_method`, `engine_edge_listener_refuses_every_unlisted_method`.
2. **A credential that is not environment-bound works on the edge.** Expected: `403`. Tests: `edge_refuses_operator_key`, `edge_refuses_org_key`, `edge_refuses_owner_key`, `edge_refuses_session_bearer`, `edge_refuses_session_cookie`, `edge_refuses_namespace_mismatch`.
3. **An `access` listener admits a request without a valid assertion.** Expected: `401`. Tests: the `access_*` cases of Task 6.
4. **A spoofed identity header changes who the engine thinks the caller is.** Expected: never. Tests: `edge_strips_tailscale_headers`, `edge_strips_access_identity_headers`, `edge_link_headers_ignored_without_link_key`.
5. **The edge listener reaches anything but the engine's edge upstream listener.** Expected: never; the upstream is one configured address. Test: `edge_upstream_is_fixed`.
6. **Compose publishes something or puts `cloudflared` next to the engine.** Expected: never. Tests: `compose_edge_has_no_ports`, `cloudflared_networks_are_edge_only`, `engine_edge_listener_on_edge_link_only`.

## ER0 — The edge door, keys only (Tasks 0–12)

### Task 0 — Rulings

Check and record in "Rulings made during execution":
- **PR #947** (`transportOptions.fetch` in the TypeScript SDK) is merged. If it is not, Task 10 waits, and Task 9 passes `fetch` through the transport as #947 defines it.
- **The engine's main-port surface:** the list of Connect services and methods, and the REST, Qdrant and Elasticsearch routes from `docs/api/route-map.md`. Then mark the first allow-list: data-plane methods that an environment-bound service-account key may call today. Owner and admin RPCs, `loams.admin.v1`, `loams.auth.v1`, `loams.internal.v1`, `LiveAdminService` and House administration stay unmarked. Record the list.
- **How the engine identifies a credential's binding:** the API key record's environment (`namespace_reach.rs`, `directory.rs`) and the principal kinds. Record the exact predicate "bound to exactly one environment" and how operator keys from `LOAMS_API_KEYS_FILE` are told apart.
- **Access service-token JWTs:** the claim that names the service token (`common_name` expected; `sub` may be empty). Use Cloudflare's docs on validating the JWT; if a test account is available, decode one real assertion. Record whether `sub` is empty, because the edge's subject is taken from `common_name`.
- **Pins:** `cloudflared` image tag and digest; the `--token-file` flag's spelling and position; `workerd` npm version; each at a release at least 14 days old.
- **The engine's streaming responses:** Connect server streaming over HTTP/1.1, and SSE for MCP. Record what the proxy must not buffer.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — The edge method option and the allow-list generator (D1322)

Files:
- `proto/loams/options/v1/options.proto`: `extend google.protobuf.MethodOptions { bool edge = 50003; }`, with a comment that points at §69 §6.3.
- The methods of Task 0's list: `option (loams.options.v1.edge) = true;`.
- `scripts/api/gen-edge-allowlist` (bash, `set -euo pipefail`; `buf build -o -` for the descriptor set and a small reader in the existing proto tooling): it writes `api/edge-allowlist.txt`. One entry per line, sorted:
  - `connect /<pkg>.<Service>/<Method>`;
  - `rest <METHOD> <path-template>`;
  - `qdrant <METHOD> <path-template>`;
  - `es <METHOD> <path-template>`.

  The REST, Qdrant and Elasticsearch entries come from a hand-kept `api/edge-routes.toml` that names, for each route, the method it maps to. A route whose method is not marked is an error.
- `crates/loams-edge-allowlist/` (new, `publish = false`, no dependencies): `include_str!("../../../api/edge-allowlist.txt")`, a parser into a matcher (`fn allows(&self, kind: Surface, method: &http::Method, path: &str) -> bool`; path templates with `{name}` segments, no wildcards), shared by the gateway and the engine.
- The CI step in `.github/workflows/ci.yml` beside the decision-ID check: `scripts/api/gen-edge-allowlist --check`.

Tests:
- `allowlist_parses`, `allowlist_has_no_wildcards`, `allowlist_matches_templates`, `allowlist_refuses_prefix_tricks` (`/loams.collection.v1.CollectionService/Search/../x`, `//`, percent-encoded slashes, a trailing slash, mixed case), in `crates/loams-edge-allowlist`.
- `edge_routes_map_to_marked_methods`, in the script's self-test.

Commit: `api: the edge method option and the generated edge allow-list (ER0 Task 1, D1322)`.

### Task 2 — The engine's edge upstream listener (D1304)

Files:
- `crates/loams/src/main.rs`: `--edge-upstream-listen <addr>` (`LOAMS_EDGE_UPSTREAM_LISTEN`). It is not loopback-only: the engine refuses to start if it is set with `LOAMS_AUTH_MODE=off`.
- `crates/loams/src/auth/edge.rs` (new): `EdgePolicy`, applied by the auth layer only on this listener.
  - **Refusals:** no credential (`401`); `ls_` bearer or the session cookie (`403`, reason `edge_session_refused`); keys from `LOAMS_API_KEYS_FILE` (`edge_operator_key_refused`); keys not bound to exactly one environment (`edge_unbound_credential_refused`); `Loams-Namespace` other than the key's environment (`edge_namespace_mismatch`); a route not on the allow-list (`404`, no body).
  - **Header handling:** Tailscale headers are dropped before the layer, whatever their value.
  - The reasons are registered in `docs/api/reasons.md`.
- `crates/loams/src/server.rs`: the extra listener serves the same router as the main port, with `EdgePolicy` and no CORS layer.
- Audit: `via=edge`, `edge_listener` and `edge_subject` on the audit event. They are filled from `Loams-Edge-Listener` and `Loams-Edge-Subject` only after Task 3's link check passes; otherwise they are `unknown`.

Tests (`crates/loams/tests/edge_upstream.rs`):
- `edge_listener_refuses_without_credential`
- `edge_refuses_session_bearer`
- `edge_refuses_session_cookie`
- `edge_refuses_operator_key`
- `edge_refuses_org_key`
- `edge_refuses_owner_key`
- `edge_refuses_namespace_mismatch`
- `edge_accepts_environment_key`
- `edge_strips_tailscale_headers`
- `engine_edge_listener_refuses_every_unlisted_method` (iterates the descriptor set)
- `edge_listener_requires_auth_mode`
- `main_port_unchanged_with_edge_listener` (a key the main port accepts still works there)

Commit: `feat(loams): the edge upstream listener, environment-bound credentials only (ER0 Task 2, D1304)`.

### Task 3 — The gateway-engine link key (Q1065)

Files:
- `crates/loams/src/auth/edge.rs`: `LOAMS_EDGE_LINK_KEY_FILE`, 32 bytes, read once.
  - A request on the edge upstream listener must carry `Loams-Edge-Link: <hex>`, compared in constant time, or it is refused with `401` (`edge_link_refused`).
  - The `Loams-Edge-*` headers are honoured only after this check.
- `crates/loams-gateway/src/edge/proxy.rs`: sends the header from `[edge] link_key_file`.
- `deploy/release/byoc/install.sh` (which already writes `secrets/`) and `deploy/release/secrets/README.md`: write `secrets/edge-link-key` (32 random bytes, mode 0600) when absent, and document making it by hand for a plain compose install.

Tests:
- `edge_link_refused_without_key`
- `edge_link_refused_with_wrong_key`
- `edge_link_headers_ignored_without_link_key`
- `edge_link_key_file_must_be_32_bytes`

Commit: `feat(edge): authenticate the gateway to the engine's edge listener with a link key (ER0 Task 3)`.

### Task 4 — The gateway's `edge` module (D1303)

Files:
- `crates/loams-gateway/src/config.rs`: `edge: Option<EdgeConfig>` with:
  - `listeners: Vec<EdgeListenerConfig { name, listen, admission: Access | Network, access: Option<AccessConfig>, refuse_hosts: Vec<String>, tls: Option<TlsFiles> }>`;
  - `upstream: Uri`, `link_key_file`, `max_body_bytes`, `head_timeout_ms`, `unary_timeout_ms`, `limits`.

  `deny_unknown_fields`. `check()` refuses `access` admission without `access`, duplicate names or ports, and an upstream that is not `http`/`https`.
- `crates/loams-gateway/src/edge/mod.rs`: the `EdgeProxy` trait (`async fn handle(&self, listener: &EdgeListener, req: Request<Incoming>) -> Response<BoxBody>`) and its hyper implementation.
- `crates/loams-gateway/src/edge/allowlist.rs`: classifies the request into a surface:
  - Connect or gRPC-Web by `content-type` and path shape;
  - Qdrant or Elasticsearch by path prefix, as `api/edge-routes.toml` declares;
  - REST otherwise.

  It then checks `loams-edge-allowlist`. A miss is a `404` with an empty body.
- `crates/loams-gateway/src/edge/hygiene.rs`: strips `Tailscale-*`, `Cf-Access-Authenticated-User-Email`, `Cf-Access-Client-*`, `Forwarded`, `X-Forwarded-*`, `Cookie`, `Loams-Via`, `Loams-Edge-*` and hop-by-hop headers. It then adds `Loams-Via: edge`, `Loams-Edge-Listener` and, from Task 6, `Loams-Edge-Subject`.
- `crates/loams-gateway/src/edge/proxy.rs`: a hyper client to the single configured upstream, with HTTP/1.1 and h2c. Bodies are streamed both ways with no buffering, and the response's trailers are kept. On the `network` listener, a request whose `Host` is in `refuse_hosts` is refused with `421`.
- `crates/loams-gateway/src/main.rs`: starts the edge listeners when `[edge]` is present. The admin `/readyz` lists them.

Tests (`crates/loams-gateway/tests/edge.rs`, with a stub engine on loopback):
- `gateway_refuses_every_unlisted_method`
- `edge_forwards_listed_connect_unary`
- `edge_streams_server_stream_without_buffering`
- `edge_streams_sse`
- `edge_upstream_is_fixed` (a request with an absolute-form URI or a `Host` naming another address still goes to the configured upstream)
- `edge_refuses_without_credential`
- `edge_strips_access_identity_headers`
- `edge_network_listener_refuses_public_host`
- `edge_config_rejects_access_without_team`
- `edge_body_limit`
- `edge_head_timeout`

Commit: `feat(gateway): the edge listener on hyper, allow-listed routes and header hygiene (ER0 Task 4, D1303)`.

### Task 5 — The compose `edge` profile (D1316)

Files:
- `deploy/release/compose.edge.yaml`: as §69 §6.7.
  - `loams-gateway` is on networks `edge` and `edge-link`. It mounts `config/gateway-edge.toml` and `secrets/edge-link-key`.
  - `cloudflared` is on `edge` only. It runs `tunnel --no-autoupdate run --token-file /run/secrets/cloudflared-token` (Task 0's spelling), and its image is pinned by digest.
  - The `loams` service gets an override: it joins `edge-link`, and `--edge-upstream-listen` is set on `0.0.0.0:8090`. That address is reachable only on `edge-link` and the engine's other networks, so Task 2's link key is required. The plan's ruling records whether the engine can bind to the `edge-link` interface address instead.
- `deploy/release/Containerfile.loams-gateway` (if GW1's image task has not added it).
- `deploy/release/config/gateway-edge.toml.example`; `.env.example` gains `LOAMS_EDGE_ACCESS_TEAM`, `LOAMS_EDGE_ACCESS_AUD`, `LOAMS_EDGE_PUBLIC_HOST` and `CLOUDFLARED_TAG`.
- `deploy/release/README.md`: an "Edge reach" section linking `docs/guides/edge/`.

Tests (`scripts/release/test-compose-edge.sh`, run by `release-compose.yml`):
- `compose_edge_has_no_ports`
- `cloudflared_networks_are_edge_only`
- `engine_edge_listener_on_edge_link_only`
- `edge_profile_off_by_default` (`docker compose config` without the profile has neither service)
- `compose_edge_starts` (the stack with the profile and a dummy tunnel token: the gateway is ready, and `cloudflared` is allowed to fail to connect)

Commit: `deploy: the compose edge profile with loams-gateway and cloudflared (ER0 Task 5, D1316)`.

### Task 6 — Access admission, limits and metrics (D1306, D1318)

Files:
- `crates/loams-gateway/src/edge/admission.rs`: verifies `Cf-Access-Jwt-Assertion` on `access` listeners.
  - **Keys:** RS256 only. The JWKS is fetched from `<team>/cdn-cgi/access/certs` and cached for 10 minutes. An unknown `kid` causes at most one refetch per 30 s, and a failed refresh keeps the old keys.
  - **Claims:** `aud` must be in the configured list, `iss` must be the team domain, and `exp` and `nbf` are checked with 30 s leeway.
  - **Subject:** from `common_name` (Task 0), else `sub`.
  - **Failures:** a missing header or a failed check is `401` with an empty body. If the JWKS cannot be fetched at all, the answer is `503`.
- `crates/loams-gateway/src/edge/limits.rs`: token buckets per credential (a SHA-256 of the credential, never the credential itself) and per subject. Per-credential open-stream counts. Defaults from Q1068.
- `crates/loams-gateway/src/observe.rs`: `loams_gateway_edge_requests_total{listener,route_class,outcome}`, `loams_gateway_edge_refusals_total{listener,reason}` and `loams_gateway_edge_active_streams{listener}`.

Tests:
- `access_valid_assertion_admitted`
- `access_missing_header_refused`
- `access_wrong_aud_refused`
- `access_wrong_iss_refused`
- `access_expired_refused`
- `access_unknown_kid_refetches_once`
- `access_alg_none_refused`
- `access_hs256_refused`
- `access_jwks_unreachable_is_503`
- `limits_rate_per_credential`
- `limits_streams_per_credential`
- `metrics_have_no_tenant_labels`

Commit: `feat(gateway): Access admission, edge limits and metrics (ER0 Task 6, D1306)`.

### Task 7 — Fuzz target

Files: `crates/loams-gateway/fuzz/fuzz_targets/edge_request_head.rs`. The target takes arbitrary bytes, parses them as an HTTP/1.1 request head with hyper's parser, and runs classification, the allow-list check and hygiene. It asserts no panic, and that no `Tailscale-*` or `Loams-Edge-*` header survives hygiene. The nightly fuzz job (`nightly.yml`) runs it next to the `pg` targets.

Tests: the target builds in CI (`cargo fuzz build`), and a 60 s smoke run happens on PRs that touch `edge/`.

Commit: `fuzz(gateway): the edge request head (ER0 Task 7)`.

### Task 8 — The refusal suite end to end (D1319)

Files: `crates/loams/tests/edge_refusals.rs`, with the gateway built as a library in process, a real engine (as the existing `crates/loams/tests/connect_auth.rs` starts one, with `LOAMS_AUTH_MODE=apikey`), an org, a project with two environments, and one key of each kind.

It runs every method in the descriptor set and every route in the REST, Qdrant and Elasticsearch tables through the gateway's `network` listener, and also directly against the engine's edge upstream listener. For each one it checks the expected outcome: listed and allowed, listed and refused for this key, or not listed. The result is written to a snapshot (`tests/snapshots/edge_refusals.snap`), so a newly marked method shows up in review.

Tests:
- `edge_refusal_matrix`
- `edge_cross_environment_refused`
- `edge_admin_services_refused_everywhere`

Commit: `test: the edge refusal suite through the gateway and the engine (ER0 Task 8)`.

### Task 9 — The `workerd` end-to-end test

Files:
- `tests/edge/workerd/`:
  - `package.json`, with `workerd` pinned exactly and `@loams/client` from the workspace;
  - `config.capnp`: a Worker with a service binding `LOAMS` to an external service at the gateway's `network` listener, and a second variant that calls a local Access stub over plain `fetch`;
  - `worker.ts`, which builds `new Loams({ endpoint: 'http://loams.internal', auth: apiKey(env.LOAMS_API_KEY), transport: { fetch: (i, init) => env.LOAMS.fetch(i, init) } })`, then creates a collection, writes three documents, searches, and calls `KeyService/CreateKey`, which must be refused;
  - `access-stub.mjs`, a tiny proxy that adds a signed `Cf-Access-Jwt-Assertion` from a test key and serves a test JWKS.
- `scripts/edge/test-workerd.sh`: starts the engine, the gateway (both listeners) and the stub, then runs `workerd test` for both variants.
- `.github/workflows/gateway.yml`: job `edge-workerd`, path-filtered on `crates/loams-gateway/src/edge/**`, `crates/loams/src/auth/**`, `api/edge-*`, `sdks/typescript/packages/client/**` and `tests/edge/**`.

Tests:
- `workerd_vpc_binding_path`
- `workerd_access_path`
- `workerd_admin_refused`

Commit: `test: a workerd Worker reaches Loams through the edge listener (ER0 Task 9, D1319)`.

### Task 10 — `accessServiceToken()` in the TypeScript SDK (D1309)

Files:
- `sdks/typescript/packages/client/src/runtime/edge.ts`:
  - `accessServiceToken(opts?: { clientId?: string; clientSecret?: string; env?: Record<string, string | undefined> }): Record<string, string>`;
  - it reads `CF_ACCESS_CLIENT_ID` and `CF_ACCESS_CLIENT_SECRET` when they are not given, and throws when either is empty;
  - it is exported from the package root.
- `sdks/typescript/README.md`: a "Run on the edge" section with the Workers VPC, Access and Vercel snippets of §69 §6.5, and the rule that keys are environment keys.
- `CHANGELOG.md`.

Tests:
- `accessServiceToken_reads_env`
- `accessServiceToken_rejects_empty`
- `access_headers_reach_the_request` (with the transport's `headers` interceptor)

Commit: `feat(sdk-typescript): accessServiceToken for Cloudflare Access service tokens (ER0 Task 10)`.

### Task 11 — Guides

Files under `docs/guides/edge/`:
- `README.md`: reach classes, the one door, two credentials; what is and is not public.
- `cloudflare-workers.md`: Workers VPC (beta) first, the Access hostname beside it; the same-zone caveat; Hyperdrive marked "ER1".
- `vercel.md`: the Access hostname; PrivateLink and Secure Compute when Loams runs on AWS.
- `aws-lambda.md`: a VPC to an internal NLB, or the Access hostname; the Tailscale recipe marked "recipe".
- `cloud-run.md`: Direct VPC egress, or the Access hostname; the Tailscale recipe.
- `fly.md`: Tailscale in the Machine, or the Access hostname.
- `security.md`: the threat table of §69 §4.3 in user terms; rotating service tokens, tunnel tokens and keys; what Cloudflare sees.

Also:
- Each page lists exactly the commands and config the tests use. A docs-snippet check, `scripts/edge/check-doc-snippets`, diffs the fenced blocks tagged `edge-test` against `tests/edge/`.
- The guides index (`docs/README.md`, where `docs/guides/connectors` and `docs/guides/studio` are linked) links the section.
- `docs/release/preview.md` gets a one-line pointer to the guides. The networking map already carries part A's approved wording (D1311, 2026-10-21).

Tests: `edge_doc_snippets_match_tests`.

Commit: `docs: edge reach guides per runtime (ER0 Task 11)`.

### Task 12 — ER0 exit

Two manual runs, recorded here: a Worker through Workers VPC and through Access, and a Vercel Node function through Access. Each runs against a compose stack on a VM with the `edge` profile, in a Cloudflare account the person running it owns. Record the date, the versions, the latency of a search (p50 of 20 calls) and any deviation.

Gate: Tasks 1–11 merged; both runs recorded; §69 §11's PR tests green on `dev`.

## ER1 — Operations and Postgres (milestone level)

- The operator's `exposure.mode: tunnel` renders a `cloudflared` Deployment with no Service, and `internal` renders the edge listeners. LOC tests: `tunnel_mode_renders_no_service`, `edge_listener_service_is_cluster_ip`.
- `scripts/edge/check-tunnel-config` reads the tunnel's ingress through the Cloudflare API and fails if a public hostname targets the `network` listener.
- `.github/workflows/edge-nightly.yml`, in the `edge-nightly` environment (Q1069):
  - deploys a test Worker with a `vpc_services` binding;
  - runs the stack and `cloudflared` on the runner;
  - runs the Workers VPC, Access and Hyperdrive scenarios;
  - tears down;
  - opens an issue on failure.
- Hyperdrive on a `tcp` VPC Service to the gateway's `pg` dedicated port, and Hyperdrive's CA verification (verify).
- Recipes for Lambda (VPC, Tailscale), Cloud Run and Fly.io, tested by hand. The Tailscale recipes measure the cold-join cost.

## ER2 — Workload identity (milestone level)

- The instance signing key and JWKS (§19 §5.3) if not built by then.
- `POST /oauth/token` (RFC 8693), on the edge allow-list with its own limit.
- Trust policies: a `loams.auth.v1` admin RPC (never on the edge), a metastore record and a console page.
- Loams access tokens accepted on the edge upstream listener.
- `IssueConnectCredential` for edge Postgres (D711).
- TypeScript `exchange()` and `@loams/client/identity` providers (`vercelOidc`, `awsWebIdentity`, `gcpIdToken`, `flyOidc`, `githubActions`), then Python and Go.
- Nightly: a Vercel preview deployment and a GitHub Actions job exchange real tokens.

## ER3 — The public edge profile (deferred with D1312, owner 2026-10-21; revisited after ER2)

- TLS 1.3 and mTLS admission on a public listener.
- Token-only credentials.
- The operator's `edge-public` mode with an acknowledgement field and a narrow VAP exception, plus LOC tests that nothing else gets through.
- A WAF guide.
- An external security review, published.

## Rulings made during execution

(none yet)
