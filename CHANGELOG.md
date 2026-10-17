# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2026-10-18

### Upgrading

From **0.1.0-preview.2** (one step at a time): `loams upgrade --to 0.1.0`.

It takes a snapshot first, and `loams upgrade rollback` restores it.

| Migration | Store | Class | What it does |
|---|---|---|---|
| `0000-adopt-preview-2` | meta | Reversible | Record preview.2's versions (preview.2 writes no stack record) |
| `0001-safekeeper-volume` | host | Reversible | Give the safekeeper a volume (after a drain) |

Third-party bumps: none.
Free space needed: 1.5× the snapshot estimate (`loams upgrade check` shows it).
Expected downtime: about 5 minutes, excluding image pulls.

See [docs/release/upgrades.md](docs/release/upgrades.md).

### Added
- **Upgrades that keep data** (design §54, plan UP1). `loams upgrade check|--to|rollback|status|snapshots`
  upgrades a self-hosted compose stack one release at a time:
  - it takes a snapshot of every store first (TiKV by BR or a cold copy, the bucket's pointers and a GC
    root, Postgres drained and branched, House, `pg-control`, Durable);
  - it runs the release's migrations in the new one-shot `migrate` service, restarts in order and verifies;
  - on a failure it rolls back automatically.

  The engine, the House and `pg-control` refuse data they cannot read (exit 78). Every persisted format
  has a version in one registry. The console works with the engine before it, and an older engine gets an
  upgrade page. CI gains `Upgrade: previous release → candidate`, which upgrades the previous release's
  stack with fixtures in every product and rolls it back.
- **Git push and fetch to the bucket from the release packages.** `git-remote-loams`, git's
  remote helper for `loams::<store-url>/ns/<ns>/repos/<repo>` URLs (`crates/loams-git-remote`),
  now ships next to `loams` in the .deb, .rpm and .pkg.tar.zst (`/usr/bin`), the macOS
  tarballs and the Windows zip, so `git clone`, `fetch` and `push` reach a Loams Git
  repository on a bucket with no server in between. It is not published to crates.io.
  `git-remote-loams --version` prints its version. See `docs/release/packaging.md`.
- **Loams Studio, an agentic development environment in the console** (design §53, plan SD1).
  The console has two modes, Console and Studio. Studio runs coding agents in threads, each
  with its own git worktree. Around them it has a chat timeline, a composer, diff review with
  comments, git actions (commit, push, pull requests, merge), per-turn checkpoints, terminals,
  a file viewer, previews, supervised approvals and parallel run groups compared side by side.
  It all goes through `loams-agentd`'s new Connect API, `loams.agentd.v1`. Loams Desktop
  opens in Studio. In the browser, Studio works against a remote daemon
  (`loams-agentd serve --remote`) and is behind the instance feature `studio`. The UI is
  vendored from T3 Code (MIT) with provenance and attribution, and an npm licence gate now
  runs in CI. Guides: `docs/guides/studio`.
- **Loams Postgres's control plane in the self-hosted stack.** `--profile postgres-control`
  runs `pg-control` (`loams.postgres.v1` on `127.0.0.1:7680`, API keys from
  `secrets/pg-api-keys.json`) beside the Postgres stack, with its computes started on demand
  and suspended when idle. Neither the engine nor `pg-control` gets a container runtime
  socket: a separate compute runner, `pg-runner` (`loams pg-runner`, new image
  `loams-pg-control`), holds it on an internal network only `pg-control` joins, needs a token
  for every call, and starts nothing but the compute images pinned in
  `deploy/pins/loams-postgres-images.toml`. Prefer a rootless Podman socket (PG2 ruling
  R-REL.1).
- **Loams House compacts lake tables** (HS1 Task 13): `OPTIMIZE TABLE … [FINAL] [DEDUPLICATE]`
  and a background scheduler rewrite a table's files with Replacing and Summing rows merged as
  `FINAL` merges them, committed as Iceberg `replace` snapshots that a concurrent commit
  always wins. `INSERT … SELECT … FINAL` now reads merged rows.
- **Loams House writes lake tables** (HS1 Task 12): `INSERT` in every declared input format,
  `VALUES` and `INSERT … SELECT`, `CREATE TABLE … AS SELECT` and `TRUNCATE`. Rows become
  Parquet data files with Iceberg statistics and are committed as Iceberg snapshots before
  the client is answered (group commit per table, deduplication by
  `insert_deduplication_token` or by the data's hash, rebase and retry on conflicts). One
  statement writes at most 1 GiB.

### Changed
- **`loams-agentd` supervises new threads by default, on your own machine too.** Unset,
  `[approvals] default_mode` is now `supervised` on a local daemon as on the remote
  listener, so a thread that has no approval mode of its own (including older threads that
  never had one set), and a run group created without one, asks before each tool call.
  **Existing local users will start seeing approval prompts** for threads that used to run
  unattended; Pi threads without a mode are refused until set to full access. Full access is
  now always asked for: choose it in Studio's Settings › Approvals, call
  `SetThreadApprovalMode`, pass a run group's `approvalMode`, or set
  `default_mode = "full-access"` in the daemon's `config.toml` to keep the old behaviour. An
  engine embedded with `EngineCore::assemble` is supervised too; pass
  `ApprovalSettings::full_access_default()` to `assemble_with_approvals` for the old default
  (threat model I-1, owner decision 2026-10-15).
- **Loams Live on the main port.** `LiveService` and `LiveAdminService` are served on the
  engine's `--listen` port and on the `tailscale serve` target, behind the engine's auth, so
  the console and SDKs reach Live over the tailnet (LV1 Task 12). Until Live has TLS of its own
  (LV1 Task 18) a main port bound beyond loopback does not serve it. `--live-listen` still
  serves the same API on Live's own loopback listener and is deprecated.
- **The Swift SDK deploys through `live.admin`.** `client.live.admin` wraps
  `LiveAdminService` (deploy, list and get deployments, activate, index status, log tail), as
  in the TypeScript and Rust SDKs; `client.tables.deploy` is deprecated and forwards there,
  since the engine no longer serves `LiveService/Deploy`. Paged calls now follow the wire's
  `nextPageToken`.
- **The desktop app reaches Live on the engine's main port** and no longer passes the
  deprecated `--live-listen` to an engine that serves Live there, so its engine log has no
  deprecation warning. If another engine holds Live's default port (127.0.0.1:7710), it still
  moves Live's own listener so the engine starts.
- **The console's Live deployments page talks to the engine.** It uses `LiveAdminService`
  (deploy history, rollback by activation, a deployment's functions with their validators,
  and the function log tail) instead of sample data, and is on in the browser console, no
  longer under "Demo data". It needs an engine that serves Live on its main port (LV1 Task
  12). Worker health is not shown until the engine reports it (LV1 Task 27).
- **Loams Live switches every node at an activation's tick.** Every catalog change (a table
  created, an index or validator changed, a deployment activated) is recorded in the commit
  journal in its own transaction, and each node's subscription manager reloads at the tick
  that holds it (LV1 Task 11). A deploy or rollback on one node no longer takes up to 60
  seconds to reach live queries on another.
- **Loams Live serves many apps per engine.** `LiveAdminService` gains `CreateApp`,
  `ListApps`, `GetApp`, `UpdateApp` and `DeleteApp`; each app has its own keyspace, and a call
  names its app with the `loams-live-app` header (the engine's own `--live-app` without it).
  An app's engine starts on its first request and stops after 10 minutes idle. A disabled or
  deleted app refuses calls (`live_app_disabled`), and a deleted app's data is purged after 7
  days (LV1 Task 13).
- **Loams Live errors carry a stable reason.** Every Live error has a
  `loams.errors.v1.ErrorInfo` with a registered reason (new: `live_function_error`,
  `live_function_timeout`, `live_function_out_of_memory`, `live_limit_exceeded`). A function's
  own error is now Connect `unknown` rather than `internal`, a CPU timeout `deadline_exceeded`.
  An internal error no longer shows raw store keys; it names a request id the server log
  carries. Function arguments are limited to 4 MiB and bundles to 16 MiB before decoding (a
  bundle over 4 MB can now be deployed), and a read timestamp ahead of the server's clock is
  refused (LV1 Task 19).
- **Loams Live authorizes every call per app and per table.** A credential bound to an
  environment (a `KeyService` key, or a token with an `env` claim) reaches only that
  environment's app (`live_app_mismatch`) and addresses it without the `loams-live-app` header.
  Scopes `live:call`, `live:read`, `live:write` (or `live:read:<table>`/`live:write:<table>`),
  `live:deploy` and `live:admin` gate functions, `_system:*` reads and writes, deploys and the
  app directory; a credential that names no scopes keeps every scope. Deploys and writes are
  audited in the log (LV1 Tasks 14 and 15).
- **Loams Live apps have end users.** An app's trusted issuers (`CreateApp`/`UpdateApp`
  `issuers`: any OIDC provider, by discovery or a `jwks_uri`) now sign its end users in: a
  token from one of them calls the app's public functions on `LiveService`, and
  `ctx.auth.getUserIdentity()` returns `{ issuer, subject, tokenIdentifier, email, name, claims }`
  (null otherwise). End users never reach internal functions, `_system:*` or
  `LiveAdminService`. A `LiveService` call with no credential is admitted only when the app's
  `allow_anonymous` is on; otherwise it is `unauthenticated` with the reason `token_missing`
  in the error's `ErrorInfo` (no longer in the `loams-auth-reason` header) (LV1 Task 16).
- **TLS on the main port, and Loams Live beyond loopback.** `LOAMS_TLS_CERT_FILE` and
  `LOAMS_TLS_KEY_FILE` put `--listen` on TLS (HTTP/2 and HTTP/1.1 by ALPN), with
  `LOAMS_TLS_CLIENT_CA_FILE` for client certificates; `LOAMS_TLS_TERMINATED_BY_PROXY=true` with
  `LOAMS_TLS_TRUSTED_PROXIES` (addresses or CIDR blocks) takes plain connections from those
  proxies only. Live is now served on a non-loopback `--listen` behind TLS (either way) and
  authentication. Browsers on an app's `allowed_origins` get CORS on `LiveService`, and a call
  from an origin the addressed app does not list is refused with `live_origin_not_allowed`
  (the console's origins keep working for platform credentials). `Access-Control-Allow-Credentials`
  is never sent (LV1 Task 18).
- **A non-loopback `--listen` with Loams Live needs TLS and authentication.** Without them the
  engine refuses to start with `--listen listen on <addr>: non-loopback addresses need [tls] and
  [auth] (MT1)`. Set the `LOAMS_TLS_*` variables and `LOAMS_AUTH_MODE`, bind `--listen` to
  loopback (the compose stack does), or pass `--no-live` (LV1 Task 18).
- **Loams Live sessions follow who is signed in.** `LiveService/Authenticate{session_id}` replaces
  an open session's identity with the call's credential: the same user only extends the session,
  another user (a sign-in or sign-out) bumps `StateVersion.identity` and reruns every query as
  them in one Transition. Subscriptions are keyed by who runs them, so a result is never shared
  across users. A session closes with `unauthenticated` (`token_expired`) 60 s after its token
  expires, with `token_revoked` when its API key is revoked or its member removed, and with
  `unavailable` (`session_rotate`, new) after 24 h; clients resume (LV1 Task 17).
- **Several Loams Live nodes serve one store.** Give each node its own `--live-node` and the
  URL its peers reach it at (`--live-advertise`, its main port). A `ModifyQuerySet` or
  `Authenticate` that lands on a node that does not hold the session is forwarded to the one
  that does, with the caller's own credential, so a load balancer needs no session affinity; a
  session whose node is gone is `not_found` (`session_gone`, new) and the client resumes it. On
  `SIGTERM` a node drains: `/ready` answers `503`, its streams close and new calls are refused
  with `unavailable` (`node_draining`, new), and running mutations finish first (LV1 Task 24).

## [0.1.0-preview.2] - 2026-10-15

The second preview. Everything marked **Preview** in the [feature status table](docs/release/preview.md#feature-status)
ships in it, built from `dev` with the work held back from preview.1 merged. Same shape as
preview.1: you run the stack with Docker Compose over your own Tailscale tailnet and use the
hosted console. Images are tagged `0.1.0-preview.2` and the rolling `:preview` (never
`:latest`). Formats can change between previews and there is no upgrade path: start a fresh
stack (`docker compose down -v`) rather than reusing preview.1 volumes. Start with
[`docs/release/preview.md`](docs/release/preview.md).

### Added
- **Loams House on your tailnet.** `--profile house` now starts the House with its own
  Tailscale sidecar, `house-tailscale`, which serves the ClickHouse HTTP interface at
  `https://loams-house.<tailnet>.ts.net` (TLS from the tailnet's certificates; user `default`,
  password in `house.key` in the `house-data` volume). The House runs on a compose network of
  its own, so nothing else in the stack can reach it and its workers reach nothing of the
  stack. New since preview.1: lake tables with a catalog and DDL (HS1 Task 10), lake reads and
  `system` tables (Task 11), and `house-cache`, the S3 read proxy (Task 9).
- **House security layers on by default.** chDB's own access control is the first line (HS1
  ruling R-ACL.1: worker users have no `SOURCES` grants and a constrained settings profile;
  `open_acl` is refused outside a test build), control DDL is checked against it (R11.11), the
  statement checks (L1) stay as the second line, and workers now run sealed (L3): the image's
  default and the compose default are `--sandbox pods` (Landlock with TCP rules and seccomp,
  no privilege needed), and `HOUSE_SANDBOX=netns` adds a user and network namespace per worker
  where the container allows unprivileged user namespaces (Podman's default).
- **Loams Graph durability.** A fenced single owner per graph with forwarding between graph
  nodes (GR1 Task 12, D744), snapshots in the bucket with a graph manifest (Task 13), and
  crash recovery, rehydration and idle eviction (Task 14). Grafeo is built from Loams' fork,
  `ostrium-labs/grafeo` v0.5.43-loams.1.
- **One console, on Connect.** The React console is retired: every page is now a cordis plugin
  that talks to the engine over Connect (`loams.auth.v1`, `loams.admin.v1` and each product's
  service), in the browser at `console.loams.dev` and in Loams Desktop. The engine serves
  `loams.auth.v1` and `loams.admin.v1` (orgs, projects, agents, keys, audit) on its main port.
  The hosted console signs you in from your **tailnet login** (the engine asks Tailscale who
  you are; no password), with a Loams account at auth.loams.dev, or with an API key; a fresh
  engine opens on setup.
- **Studio.** A second mode of the same console (the desktop's default): projects, threads in
  their own git worktrees, run groups and compare, and the Data Studio, on the desktop daemon
  (`loams-agentd`).
- **Loams Live deploys, on the main port.** `LiveAdminService` with deploy, rollback, index
  status and function logs (LV1 Task 10), validators (Task 8) and online index changes
  (Task 9), now served with `LiveService` on the engine's main port behind its auth and over
  the tailnet (Task 12); deployments record who made them. The console's Live deployments view
  reads your engine. Catalog changes ride the commit journal, so a deploy reaches every node at
  once (Task 11).
- **Loams Postgres control plane in the stack.** `--profile postgres-control` adds `pg-control`
  and `pg-runner` (new image `loams-pg-control`): pg-control provisions computes and endpoints,
  and only `pg-runner`, on an internal network with no ports, holds the container runtime's
  socket and starts nothing but the pinned compute images (PG2 rulings R-REL.1–6). A
  Kubernetes compute runtime is in as well (Task 13).
- **Desktop and CLI on macOS and Windows.** Loams Desktop ships for macOS (arm64, x64) and
  Windows (x64) beside Linux, and the `loams` CLI as macOS arm64/x86_64 tarballs and a Windows
  zip beside the Linux packages. Preview builds are unsigned: macOS is ad-hoc signed, not
  notarized; Windows is unsigned (SmartScreen warns). Fixed on the way: the Windows engine build,
  the macOS x64 timeout, the missing arm64 signature ("damaged" on Apple silicon), the CLI Linux
  packages, and a cookie race when leaving a server in the desktop app.
- **SDK release pipeline.** One workflow builds, tests and publishes the seven supported SDKs
  from the release tag: npm `@loams/client` and `@loams/live` (dist-tag `preview`), PyPI `loams`
  (`0.1.0a2`), crates.io `loams-proto`, `loams-live-proto` and `loams`, NuGet `Loams`, the Go
  module `loams.dev/go` (tag `sdks/go/v0.1.0-preview.2`), Swift Package Manager from this
  repository's tag, and a C++ CMake source archive on the GitHub release. Each registry
  publishes once it is enabled for the project; see the preview guide for which are live.

### Changed
- The engine accepts browser calls only from `https://console.loams.dev` by default
  (`LOAMS_CORS_ORIGINS` in `deploy/release/.env`; add your own console's origin there).
- Workspace and image version `0.1.0-preview.2`; `deploy/release` (compose, `.env.example`,
  `byoc/install.sh`) defaults to it.
- `loams-fabric`'s image runs its workers sealed (`--sandbox pods`) instead of
  `--sandbox none --unsafe-no-sandbox`. A worker whose sandbox cannot be put in force refuses
  to boot and the House stops (Landlock needs Linux 6.7 or later; `netns` cannot start under
  Docker's default seccomp profile).

### Fixed
- The ClickHouse HTTP interface returns one document per result for any row count (HS1
  R8.4/R8.12). This fix was already in preview.1, whose notes still listed it as pending.

### Known limitations
- **House writes:** `INSERT` into House tables arrives with HS1 Task 12; this preview serves
  queries over the allowed table functions, lake-table DDL and reads, and `system` tables.
- **The engine does not reach the House yet.** The engine's link to it needs TLS (HS1 Task
  20), so `loams.house.v1` and the console's House page still answer `house_not_configured`.
  Use the House directly over its tailnet URL with any ClickHouse HTTP client.
- **Loams Postgres:** the compute images are still Neon's last public build until the fork's
  own builds (NF1 Task 14); the link between pg-control and pg-runner is plaintext on its
  internal network until PG2 Task 23.
- **Not in this release** (built on `dev`, held): Loams Git (the bucket WAL, ref log,
  checkpoints, forks, GC and `loams.repos.v1`'s catalog are built, but there is no push or
  fetch path yet: no `git-remote-loams`, shim or Smart HTTP), first-party connectors (CN2:
  the native connector families are built and tested, but no image hosts the runtime and
  several families are not done), Loams Jobs, the Loams SQL gate (`sqldb`), and Headscale.
- See [`docs/release/preview.md`](docs/release/preview.md#known-limitations).


## [0.1.0-preview.1] - 2026-10-12

The first release of Loams, v0.1.0-preview.1, published as a **preview**: usable, tested in CI, not production
ready. Everything marked **Preview** in the [feature status table](docs/release/preview.md#feature-status) ships in
it, and formats, APIs and flags can still change before the next preview. The entries after
the known limitations, recorded before the preview was cut, are part of it too. Start with
[`docs/release/preview.md`](docs/release/preview.md).

### Added
- **Self-hosted preview stack.** `deploy/release/compose.yaml` (with `.env.example`) runs the
  whole stack with Docker Compose, locally or on your own VMs (BYOC) over SSH: `pd`, `tikv`,
  `tidb` (`ghcr.io/ostrium-labs/tidb:v8.5.8-loams.1`), `loams`, `house` (`loams-fabric`), the
  Postgres stack included from `deploy/loams-postgres-dev`, an optional local `console`, and a
  `tailscale` sidecar (`TS_AUTHKEY`, `TS_HOSTNAME=loams`). The `house` profile starts the
  House (not wired to the engine in this release). The `headscale` profile and `TS_LOGIN_SERVER`
  exist but are coming soon and unsupported. `deploy/release/byoc/install.sh` installs the stack
  on your own VM over SSH. Only the `tailscale` service is reachable
  from outside; nothing binds `0.0.0.0` on the host by default.
- **Container images** on `ghcr.io/ostrium-labs`, built by `deploy/release/build.sh` from
  `deploy/release/Containerfile.<name>` and published by CI on tags as `0.1.0-preview.1`
  and the rolling `:preview` (never `:latest`): `loams` (the engine, with `live,live-tikv,durable,graph,tikv,pgwire,mysql-wire,stream-grpc`),
  `loams-fabric` (the House front, with `loams-house-worker` inside) and the optional
  `loams-agentd`. The patched TiDB image `ghcr.io/ostrium-labs/tidb:v8.5.8-loams.1` is public, amd64 only.
- **Engine auth.** `LOAMS_AUTH_MODE` is `tailscale` (the default: the identity `tailscale serve`
  adds, trusted only on the dedicated `LOAMS_TAILSCALE_LISTEN` listener `127.0.0.1:7781`, narrowed
  by `LOAMS_TAILSCALE_ALLOWED_USERS`, CORS limited to `console.loams.dev`), `oidc` (JWTs verified
  against `LOAMS_OIDC_ISSUER`'s JWKS, audience `LOAMS_OIDC_AUDIENCE=loams-console`, and one of
  `LOAMS_OIDC_ALLOWED_EMAILS`, `_SUBJECTS` or `_GROUPS` required), `apikey` (hashed keys, created
  with `loams auth new-key`) or `off` (loopback clients only).
- **Hosted console.** Ostrium hosts the React console at `https://console.loams.dev`. It calls
  your engine at `LOAMS_ENGINE_URL` over your own Tailscale tailnet. The shipping path is your
  tailnet plus the hosted console. Coming soon: the console on your own Vercel or Cloudflare,
  `auth.loams.dev/register` self-service clients, Headscale networks, one-click OAuth deploy, and
  House/Analytics wired to the engine.
- **SDKs (preview, from the repository).** Seven SDKs build and pass the shared conformance suite
  against a live server in CI: TypeScript, Python, Go, Rust, C#, C++ and Swift (`sdks/<language>`).
  They are not yet published to package registries. Coming soon: Java, Kotlin, Ruby, PHP, Dart and
  Objective-C (present in `sdks/` but not yet built or tested in CI), and published packages.

### Known limitations
- Loams Postgres: the control plane runs (PG2 Tasks 1 to 9), but computes and endpoints are not
  provisioned through it yet.
- Loams Graph: writes reach the Loams log before they are acknowledged (GR1 Task 11), but crash
  recovery (GR1 Task 14) is not done.
- Loams Live: a change can take up to 60 s to reach another node; the versioned deploy flow is
  in progress.
- Loams Git, connectors and jobs are not in this release.
- Loams House: multi-block results in JSON, CSVWithNames, Parquet or Arrow are malformed over
  the ClickHouse HTTP interface until a pending fix (HS1 ruling R8.4); `loams.house.v1` is not
  affected.
- Several console pages still run on mocked clients; House and Analytics are not wired to the
  engine.
- See [`docs/release/preview.md`](docs/release/preview.md#known-limitations) for the full list
  and the security model.


### Changed
- **Loams Live runs on the embedded store by default** (LV1 plan Task 23, design §45 §8). The `live` feature is in `loams`'s default set and has no `tikv-client`. `loams dev` and `loams standalone` serve Live on 127.0.0.1:7710, with data in an embedded MVCC store at `<data-dir>/live/store.redb` (redb), and need no PD.
  - **TiKV is the new `live-tikv` feature:** `--live-store tikv://<pd>[,<pd>]/<keyspace>` selects it. A build without `live-tikv` refuses that flag.
  - **`--live-pd` and `--live-keyspace` are deprecated**, and stay one release as aliases of `--live-store tikv://…` with a warning. A build without `live-tikv` refuses them, as it refuses `--live-store tikv://…`. It never runs Live on local data when a cluster was asked for. The desktop's engine builds add `live-tikv`.
  - `--no-live` is unchanged.
  - "Loam Live" is now "Loams Live" in messages and docs.
  - The embedded store: snapshot isolation with first-committer-wins, group commit, an oracle that survives restarts, and incremental GC (LV1 Tasks 21 to 23).

### Added
- **Loams Desktop (Electron)** (design §37 §19, D652–D679, plan AP1e; `apps/desktop-electron`). The shipping desktop app, replacing the Tauri and zeron tracks (AP1n is paused).
  - A cloud console against any server (Loams Cloud, BYOC, self-hosted, apps mock), with a server switcher, a product navigation and a docked agent panel.
  - A supervised local engine (`loams dev`), with Data Studio (browse, hybrid search, SQL, ingest).
  - Local Postgres (Neon) and WeSQL stacks via podman or docker compose, with branches, connection details and SQL consoles; TiKV for Live; pages for Live, Durable (Resonate), Streams & Links, Connectors and Graph.
  - Software Factory: Forgejo, Zulip, ItsAPlan, GlitchTip, OpenPanel, Matomo, Langfuse and OpenObserve, as read-only panels and embedded full UIs. Credentials stay in the main process under `safeStorage`.
  - An agent panel with Anthropic and OpenAI-compatible providers, read and write tools, and approval for every write; SQL tools run least-privilege.
  - Linux packages (AppImage, deb, rpm, pacman; x64 and arm64), a signed Windows installer through SignPath, unsigned macOS builds, and an update feed with signed manifests. See `docs/release/desktop.md`.
- **Core plugins renamed to `@loams-core/*`** (under `plugins/core/`): charts, flows, chart-specs, bi, dashboards, data, store, agent-tools, http, live, engine, host, types, bi-rpc, dashboard-ui. Factory adapters keep their names. `SUPERSET_*` environment variables are still read as deprecated fallbacks, and `add_flint_widget` / `flintSpec` stay as deprecated aliases for one release.
- **Backend production plans (docs only)**: LV1 Loams Live, PG2 Loams Postgres, SQ1 Loams SQL, GR1 Loams Graph and HS1 Loams House, with designs 45 to 49 (D680–D779). No code changes.
- **WebMCP tools generated from the plugin action registry, with OpenFGA and approval enforcement** (design §42 §5, D569, **D636**, AP1d Task 2). The repository had **no plugin action registry** — Task 1's seam had no notion of a tool's meaning — so `@loams/console-host` grows `ActionRegistry` (`src/actions.ts`), provided by the host as the **`actions` service** beside `slots`. A row is a name, a description, an input schema, a **required** risk (`read` / `write` / `destructive`, §39 §8 rule 1's tags) and an execute closure — the risk has no default because it is the one field that decides whether a person is asked, and an absent one would be a silent `allow`. `register` is non-transactional and returns a disposer, mirroring `SlotRegistry.register`, so a plugin registers inside `ctx.effect` and disposing the plugin removes the row. A plugin may register only the names its manifest's new **`actions`** key lists — §37 §5.3's `slots` gate, applied to the callable surface. **Enforcement reuses the primitives rather than re-implementing them:** a method-backed action *derives* its permission from `METHOD_PERMISSIONS` through the existing `decideCall`, so a generated tool cannot be more permissive than the button behind it, and naming a different permission is refused at registration as drift; at call time `decideCall` decides again, so both halves of a bridged call apply — the service is in the plugin's `inject` list *and* the permission is held. A local action names one of `PERMISSIONS`. Client-side code can only ever **refuse** — OpenFGA and the credential broker remain the boundary. **A destructive action is never auto-approved:** §39 §8 rule 2 puts the gate in the credential broker and forbids the requester approving their own request, so the gate is a seam with no default — a generated destructive tool with no gate wired resolves `approval_required` and `execute` never runs, and the two checks a client can honestly make are that a decision came back and that its `decidedBy` is not the requester. Rule 2 says the call *carries* the approval id for the broker to verify, so it reaches `execute` as `context.approval` rather than being checked and dropped; it is absent when no gate was needed. A `write` whose environment was never resolved to be unprotected is likewise gated — rule 1's default is "allow in non-protected environments", a claim about a named environment — and the environment a generated tool acts in is bound at generation by the host, never per call, since an agent naming its own `protected: false` would be approving its own request. **Refusals travel in the resolved value**, because the WebMCP draft reports a rejected `execute` as an opaque `UnknownError`: every outcome resolves `{ok:true,value}` or `{ok:false,error:{code,message}}` with codes `unknown_action`, `permission_denied`, `approval_required`, `approval_denied`, `invalid_input`, `aborted`, `failed`. Tool annotations are derived from the action rather than described: `readOnlyHint` follows the risk, `consequentialHint` is its inverse, and `untrustedContentHint` is **always true** because the input is an agent's (§39 §8 rule 5). Action names are 1–128 characters of `[A-Za-z0-9_.-]`, the intersection the WebMCP tool-name rule and MCP's own both require, because D569's "one description per action" means §30's stdio server reads these rows too — that consumer is **not** wired yet, and D289 omits destructive tools outright. With `document.modelContext` absent (per **D635**, the normal case on Safari) nothing is registered and `WebMcpTools.register` still resolves `undefined`, so the console is unchanged.
- **The console's WebMCP feature detection, tested in a real browser** (design §42 §5, **D638**, AP1d Task 4, GitHub issue #273). `document.modelContext` is `[SecureContext]`, so it does not exist under jsdom and `web/apps/console/test/webmcp.test.ts` can only ever compare the module against an object the test itself builds. A new `web-browser` CI job launches the runner's Chrome, serves a fixture from loopback so the page is a secure context, and loads `src/webmcp/index.ts` through TypeScript's own transpiler — so it runs the module as written, not a bundled copy. **It asserts that the console agrees with whatever the browser actually reports, in both directions**, because the API's availability moves with the Chrome channel: measured 2026-10-05 under the same `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`, **Chrome 149.0.7827.155** has no `document.modelContext` and both `navigator.modelContext` and `navigator.modelContextTesting` present, while **Chrome 154.0.8037.57** is the inverse — the real attribute is present, as a real `ModelContext`, and the `navigator` spellings are gone; on 154 it is present with **no feature flags at all**. This corrects the earlier reading of this entry, which asserted the absent-API path from Chrome 149 alone and called the real attribute unreachable token-free; it arrived unflagged five channel versions later, and the job's first run on 154 failed 8 of 12 assertions for exactly that reason. Absent → `detectModelContext` reports `not-exposed` and `register` resolves `undefined`; present → detection available, a handle returned, `exposedTo` carrying only potentially-trustworthy origins, and sign-out unregistering one abort per tool. **Unconditionally:** the `navigator` decoys are never mistaken for the document attribute (each channel rules out one half of that mistake, an identity check the other half), a plaintext `http://` origin never reaches `registerTool`, `clear()` is idempotent, and a partial registration failure keeps the working tools. **No new dependency and no browser binary in the lockfile** — Chrome over `Target.createTarget` and `Runtime.evaluate` on Node 22's built-in `WebSocket` — and every assertion holds whether or not the flags work, verified with the flags on and explicitly disabled on both channels, so a Chrome rename quiets the diagnostics instead of reddening the job. The browser's real answer is logged every run, so a channel change is visible rather than only a red build. A missing browser **fails** the job on CI and skips it locally. The Chrome channel is resolved from the runner image and its version printed, **not** version-pinned. The seam properties run against a spec-shaped `ModelContext` on the real document on every build, since no browser reports its own `registerTool` arguments back; where a native API exists the fixture shadows it and records that it did. D638 records which claims are primary (Chrome's WebMCP page; Chromium's `runtime_enabled_features.json5`) and which are **unverified** — the `--enable-features=WebMCPTesting` spelling and the name `navigator.modelContextTesting` came from a vendor integration doc, and that doc's `getTools()` is wrong; the surface has `listTools()`.
- **Loams Flow's connector registry** (design §33 D352–D359, CN1 Tasks 1–2, GitHub issue #211). `loams.flow.v1` protos (`fabric/proto/loams/flow/v1/{connector,instance}.proto`), the manifest schema `connectors/schema/connector.schema.json`, and `loams-flow` — the registry, its manifest loader, the six semantic rules and `check_use`, the D359 licence gate, and the `connectors gen|list|describe|validate` groundwork. **203 connectors** in `connectors/registry/`, generated from the single source `connectors/registry/catalog.csv`: 21 hand-written ★ manifests, 3 hand-written P1 collaboration targets (Zulip, ItsPlane and Forgejo, **D628**), and 179 generated stubs, each with a resolving instance-config JSON Schema. `scripts/connectors/` holds the generator plus the three drift checks against the real Camel and Kestra catalogs (`matrix.py --check` renders design §33's Appendix A from the CSV and fails CI on a divergence). The `fabric/` workspace, its `deny.toml` and `loams-flow-proto`. Every ★ connector's declared capabilities are proven by tests rather than claimed: `proto_and_yaml_agree` round-trips all 21 manifests through the protobuf form, and `registry_has_203_entries_and_21_starred` is the CN1 gate.
- `loams-web-bridge`: the v2 **WebMCP** tools `list_webmcp_tools` and `call_webmcp_tool` (design §37 §18.14.2, §42 §5, **D570**, **D637**, AP1d Task 3). Two tools added to the v1 toolbox contract, and their point is to be **preferred over snapshot-driven clicks where a page registers tools**: an agent calls the site's own function rather than finding a uid and clicking. Feature detection is **per call** and lives in the injected script, because `document.modelContext` is `[SecureContext]` and a page may register, unregister and re-register between two calls — so nothing is cached and nothing is inferred from the engine. **Absence is a typed answer, never an error and never a bare empty list**: the browser has no API (every WebKit engine, since WebKit's standards position is closed and `oppose`, D635), the page is not a secure context, the `tools` Permissions Policy does not allow this origin (its default allowlist is `['self']`, so only the page can fix it), the driver cannot evaluate a script, or the page refused — and every one of them names the way on, `take_snapshot` and `click`, because an empty list with no explanation is indistinguishable from a page that chose not to register anything, which is the one case where an agent should give up on WebMCP. **Both** scripts answer an absence the same way, so a call against a page with no API reports the absence instead of a tool that failed, and a `NotAllowedError` is typed from its **name** rather than from the whole string, because it is a DOMException whose message the page writes. A tool name is validated against the draft's own rule (1–128 characters of `[A-Za-z0-9_.-]`) and an `inputObject` must be a JSON object, both before anything is sent; `call_webmcp_tool` asks the page for its own list first, so an invented or stale name answers "not registered; list again" instead of silently doing nothing. D504 is honoured: a listing is one line per tool with descriptions truncated and the whole inside the token budget and a marker naming the continuation, and a call that ran answers in one line **plus a change summary measured by re-reading the page** — a tool is arbitrary page code, and its return value cannot say what it did to the page — while a call that did not run answers in one line only, because a change summary beside a call that never happened would be a lie. The driver seam is `PageDriver::webmcp`, whose **default refuses**, so a host that cannot evaluate a script needs no knowledge of WebMCP at all and degrades instead of failing; the input is marshalled as a JSON literal the page parses, never pasted into the script, and a hung tool is bounded inside the page as well as in the transport. The same contract is asserted against **both** providers and against a page with no API, including a bare `PageDriver` that never implemented the seam. **Not run against a browser that ships WebMCP**: the injected script is written against the 2026-10-02 draft's IDL and exercised against deterministic fixtures.
- `loams-web-bridge`: the web toolbox's browser provider abstraction (design §37 §18.14, §42 §4, AP1c). One `BrowserProvider` trait with two implementations behind it — a local (agent-driven) provider over a host-supplied `PageDriver`, and a Cloudflare Browser Run provider speaking CDP over its WebSocket endpoint — selected by `provider` in `[web_bridge]` and validated at load time. The tool contract implemented once for both: snapshots with `s7_12` uids, `find` with context, `click`, `fill`, `wait_for`, screenshots written with a SHA-256, and content extraction; budgeted, escaped, with URLs query-redacted and credential fields value-elided. Secrets by reference (`SecretRef` + `SecretResolver`), a `SecretValue` that cannot be printed, and a scrubber for every string that leaves the process. An egress policy (schemes, private and metadata addresses, deny and allow lists) enforced before every navigation, plus Browser Run session guardrails for sub-resources and redirects. Chromium is the default engine; Kitesurf is offered for stateless public-web work only and refused under a hostname allow list, because Cloudflare does not support guardrails there. A concurrency ceiling, a minimum gap between sessions and a per-UTC-day local budget bound the spend. Implemented against Cloudflare's documented interface and not yet run against a live account; see `docs/remote-browser-provider.md`.
- `loams-sidebar-browser`: the docked sidebar browser, on **Obscura over CDP** rather than a per-platform webview (design §37 §18.5, **D627**, superseding SF1 Task 0's ruling E1 and closing Q482). The `sidebar-browser` cargo feature is **on by default**. Obscura 0.2.3 runs as a pinned child process bound to `127.0.0.1` and is driven over `ws://127.0.0.1:<port>/devtools/browser`; frames come from `Page.startScreencast` and interaction is routed back as `Input.dispatchMouseEvent` / `dispatchKeyEvent`. No webview is forked or embedded: WKWebView and WebView2 are proprietary and cannot be forked, and forking WebKit is a multi-year effort. Each `(environment, app)` pair gets its own persistent profile directory and its own cookie ledger, so a session survives a restart and profiles are isolated. Loams runs the OIDC/Authentik ceremony itself and hands the engine only host-only scoped session cookies — the credential boundary is audited against the engine's cookie jar and the whole profile directory by `tests/token_boundary.rs`, including its negative cases. A loopback or RFC1918 embedded origin launches the engine with `--allow-private-network`; a public one does not, and the cloud metadata endpoint and the rest of link-local are refused by Loams whatever the engine's flag says. **Fidelity:** this is an agent-driven surface, not a general-purpose human-interactive browser — no scroll momentum, no caret, no text selection, no compositor-driven animation, and not pixel-identical to Chromium. `docs/wiki/The sidebar browser.md` states this for users; `crates/loams-sidebar-browser/README.md` states what would falsify the bound that makes it acceptable (the embedded targets are Zulip, Plane and Forgejo).
- **Linux release packages**: `.deb` (Debian, Ubuntu), `.rpm` (Fedora, RHEL, openSUSE) and **`.pkg.tar.zst` (Arch and CachyOS — CachyOS is Arch-based, so it takes the pacman package, not the `.rpm`)**. All three come from **one** nFPM config (`release/nfpm.yaml`) staged from one tree, so the `.deb` and the pacman package of a release cannot drift; plus `release/aur/PKGBUILD` and the documented AUR submission path (**D628**). The packages install `/usr/bin/loams`, the built console under `/usr/share/loams/console`, `LICENSE`/`NOTICE`/`README.md` under `/usr/share/doc/loams`, and the `loams.service` / `loams@.service` units under `/usr/lib/systemd/system`. Only the `loams` binary is packaged: `compat-replay`, `loams-wal` and the two `loams-specview` binaries are developer or Neon-internal tooling and are deliberately not shipped. Built from the **default feature set** (`es`, `flight`, `hnsw`, `qdrant`); `durable`, `tikv`, `mysql-wire`, `pgwire`, `stream-grpc` and `failpoints` are not in the packaged binary. Packages bake in the **release commit's** timestamp rather than the build machine's clock, so an **unsigned** package is byte-identical across builds of one commit (verified: identical sha256 for all three formats); a *signed* `.deb` is not, because a GPG signature embeds its own creation time.
- **Signing for the Linux packages, per format**: the `.deb` and the `.pkg.tar.zst` are signed at build time with the project GPG key (nFPM's `debsign` for the `_gpgorigin`, `gpg --detach-sign` for pacman's `.sig`), and **the `.rpm is built unsigned and signed by SignPath's `<rpm-sign>`** — nFPM cannot sign an RPM at all (**D629**). This closes the gap `docs/release/signing.md` named as the binding constraint: `release-sign.yml`'s `sign` job refuses to submit unless an `.rpm` is in the set (D625), and until now the repository built no packages at all, so it had nothing to sign. Every release carries `signing.json`, which records what is signed **read back off the finished artifacts** rather than off a flag, and the workflow fails loudly — naming what is missing — when dispatched with `require_signing: true` and no key is configured.
- **Windows: an unsigned `.zip` bundle** containing `loams.exe`, the built console and the licence files, built on `windows-latest` with the MSVC host and the two prerequisites `docs/build-from-source/windows.md` identifies (`protoc` for the six `build.rs` scripts, `nasm` for `aws-lc-sys`). **Not** an MSI or a setup `.exe`, and `loams.exe` carries **no Authenticode signature** (**D630**): there is no Windows certificate in this project (D620, Q615). A downloaded copy triggers SmartScreen's *Windows protected your PC* — an unknown-publisher warning, not a malware detection, cleared by `Unblock-File`; a locally built copy does not prompt at all.
- `scripts/release/make-repo.py`: builds **all three** repository layouts — an `apt` repository generated in Python (so it is testable on any machine, where `dpkg-scanpackages` is Debian-only), a `dnf` repository via `createrepo_c`, and a `pacman` repository via `repo-add` — and **refuses by default to build a repository from an unsigned package**, because a repository holding one is a public registry publishing unsigned artifacts. `--allow-unsigned` is for a tree a maintainer inspects locally, is never published, and `make-repo.py verify`, the publication gate, has **no** bypass. **No repository is published yet**: the `.rpm` is unsigned when packaging finishes and SignPath runs afterwards, so publishing would put a half-signed release in front of users (**D631**).
- `.github/workflows/release-package.yml`: builds the three Linux packages and the Windows zip for a published release, attaches them to the release, and appends a per-artifact signature table to the release notes — including the Windows unsigned warning, and an explicit statement of what is **not** in the release. Guarded by the same "published release on the release branch at the workspace version" check `release-crates.yml` and `release-sign.yml` run. The console build is `continue-on-error`: a TypeScript error in the console cannot stop the server packages from being produced.
- `docs/release/packaging.md`: what each package contains, the feature set that is and is not in the packaged binary, install and signature-verification commands per format, the systemd units and `/etc/default/loams`, the reproducibility rule, and the handoff for the two configuration values packaging needs. Run the packaging scripts' self-tests with `scripts/release/build-artifacts.py --self-test` and `scripts/release/make-repo.py --self-test` — both build **real** packages from fixtures, and both *report what they skip* rather than passing quietly. A new `packaging` job runs them on every pull request, because nFPM is the only thing that validates a packager field.
- `loams-sidebar-browser`: product-demo **video recording** (**D628**), built on the screencast path the docked panel already uses. `record::VideoRecorder` consumes the frames the panel already receives from `Page.startScreencast` and writes one playable `.webm` (VP9, default) or `.mp4` (H.264); it starts no browser and opens no CDP connection of its own. **Encoder: an external `ffmpeg`, chosen over a pure-Rust one and justified in the crate docs** — a pure-Rust path needs three new dependencies (PNG decode, encode, mux) and its only encoder-quality option is `rav1e`, which is heavyweight and slow on flat UI content, whereas `ffmpeg` adds zero Rust dependencies, gives `cargo deny` nothing new to check, and attaches the `libvpx`/`libx264` licence questions to a distribution of ffmpeg rather than to Loams. It is an *optional* runtime dependency resolved like the engine (`LOAMS_FFMPEG_BIN`, else `PATH`): the sidebar browser works without it and only recording is refused, with an error naming the variable. The required encoder is probed at `start`, so an `ffmpeg` built without `libvpx` fails immediately with the list of what it does have. **Starting twice is safe** (`AlreadyRecording`, no second screencast and no second recorder — proven against a fake CDP endpoint), **stopping an idle recorder is `Ok(None)`**, and **the failure path publishes the same playable file as the graceful one**: frames go to a scratch directory, finalisation encodes to a `.loams-partial` file that is fsynced and **renamed** into place, and nothing else ever writes to the output path — a half-written file is not a possible state. Every timestamp is a monotonic `std::time::Instant` delta (asserted by a source-level test), so a pause in the demo is a pause in the video at true speed and a clock step cannot corrupt a finished file. Stop, error and drop all release the screencast the recorder started, reap the encoder, and delete the scratch directory and partial. **Out of scope by ruling: audio capture of any kind, editing, and post-processing.** Fidelity inherits the panel's: no scroll momentum, caret or text selection, coarser-than-60 Hz updates, and not pixel-identical to Chromium. Tested against a fake CDP endpoint and a synthetic frame source, including opening the output with `ffprobe` and asserting the codec, dimensions, frame count and duration — **end-to-end playback with real Obscura is not verified here**, because the 70 MB engine binary is not installed on the build machine.
- `loams-sidebar-browser`: the docked sidebar browser, on **Obscura over CDP** rather than a per-platform webview (design §37 §18.5, **D620**, superseding SF1 Task 0's ruling E1 and closing Q482). The `sidebar-browser` cargo feature is **on by default**. Obscura 0.2.3 runs as a pinned child process bound to `127.0.0.1` and is driven over `ws://127.0.0.1:<port>/devtools/browser`; frames come from `Page.startScreencast` and interaction is routed back as `Input.dispatchMouseEvent` / `dispatchKeyEvent`. No webview is forked or embedded: WKWebView and WebView2 are proprietary and cannot be forked, and forking WebKit is a multi-year effort. Each `(environment, app)` pair gets its own persistent profile directory and its own cookie ledger, so a session survives a restart and profiles are isolated. Loams runs the OIDC/Authentik ceremony itself and hands the engine only host-only scoped session cookies — the credential boundary is audited against the engine's cookie jar and the whole profile directory by `tests/token_boundary.rs`, including its negative cases. A loopback or RFC1918 embedded origin launches the engine with `--allow-private-network`; a public one does not, and the cloud metadata endpoint and the rest of link-local are refused by Loams whatever the engine's flag says. **Fidelity:** this is an agent-driven surface, not a general-purpose human-interactive browser — no scroll momentum, no caret, no text selection, no compositor-driven animation, and not pixel-identical to Chromium. `docs/wiki/The sidebar browser.md` states this for users; `crates/loams-sidebar-browser/README.md` states what would falsify the bound that makes it acceptable (the embedded targets are Zulip, Plane and Forgejo).
- The cordis console (design §37 §5, AP1a, scaffold): `@loams/cordis` (cordis 4.0.0-rc.10, pinned), `@loams/slots`, `@loams/console-host` (the `loams.yml` catalog with no YAML tags, plugin manifests, host-decided trust tiers, the guard proxy, the permission model, the sandbox bridge, boot and the pending sweep), `@loams/platform-web`, the plugins `shell`, `rpc`, `identity`, `stack-status`, `namespaces` and `approvals`, the sandbox runtime and a third-party sample plugin; served at `/ui/cordis.html`, with a demo mode backed by an in-browser mock.
- Router track RT0 (§31 §22): TLA+ models `ShardMap` and `ReshardCutover`, checked with TLC and Apalache, plus skeletons of three more specs, and `scripts/spec/check.sh` with the `tla` CI job; Lean 4 proofs of the key-range partition lemmas and the `loams-router-oracle` differential oracle, with the `lean` job; `loams-sqlrouter`, the sans-I/O kernel (shard-map record, Vitess key ranges, Postgres hash partitioning ported from PostgreSQL 17, Vitess `hash` and `xxhash`, the `Machine`/`Ctx`/`TraceSink` seams); `loams-compat` and the compatibility inventory of PgDog→Postgres and Vitess→WeSQL; `loams-specview`, a live browser view of spec and test runs.
- `loams-es`: the Elasticsearch 8 Phase A REST API on port 9200, with document and bulk writes, document reads, search and count, Query DSL and vector search, multi-search, by-query writes, and index, mapping and alias administration.
- `loams`: `--es-listen`, `--es-namespace` and `--no-es` flags, with the `es` feature enabled by default.
- CI: an elasticsearch-py 8.19 client test against `loams dev`.
- App protos (design §37 §8, AP0): `loams.instance.v1`, `loams.devices.v1`, `loams.approvals.v1`, `loams.operations.v1`, `loams.notifications.v1` and `loams.errors.v1`; `@loams/proto` (generated protobuf-es v2 messages and Connect service descriptors, `web/packages/proto`); the `buf.gen.swift.yaml` and `buf.gen.kotlin.yaml` templates for `loams-mobile`; the `app-protos` CI job.
- `loams-apps-mock`: a stateful mock of the app protos over Connect, gRPC and gRPC-Web with seed data, the approval decision rules (`acceptance`), idempotency keys and resumable watch streams (AP0 Task 5, scaffold).
- Design documents (`docs/design`) and repository governance files.
- Design: durable execution through the Resonate protocol (§14); `arrow` segment encoding and changelog streams, adapted from Apache Fluss (§02).
- Cargo workspace, CI (fmt, clippy, tests, cargo-deny license policy).
- `loams-store`: object storage access with create-only and compare-and-swap writes, range reads, S3/GCS/Azure/local/in-memory backends from URLs, and `FaultyStore` fault injection.
- `loams-cache`: read-through RAM + NVMe byte-range cache with per-block crc32c verification.
- `loams-common`: namespace and stream id types.
- `loams-meta`: the metastore (namespaces, streams, the stream sequencer and offset index, leases with epochs, fenced pointer compare-and-swap), replicated with openraft, with the Raft log in a local redb database, snapshots in object storage, and in-process multi-node clusters.
- `loams-meta`: segment swaps (`SwapSegment`, lease-fenced), partition trimming and log start offsets (`TrimPartition`), per-stream retention (`SetRetention`), a 15-minute WAL commit window with pruning of commit records (`PruneWalCommits`), deterministic tracking of live WAL chunks and retired objects (`ForgetObjects`), an applied-index watch (`MetaNode::watch_applied`), and `MetaClient`, which follows the leader and retries.
- `loams-log`: the internal log on the `standard` WAL class: Kafka `RecordBatch` v2 records, the WAL object and segment formats (version 1, with the `encoding` field reserved), the leaderless write path (`LogWriter`), fetch with long-poll (`LogReader`), and the segmenter and retention loops.
- `loams` binary: `loams dev` and `loams standalone` run the metastore, the log and a native HTTP/JSON produce/fetch API in one process.
- `loams-worker`: the lease-based task framework: task sources, priorities, round-robin fair share across namespaces, per-task leases `task/<key>` renewed every third of their TTL and re-taken at the same epoch after a late renewal, fences and cancellation, and `run_once` for one deterministic pass.
- `loams-link`: the link framework with exactly-once apply (`LinkTarget`, `LinkApplySource`, one task per link) and the `CounterTable` test target, whose commits carry data and applied offsets under a fenced pointer CAS; `LinkGcRoots` for garbage collection.
- `loams-pk`: `PkIndex`, a fenced single-writer SlateDB per keyed object, and the read-only `PkReader`.
- `loams-log`: garbage collection (`gc::GcSource`) of retired objects, never-committed WAL objects, unreferenced segments and unreachable link objects, after a grace period.
- `loams-meta`: the link catalog (`CreateLink`), `ReacquireLease`, fences on `TrimPartition`, `PruneWalCommits` and `ForgetObjects`, `MetaState::check_invariants`, and `MetaNode::fatal_error`.
- `loams`: link endpoints (`POST /v1/namespaces/{ns}/links`, `GET /v1/namespaces/{ns}/links/{link}`); the server runs link apply and garbage collection; with the `failpoints` feature, `LOAMS_FAILPOINTS` / `LOAMS_FAILPOINT_HIT` abort the process at named failpoints.
- `loams-sim` (test-only): a seeded in-process cluster simulation and a Wing–Gong–Lowe linearizability checker.
- M0 exit gates: the kill -9 crash gate (`crates/loams/tests/crash.rs`), the object-store fault matrix (`crates/loams/tests/fault_matrix.rs`) and the simulation seed sweep, all in CI.
- `loams-store` / `loams-cache` (M1.1): `Store::get_range_with_info` and `RangeCache::read_with_size` (a known-size read, so a split opens with one GET).
- `loams-common` (M1.1): `CollectionId` and the collection schema types (`schema`).
- `loams-meta` (M1.1): the collection catalog: `CreateCollection` (with its implicit stream and link), `DropCollection` (retires the collection's prefixes), additive `UpdateCollectionSchema`, `UpdateAliases`, and the matching `MetaClient` methods; `ApplyError::CollectionNotFound` and `SchemaVersionMismatch`.
- `loams-log` (M1.1): `LogWriter::append_many`, which commits one request's batches for several partitions in one WAL object and one `CommitWal`.
- `loams-link` (M1.1): the link-target registry (`TargetRegistry`, `LinkTargetFactory`, `CounterTargetFactory`); links of an unregistered kind are reported by `LinkApplySource::unregistered`, never applied.
- `loams-quickwit` (M1.1): Quickwit files vendored file by file from `af0591a3` and adapted to Tantivy 0.26.2 (split bundle and footer trailer, hotcache, async directories and warmup, ES DSL → `QueryAst` → Tantivy, the doc-mapper query builder, aggregation merge glue, `StableLogMergePolicy`), with Quickwit's attribution in `NOTICE`.
- `loams-text` (M1.1): the Lucene-compatible `standard`, `english`, `simple`, `whitespace` and `keyword` analyzers with the original Porter stemmer; Quickwit's `Storage` over `loams-store` and `loams-cache`; splits that open with one ranged GET; the `OPDB` delete-bitmap format.
- `loams-collection` (M1.1): primary keys, `DocOp` and the implicit-stream record codec; value validation and ES dynamic mapping; `CollectionWriter` and `ConsistencyToken`; Lance datasets committed as detached versions over `loams-store`; the Tantivy schema mapping (JSON fields, sparse vectors); the `OPCM` collection manifest and `CollectionSnapshot` (the read API); the `collection` link target with PK-index repair, PK deltas (`OPPD`) and dead letters (`OPDL`); vector (IVF_PQ, IVF_RQ, IVF_HNSW_SQ) and `_pk` BTREE index builds as worker tasks.
- `loams-collection` (M1.1), GC and trimming: `CollectionGcRoots` (Lance reachability, splits, bitmaps, manifests, PK deltas and dead letters of the retained chain), `PkGcRoots`, GC of retired prefixes after the grace period, and `CollectionTrimSource`, which trims implicit streams below the oldest retained manifest.
- `loams` (M1.1): the server runs collection link apply, index builds, trimming and collection GC. `loams dev` gains hidden tuning flags for tests such as the crash gate: `--collection-trim`, `--collection-index-min-rows`, `--collection-index-delta-min-rows`, `--collection-retention-ms` and `--collection-index-poll-interval-ms`.
- Collection coverage in the M0 gates (M1.1): crash-gate rows for every collection commit failpoint (`collection.after_lance_commit`, `.after_split_put`, `.after_manifest_put`, `.after_cas`, `.after_pk_write`) and both index-build failpoints (`collection.index.after_lance_commit`, `.after_cas`); the fault matrix's `CollectionCommit` row; and a collection in the `loams-sim` simulation. Its workload writes upserts, patches (including patches that change nothing) and deletes through `CollectionWriter`, plus schema-invalid records appended directly, which link apply dead-letters. Its model check requires that the committed collection equals `fold_stream` of the implicit stream, that every acknowledged write is in the stream at its offsets, that every upsert and patch is there at most once (and never one of a failed write), and that the live manifest names no missing object. The crash gate starts each server child with core dumps off (`ulimit -c 0` through `sh`), so it needs a POSIX `sh`.
- `trait MetaStore` (M1.2a), in `loams-common`: the semantic metastore trait (D47), covering the whole client surface as named domain operations, implemented for the openraft `MetaClient`.
- `loams-meta-conformance` (M1.2a): the backend-agnostic `MetaStore` conformance suite (catalog, sequencer, leases, pointers, collections, GC, changes, the linearizability checker), run against the openraft single node and the in-process 3-node cluster; `metastore_conformance!` expands it for a backend.
- CI (M1.2a): the `metastore boundary` step, which fails when a downstream crate depends on `loams-meta` directly, `loams-common` gains a dependency, or downstream source names an openraft type.
- `loams-query` (M1.2): the read side of collections and the facade every gateway uses: the search IR and its JSON wire form; read views per consistency level (strong, at-least, eventual and pinned reads without a metastore hold); the in-memory tail index, which makes writes visible to strong reads before the link applies them; DataFusion operators for BM25 search with global live-only statistics, filter bitmaps, ANN over Lance with exact rescoring, exact sparse search, fusion (RRF, DBSF, weighted sum), document fetch and tail merge; aggregations, highlighting, sorting, paging, groups, counts and scroll; the hot-tier and placement hooks; `CollectionService`; a read-only DataFusion catalog with the SQL table functions `vector_search`, `text_search`, `hybrid_search` and `rrf` (`rerank` reserved for M3); an Arrow Flight SQL service; Flight `DoPut` bulk ingest into collections and streams (path descriptors with `PutAck` tokens, and ADBC `adbc_ingest`); scan plans that pin a collection's detached Lance version for external readers (D53).
- `loams` (M1.2): the native collection routes (`…/collections`, `…/fields`, `…/versions`, `/v1/namespaces/{ns}/aliases`), document routes (`…/documents`, `…/documents/get`, `…/documents/scroll`, `…/documents/count`), the hybrid query route (`POST /v1/namespaces/{ns}/query`), the SQL route (`POST /v1/namespaces/{ns}/sql`) and the scan plan route (`POST …/collections/{c}/scan`); the Flight SQL listener with `--flight-sql-listen` and `--no-flight-sql`; `--hot on|off` and the `Loams-Hot` request header.
- `loams-log` (M1.2): ranged WAL fetches: `LogReader` learns each chunk's batch boundaries and reads from the batch that holds the requested offset instead of the whole chunk.
- CI (M1.2): the nightly `sim` job also runs the tail-merge property test with 256 cases.
- Per-collection unapplied-data budget and write backpressure: HTTP 429 / `RESOURCE_EXHAUSTED` with `Retry-After`, `Loams-Unapplied-Records` and `Loams-Unapplied-Bytes` headers, the `Loams-Backpressure: off` override, `--backpressure` and `--max-unapplied-*` flags, `unapplied_bytes` and `backpressure` in `CollectionInfo` (D86).
- `loams-collection` (M1.3): background maintenance: split merges (`SplitMergeSource`, `collection-merge/<cid>`) that re-index live docs from `_source` with each split's own schema under Quickwit's `StableLogMergePolicy`, bounded at 10 M docs and 1 GiB, and Lance compaction (`LanceCompactionSource`, `collection-compact/<cid>`) committed as a detached `Rewrite`; both commit `Maintenance` manifests and rebase on conflict. `CachedObjectStore`, so Lance reads go through the range cache.
- `loams-hnsw` (M1.3): the `HnswEngine`/`HnswBuilder`/`HnswIndex`/`AppendableHnsw` traits, the `qdrant-edge` 0.8 engine (feature `qdrant`) and an exact `FlatEngine`.
- `loams-hot` (M1.3): the hot tier. Hot artifacts (the `OPHD` descriptor, the `OPHC` covered set, zstd-chunked engine files under `hot/hnsw/<column>/<source_version:020>-<ulid>/`), artifact currency and `HotBuildSource` (`hot-build/<cid>/<i>`); `HotTierImpl`, which loads artifacts on owners and serves per-manifest views with an appendable delta index; pinned splits on NVMe verified by block checksums; fragment prefetch into the range cache; budgets with per-namespace fair share, heat and demotion; lease-based auto-promotion (off by default); the node registry (`node/<id>` leases), rendezvous placement and `RemoteReadsImpl`; the hot on/off differential harness (feature `test-util`).
- `loams` (M1.3): `PUT …/collections/{c}/hot`, `POST …/collections/{c}/warm` and the hot status in `GET …/collections/{c}`; `--hot-pin-all`, `--hot-dir`, `--hot-nvme-bytes`, `--hot-ram-bytes`, `--maintenance` and hidden hot tuning flags; `loams warm <ns>/<collection>`; `loams cluster --roles meta,log,query,worker,gateway` with `--node-id`, `--listen`, `--advertise`, `--peers`, `--bucket`, `--data-dir`, `--zone` and `--replication`; the internal routes (`/internal/v1/{raft,meta,reads,hot,node}/…`); read forwarding to the owner with local fallback; the `meta-membership` task that evicts stale learners; multi-process cluster tests (feature `cluster-tests`).
- `loams-meta` (M1.3): the metastore over the network: openraft RPCs over HTTP (postcard), a networked `MetaClient` over a local replica on every node (voters on `meta` nodes, learners elsewhere), `join` and `leave`; `SetCollectionHot` and the `MetaStore` operations `set_collection_hot`, `collection_hot` and `leases_with_prefix`, with conformance cases; the conformance suite runs over HTTP on the leader and on a learner.
- Gates (M1.3): crash-gate rows for the merge, compaction and hot-build failpoints; fault-matrix rows for `SplitMerge`, `LanceCompaction`, `HotBuild` and `HotLoad` (`FAULT_MATRIX_COMPONENTS` runs a subset and refuses unknown names); the simulation runs merges and compactions and admits writes against the backlog budget.
- `loams-qdrant` (M1.4): the Qdrant-compatible gateway (Phase A) over `CollectionService`: Qdrant 1.19.1's REST API (6333) and gRPC API (6334, stubs from nine vendored Qdrant protos): collections, aliases, named dense and sparse vectors, payload indexes, point writes and reads, the universal query (prefetch, RRF and DBSF fusion, rescore, sparse nearest with `params.idf`), `recommend`, `discover`, `context`, MMR, groups with lookups, the legacy search, recommend and discover routes, and snapshots as manifest versions. Its divergences from Qdrant are listed in its crate docs and design §06 §8.
- `loams` (M1.4): the feature `qdrant` (default on) and the flags `--qdrant-listen`, `--qdrant-grpc-listen`, `--qdrant-namespace` and `--no-qdrant` on `dev`, `standalone` and `cluster`.
- CI (M1.4): the `qdrant-client` job runs the official Python `qdrant-client` 1.15.1 and 1.19.1 against `loams dev`; the `check` job builds the `loams-qdrant` docs with warnings denied.

### Changed
- **Unified the Loams identity** (D33, D400–D402, D407). Rust crates use `loams-*` and paths `loams_*`; the server binary is `loams` (`loams dev`) and the WAL binary is `loams-wal`. Protobuf packages use `loams.*`, CloudEvents types use `io.loams.dev.*`, npm packages use `@loams/*`, environment variables use `LOAMS_*`, HTTP headers use `Loams-*`, the local data directory is `.loams/`, and Kubernetes labels use `loams.dev/*`. The pre-release protobuf migration is wire-breaking; there are no engine compatibility aliases.
- `loams-query` (M1.4): a JSON numeric `range` compares by value on integer and float columns (a fractional bound no longer matches the integer Tantivy truncates it to, nor widens a float column); each `must_not` clause is wrapped in `Excluded`, so a `must_not` phrase no longer panics in debug builds.
- `loams-meta` (M1.3): a new snapshot format version (the version-5 body followed by the hot configuration), written only while some collection has a hot configuration, so every earlier state still snapshots as version 5; `MetaClientConfig.apply_wait`; a 4xx answer to a metastore POST is a definite failure, and a membership request tries the remaining seeds after one rejects it.
- `loams-collection` (M1.3): `LanceEnv::with_cache`, which the server now uses; `CollectionGcRoots` keeps the hot artifacts of retained and lingering manifests; `PointerCas` and `put_manifest` are public.
- `loams-query` (M1.3): `RemoteReads::{get, count, scroll}` return the owner's read token; `HotTier::quarantine_split`.
- Workspace (M1.3): `serde_json` is built with `preserve_order` (qdrant-edge enables it), so `_source` and payload keys keep insertion order, as Elasticsearch and Qdrant return them (reverses M1.1 Ruling 23).
- `loams` (M1.3): the server runs merges, compaction and an idle hot tier by default; the `hnsw` feature (default on) builds `qdrant-edge`.
- Workspace (M1.1): `serde_json` is built with `float_roundtrip`, so `_source` floats survive exactly.
- `loams-store` (M1.1): `StoreError::is_retryable` (true for `Backend`, `AlreadyExists` and `PreconditionFailed`); `FaultyStore`'s `Precondition` fault on a create-only PUT now writes an absent object before it reports already-exists (a lost acknowledgement, as a real store would show it), and the fault matrix's `LinkCommit` and `CollectionCommit` precondition cells are re-blessed.
- `loams-meta` (M1.1): the snapshot envelope is format version 5 (the collection catalog); version-4 snapshots are rejected. Stream, link, collection and alias names starting with `_` are reserved for implicit objects; `SetRetention` refuses a collection's implicit stream, and `CreateLink` refuses an implicit stream as source and a `collection` target.
- `loams-log` (M1.1): `GcRoots::reachable` returns a `GcKeep` (objects and whole prefixes to keep, from one call), and `GcRoots` gains `object_time_ms` (the root's notion of an object's age); GC deletes everything under a retired prefix once it is older than the grace period.
- `loams-link` (M1.1): `LinkApplySource::new` takes a `TargetRegistry` (replacing the store argument and `with_hook`); `LinkError::Target`; the link endpoint shows `version` and `applied` for every registered kind.
- `loams-pk` (M1.1): a create-only SlateDB manifest write that lost a race is `PkError::Store` (retryable), not `Corrupt`.
- `loams` (M1.1): startup rejects any freshness deadline at or above `gc.grace` (`segmenter.swap_deadline`, `link.max_commit_delay`, `collection.index_commit_delay`) with `ServerError::Config` instead of clamping it; `--gc-grace-ms` also sets every freshness deadline (including the collection commit and index commit delays) to half the grace.
- `loams-log` (M0.4): the segmenter and retention run as worker tasks (`SegmenterSource`, `RetentionSource`); their background loops and `BackgroundTask` are gone. The segmenter deletes a segment it wrote only when its swap was definitely not applied. `SegmenterConfig` loses `interval` and gains `swap_deadline`.
- `loams-meta` (M0.4): the snapshot envelope is format version 4 (link catalog, incremental byte counts); `PartitionState::bytes` is O(1); `max_clock_skew` defaults to 5 min; `MetaNode::shutdown` waits for the database file to close; a snapshot upload is bounded by a total deadline (`MetaConfig::snapshot_io_budget`).
- `loams-store` (M0.4): `ObjectInfo` carries the modification time; `FaultyStore` classifies PUTs by mode, adds `Precondition` and `Delay` faults, random seeded faults (`FaultyStore::random`) and `inject_nth`.
- `loams` (M0.4): `405 Method Not Allowed` uses the JSON error body.
- `loams-meta` (M0.4 review): `SwapSegment` carries the segment's `Freshness`, and `CasPointer` an optional one. The metastore refuses a reference to an object older than its deadline with `ApplyError::StaleObject`. GC ages objects by the metastore clock. `CounterTable` manifests are named `<version:020>-<ulid>.man`, and orphaned manifests are no longer adopted.
- `loams-store`: `file://` stores fsync each written file and its directory before a write returns, so a write that returned survives power loss.
- `loams-meta`: the snapshot envelope is format version 2; version-1 snapshots are rejected. `CommitWal` carries the WAL object's creation time.
- `loams-meta`: the leader refuses commands stamped more than `MetaConfig::max_clock_skew` (default 60 s) ahead of its own clock (`MetaError::ClockSkew`). `CreateStream` carries the stream's retention. `MetaClient::inject_lost_ack` needs the `test-util` feature.
- `loams-log`: a commit rejected after an attempt whose outcome was unknown fails its appends with `CommitUnknown` (they may be committed). `LogWriter::start` validates its config and returns a `Result`. Size retention keeps each partition's newest entry. The segmenter renews its lease before swapping and never deletes a segment the metastore references or retired.
- `loams`: every error, including malformed paths and queries and bodies over 16 MiB (413), uses the JSON error body; fetch `max_bytes` is capped at 16 MiB.
- `loams-meta`: a node with no local state refuses to start when the snapshot store already holds metastore snapshots, since its data directory was most likely lost (override with `MetaConfig::allow_fresh_start_with_existing_snapshots`). Transient object-storage failures while writing or reading a snapshot are retried instead of stopping the node.
- Downstream crates (`loams-worker`, `loams-log`, `loams-link`, `loams-collection`, `loams`) (M1.2a) take `Arc<dyn MetaStore>` instead of the concrete `MetaClient`; `loams-meta` is now a dependency only of the composition roots (`loams`, `loams-sim`).
- The shared metastore types, errors and limits (M1.2a) moved from `loams-meta` to `loams_common::meta`, byte-for-byte the same on disk, and are re-exported at `loams-meta`'s crate root. `MetaError::UnexpectedReply` carries the reply's `Debug` text instead of a `Reply`, with the same `Display` text.
- The linearizability checker (M1.2a) moved from `loams-sim` to `loams-meta-conformance`, re-exported as `loams_sim::linearizability`.
- `loams` (M1.2): metastore conflicts (`VersionMismatch`, `Fenced`, `LeaseHeld`, `LeaseLost`, `SchemaVersionMismatch`) answer 409 `conflict`, and stale objects (`StaleObject`, `StaleCommit`, `IndexMismatch`) 503 `unavailable`; every 503 carries `Retry-After: 1`. A produce onto an implicit stream (`_collection.*`) checks that each record's key decodes as a primary key of its partition (400 otherwise), and the HTTP produce decodes every record before it resolves the stream.
- `loams` (M1.2): server shutdown waits at most 10 s for in-flight Flight SQL calls, then aborts them, and stops `DoPut` ingests before their next chunk.
- Per-crate integration tests are one binary (`tests/it`, module per former file) instead of one binary per file, to cut build time and disk/RAM use; `loams-collection` keeps its named-failpoint tests in a second binary (`tests/it_failpoints`, `required-features = ["failpoints"]`), and `loams`'s kill -9 crash gate (`tests/crash.rs`) and object-store fault matrix (`tests/fault_matrix.rs`) are unchanged, each its own binary.
