# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- **The Temporal gateway ships as an image (preview).** `ghcr.io/ostrium-labs/loams-temporal` is
  built and attested with the other release images, and `compose.compute.yaml` runs it as the
  profile `temporal-gateway`: in the engine's network namespace, on its own `loams_temporal`
  database and the engine's Resonate, with API keys and no host port (deploy/release/README.md,
  "The Temporal gateway"). The Go and TypeScript SDK legs run through that profile on pull
  requests that touch the gateway. The gateway now refuses task-token keys shorter than 32 bytes.
- **Loams Desktop can update itself from product releases.** A `vX.Y.Z` release now keeps its
  signed update manifests (`latest*.yml` + `.sig`), and the desktop release run promotes them onto
  the `desktop-latest` feed, which Windows and Linux AppImage installs update from (deb, rpm,
  pacman and macOS get "Download vX"). Pre-releases are never offered. The update public key is
  pinned in `apps/desktop-electron/update-pubkey.txt` (the repository variable `LOAMS_UPDATE_PUBKEY`
  is no longer read); the release checks the signing secret matches it, and `desktop-promote.yml`
  verifies every signature and payload before it uploads (`dry_run` stages without uploading).
  **Not active yet:** until the owner sets the key (docs/release/desktop.md, "The update key"),
  releases attach no manifests, the feed step is skipped, and the in-app updater stays off.

## [0.1.2] - 2026-10-26

Highlights:

- **The console ships with your stack.** The engine serves the console built into its image at
  `https://loams.<tailnet>.ts.net/ui/`, same-origin with its API and reachable only on your
  tailnet; Loams Desktop carries the same console. The hosted console at `console.loams.dev` is
  retired, CORS allows no other origin by default, and Studio in the browser reaches remote
  agentd hosts listed in `LOAMS_AGENTD_ORIGINS` (calls and previews).
- **CLI packages install on Fedora and Arch.** The `.rpm` and pacman packages declared Debian
  package names (`libc6`, `libgcc-s1`) and no zlib, so dnf and pacman could not install 0.1.1's.
  Each format now names its own packages, zlib included (#1110).
- **`loams-agentd` for Linux as release tarballs** (x86_64 and aarch64), in the layout
  `loams-agentd install-runtime --from` reads, for remote agentd hosts without Loams Desktop.
- **The `kafka` compose profile (preview).** Ostrium's fork of Apache Iggy behind the Kafka wire on
  `127.0.0.1:9093`, off by default, not wired to the engine yet.
- **Durable on TiKV runs the Resonate fork's TiKV store** (FW1), for stacks that set
  `LOAMS_DURABLE_STORE=tikv://…` (Durable stays on SQLite by default, D1342). The keyspace layout
  is unchanged from 0.1.1, so `loams_durable` data carries over.
- **Loams House** (the chDB-based House in `loams-fabric`) takes more of the gate applications'
  DDL and queries on lake tables: Langfuse's and SigNoz's schemas, mutations, views,
  `Distributed` tables, `VersionedCollapsingMergeTree` and readable `AggregatingMergeTree`. chDB
  remains the House engine; the Rust House engine's crates are in the tree but not wired.
- **Software Factory apps (preview):** SigNoz replaces OpenObserve.
- **Security fixes:** the CodeQL alerts open at 0.1.1 (ReDoS in Studio, credentials in a
  connector script's output), RUSTSEC-2026-0381/0382/0383 (rustls-webpki) and RUSTSEC-2026-0386
  (tokio).

### Upgrading

**No migrations and no third-party pin changes.** PD, TiKV and TiDB stay on v8.5.8, Loams
Postgres, chDB and Grafeo on 0.1.1's pins.

- **A stack with the `tikv` profile** (a 0.1.0 stack kept on its stores, or one started with
  `tikv`): `loams upgrade --to 0.1.2`. It takes a snapshot first, and `loams upgrade rollback`
  restores it. With `LOAMS_DURABLE_STORE=tikv://…`, Durable's `loams_durable` keyspace is read
  as it is: the new store keeps 0.1.1's layout byte for byte, and the fork's test loads a
  keyspace dumped from a 0.1.1 engine. Durable on SQLite (the default) is unchanged.
- **The light default stack** (embedded stores, no `tikv` profile, 0.1.1's default):
  `loams upgrade` does not upgrade it yet (`stack_unrecognised`). Upgrade it by hand: back up
  the volumes, `docker compose stop`, set `LOAMS_VERSION=0.1.2` in `.env`, then
  `docker compose up -d` (docs/release/upgrades.md). Take `compose.yaml` and `config/` from the
  0.1.2 bundle to get `LOAMS_AGENTD_ORIGINS` and the `kafka` profile; keep the directory name,
  which names the volumes.
- **CORS:** remove `LOAMS_CORS_ORIGINS=https://console.loams.dev` from `.env` (an upgrade keeps the
  old value). List only the origins of consoles you serve elsewhere.
- **Studio with a remote agentd:** set `LOAMS_AGENTD_ORIGINS` to the agentd host's origin, and
  the host's `--allow-origin` to `https://loams.<tailnet>.ts.net`
  (docs/guides/studio/remote-agentd.md).
- **CLI packages:** on Fedora or Arch, install the 0.1.2 package; it replaces 0.1.1's, which may
  not have installed. No data changes.
- **Loams Desktop does not update itself yet:** the update feed (`desktop-latest`) is not
  published and its manifests are unsigned, so download each new version from the release.
- Expected downtime is about 5 minutes, excluding image pulls.

### Added
- **The engine-served console frames your agentd hosts.** `LOAMS_AGENTD_ORIGINS` origins now join
  the console's `frame-src` as well as `connect-src` (same validation; `*` is still refused), so
  Studio previews render in the console at `/ui/`.
- **The `kafka` compose profile (preview; KW-L2, D1798).** `COMPOSE_PROFILES=kafka` runs Ostrium's
  fork of Apache Iggy (`ghcr.io/ostrium-labs/iggy`, one image pinned by digest, built and attested
  by the fork's CI) as `iggy` and `kafka-gateway`, with the Kafka wire on `127.0.0.1:9093`. Off in
  the default stack. Every Kafka API except transactions and exactly-once. Not wired to the engine
  yet. `release-compose` gains a smoke that starts the profile and runs kcat through it.
  The profile refuses to start while `KAFKA_IGGY_PASSWORD` is empty, `.env.example`'s
  placeholder (`change-me`) or shorter than 12 characters (`config/kafka-guard.sh`, with a
  self-test in `release-compose`); `byoc/install.sh` writes a random one into a new `.env`.
- **`loams-agentd` as release tarballs for Linux** (#874, #1066). `release-package.yml` attaches
  `loams-agentd-<version>-linux-x86_64.tar.gz` and `-linux-aarch64.tar.gz`, each holding
  `loams-agentd`, `loams`, `SHA256SUMS` and `VERSION`: the layout `loams-agentd install-runtime --from`
  reads, so a headless host no longer takes the binary out of the image. "Get the binary" in
  `docs/guides/studio/remote-agentd.md` downloads them.
- **The TypeScript SDK runs in Cloudflare Workers**: the default transport sends `redirect: "manual"` in workerd
  (connect-web's `"error"` is rejected there), and `transportOptions.fetch` accepts a custom `fetch`. README
  has a Cloudflare Workers section.
- **Durable on TiKV through the Resonate fork's store** (design 74 FW1, #1226). The engine's
  `durable-tikv` feature uses `resonate-server-tikv` from `ostrium-labs/resonate` (`loams/main`),
  and `crates/loams-durable/src/tikv.rs` is gone. Same keyspace layout as 0.1.x, so existing
  `loams_durable` keyspaces and `--durable-store tikv://…` carry over; the fork tests a keyspace
  dumped from a 0.1.1 engine. The fork also brings OpenTelemetry spans and `traceparent`.
- **Loams House on lake tables** (HE0 Task 9; in `loams-fabric`, the chDB House): Langfuse's and
  SigNoz's DDL, copy-on-write mutations, plain views and `Distributed` tables on the catalog,
  `VersionedCollapsingMergeTree` (D1570), readable `AggregatingMergeTree`, `Null` and inner-table
  materialized views, `SAMPLE` (reads every row), nanosecond times as longs in v2 tables (D1572),
  JSON columns as their text, `system.clusters`, `macros`, `mutations` and
  `distributed_ddl_queue` for one node, compressed native-protocol blocks, ClickHouse's insert
  ordering, comments in a column list, and the gate applications' client settings. The statement
  time cap is 900 s and configurable end to end (D1573).
- **BYOC on AWS, Google Cloud and Azure (preview)** (BY0, BY1): cloud-init for one VM running the
  light stack over Tailscale only (no inbound rule, no static cloud key), `byoc/install.sh
  --cloud`, `--bucket` and `--render-env-only`, OpenTofu modules in `deploy/tofu/`, and guides in
  `docs/guides/byoc/`. The engine's bucket builds with the VM's identity (S3, GCS, Azure managed
  identity). Tested without a cloud account; not yet run on a real one.
- **SDKs: MachineKey token sources** for agents in Rust, Go, TypeScript and Python (IA4 Task 2):
  a Zitadel machine user's key signs a JWT profile grant, refreshed before expiry. Zitadel is
  planned (design 76), so this is ahead of the server side.
- **agentd (experimental):** worktree chats with diff, review, change-request, approval and merge
  tools for the factory loop (FT0), CI check status and log tails for Forgejo and GitHub, and
  the marketplace groundwork: `MarketplaceService` with signed-index verification, install plans
  and installers for node agents, harnesses and MCP servers (MK0). API only; no console UI yet.
- **`loams cloud admin`** (LC2a): the invite-only Loams Cloud operator commands (WorkOS device
  login, TOTP step-up, waitlist, invites, audit). For Ostrium's operators; Loams Cloud is not
  deployed.
- **In the repository, not in the release images:** `loams-iggy-sink` (the Iggy to Loams hop,
  not wired to the engine yet), the Temporal gateway crates and `loams-temporal` binary (DX1,
  experimental), the KV gateway groundwork
  (KV0), the Rust House engine's crates (HE1: not wired, chDB remains the House engine; an
  opt-in read-only preview is planned for 0.1.3), `loams-authz` and `loams-zitadel` (IA2, IA4),
  the Loams Cloud Worker (LC2a, not deployed) and the `loams-listing` CLI (MK0).

### Changed
- **The engine sends the console's Content-Security-Policy as a header, and Studio there can
  reach a remote agentd** (0.1.2; owner, 2026-10-23). On `/ui/` the engine takes the build's
  `<meta>` CSP out of `index.html` and `cordis.html` (a header cannot loosen a meta policy) and
  sends the same policy as the `Content-Security-Policy` header, with the origins in
  `LOAMS_AGENTD_ORIGINS` added to `connect-src`. Every other directive stays the build's; empty
  means `connect-src 'self'`, as before. Entries follow `LOAMS_CORS_ORIGINS`'s exact-origin rules,
  and `*`, `null`, paths or credentials stop the engine at start. The compose stack passes the
  variable through. Statically hosted builds keep their meta tag. `frame-ancestors` on the
  console's responses is now `'self'` (it was `'none'`, which also blocked the console's own
  plugin sandbox frame).
- **The console ships with each deployment, and CORS is deny by default** (0.1.2; owner,
  2026-10-23; D1705 amended). The hosted console at `console.loams.dev` is retired (design 78),
  so:
  - **The engine serves the console** at `/ui/` on its own HTTP listeners (the main port and the
    `tailscale serve` target), next to its API: in the compose stack at
    `https://loams.<tailnet>.ts.net/ui/`, reachable only on your tailnet. `/` redirects to `/ui/`.
    The engine image builds the console from the same commit into `/usr/share/loams/console`
    (where the Linux packages already put it), and the engine serves that directory when it holds
    a build. `--console-dir <DIR>` serves another build; `--no-console` serves none. The files are
    public, like any web app's shell; every call the console makes still goes through the
    engine's authentication.
  - **`LOAMS_CORS_ORIGINS` allows no origin by default**, on every listener, with authentication
    on or off. Only the engine's own origin (same-origin) is answered. The compose default
    `https://console.loams.dev` is gone, and so is the rule that an empty list allowed **any**
    origin when authentication was on. Entries must be exact origins (`scheme://host[:port]`);
    `*`, `null`, paths and credentials are refused at start. Loams Desktop needs no entry: its main
    process sends the engine's own origin. **Upgrading:** remove
    `LOAMS_CORS_ORIGINS=https://console.loams.dev` from `.env`, and list the origin of any console
    you serve elsewhere (`--profile console`, a Cloudflare or Vercel deploy).
  - Docs and guides point at the console your engine serves, Loams Desktop, and
    `loams.dev/cloud` for the try-out. The remote agentd guide
    (`docs/guides/studio/remote-agentd.md`) now sets `--allow-origin` to the engine's origin and
    lists the host in `LOAMS_AGENTD_ORIGINS`; the Headscale section no longer cites the hosted
    console (still unsupported, not tested end to end).
- **The Software Factory's observability app is SigNoz on Loams House** (D1666, owner, 2026-10-22).
  The factory lists **SigNoz** where it listed OpenObserve: full UI only, health checked at
  `/api/v2/healthz`, and labelled **"Preview: SigNoz on Loams House"** in Studio until SigNoz
  passes the House gate (D1571). Only SigNoz's MIT community build is used
  (`signoz/signoz-community` v0.145.0, never `ee/`), with its OTLP collector
  (`signoz/signoz-otel-collector` v0.144.12, AGPL-3.0, unmodified), both pinned by digest in
  `deploy/factory/images.lock`. `deploy/factory/signoz/compose.yaml` is an opt-in overlay on the
  Loams stack that points SigNoz at the install's House (native and HTTP endpoints, user,
  password); no ClickHouse server is bundled. Idle, SigNoz and its collector use about 85 MiB.
- **Toolchain and dependencies:** Node 24 for the JavaScript workspace (#1172), zod 4,
  portable-pty 0.9 and notify 8.2, and the grouped npm, Cargo and Go updates.

### Removed
- **The `console.loams.dev` custom-domain route** is gone from `web/apps/console/wrangler.jsonc`
  (Ostrium runs no hosted console; a deploy in your own account stays on workers.dev).
- **The console deploy workflow** (`.github/workflows/console-deploy.yml`; 0.1.2, owner,
  2026-10-23, D1705). It deployed `web/apps/console` to `console.loams.dev`, which is retired:
  the console ships with each deployment (the engine serves it at `/ui/`) and with Loams
  Desktop. `wrangler.jsonc` and `deploy/stage.mjs` stay for a console on your own Cloudflare or
  Vercel account (coming soon). The GitHub `production` environment, its variables and the
  Cloudflare Worker and DNS record are left for the owner to remove.
- **Authentik** (IA1 Task 1, D1600). Loams cuts over to Zitadel for identity and OpenFGA for
  authorization (design 76), with no migration, no compatibility period and no dual running.
  The Authentik deploy kit (`deploy/authentik/`), the Authentik bundle in `deploy/release/auth/`,
  its tailnet node and every code and doc reference are gone. Until the Zitadel bundle for
  `auth.loams.dev` lands, an install signs people in with built-in single-org mode (`tailscale`,
  console sessions with first-run setup, `lk_` keys) or `oidc` mode against any issuer.
  `SIGN_IN_KIND_AUTHENTIK` and `SIGN_IN_KIND_AUTHENTIK_DEVICE_CODE` are deprecated in favour of
  the new `SIGN_IN_KIND_OIDC` and `SIGN_IN_KIND_OIDC_DEVICE_CODE`; `OIDC_PROVIDER_KIND_ZITADEL`
  is added. The tailnet tag `tag:authentik` is now `tag:zitadel`.
- **OpenObserve, from the Software Factory** (D1666). SigNoz replaces it as the factory's
  observability app (see Changed). A stored OpenObserve connection is no longer listed.

### Fixed
- **`secrets/api-keys` readable by the engine** (#881, #1068). `loams auth new-key --append` creates the key
  file as its writer, mode 0600, which the engine (uid and gid 10001) cannot read through the release
  stack's bind mount. The release README's new "API keys" section gives the file to the engine's group
  with mode 0640 (`sudo chgrp 10001` under rootful Docker, `podman unshare chgrp 10001` under rootless
  Podman), `byoc/install.sh` does the same for the empty file it creates, and an engine that cannot read
  the file now names the uid and gid it runs as and the file's owner and mode.
- **The `.rpm` and pacman packages install** (#1110, #1105). nFPM does not translate `depends`,
  so 0.1.1's `.rpm` and `.pkg.tar.zst` required Debian names (`libc6`, `libgcc-s1`) that dnf and
  pacman cannot resolve, and no package declared zlib, which the binary links through
  `mysql-wire`. Each format now declares its own names, zlib included, and the release job
  fails when the binary links a library the packages do not declare.
- **agentd:** a run group's member whose run was taken settles after a restart instead of
  showing `running` for good (#1065); worktree names are reserved atomically, so concurrent
  worktrees under one repository name never collide (#962); the Connect/ndjson parity race and
  stack overflow (#1059).
- **TiKV:** metastore garbage collection claims the orphans it returns, so a held-up swap can no
  longer reference a deleted segment (#968); the client-rust pin moves so a crashed writer's
  locks never stall a restarted server (#981).
- The proto comment for `CompleteSetupRequest.setup_token`: it is the operator's
  `LOAMS_SETUP_TOKEN`, needed whenever authentication is on (#1048).

### Security
- **CodeQL alerts open at 0.1.1** (#1105): linear-time replacements for six ReDoS patterns in
  Studio (#1106); `scripts/connectors/profiles.py` no longer prints a credential from an invalid
  file or a refused value (#1107). The alerts on the default branch close when 0.1.2 reaches
  `main`.
- rustls-webpki 0.103.16 and webpki-roots 1.0.10 (RUSTSEC-2026-0381, -0382, -0383, #967); tokio
  1.54.1 in the root and `fabric/` (RUSTSEC-2026-0386, #1021, #1158). RUSTSEC-2026-0385
  (ctutils) is ignored until its fix is 14 days old (#1134).

## [0.1.1] - 2026-10-21

Highlights:

- **The UI upgrade.** The console, Studio and Loams Desktop move onto the design 59 foundation
  (CF1): new tokens, component states and a motion system, polished data screens, a reworked
  shell (sidebar, inspector and drawer regions, the palette, one keymap), Studio's home, threads
  and approval cards, and on the desktop native menus, per-OS window chrome synced to the theme,
  a splash, OS notifications and an update toast with "Skip this version".
- **The app-building API in all 13 SDKs.** TypeScript, Python, Go, Rust, C#, C++, Swift, Java,
  Kotlin, Ruby, PHP, Dart and Objective-C each have the `namespaces`, `collections`, `documents`,
  `search`, `vector` and `graph` modules, the `Schema`, `Filter` and `Search` builders,
  collection handles, `write_batch`, `scroll_all` and `query_all`, consistency tokens with a
  read-your-writes session store, a default namespace and client deadlines, plus `sql` and
  `streams`. Each SDK replays one recorded app scenario in CI.
- **New APIs: `loams.sql.v1` and `loams.stream.v1`** on the engine's main port. Read-only SQL
  with `$n` parameters, answered as typed rows or Arrow; streams with keyed records, idempotent
  produce (`producer_id`, `producer_seq`) and long-poll fetch, over Connect, gRPC and gRPC-Web.
- **Self-host first-run fixes.** `LOAMS_SETUP_TOKEN` reaches the engine from `.env` (and
  `secrets/engine.env` still wins), and a one-shot `pg-bootstrap` service creates `compute1`'s
  tenant and timeline, so a fresh stack comes up without manual steps from the bundle or the tag.
- **A light default self-host stack** (D1340): `docker compose up -d` in `deploy/release` now starts the
  engine on its embedded stores (metastore on openraft + redb, Live on redb, Durable on SQLite) with a
  local-folder bucket, Loams House linked to the engine, Loams Git (the engine image gains the `git`
  feature), Loams Postgres with its remote storage in local folders, and agentd: about 8 GB of images and
  1 GB of RAM at idle, down from about 10 GB and 2.6 GB. PD, TiKV and TiDB (Loams SQL) are the `tikv`
  profile and RustFS the `rustfs` profile, set in `.env` as `COMPOSE_PROFILES`; the House's own tailnet
  node is `house-tailnet` (was part of `house`). The engine records the store its data was started on and
  refuses the other. `byoc/install.sh` gains `--tikv`, `--rustfs`, `--profiles` and `--no-postgres`
  (`--all` is now both store profiles). Upgrading a 0.1.0 stack: add `COMPOSE_PROFILES=tikv,rustfs` to its
  `.env` first. `loams upgrade` follows the profiles and refuses a stack on the embedded stores for now;
  see docs/release/upgrades.md.
- **Elasticsearch and Mastra compatibility.** The Elasticsearch API gains `GET /_cat/indices`,
  `GET` and `PUT /{index}/_settings`, sort on `_seq_no`, the scroll API (as `helpers.scan` uses
  it) and aggregations on the engine (composite and `top_hits` included), and `_update_by_query` replaces a key when its param is an object. The Qdrant gateway serves
  `DELETE` of a payload index (REST and gRPC), and CI runs `@qdrant/js-client-rest` 1.18 and 1.19
  and a nightly Mastra gate against a dev build. With these, the NVIDIA RAG Blueprint harness
  passes 38 of 38.
- **Durable in the CLI packages.** The `.deb`, `.rpm`, pacman, macOS and Windows packages are built
  with the image's features, so `loams dev` from a package serves Durable on 127.0.0.1:8001 (#925).
- **agentd ordering fix.** A steer orphaned by a closing run keeps its place ahead of the queue, so
  queued messages no longer run out of order or two in one turn.
- **Dependency updates** across Cargo, npm and Go.

### Upgrading

**0.1.1 changes the default stack; reinstall.** There are no 0.1.0 installs to carry forward, so
start from the 0.1.1 compose bundle with a fresh `.env`, rather than upgrading a 0.1.0 stack in place: the light default keeps its data on the
embedded stores and local folders, not in 0.1.0's TiKV and RustFS. To keep a 0.1.0 stack's data,
add `COMPOSE_PROFILES=tikv,rustfs` to its `.env` before `loams upgrade --to 0.1.1`; it then keeps
0.1.0's shape, Durable on SQLite included (docs/release/upgrades.md).
`loams upgrade rollback --keep-writes` back to 0.1.0 is refused (`KEEP_WRITES_UNSERVED`, D1343):
0.1.0's startup gate cannot serve data a later release has written, so roll back from the snapshot.

### Added

- **Elasticsearch API** (MA1, #937, #954, #958, #963, #938, #964): `GET /_cat/indices` in the json and
  text formats; `GET` and `PUT /{index}/_settings`; sort on `_seq_no`; the scroll API
  (`scroll`, `_search/scroll`, clearing a scroll), as the Python client's `helpers.scan` uses it;
  `_update_by_query` assigning an object param by replacing the key; and aggregations computed on
  the engine, `composite` and `top_hits` included (#964).
- **The Mastra gate** (MA1 Task 27, #956): a nightly run of Mastra's suites against a dev build,
  which files an issue on failure. `loams-postgres-dev` computes run with 128 MB `shared_buffers`
  and a 256 MB file cache (#946).
- **The UI upgrade** (phases 1 and 2, extending CF1, design 59): tokens, component states and the
  motion system (`motion`, `useMotionPreset`, `StaggerList`, `ListRow`, `Segmented`); data
  screens polish; shell and navigation motion, keyboard and polish; Studio's home, threads and
  approval cards and the desktop chrome on the same primitives (#801, #802, #803, #804).
- **CF1 tasks 12–16** (design 59): one keymap for D1081 in which editable targets keep their
  keys; per-OS window chrome synced to the page theme; native menus and the context menu; a splash
  with Loams' loader; OS notifications and an update toast with "Skip this version" (#819–#822, #825).
- **Durable on TiKV** (#796): the image adds `durable-tikv`, and PD pre-allocates the keyspace;
  with the `tikv` profile, `LOAMS_DURABLE_STORE=tikv://pd:2379/loams_durable` puts the stack's
  Durable there (D1342; SQLite stays the default). The TiKV durable server creates its keyspace if PD has none, answers
  `promise.search`, `task.search` and `schedule.search` (the console's Durable page uses them;
  `--durable-set servers.server_tikv.search=false` turns them off), and its start log names its
  store.
- **The Qdrant gateway deletes payload indexes** (MA1 Task 23, D1210, #929):
  `DELETE /collections/{c}/index/{field}` and gRPC `Points/DeleteFieldIndex` retire the index, which
  leaves `payload_schema`; filters are unchanged, and `PUT` with the same type revives it. CI runs
  `@qdrant/js-client-rest` 1.18 and 1.19 against the gateway (MA1 Task 25, #931).
- **Loams House observability** (HS1 Task 23 part 2, #797): every design §49 §15 metric series, one
  trace per statement, redacted operator logs and the query log.
- **Loams Git `CommitWorkspace` and `PushWorkspace`** (GT1 Task 27, #734): commit a workspace
  snapshot and push it without re-uploading objects, with per-ref checks and keyed idempotency.
- **Software factory apps in Studio** (#773, #798): an end-to-end pass over the factory apps on the
  desktop and the browser console, with their images pinned.
- **`platform/`** (#860): the former private `loams-platform` repository, imported as a separate
  Cargo workspace (`loams-meter`, the node metering agent), with the Authentik and tailnet deploy
  files and the platform design docs, Apache-2.0 (D1119, design 63).
- Release: the product release's `SHA256SUMS` is signed by `release-checksums.yml` (#827), and the
  desktop's update manifests are dropped before `DESKTOP-SHA256SUMS` on a product release (#829).
- **The Kotlin SDK replays the app scenario** (SDKP P0d): `kotlin_app_collections`, `kotlin_app_search`,
  `kotlin_app_graph`, `kotlin_app_sql` and `kotlin_app_streams` replay `sdks/fixtures/recorded/app/`
  through the facade against the fixture server, and CI runs `check-languages --suite app` for Kotlin.
  Fixed on the way: the Connect JSON writer sent every `bytes` field empty and failed on any map field,
  wrote a `float` as its widened double (`0.699999988079071` for `0.7`), and `Sort.score()` and
  `Sort.pk()` now leave the order unset by default and `Search.select` no longer sends an empty
  `exclude`, as the builder corpus does.
- **The C# SDK replays the app scenario** (SDKP P0d): `csharp_app_collections`, `csharp_app_search`,
  `csharp_app_graph`, `csharp_app_sql` and `csharp_app_streams` replay `sdks/fixtures/recorded/app` through
  the public facade against the shared fixture server, the report's `ran` names them, and the `SDK: C#`
  job runs `check-languages --check csharp --suite app`.
- **The C# SDK builds apps** (SDKP L5): `Namespaces`, `Collections`, `Documents`, `Search`, `Vector` and
  `Graph` modules over the generated `loams.collection.v1` and `loams.graph.v1` stubs, the `Schema`,
  `Filter` and `Search` builders, `client.Collection(name)` handles, `WriteBatchAsync`, `ScrollAllAsync`
  and `QueryAllAsync`; consistency tokens that parse and merge, a session store that gives reads
  read-your-writes, a default namespace, client deadlines (`client_deadline_exceeded`), and the
  `examples/Notes` app.
- **The Java SDK replays the app scenario** (SDKP P0d): `java_app_collections`, `java_app_search`,
  `java_app_graph`, `java_app_sql` and `java_app_streams` replay `sdks/fixtures/recorded/app` through the
  facade, and CI's `SDK: Java` job runs `check-languages --suite app`. `Options.json(true)` sends unary
  calls as proto3 JSON (a dependency-free codec, so the runtime still needs only `protobuf-java`), and
  a Connect unary call in JSON is no longer enveloped. `sdks/java/gen` is regenerated, so
  `CountDocumentsRequest.where` and the other typed-query fields reach Java.
- **The Java SDK builds apps** (SDKP): namespaces, collections, documents, search, vector and graph
- **The Kotlin SDK builds apps** (SDKP): namespaces, collections, documents, search, vector and graph
- **The Swift SDK builds apps** (SDKP): namespaces, collections, documents, search, vector and graph
  modules, the `Schema`, `Filter` and `Search` builders, collection handles, `writeBatch`, `scrollAll` and
  `queryAll`; consistency tokens that parse and merge, a session store that gives reads read-your-writes,
  a default namespace, client deadlines (`client_deadline_exceeded`), and the notes example app.
- **The Swift SDK replays the app scenario** (SDKP P0d): `swift_app_collections`, `swift_app_search`,
  `swift_app_graph`, `swift_app_sql` and `swift_app_streams` replay `sdks/fixtures/recorded/app` through
  the facade against the fixture server, and the `SDK: Swift` job runs `check-languages --suite app`.
  `Code.invalidArgument` now reads Connect's `invalid_argument` (a stream's end frame read it as
  `unknown`), and the test fixture server no longer depends on the working directory.
- **The Ruby SDK builds apps** (SDKP L10): `namespaces`, `collections`, `documents`, `search`,
  `vector` and `graph` modules over the descriptor set, `Client#call` for any facade call, the `Schema`,
  `Filter` and `Search` builders, `client.collection(name)` handles, `write_batch`, `scroll_all` and
  `query_all`; consistency tokens that parse and merge, a session store that gives reads read-your-writes,
  a default namespace, `Loams.from_env`, client deadlines (`client_deadline_exceeded`), and the notes
- PHP SDK: `sql()` (`query` with `$n` parameters, `queryArrow`, `Sql::records`) and `streams()` (`create`, `get`, `list`, `produce` with producer deduplication, `fetch`, `produceEvents`, `fetchEvents`, `subscribe`).
- **The PHP SDK builds apps** (SDKP L11): `namespaces()`, `collections()`, `documents()`, `search()`,
  `vector()` and `graph()` modules over Connect JSON, `Client::call()` for any facade call, the `Schema`,
  `Filter` and `Search` builders, `$client->collection($name)` handles, `writeBatch`, `scrollAll` and
  `queryAll`; consistency tokens that parse and merge, a session store that gives reads read-your-writes,
  a default namespace, `Client::fromEnv()`, client deadlines (`client_deadline_exceeded`), and the notes
  example app.
- **The C++ SDK builds apps** (SDKP L6): `Namespaces()`, `Collections()`, `Documents()`, `Search()`,
  `Vector()` and `Graph()` modules over the generated `loams.collection.v1` and `loams.graph.v1` stubs,
  `Loams::Call` for any facade call, the `Schema`, `Filter` and `Search` builders, `Collection(name)`
  handles, `WriteBatch`, `ScrollAll` and `QueryAll`; consistency tokens that parse and merge, a session
  store that merges rather than refusing a second token, `Options::default_namespace`, `OptionsFromEnv()`,
  and a 30 s per-call budget (`connect-timeout-ms`, `client_deadline_exceeded`, never retried).
- **The Ruby SDK's `sql` and `streams` modules** (SDKP L10 part C): `sql.query` with `$n` parameters,
  `query_arrow` and `Sql.records`; `streams.create`, `get`, `list`, `produce` (an optional record key,
  producer deduplication), `fetch`, `produce_events`, `fetch_events` and `subscribe`.
- Dart SDK: `sql` (`query` with `$n` parameters, `queryArrow`, `SqlModule.records`) and `streams` (`create`, `get`, `list`, `produce` with producer deduplication, `fetch`, `produceEvents`, `fetchEvents`, `subscribe`).
- **The Dart SDK builds apps** (SDKP L12): `namespaces`, `collections`, `documents`, `search`, `vector`
  and `graph` modules over Connect JSON, `LoamsClient.call` for any facade call, the `Schema`, `Filter`
  and `Search` builders, `client.collection(name)` handles, `writeBatch`, `scrollAll` and `queryAll`
  (as `Stream`s); consistency tokens that parse and merge, a session store that gives reads
  read-your-writes, a default namespace, `LoamsClient.fromEnv`, client deadlines
  (`client_deadline_exceeded`), and the notes example app.
- Objective-C SDK: `sql` (`query` with `$n` parameters, `queryArrow`, `records`) and `streams` (`create`, `get`, `list`, `produce` with producer deduplication, `fetch`, `produceEvents`, `fetchEvents`, `subscribe` with a block callback and a cancel token).
- **The Objective-C SDK builds apps** (SDKP L13): `namespaces`, `collections`, `documents`, `search`,
  `vector` and `graph` on `LoamsClient` over Connect JSON, `LoamsClient.call` for any facade call, the
  `LoamsSchema`, `LoamsFilter` and `LoamsSearch` builders, `collection:namespace:` handles,
  `writeBatch`, `scrollAll` and `queryAll`; consistency tokens that parse and merge, a session store that
  gives reads read-your-writes, a default namespace, `LoamsClientConfiguration.fromEnvironment`, client
  deadlines (`client_deadline_exceeded`), and `LoamsSubscription` with a cancel token.
- **The C++ SDK's `Sql()` and `Streams()` modules** (SDKP L6 part C): `Query` with `$n` parameters,
  `QueryArrow` and `SqlModule::Records`; `Create`, `Get`, `List`, `Produce` (an optional record key,
  producer deduplication), `Fetch`, `ProduceEvents`, `FetchEvents` and `Subscribe`. The generated stubs
  now include `loams.sql.v1` and `loams.stream.v1`.
- **`loams.sql.v1` on the main port** (SDKP P0b, API2 Task 1): `SqlService/Query` runs one read-only
  statement over a namespace's collections with `$1…$n` parameters (each cast to its placeholder's
  inferred type) and answers typed columns and rows; `QueryArrow` answers the result as one Arrow IPC
  stream, capped at 16 MiB (`result_too_large`, use Flight SQL past it). A parameter count mismatch is
  `invalid_argument` with reason `sql_param_count`. `@loams/proto` exports the package as `sql`.
- **`loams.stream.v1` on the main port** (SDKP P0c, API2 Task 2): create, describe and list streams;
  produce and fetch records and CloudEvents over Connect, gRPC and gRPC-Web. A produce with
  `producer_id` and `producer_seq` is idempotent (a repeat answers the first append with `duplicate`);
  `Fetch` takes `max_records` and a long poll (`max_wait_ms`, at most 30 s). The tonic listener of
  `--stream-grpc-listen` is unchanged and wire-compatible.
- **Desktop: Settings > Postgres** (PG2 R9.1): the pg-control URL (default `127.0.0.1:7680`) and the
  API key file (`postgres.control.apiKeyFile`), with checks, a Test connection button, and a file
  picker. Main reads the key file; the key never reaches the renderer. The Postgres page follows a
  saved change without a restart, and its subtitle names pg-control's own address. The environment
  variables `LOAMS_PG_CONTROL_URL`, `LOAMS_PG_CONTROL_API_KEY` and `_API_KEY_FILE` still win, and the
  page says so when they are set.

### Changed

- **Self-host first run** (#866, #880, #883): `LOAMS_SETUP_TOKEN` passes from `.env` to the engine
  through `config/setup-token.env`, so `secrets/engine.env` still wins when both set it; the
  bundle is the primary install path, and the release's bundle job checks first-run setup; a
  one-shot `pg-bootstrap` service creates `compute1`'s tenant and timeline on the first `up`
  (keeping their ids in the `pg-bootstrap` volume), and `compute1` starts after it.
  `.env.example` documents `LOAMS_SETUP_TOKEN` and `LOAMS_TAILSCALE_ALLOWED_USERS`.
- **A patch release upgrades from its line.** `release/upgrade-path.toml` lists 0.1.1 with
  `from = "0.1"`; the upgrade gate and the SDK public-symbol check resolve a line to the newest
  earlier release of it (here `v0.1.0`), and the generated Upgrading notes say "any earlier 0.1.x".
- CI runs on GitHub-hosted runners only; Blacksmith is removed (#826).
- Dependencies: the Cargo minor-and-patch group (14 updates, #864), the npm minor-and-patch group
  (30 updates, #878), `uuid` 11.1.1 → 14.0.2 (#808), `react-resizable` 3.2.0 → 4.0.2 (#810),
  `go-sql-driver/mysql` 1.9.3 → 1.10.1 in the sqlgate capture tool (#805); Dependabot ignores
  `@types/node` majors while the repo targets Node 22 (#894).
- **Desktop: Durable on a remote server**: the protocol proxy forwards `/durable/` to the active
  remote server's main port (`POST /durable/`, B12) with the session the other engine routes use,
  instead of answering 404. The Durable page and the Overview's Durable card work on remote servers;
  an engine without the API reads as "Not on this server".
- Loams is fully open source; there is no open core (D1119). README, CONTRIBUTING, GOVERNANCE, ECOSYSTEM, the design docs and the decision log now say so; the metering, billing and control-plane code is in this repository (`platform/`, design §63). Ostrium Labs is a research lab building open-source Loams (D1323); the services wording is gone.
- The app modules in every generated SDK facade (SDKP P0a): `namespaces`,
  `collections`, `documents`, `search`, `vector` and `graph`, from facade
  annotations on `loams.collection.v1` and `loams.graph.v1`. The facade index
  `sdks/fixtures/facade-index.json` lists every SDK call for the hand-written
  SDKs to check themselves against (`scripts/sdk/gen.sh index`), and
  `gen.sh descriptors` regenerates Ruby's and Kotlin's descriptor sets.
- `where` on `ScrollDocuments`, `CountDocuments`, `DeleteByFilter` and
  `PatchByFilter`: a typed `Query`'s proto3 JSON, the shape the SDK `Filter`
  builders write. Setting both `filter` and `where` is `invalid_argument`,
  reason `filter_conflict`.

- The app conformance corpus (SDKP P0d): the notes-app scenario recorded from
  `loams dev` (`sdks/fixtures/recorded/app/`, `app-scenario.md`), the builder
  corpus (`sdks/fixtures/builders/`) and the consistency-token corpus
  (`sdks/fixtures/tokens.json`, also run by `loams-collection`), and
  `check-languages.mjs --check <lang> --suite app`.

- Seven registered reasons (SDKP P0e): `sql_param_count` and
  `result_too_large` for `loams.sql.v1`, and the client-side
  `namespace_required`, `invalid_consistency_token`,
  `search_after_needs_tiebreak`, `client_deadline_exceeded` and
  `batch_partially_applied`, in every SDK's reason list. The SDK runtime
  contract gains R11 (default namespace), R12 (timeouts), R13 (`write_batch`)
  and R14 (`subscribe`), and R4 the token encoding and the session store.
- TypeScript SDK (`@loams/client`): the app-building layer of SDKP. `Schema`,
  `Filter` and `Search` builders checked against the new builder corpus
  (`sdks/fixtures/builders/`); `loams.collection(name)` handles;
  `ConsistencyToken` parse/merge/print checked against `sdks/fixtures/tokens.json`,
  and a session store that merges every response token and sends
  `consistency.at_least`; a default namespace (R11); a 30 s unary deadline
  raising `client_deadline_exceeded` (R12); `documents.writeBatch` (R13);
  `streams.subscribe` (R14); `search.text`, `search.hybrid`, `search.queryAll`,
  `documents.upsert/delete/patch/scrollAll` and `<call>All` for paged calls;
  `Loams.fromEnv()`.
- TypeScript SDK: `loams.sql` and `loams.streams` (SDKP L1 part C):
  `sql.query`, `sql.queryArrow`, `sql.records(statement, [$1, …])` and
  `sqlRecords(answer)`; `streams.create`, `get`, `list`/`listAll`, `produce`,
  `fetch`, `produceEvents`, `fetchEvents` and `subscribe`; replayed by
  `typescript_app_sql` and `typescript_app_streams`.

- The `sql` and `streams` modules in every generated SDK facade (SDKP Part C):
  `sql.query` and `sql.query_arrow` from facade annotations on `loams.sql.v1`,
  `streams.create`, `get`, `list`, `produce`, `fetch`, `produce_events` and
  `fetch_events` from `loams.stream.v1`, the Python and Go stubs for both, and
  the `app_sql` and `app_streams` scenarios recorded from `loams dev`
  (`record-fixtures.mjs --source app --scenario …`).
- Python SDK (`loams`): the app-building layer of SDKP, in both `Loams` and
  `AsyncLoams`. `Schema`, `Filter` and `Search` builders checked against the
  builder corpus (`sdks/fixtures/builders/`); `loams.collection(name)` handles;
  `ConsistencyToken` parse/merge/print checked against
  `sdks/fixtures/tokens.json`, and a session store that merges every response
  token (fields and the `loams-consistency-token` header) and sends
  `consistency.at_least`; a default namespace (R11); a 30 s unary deadline
  raising `client_deadline_exceeded` (R12); `documents.write_batch` (R13);
  `streams.subscribe` (R14); `search.text`, `search.hybrid`,
  `search.query_all`, `documents.upsert/delete/patch/scroll_all` and
  `<call>_all` for paged calls; `Loams.from_env()`. The package now carries the
  `loams.collection.v1` and `loams.graph.v1` messages and Connect stubs.

- Python SDK: the app modules on `Loams` and `AsyncLoams` (`namespaces`,
  `collections`, `documents`, `search`, `vector`, `graph`), typed with the
  generated Protocols plus the conveniences; the app scenario replayed through
  the public facade in `python_app_collections`, `python_app_search` and
  `python_app_graph`; `examples/notes/main.py`.
- Python SDK: `loams.sql` and `loams.streams` on both clients (SDKP L2 part C):
  `sql.query`, `sql.query_arrow`, `sql.records(statement, [$1, …])` and
  `loams.sql_records(answer)`; `streams.create`, `get`, `list`/`list_all`,
  `produce`, `fetch`, `produce_events`, `fetch_events` and `subscribe`;
  replayed by `python_app_sql` and `python_app_streams`.

- Python SDK: `session_consistency=True` now works: the response header is read
  through `connectrpc.client.ResponseMetadata`, and two different tokens merge
  instead of being refused. The session keeps one token per proto package, and
  only collection and SQL reads take it (SDKP-17). `timeout_ms` is now the
  SDK's own deadline over all of a call's retries rather than the transport's
  per-attempt one.
- Go SDK (`loams.dev/go`): the app-building layer of SDKP. `NewSchema`,
  `Filter` and `NewSearch` builders (with `RRF`, `DBSF`, `Weighted`, `Sort`)
  checked against the builder corpus (`sdks/fixtures/builders/`);
  `client.Collection(name)` handles; `ConsistencyToken` parse/merge/print
  checked against `sdks/fixtures/tokens.json`, and a session store that merges
  every response token and sends `consistency.at_least`; `Options.Namespace`
  and `FromEnv` (R11); a 30 s unary deadline raising `client_deadline_exceeded`
  (R12); `WriteBatch` and `*BatchError` (R13); `Subscribe` (R14);
  `DocumentCalls` (Upsert, Delete, Patch, WriteBatch, ScrollAll) and
  `SearchCalls` (Text, Hybrid, QueryAll). The generated stubs now include
  `loams.collection.v1` and `loams.graph.v1`.
  The app modules `Namespaces()`, `Collections()`, `Documents()`, `Search()`,
  `Vector()` and `Graph()` (the binding table checked against
  `facade-index.json` by `go_facade_matches_index`), the app scenario replayed
  through them (`go_app_*`), and `examples/notes`.
- Go SDK: `client.Sql()` and `client.Streams()` (SDKP L3 part C): `Query`,
  `QueryArrow`, `Records(ctx, statement, params)` and `SqlRecords(answer)`;
  `Create`, `Get`, `List`/`ListAll`, `Produce`, `Fetch`, `ProduceEvents`,
  `FetchEvents` and `Subscribe`; replayed by `go_app_sql` and
  `go_app_streams`.
- Rust SDK (`loams`): the app-building layer of SDKP part A. `Schema`, `Filter`
  and `Search` builders checked against the shared builder corpus
  (`sdks/fixtures/builders/`); `ConsistencyToken` checked against
  `sdks/fixtures/tokens.json`, and a session that merges every response token;
  a default namespace (`with_namespace`, R11); a 30 s unary deadline over the
  whole call with `client_deadline_exceeded` (R12); `write_batch` and
  `BatchError` (R13); `subscribe` as a `futures::Stream` (R14);
  `CollectionHandle`, `scroll_all`, `query_all`; `Loams::from_env`. The app
  modules `namespaces()`, `collections()`, `documents()`, `search()`,
  `vector()` and `graph()`, `loams.collection(name)`, and the app scenario
  replayed through them (`rust_app_*`); `examples/notes`.
- Rust SDK: `loams.sql()` and `loams.streams()` (SDKP L4 part C): `query`,
  `query_arrow`, `records(statement, params)` and `sql_records(&answer)`;
  `create`, `get`, `list`/`list_all`, `produce`, `fetch`, `produce_events`,
  `fetch_events`, and `Loams::subscribe` over `fetch` (now a local, not
  `Send`, stream); replayed by `rust_app_sql` and `rust_app_streams`.

### Fixed

- **The CLI packages had no Durable** (#925). The `.deb`, `.rpm`, pacman package, macOS tarballs
  and Windows zip were built from the default features, so `loams dev` from a package served
  nothing on 127.0.0.1:8001. They now build with the release image's features (`durable`,
  `durable-tikv`, `tikv`, `live-tikv`, `graph`, `pgwire`, `mysql-wire`, `stream-grpc`), a check
  keeps that list equal to the image's, and `scripts/release/smoke-durable.sh` checks that
  Durable answers on 8001 in the Linux, Windows and macOS package jobs.
- **agentd: queued messages ran out of order** (#926). When a run closed with a steer still in its
  mailbox, agentd published `Idle` before re-dispatching it, so the queue drain started the next
  row first and the orphan steered into it. The chat now stays `Working` until the re-dispatch
  starts its own turn.
- Release: nFPM gets the 16-digit long key id, not the fingerprint (#830), and
  `sign-checksums.sh` resolves the public key path before changing directory (#832).
- A search retriever without `k` was refused ("k must be between 1 and
  100000"); it now takes `max(10, offset + limit)` on REST and Connect.
- `search_after` refused the primary-key sort value a Connect answer had just
  returned (a `google.protobuf.Value` number is a double): a whole double is now
  the key it names, and an id past 2^53 is answered as `{"uint": "…"}` and read
  back exactly.
- TypeScript SDK: a facade call whose name differs from its RPC
  (`admin.keys.create` for `KeyService/CreateKey`) failed with "has no method";
  and the paging iterator dropped items whose field name has an underscore
  (`service_accounts`).

## [0.1.0] - 2026-10-19

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
- **`NamespaceService/ListNamespaces`** (B7): the namespaces the caller can reach, by name and paged, with
  their collection and stream counts. The console's Namespaces page and the org home's Namespaces tile
  read it, and fall back to the browser's list on an older engine.
- **Durable on the main port** (B12): `POST /durable/` on `--listen` is the embedded Resonate server's
  protocol endpoint, behind the engine's authentication (org members and the operator; a credential bound
  to one environment is refused), on a loopback main port or behind TLS and authentication. The loopback
  durable listener keeps working. The browser console now has the Durable page.
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
  remote helper for `loams::<store-url>/ns/<namespace-id>/repos/<repo>` URLs (`crates/loams-git-remote`),
  now ships next to `loams` in the .deb, .rpm and .pkg.tar.zst (`/usr/bin`), the macOS
  tarballs and the Windows zip, so `git clone`, `fetch` and `push` reach a Loams Git
  repository on a bucket with no server in between. The namespace is its numeric id (the bucket
  has no namespace names), and a push with `LOAMS_GIT_CREATE_ON_PUSH=1` creates the repository:
  the repository service (`loams.repos.v1`, cargo feature `git`) is not in the release images.
  It is not published to crates.io.
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
- **Loams Postgres `lp-2026.11.0`: Postgres 16.15 and 17.11.** The Postgres stack's images move
  from the fork's `lp-2026.10.1` (16.9 and 17.5) to `lp-2026.11.0`
  (`deploy/pins/loams-postgres-images.toml`). 17.11 keeps the fork's heap WAL format (upstream's
  visibility-map block references are not taken), so existing timelines read as before. Logical
  decoding now accepts only the output plugins `pgoutput`, `test_decoding` and `wal2json`
  (`output_plugin_libraries`, the fix for CVE-2026-6471).
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

### Fixed
- **`loams upgrade` on Podman.** Closing the door now removes the Tailscale sidecars (and any other service with
  `network_mode: service:…`) instead of stopping them, and restarting the old release after a failure removes them
  first, so compose can recreate the engine and the House: Podman refuses to remove a container while another one,
  even stopped, is in its network namespace ("has dependent containers"). Opening the door creates them again.
  To recreate the engine by hand after an `.env` change: `docker compose rm -sf tailscale && docker compose up -d
  loams tailscale` ([upgrades](docs/release/upgrades.md#recreating-a-service-by-hand)).
- **Postgres role passwords work.** The password `CreateRole` or `ResetRolePassword` answers now logs in:
  the compute spec carries each role's SCRAM-SHA-256 verifier (never the password), so `compute_ctl` sets it
  at start, and a reset reaches a running compute through `/configure` within seconds (the old password is
  then refused). Roles made earlier get their stored password applied on the next spec push.
- **`GetConnectionInfo`** answers the endpoint's host and port, the database, the role and `sslmode` (no
  password) instead of `not_implemented`.
- **`endpoint-storage` no longer restart-loops** ("Unterminated quoted string" in its wait loop), so
  computes start with their file cache prewarmed.

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
