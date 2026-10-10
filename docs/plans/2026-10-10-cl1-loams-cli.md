# CL1 — The Loams CLI in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, flags, ports, error codes or exit codes, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file. The code is not pre-written in this plan.
>
> **Status: Planned** (2026-10-10). Track CLI, design [§30](../design/30-loams-cli.md) (D281–D299, Q281–Q294). CL1 takes §30 to production now that the product line has changed under it: Loams Live (§45), Loams Postgres (§46, §51), Loams SQL on TiDB (§47), Loams Graph (§48) and Loams House (§49) each have a public `loams.*.v1` API, and §30 predates all five.
>
> **Relation to CLI1, CLI2 and CLI3.** None of CLI1 is built: `crates/loams-cli` does not exist on `dev` at `1dc6e8a3`, and `crates/loams/src/main.rs` still has only `dev`, `standalone`, `cluster`, `warm` and `durable migrate`. CL1 therefore:
> - **absorbs [CLI1](2026-10-01-cli1-local-cli-and-mcp.md)**. CL1 Tasks 1, 2, 5, 6, 7, 9, 10, 20, 21, 22 and 27 execute CLI1 Tasks 1–11. CLI1's task text stays the detailed contract for those tasks, and the CL1 task lists only the amendments. Where the two disagree, CL1 wins;
> - **amends [CLI2](2026-10-01-cli2-release-and-install.md)** (Task 24: the variant matrix and the House artifacts) and otherwise leaves it to run as written, after CL1a;
> - **is CLI3** (§30 §19's "not yet planned" row): login, keys, agent tokens, companions and remote endpoints (CL1d, CL1b Task 8).
>
> PG2 Task 57's CLI half (`loams pg up|down|projects|branches|connect`) and LV1 Task 37's Rust half (`loams live deploy`) move here. Those tasks keep their server-side halves (Ruling R6).

**Goal:** One `loams` binary that a person, a script or a coding agent uses to run and drive every Loams product, locally or against a remote or cloud endpoint, with:
- §30's foundation: the output and exit-code contract, `LOAMS_HOME` and profiles, local stacks, NVMe, `.env.loams`, the stdio MCP server, `mcp install`, embedded docs (CLI1);
- one API client layer over the public Connect API (§44): endpoint resolution, `GetInstance` feature detection, `loams.errors.v1` mapping, idempotency keys, operation waits and server streams;
- **five product groups**, `loams live`, `loams pg`, `loams sql`, `loams graph` and `loams house`, each a thin client of its product's API, with one shared vocabulary of nouns, verbs and flags;
- local stacks that can run each product: in-process engines (`live`, `graph`), engine roles that drive compose (`postgres`, `sql`), a supervised sidecar binary (`house`), and container companions (`tikv`);
- sign-in, API keys and vended agent tokens once the unified auth plan lands (§30 §15, MT1);
- secrets that never reach MCP output, JSON output or logs, now including Postgres role passwords, Loams SQL credentials and connection strings.

The exit is the checklist under "Exit criteria for production", with the owning tasks.

**Architecture** (§30 §4, §44 §4–§7):
- **`crates/loams-cli` is a library with no server dependencies** (D281). The `loams` binary flattens `loams_cli::ClientCommand` beside its server commands. Product groups call only public `loams.*.v1` RPCs, through `loams-proto`'s generated clients (feature `client`). No product crate (`loams-live`, `loams-graph`, `loams-pg-control`, `loams-sqldb`, `loams-house*`) is ever a dependency. This is what keeps the `cli` variant (D297) able to drive every product remotely.
- **One API layer**, `loams_cli::api`. It resolves an `Endpoint` (a local stack, the profile's URL, `--endpoint`, or the desktop's engine), calls `GetInstance` once per process, and gates each command on the service it needs (`service_not_served`, exit 6). It also maps `loams.errors.v1` to the §30 §6 error object, gives each logical mutation one UUIDv7 `idempotency_key` that it reuses on every retry (D610), waits on `loams.operations.v1` operations, and renders server streams.
- **Local stacks grow from one process to a small process tree.** `stack run` still execs `loams dev|standalone` with registry-generated flags. Engine roles inside that process (`--postgres`, `sqldb`) start their own compose projects, as PG2 Task 56 and SQ1 Task 20 define them. The CLI also supervises sidecars (`loams-fabric house --single-node`) and container companions (`tikv`). Every address stays loopback (D111).
- **Secrets** (D288) gain three sources: `CreateRole` and `ResetRolePassword` (Postgres), `CreateRole`, `RotateRolePassword` and `CreateEphemeralCredential` (SQL), and `IssueConnectCredential` (Postgres). Each lands in a `Secret`. A secret is written only to `.env.loams` (0600), to a 0600 client-tool file for `connect`, or to a terminal on an explicit human request.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints (`unsafe_code = "forbid"`).
- CLI1's dependency set (clap 4, clap_complete, serde, serde_json, toml_edit, schemars 1, rmcp, tokio, reqwest with rustls, tantivy, zstd, tar, comfy-table, dialoguer, fs4, nix, humantime, thiserror, tracing). New in CL1:
  - `loams-proto` with feature `client` (connectrpc's client transport);
  - `uuid` (workspace, `v7`);
  - `rcgen` (workspace pin, as PG2 Task 10 and MT4 use it) for the stack's loopback CA. The SQL gate requires TLS material (`--sqlgate-tls-cert`, `--sqlgate-tls-key`, `--sqlgate-upstream-ca` in `crates/loams/src/main.rs`);
  - `arrow-ipc` (workspace arrow 58) for `house query --out` in Arrow and Parquet. It is feature `house-export`, on in every variant, so a `cli`-variant size check can drop it (Task 24).
- System tools used at run time and never linked: `psql`, `mysql`, `clickhouse client` (for `connect`), `docker` or `podman` with compose (companions), plus CLI1's list.
- External, pinned by digest in `release/companions.toml`: the TiKV and PD images of `deploy/tikv/compose.yaml` (`v8.5.8`, SQ1 Task 20).
- CI: Ubuntu runners; the e2e jobs run on the compose-capable runner pool, one at a time, never beside a cargo build.

**Spec:**
- [§30](../design/30-loams-cli.md) (all), D281–D299 and Q281–Q294 in the [decision log](../design/13-decision-log.md).
- [§44](../design/44-unified-api-and-sdks.md) §4–§7 (API rules, the catalogue, `feature_not_in_variant`, streams), including its paragraph on the binary variants.
- [§45](../design/45-loams-live-production.md) §13 (`@loams/live-cli`, `loams live deploy`) and [LV1](2026-10-08-lv1-live-production.md) Tasks 10, 12, 13, 23, 29, 30, 37.
- [§46](../design/46-loams-postgres-production.md) §4 (the API), §18 (single-node mode, D719) and [PG2](2026-10-08-pg2-postgres-production.md) Tasks 1, 9, 25, 56, 57.
- [§47](../design/47-loams-sql-production.md) §11 (`loams.sqldb.v1`) and [SQ1 (TiDB)](2026-10-08-sq1-loams-sql-tidb.md) Tasks 7, 12, 20.
- [§48](../design/48-loams-graph-production.md) and [GR1](2026-10-08-gr1-graph-production.md) Tasks 1–8 (`loams.graph.v1`, the `graph` feature, Q675).
- [§49](../design/49-loams-house-production.md) §17–§18.2 and [HS1](2026-10-08-hs1-house-production.md) Tasks 7, 8, 26, 28 (`loams-fabric house`, `loams.house.v1`, the analytics-engine download).
- [§19](../design/19-console-identity-and-agents.md) §5 and [MT1](2026-10-02-mt1-authentik-identity.md) Task 5 (`loams login`, the device flow, the `loams-cli` OAuth client).
- [§50](../design/50-loams-desktop-daemon.md) and [DD1](2026-10-09-dd1-desktop-daemon.md) Task 9 (the daemon supervises the desktop's `loams dev`).
- [§41](../design/41-multitenant-byoc-control-plane.md) §5 (`cluster enrol`, Q-CL1-4).

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/cl1-loams-cli`. Use one branch per milestone, `feat/cl1a-foundation`, `feat/cl1b-stacks`, `feat/cl1c-products`, `feat/cl1d-identity`, `feat/cl1e-mcp-docs` and `feat/cl1f-release`, each based on `dev`, with stacked PRs targeting `dev`. CLI1 said `main`; `dev` is the integration branch now. Use `git commit -s` (DCO). Commit areas: `cli`, `loams`, `cache`, `docs`, `ci`, `release`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time on the build machine (jobs and linker from `~/.cargo/config.toml`). Build the touched crates (`cargo test -p loams-cli`), not the workspace. Compose and e2e stacks run in CI, or locally only when no cargo build is running.
- **No server dependency in `loams-cli`** (CLI1's guard, extended). `cargo tree -p loams-cli -e normal --prefix none` must contain none of `loams-server`, `loams-query`, `loams-collection`, `loams-meta*`, `loams-log`, `loams-store`, `loams-mcp`, `loams-live`, `loams-live-js`, `loams-kv`, `loams-graph`, `grafeo*`, `loams-pg-control`, `loams-postgres`, `loams-sqldb`, `loams-sqlgate`, `loams-house*`, `libchdb*` or `loams-tikv`. Task 1 adds the CI step and Task 26 re-runs it.
- **Products are reached through their public API only.** A product command never reads a product's data directory, record store, compose file or admin socket. The one exception is the stack supervisor, which starts server processes and reads their logs.
- **The default build does not change** beyond CLI1's addition of `loams-cli`. `cargo tree -p loams -e normal` on default features gains only `loams-cli`, `loams-proto/client` and their dependencies (Task 1 test `default_build_gains_only_the_cli`).
- **AP0 API rules** (§44): every mutating call carries an `idempotency_key`, generated once per command invocation and reused on every retry of that call. Reads are retried; a mutation is retried only on the codes §44 marks safe, and always with the same key. Errors are read from `loams.errors.v1` (`ErrorInfo.reason`, `RetryInfo`, `BadRequest`). Pagination is `page_size`/`page_token`. List commands follow pages up to `--limit` (default 1000) and report `next_page_token` in JSON.
- **The output contract is frozen and additive** (D283, CLI1 Rulings 3, 4 and 12). Every JSON output type derives `schemars::JsonSchema` and has a schema snapshot. A change that is not additive fails `tests/schemas`. The one contract change CL1 makes is the `jsonl` format for streams (Ruling R3, Q-CL1-3).
- **Secrets** (D288). `Secret` has no `Serialize` and no `Display`. `Secret::expose` stays a `clippy::disallowed_methods` entry. CL1 allows it in exactly four places, each with `#[allow(clippy::disallowed_methods)] // §30 §11.1`:
  - `env.rs` (the dotenv writer);
  - `api/auth.rs` (the auth header);
  - `exec.rs` (the 0600 client-tool file);
  - `reveal.rs` (the TTY-only reveal).
  A product response that carries a password is converted to `Secret` in the generated-to-view mapping, before any rendering code sees it.
- **Loopback only for local stacks** (D111). Remote endpoints must be `https://`, except `http://127.0.0.1`, `http://[::1]` and `http://localhost`. A credential is never sent over plaintext to a non-loopback host (`insecure_endpoint`, exit 3).
- **No destructive action without `--yes`** or a typed TTY confirmation. Deleting a project, database, graph, branch, endpoint or role, restoring over a live branch or table, rotating a password, and activating an older Live deployment are all destructive. Without a TTY and without `--yes`, they exit 3 with `confirmation_required`.
- **No billing names** (D548, D550). `scripts/ci/no-metering.sh` covers `crates/loams-cli` (Task 26). No CLI field, flag or output key is named `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage`. House's `system.*` usage tables are rendered under the server's own column names and never renamed.
- **Pins.** Exact versions for every new crate and image, images by digest. A new dependency must be at least 14 days old (`cargo info` or the registry date). Record each pin in the task's commit message.
- **Hermetic tests** (CLI1). Unit tests use `Context::for_test(tempdir)` and fake services built from `loams-proto`'s generated service traits (`tests/fakes/<product>.rs`). End-to-end tests set `LOAMS_HOME` under the shared target's `tmp` (never `/tmp`), set `LOAMS_NO_UPDATE_CHECK=1`, and take port bases from `crates/loams/tests/cli/ports.rs`.

## Review Focus

1. **A secret leaks** into JSON output, MCP output, an error, a log line or a child process's argv: a Postgres role password, a SQL credential, a connection string, a login token or an API key. Expected: never. Tests: Task 9 `bootstrap_password_only_in_env_file`; Task 11 `connect_never_puts_password_in_argv`, `client_file_is_0600_and_removed`; Task 13 `create_role_json_has_no_password`, `reveal_requires_tty`; Task 14 `ephemeral_credential_json_redacted`; Task 23 `canary_never_appears_across_products`.
2. **A retried mutation does something twice**, such as a second project, a second branch or a double deploy. Expected: never. Every retry reuses the first key. Tests: Task 3 `retry_reuses_idempotency_key`, `mutation_not_retried_on_unsafe_code`; Task 12 `deploy_retry_is_one_deployment`; Task 13 `branch_create_retry_same_operation`.
3. **A destructive product command runs without confirmation** from a script or an agent. Expected: exit 3. Tests: Task 13 `delete_project_requires_yes`; Task 14 `delete_database_requires_yes`; Task 15 `delete_graph_requires_yes`; Task 16 `restore_table_requires_yes`; Task 20 `no_destructive_product_tool`.
4. **The CLI talks to the wrong place**: a credential sent over plaintext to a remote host, or a stack command that touches the desktop's engine. Expected: refused. Tests: Task 3 `credential_never_sent_over_plaintext_remote`; Task 7 `stack_refuses_desktop_data_dir`.
5. **A command misreports a server error**: the wrong exit code, a lost reason, or a `feature_not_in_variant` shown as an internal error. Expected: the mapping of Shared contracts. Tests: Task 3 `reason_and_exit_code_table`, `unimplemented_variant_is_exit_6_with_hint`.
6. **Stdout purity.** A stream, progress line or notice reaches stdout in `json` or `jsonl` mode. Expected: never. Tests: Task 4 `jsonl_stream_lines_are_documents`, `json_mode_buffers_stream_into_one_document`; every product task's `*_json_output_matches_schema`.
7. **Generated server flags drift** from the real clap tree as products add flags. Expected: caught. Test: Task 6 `generated_server_args_parse_with_the_real_cli` for every engine combination in `tests/golden/args/`.
8. **`loams-cli` gains a server or product dependency.** Expected: CI fails. Tests: Task 1 `cli_has_no_server_dependencies`; Task 26 `cli_variant_builds_without_server`.

---

## File structure

```
Cargo.toml / Cargo.lock                              Task 1 (loams-cli member; uuid v7, rcgen, arrow-ipc as workspace deps)
clippy.toml                                          Task 1 (Secret::expose)
crates/loams-cli/
  Cargo.toml  build.rs                               Tasks 1, 22
  src/lib.rs  context.rs  output.rs  error.rs  version.rs           Task 1
  src/home.rs  config.rs  configure.rs                              Task 2
  src/api/{mod.rs,endpoint.rs,client.rs,instance.rs,errors.rs,idempotency.rs,retry.rs,auth.rs}   Task 3
  src/api/{operations.rs,stream.rs}  src/instance_cmd.rs  src/operations_cmd.rs                  Task 4
  src/engines.rs  src/ports.rs  src/stack/{mod.rs,spec.rs,resolve.rs}                            Task 6
  src/stack/{supervise.rs,health.rs,logs.rs,sidecar.rs,desktop.rs}                               Task 7
  src/companions/{mod.rs,runtime.rs,tikv.rs,images.rs}                                           Task 8
  src/env.rs  src/secret.rs  src/project.rs  src/bootstrap.rs                                    Task 9
  src/storage/{mod.rs,lsblk.rs,plan.rs,exec.rs}                                                  Task 10
  src/exec.rs  src/reveal.rs  src/connstr.rs                                                     Task 11
  src/products/{mod.rs,select.rs}                                                                Task 11
  src/products/live.rs                                                                           Task 12
  src/products/pg.rs                                                                             Task 13
  src/products/sql.rs                                                                            Task 14
  src/products/graph.rs                                                                          Task 15
  src/products/house.rs  src/products/house_export.rs                                            Task 16
  src/auth/{mod.rs,login.rs,credentials.rs,refresh.rs}                                           Task 17 (login.rs with MT1 Task 5)
  src/auth/keys.rs                                                                               Task 18
  src/auth/agent.rs                                                                              Task 19
  src/mcp/{mod.rs,server.rs,tools.rs,install.rs,agents/*.rs}                                     Tasks 20, 21
  src/docs/{mod.rs,bundle.rs,index.rs,snippet.rs}                                                Task 22
  src/doctor.rs                                                                                  Task 25
  src/init.rs                                                                                    Task 27
  tests/{tree.rs,output.rs,api.rs,operations.rs,engines.rs,spec.rs,companions.rs,env.rs,
         storage.rs,exec.rs,live.rs,pg.rs,sql.rs,graph.rs,house.rs,auth.rs,keys.rs,
         agents.rs,mcp_tools.rs,docs.rs,canary.rs,doctor.rs}
  tests/fakes/{instance.rs,operations.rs,live.rs,postgres.rs,sqldb.rs,graph.rs,house.rs,auth.rs}
  tests/fixtures/{lsblk/,blkid/,agents/,errors/,streams/}
  tests/golden/{help/*.txt,args/*.txt,agents/**,env/*.env}  tests/schemas/*.json
crates/loams/Cargo.toml                              Task 1 (loams-cli dep; dev: assert_cmd)
crates/loams/src/main.rs                             Tasks 1, 5 (Cli { globals, command }, Command::Client, BuildInfo, --cache-* flags)
crates/loams/tests/cli/{main.rs,ports.rs,stack.rs,env.rs,mcp.rs,init.rs,
                        products_live.rs,products_pg.rs,products_sql.rs,products_graph.rs,products_house.rs}
release/companions.toml                              Task 8
release/variants.toml                                Task 24 (CLI2 Task 2's file, amended)
docs/snippets/{native,qdrant,es,pg-wire,live,postgres,sql,graph,house}/{python,typescript,shell}/*.*   Task 22
docs/guides/cli.md  docs/guides/cli/{live,postgres,sql,graph,house,auth}.md                   Tasks 12–19, 27
scripts/snippets/run.sh                              Task 22
scripts/ci/no-metering.sh                            Task 26 (covers crates/loams-cli)
.github/workflows/cli.yml                            Tasks 1, 26 (tree guard, schemas, cli-nvme, snippets, products-e2e)
docs/design/30-loams-cli.md  docs/design/13-decision-log.md  docs/plans/README.md   Tasks 0, 27
```

## Shared contracts (all tasks use these names)

### The command tree (§30 §5, extended)

```
loams [--output table|json|text|jsonl] [--profile NAME] [--endpoint URL|local:STACK|desktop]
      [--namespace NS] [--no-input] [--quiet] [-v…] [--color auto|always|never]
├── init | configure | stack | storage | env | mcp | docs | version | completions     (§30 §5, unchanged)
├── login [--issuer URL] [--no-browser] | logout | whoami                              (Task 17)
├── keys create|list|rotate|revoke                                                     (Task 18)
├── instance describe                                                                  (Task 4; GetInstance)
├── operations list|describe|wait|cancel                                               (Task 4)
├── live                                                                               (Task 12)
│   ├── apps        create|describe|list|update|delete
│   ├── deploy      --bundle FILE.js --schema FILE.json [--app] [--message] [--dry-run]
│   ├── deployments list|describe   ·  rollback [--to ID] --yes
│   ├── logs        [--follow] [--since 10m] [--function NAME]
│   ├── query FN [--args JSON]  ·  mutate FN [--args JSON]  ·  watch FN [--args JSON]
│   ├── indexes     status
│   └── backups     create|list  ·  restore --to-time T --yes      (LV1 Tasks 29–30, names as built)
├── pg                                                                                 (Task 13)
│   ├── up [--stack NAME] | down [--stack NAME]                    (the stack's `postgres` engine)
│   ├── projects    create|describe|list|update|delete|upgrade
│   ├── branches    create [--parent B] [--at-lsn L | --at-time T]|describe|list|update|delete|restore|set-default
│   ├── endpoints   create|describe|list|update|start|suspend|restart|delete
│   ├── roles       create|list|reset-password|delete
│   ├── databases   create|list|delete
│   ├── connection-string [--role R] [--pooled] [--show-password]
│   └── connect     [--role R] [--read-only] [-- PSQL_ARGS…]
├── sql                                                                                (Task 14)
│   ├── databases   create|describe|list|update|delete|suspend|resume
│   ├── branches    create [--parent B] [--at-time T]|describe|list|delete
│   ├── roles       create|list|reset-password|delete
│   ├── credentials create [--ttl 1h] [--role R]
│   ├── backups     window|list|export --to URL
│   ├── connection-string [--role R] [--show-password]
│   └── connect     [--role R] [-- MYSQL_ARGS…]
├── graph                                                                              (Task 15)
│   ├── create|describe|list|update|delete|info|schema
│   ├── query  GQL|--file F [--param K=V…] [--stream] [--timeout 30s]
│   ├── explain GQL|--file F
│   └── export --to URL | import --from URL | restore --to-time T --yes
├── house                                                                              (Task 16)
│   ├── install [--version V] [--allow-download] | uninstall                (the local sidecar binaries)
│   ├── query  SQL|--file F [--database D] [--setting K=V…] [--param K=V…] [--max-rows N]
│   │          [--read-only] [--out FILE --out-format arrow|parquet|csv|jsonl]
│   ├── explain SQL [--kind plan|pipeline|syntax|estimate]  ·  cancel QUERY_ID
│   ├── databases list  ·  tables list|describe|preview  ·  pipes list
│   ├── history list|describe
│   ├── restore-table --database D --table T (--snapshot ID | --to-time T) --yes
│   └── connect [-- CLICKHOUSE_CLIENT_ARGS…]
│
└── dev | standalone | cluster | warm | durable migrate | pg-control | live-worker | serve   (server commands; never client groups)
```

**Vocabulary rules** (they extend §30 §5's table; `tests/tree.rs` checks them):
- A product group's resources are **plural nouns**: `projects`, `branches`, `databases`, `roles`, `endpoints`, `apps`, `deployments`, `tables`. PG2 Task 57 chose this form. A product's top-level resource is the group itself when it has only one kind (`loams graph create`).
- **Verbs come from one set:** `create`, `describe`, `list`, `update`, `delete`, `start`, `suspend`, `resume`, `restart`, `restore`, `reset-password`, `set-default`, `export`, `import`, `status`, `wait`, `cancel`. One concept has one verb. `ResetRolePassword` (pg) and `RotateRolePassword` (sql) are both `reset-password`.
- **Selection flags:** `--project`, `--branch`, `--database`, `--app`, `--graph` and `--endpoint-id`. Each defaults to the profile's `[profile.X.defaults]` (Task 2), then to the only resource that exists, then fails with `selection_required` (exit 3), which lists the candidates in `details`. `--endpoint` is reserved for the API endpoint (global).
- **Time flags:** `--at-time` and `--to-time` take RFC 3339 or a humantime offset (`-15m`). `--at-lsn` takes `X/Y`.
- **Client and server names never collide.** No client group may equal `dev`, `standalone`, `cluster`, `warm`, `durable`, `pg-control`, `live-worker` or `serve` (Ruling R1).

### Global flags added to CLI1's `Globals`

```rust
#[arg(long, global = true, env = "LOAMS_ENDPOINT")] pub endpoint: Option<EndpointArg>,  // URL | local:<stack> | desktop
#[arg(long, global = true, env = "LOAMS_NAMESPACE")] pub namespace: Option<String>,
// OutputFormat gains `Jsonl` (Ruling R3).
```

Mutating product commands also take `--wait <dur>` (default `10m`) or `--no-wait`, and `--idempotency-key <uuid>` (hidden, for scripts that retry a whole command).

### Rust interfaces (`loams-cli`)

```rust
// api/endpoint.rs
pub enum EndpointSource { Flag, Env, Profile, LocalStack(StackName), Desktop }
pub struct Endpoint { pub base: url::Url, pub source: EndpointSource, pub loopback: bool }
pub fn resolve_endpoint(ctx: &Context, globals: &Globals, profile: &Profile) -> Result<Endpoint, CliError>;

// api/client.rs
pub struct ApiClient { /* connectrpc client transport, Endpoint, Option<Credential>, RetryPolicy, cached InstanceInfo */ }
impl ApiClient {
    pub async fn connect(ctx: &Context, ep: Endpoint, cred: Option<Credential>) -> Result<Self, CliError>;
    pub async fn instance(&self) -> Result<&InstanceInfo, CliError>;         // GetInstance, once per process
    pub async fn require(&self, service: &'static str) -> Result<ServiceInfo, CliError>; // service_not_served, exit 6
    pub fn mutation(&self) -> MutationCtx;                                    // one IdempotencyKey per logical mutation
    pub async fn unary<Req, Resp>(&self, call: Call<Req, Resp>) -> Result<Resp, CliError>;
    pub fn stream<Req, Item>(&self, call: StreamCall<Req, Item>) -> BoxStream<'static, Result<Item, CliError>>;
}
pub struct ServiceInfo { pub service: String, pub unstable: bool, pub variant: String }

// api/idempotency.rs
pub struct IdempotencyKey(uuid::Uuid);            // UUIDv7 (D610)
pub struct MutationCtx { pub key: IdempotencyKey } // reused by every retry of the call

// api/errors.rs
pub fn from_connect(err: &connectrpc::ConnectError) -> CliError;          // the mapping table below

// api/operations.rs
pub enum WaitMode { Wait(Duration), NoWait }
pub async fn await_operation(api: &ApiClient, op: Operation, mode: WaitMode, progress: &dyn Progress)
    -> Result<OperationOutcome, CliError>;                                // WatchOperations, else GetOperation polling at 1 s
pub enum OperationOutcome { Done(Operation), Pending(Operation) }         // Pending only under NoWait

// api/stream.rs
pub trait StreamRender { fn begin(&mut self, schema: &StreamSchema); fn item(&mut self, v: &serde_json::Value); fn end(&mut self, summary: Option<&serde_json::Value>); }

// products/mod.rs
pub trait Selection { fn resolve(api: &ApiClient, profile: &Profile, flag: Option<&str>) -> impl Future<Output = Result<String, CliError>>; }

// exec.rs (Task 11)
pub enum ClientTool { Psql, Mysql, ClickhouseClient }
pub struct ClientLaunch { pub tool: ClientTool, pub host: String, pub port: u16, pub user: String,
                          pub database: String, pub tls: ClientTls, pub secret: Secret, pub extra_args: Vec<OsString> }
pub fn launch(ctx: &Context, l: ClientLaunch) -> Result<std::process::ExitStatus, CliError>; // secret only in a 0600 file

// secret.rs (CLI1 Task 6, unchanged)
pub struct Secret(String); // no Serialize, no Display, Debug = "Secret(<redacted>)"
```

### Error mapping (`api/errors.rs`)

The §30 §6.2 error object gains one field, `reason`: the server's `ErrorInfo.reason`, verbatim. `code` is the CLI's code. For server errors, `code` equals `reason`. Server reasons are snake_case and registered in `docs/api/reasons.md`. CL1's own codes must not collide with any registered reason (`cli_codes_disjoint_from_reasons`).

| Connect code | Exit | Notes |
|---|---|---|
| `invalid_argument`, `out_of_range` | 2 | `details.field_violations` from `BadRequest` |
| `not_found` | 4 | |
| `already_exists`, `aborted`, `failed_precondition` | 5 | |
| `resource_exhausted` | 7 with `RetryInfo`, else 5 | Quotas without `RetryInfo` are conflicts |
| `unimplemented` | 6 | `feature_not_in_variant` adds the hint `install a variant with <feature>: loams self-update --variant <v>` |
| `unavailable`, `deadline_exceeded` | 7 | Retried first, by the retry policy |
| `unauthenticated` | 8 | Hint `run loams login` (after Task 17) |
| `permission_denied` | 8 | |
| `cancelled` | 130 | |
| `internal`, `unknown`, `data_loss` | 1 | |

New CLI codes: `service_not_served` (6), `selection_required` (3), `insecure_endpoint` (3), `operation_failed` (the exit of the operation's error code), `operation_timeout` (7, with `details.operation`), `client_tool_missing` (6), `engine_exclusive` (5), `companion_runtime_missing` (6), `desktop_not_running` (7), `secret_reveal_requires_tty` (3).

### The engine registry, v2 (§30 §8.2, amended; Task 6)

| Engine | Kind | Feature / artifact | Flags the stack sets | Default port (offset) | `.env.loams` | Variant |
|---|---|---|---|---|---|---|
| `native`, `flight-sql`, `mcp`, `qdrant`, `es`, `durable`, `stream-grpc`, `jobs` | in-process | as §30 §8.2 | as §30 §8.2 | as §30 §8.2 | as §30 §8.2 | as §30 §9.2 |
| `pg-wire` (was `pg`) | in-process | `pgwire` | `--pg-listen` | 15432 (+32) | `LOAMS_PG_WIRE_URL` | standard, full |
| `mysql-wire` (was `mysql`) | in-process | `mysql-wire` | `--mysql-listen` | 13306 (+06) | `LOAMS_MYSQL_WIRE_URL` | full |
| `live` | in-process, **on by default in the server** | `live` (default since LV1 Task 23) | `--live-listen` / `--no-live`, `--live-store` (embedded) | 7710 (+10) | `LOAMS_LIVE_URL`, `LOAMS_LIVE_APP` | all server variants |
| `graph` | in-process, on when compiled | `graph` | `--graph-data-dir <stack>/data/graph` / `--no-graph` | main port | — (uses `LOAMS_URL`) | full (standard per Q675) |
| `postgres` | engine role plus compose | `postgres` (PG2 Task 9) | `--postgres --postgres-stack <stack>/postgres` (PG2 Task 56) | PgDog 15433 (+43) | **`DATABASE_URL`**, `LOAMS_PG_PROJECT` | full (Q-CL1-5) |
| `sql` | engine role plus compose, needs `tikv` | `sqldb` | `--sqlgate-listen`, `--sqlgate-tls-cert/-key/-upstream-ca` (the stack CA), `--sqldb-pd` | 13307 (+07) | `LOAMS_SQL_URL` | full |
| `house` | sidecar binary | `loams-fabric` + `libchdb` (Task 16 `house install`) | engine: the House proxy endpoint flag (HS1 Task 7, name as built); sidecar: `loams-fabric house --single-node --house-listen --native-listen --admin-listen` | 18123 (+23), native 19000 (+24), admin 18125 (+25) | `CLICKHOUSE_URL`, `LOAMS_HOUSE_URL` | separate artifact |
| `tikv` | container companion | `release/companions.toml` | `--sqldb-pd 127.0.0.1:<pd>` (and `--meta`/`--live-store` only when asked) | PD 12379 (+79) | `LOAMS_TIKV_PD` | — |

- **Engines that are on by default in the server get their `--no-*` flag when not requested**: `flight-sql`, `qdrant`, `es`, `mcp`, `live` and `graph` (when compiled).
- **`postgres` and `sql` are exclusive per `LOAMS_HOME`** in CL1. At most one stack may enable each, because PG2's compose runtime allocates host ports from a fixed range (55500..56000) and SQ1's local runtime has one TiDB pool set. A second stack that asks for one exits 5 with `engine_exclusive`, naming the stack that holds it.
- **`DATABASE_URL`** is written only by `postgres` (D285 kept). `env export --database-url sql` writes the `sql` URL to `DATABASE_URL` instead, on request.

### Stack spec v2 (`stack.toml`, `schema = 2`)

CLI1's spec, plus:

```toml
schema = 2
[engines.postgres]   # present only when enabled
project = "default"  # created by bootstrap (Task 9) if absent
role = "app"
[engines.sql]
database = "app"
role = "app"
[engines.house]
fabric_binary = "/home/u/.loams/variants/0.5.0/house/loams-fabric"
[companions.tikv]
image_digests = { pd = "sha256:…", tikv = "sha256:…" }
runtime = "podman"   # recorded at create time
[ca]
cert = "/home/u/.loams/stacks/dev/ca/ca.pem"  # rcgen, ECDSA P-256, 10 years, loopback SANs only
```

A `schema = 1` file is upgraded in memory and rewritten only by `stack restart --upgrade`. An unknown schema is refused with `config_unparseable` (exit 5), as CLI1 Ruling 5 says.

### Contract decisions this plan makes

Each is a proposal until Task 0 records the owner's answer or the default. Its open question is named.

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| R1 | **Server command names are reserved**: `dev`, `standalone`, `cluster`, `warm`, `durable`, `pg-control` (PG2 Task 9), `live-worker` (LV1 Task 5) and `serve` (GR1 §48). §41's `loams cluster enrol` cannot be a client verb under the server's `cluster`; it becomes `loams byoc enrol` (Q-CL1-4) | CLI1 Ruling 1 | A rename after release breaks scripts |
| R2 | **`pg` and `postgres` mean Loams Postgres; the analytics wire engines become `pg-wire` and `mysql-wire`**. Nothing is built or published, so the rename is free now (Q-CL1-1) | PG2 Task 57 named the group `pg`. A `pg` engine that is not Postgres beside a `pg` group that is Postgres repeats the confusion §30 §8.2 warned about | One more rename if the owner prefers to keep `pg` for the wire |
| R3 | **A fourth output format, `jsonl`**, valid only for streaming commands (`live watch`, `live logs --follow`, `graph query --stream`, `house query`, `operations wait --follow`). Each line is one JSON document `{"type": "header"\|"item"\|"progress"\|"summary", …}`. In `json` mode, a stream command buffers into one document (`{"items": […], "summary": …, "truncated": bool}`) up to `--max-rows` (default 10 000), then truncates and says so. **Amends D283** (Q-CL1-3) | D283 promises "exactly one JSON document", and an unbounded `watch` cannot keep it | Scripts that want one document still get it |
| R4 | **`postgres` is Loams Postgres single-node** (`loams dev --postgres`, D719), not D299's `postgres:17` container. `pg up` is `stack create|start` with the `postgres` engine. **Amends D299** (Q-CL1-5) | D299 predates §46 §18. The desktop and the CLI should run the same Postgres | Small machines pay for the pageserver and PgDog; Q-CL1-5 asks whether to keep a plain-container fallback |
| R5 | **The CLI has its own thin client layer** over `loams-proto` (`client`), not a dependency on the Rust SDK. `sdks/rust` is its own workspace, and its crate is named `loams`, the same name as the server binary's crate, so it cannot be a dependency here. The CLI copies the SDK's retry, idempotency and reason conventions and runs the SDK conformance fixtures (`sdks/fixtures`) against itself (Q-CL1-8) | Name collision; no extra workspace | Two retry layers to keep in step until a shared `loams-client-core` exists |
| R6 | **Ownership moves.** PG2 Task 57 keeps `GetInstance` advertising and drops its CLI half (Task 13 here). LV1 Task 37 keeps `@loams/live-cli` (bundling, codegen, `dev`, `migrate`); the Rust `loams live deploy` moves here (Task 12). §46's `pg archive-restore` and `pg migrate-wal` talk to the pageserver and `loams-wal`, not to the API, so they are server commands under `loams pg-control` (PG2 Tasks 40, 43), not `loams pg` | A product's public API is the CLI's only contract; operator tools that bypass it belong to the server | None |
| R7 | **Secrets are revealed only on a TTY.** `--show-password` (connection strings) and the one-time password of `roles create` print only when stdout is a TTY and `--output table`. Otherwise the secret goes to `--write-env PATH` (default `.env.loams`) and the output says where. In JSON the field is `"<redacted: written to .env.loams>"` | D288; agents run commands in pseudo-terminals less often than people, and a pipe is never a person | A person who pipes `connection-string` gets a redacted value and must use `--write-env` |
| R8 | **No product mutation is an MCP tool in CL1.** The stdio tools stay CLI1's seven (eight with `add_package`). `stack_create` accepts the new engines, but `postgres`, `sql`, `tikv` and `house` need `allow_download: true`, because they pull images or binaries (Q-CL1-12) | D289: no destructive tools; product mutations create resources with secrets | Agents ask the person to run `loams pg branches create` |
| R9 | **The desktop's engine is reachable, never managed.** `--endpoint desktop` reads DD1's engine adoption record (`store/engine.json` in the daemon's data directory) for the engine's loopback URL. `stack *` refuses a data directory under the daemon's data directory (Q-CL1-13) | DD1 supervises the desktop's `loams dev`; two supervisors would fight | None |
| R10 | **Commands over `unstable: true` packages** print `note: loams.<pkg> is unstable in this server` on stderr in table mode, and carry `"unstable": true` in JSON. They are not refused | §44 marks preview packages; scripts must be able to tell | None |

### PgDog-routed and SQL connection strings (`connstr.rs`)

- **Postgres:** `postgresql://<role>:<secret>@<host>:<port>/<routed-name>?sslmode=verify-full&sslrootcert=<path>`. `<routed-name>` follows PG2's grammar (`project__branch.database`), formatted by `GetConnectionInfo`, never by the CLI. `<path>` is the CA PEM from `GetConnectionInfo.ca_pem`, written to `$LOAMS_HOME/ca/<endpoint-host>.pem`.
- **SQL:** `mysql://<role>:<secret>@<host>:<port>/<database>?ssl-mode=VERIFY_IDENTITY&ssl-ca=<path>`, from `DatabaseService/GetConnectionInfo`. Loopback plaintext is allowed only when the gate says so (SQ1 M6).
- **House:** `http://<host>:<port>/?database=<db>` (loopback), or `https://…` with the user's token as the password (Task 17), never a stored secret.

---

## Execution order

1. Task 0.
2. **CL1a** (Tasks 1–4), in order.
3. **CL1b** (Tasks 5–10) after CL1a. Task 5 and Task 10 are CLI1 Tasks 3 and 7 unchanged, so they can run beside Tasks 6–9. The `postgres` engine row in Task 6 needs PG2 Task 56 merged, and the `sql` and `tikv` rows need SQ1 Task 20. Until then those rows exist with `available = false`, and choosing them exits 6 with `engine_unsupported`, naming the plan task.
4. **CL1c** (Tasks 11–16) after Task 4. Task 11 comes first. Each product task runs against its fake service at once, and its `#[ignore]` e2e turns on when the product's API task has merged:
   - Task 12 needs LV1 Tasks 10 and 13;
   - Task 13 needs PG2 Task 9 (and Task 56 for `pg up`);
   - Task 14 needs SQ1 Tasks 7 and 12;
   - Task 15 needs GR1 Task 5;
   - Task 16 needs HS1 Tasks 7 and 8.
5. **CL1d** (Tasks 17–19) after MT1 Task 5 (`login`) and §19's key API (`keys`). Tasks 17–19 may land with `auth_not_available` (exit 6) behaviour first, and switch on with the server feature.
6. **CL1e** (Tasks 20–23) after CL1b. Task 23 runs again at the end of CL1c and CL1d.
7. **CL1f** (Tasks 24–27). Task 24 runs before CLI2 Task 2 if CLI2 has not started, and otherwise as an amendment PR. Task 26 is the last code task. Task 27 closes the plan.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution", and the decision log (open questions Q-CL1-1 … Q-CL1-13 registered as Q735 onward).

Steps:
1. Answer each of the following and record the answer, with file paths, as a ruling:
   - Is CLI1 still unbuilt (`crates/loams-cli` absent; `main.rs`'s `Command` has `Dev`, `Standalone`, `Cluster`, `Warm` and `Durable` only)? If any CLI1 task has merged since `1dc6e8a3`, the matching CL1 task keeps only its amendments.
   - The as-built server flags for every engine row. Check `--no-live`, `--live-listen`, `--live-store`, `--graph-data-dir`, `--no-graph`, `--sqlgate-*`, `--pg-listen` and `--mysql-listen` in `crates/loams/src/main.rs`. Check whether `--postgres`, `--postgres-stack` (PG2 Task 56), `--sqldb-pd` (SQ1 Task 20) and the House proxy flag (HS1 Task 7) exist yet, and with which names.
   - Whether Live still has its own listener (7710), or only the main Connect port after LV1 Task 12. That decides whether `LOAMS_LIVE_URL` points at 7710 or at `LOAMS_URL`.
   - The `CATALOGUE` in `crates/loams/src/api/connect.rs`: which packages and services exist, with their `available` and `unstable` flags. Which RPC names LV1 Tasks 13, 29 and 30 gave the app, backup and restore RPCs. Whether `loams.sqldb.v1` and `loams.house.v1` are in `loams-proto`'s build. If `house.proto` builds only under `fabric/`, `loams-proto` adds `proto/loams/house/v1` to its package list.
   - The `loams-proto` `client` feature: does `connectrpc`'s client transport build without hyper server features, and what does it add to the `cli` variant's size? Measure `cargo build -p loams-cli --release` once.
   - DD1's engine adoption record: its path and schema (DD1 Task 9). If DD1 has not merged, `--endpoint desktop` exits 7 (`desktop_not_running`) with the hint "start Loams Desktop".
   - MT1 Task 5's state (`crates/loams-cli/src/login.rs` is listed in MT1's file structure). If MT1 has merged a `login.rs` elsewhere, Task 17 moves it.
   - PG2's compose port range and SQ1's local runtime: confirm the exclusivity rule (one `postgres` stack and one `sql` stack per `LOAMS_HOME`).
2. Record the owner's answers to Q-CL1-1 … Q-CL1-13, or their defaults (the proposals in this plan).
3. Commit `docs(cli): cl1 task 0 rulings`.

## CL1a — Foundation and the API layer (Tasks 1–4)

### Task 1: The crate, the global flags and the output contract

**Files:** CLI1 Task 1's files, plus `crates/loams-cli/tests/fixtures/errors/` and `.github/workflows/cli.yml` (the dependency guard).

**Interfaces:** CLI1 Task 1, verbatim, with these amendments:
- `OutputFormat::{Table, Json, Text, Jsonl}`. `Jsonl` on a non-streaming command is a usage error (exit 2) with the hint "use --output json".
- The error object gains `reason: Option<String>` (Shared contracts).
- `Globals` gains `endpoint` and `namespace`.
- `ErrorCode` gains the new CLI codes of Shared contracts, each with its exit code in one `const` table.

Tests: CLI1 Task 1's tests, plus:
- `cli_has_no_server_dependencies`: the `cargo tree` guard of Global Constraints, as a test that shells out to `cargo tree --offline` (skipped when cargo is not on `PATH`) and as the CI step.
- `default_build_gains_only_the_cli`
- `jsonl_refused_on_unary_commands`
- `cli_codes_disjoint_from_reasons`: parses `docs/api/reasons.md`.
- `client_group_names_never_equal_server_commands`: covers R1's list.

Steps: tests (FAIL) → CLI1 Task 1 → amendments → PASS → commit `feat(cli): loams-cli crate, globals and the output contract`.

### Task 2: `LOAMS_HOME`, profiles and `configure`

**Files:** CLI1 Task 2's files.

**Interfaces:** CLI1 Task 2, plus:
- A profile gains `endpoint` (`local`, a URL or `desktop`), `namespace`, and a `[profile.<name>.defaults]` table with keys `pg.project`, `pg.branch`, `pg.role`, `sql.database`, `sql.branch`, `sql.role`, `live.app`, `graph.graph`, `house.database`. Each is set with `configure set defaults.pg.project my-app`.
- `credentials.toml` (0600) gets its schema now: `[profile.<name>] kind = "oidc" | "api_key"`, with the fields Task 17 fills. CL1a writes nothing to it.

Tests: CLI1 Task 2's tests, plus `defaults_round_trip`, `unknown_default_key_is_usage_error` and `credentials_file_created_0600`.

Commit `feat(cli): home, profiles, product defaults`.

### Task 3: The API client

**Files:** `src/api/{mod.rs,endpoint.rs,client.rs,instance.rs,errors.rs,idempotency.rs,retry.rs,auth.rs}`, `tests/api.rs`, `tests/fakes/instance.rs`.

**Interfaces:**
- `resolve_endpoint`. The order is `--endpoint`, then `LOAMS_ENDPOINT`, then the profile's `endpoint`, then `local:<default_stack>` (the stack's native URL from `stack.toml`). `desktop` follows R9.
- `ApiClient` as in Shared contracts:
  - Connect over HTTP/1.1 and HTTP/2, with binary protobuf. JSON is used only for `-vv` request dumps, which are redacted.
  - `require` reads `GetInstance.services[]`.
  - `auth.rs` adds `Authorization: Bearer …` from a `Credential` (Task 17), or nothing.
- `RetryPolicy`: reads retry on `unavailable`, `deadline_exceeded` and `resource_exhausted` with `RetryInfo`, with exponential backoff (200 ms base, ×2, full jitter, 5 attempts, 30 s cap) that honours `RetryInfo.retry_delay`. Mutations retry on the same codes **only** with their `MutationCtx` key.
- `from_connect`: the error mapping table.
- `insecure_endpoint`: a credential plus a non-loopback `http://` base.

Tests:
- `endpoint_resolution_order`
- `local_stack_endpoint_reads_stack_toml`
- `require_reports_service_not_served_with_variant`
- `unimplemented_variant_is_exit_6_with_hint`
- `reason_and_exit_code_table`: one fixture per row, from `tests/fixtures/errors/`.
- `retry_reuses_idempotency_key`: the fake fails twice with `unavailable`, then records three calls with one key.
- `mutation_not_retried_on_unsafe_code`: `internal` is not retried.
- `retry_honours_retry_info`
- `credential_never_sent_over_plaintext_remote`
- `request_dump_redacts_authorization`

Commit `feat(cli): connect api client, errors and idempotency`.

### Task 4: Operations, streams, `instance` and `operations`

**Files:** `src/api/{operations.rs,stream.rs}`, `src/{instance_cmd.rs,operations_cmd.rs}`, `tests/operations.rs`, `tests/fakes/operations.rs`, `tests/fixtures/streams/`.

**Interfaces:**
- `await_operation`:
  - With `Wait(d)`, it follows `WatchOperations` filtered to the id, falling back to `GetOperation` every 1 s when the stream is not served, until the operation is done or `d` passes (`operation_timeout`, exit 7, with `details.operation`).
  - A failed operation becomes `operation_failed`, with the operation's error reason and that reason's exit code.
  - `NoWait` returns the operation at once.
  - Table mode shows progress on stderr.
- `StreamRender` with three implementations, `TableStream`, `JsonBufferStream` (R3: buffer, cap and `truncated`) and `JsonlStream`. SIGINT ends a stream cleanly: the summary is written and the exit is 130.
- `loams instance describe` prints `GetInstance` (version, `api_versions`, services with `available` and `unstable`, the variant and features). `loams operations list|describe|wait|cancel` map to `OperationsService`.

Tests:
- `wait_until_done_via_watch`
- `wait_falls_back_to_polling`
- `wait_timeout_is_exit_7_with_operation`
- `failed_operation_maps_reason`
- `no_wait_returns_pending`
- `jsonl_stream_lines_are_documents`
- `json_mode_buffers_stream_into_one_document`
- `json_buffer_truncates_at_max_rows`
- `sigint_writes_summary_and_exits_130`
- `instance_describe_json_output_matches_schema`
- `operations_cancel_requires_yes`

Commit `feat(cli): operations, streams, instance`. **CL1a exit:** every Task 1–4 test passes, and `loams instance describe --endpoint local:dev --output json` works against `loams dev`.

## CL1b — Local stacks with the products (Tasks 5–10)

### Task 5: The H1 cache flags on the server

**Files and interfaces:** CLI1 Task 3, verbatim (`--cache-ram-bytes`, `--cache-dir`, `--cache-disk-bytes` in the `Native` group; CLI1 Ruling 11).

Tests: CLI1 Task 3's. Commit `feat(loams): h1 cache flags`.

### Task 6: The engine registry v2 and the stack spec v2

**Files:** CLI1 Task 4's files, plus `tests/golden/args/*.txt` (one per engine combination).

**Interfaces:** CLI1 Task 4, with:
- The registry v2 table of Shared contracts:
  - `EngineKind::{InProcess, Role, Sidecar, Companion}`;
  - `Engine::requires`: `sql` → `tikv`, `house` → `native`;
  - `Engine::exclusive`: `postgres` and `sql`;
  - `Engine::available(&BuildInfo, &Task0Rulings)`.
- The renames `pg` → `pg-wire` and `mysql` → `mysql-wire` (R2). The old names are rejected with a hint that names the new one. Nothing was ever published, so there is no alias.
- The port offsets +07, +23, +24, +25, +43 and +79 are added to CLI1's offset table. `ports_offsets_are_unique` covers all of them.
- Stack spec v2 (`schema = 2`), with the in-memory upgrade from `schema = 1`.
- The stack CA. `stack create` makes `ca/ca.pem` and `ca/ca.key` (0600) with rcgen when `sql` or `postgres` is enabled. It issues the gate's server certificate for `127.0.0.1`, `::1` and `localhost`, 30 days, renewed by `stack start` when under 15 days remain.

Tests: CLI1 Task 4's tests, plus:
- `generated_server_args_parse_with_the_real_cli`: every golden combination, including `live`, `graph`, `postgres`, `sql` and `house`, parsed by the real `loams` clap tree. Feature-gated rows are skipped, with a printed reason, when the test binary lacks the feature.
- `unrequested_default_engines_get_no_flags`: `--no-live` and `--no-graph` are emitted.
- `sql_pulls_in_tikv`
- `second_postgres_stack_is_engine_exclusive`
- `old_engine_names_hint_the_rename`
- `schema1_stack_upgrades_in_memory_only`
- `stack_ca_has_loopback_sans_only`

Commit `feat(cli): engine registry v2 with the product engines`.

### Task 7: The supervisor with sidecars, exclusivity and the desktop

**Files:** CLI1 Task 5's files, plus `src/stack/{sidecar.rs,desktop.rs}`.

**Interfaces:** CLI1 Task 5, with:
- **Readiness.** `/ready`, then `GetInstance`, which must list each enabled engine's service (`loams.live.v1.LiveService`, `loams.graph.v1.GraphService`, `loams.postgres.v1.PostgresService`, `loams.sqldb.v1.DatabaseService`, `loams.house.v1.HouseService`). Then TCP accept on each engine port. Role engines get their own wait: `--wait-timeout` defaults to 180 s when `postgres` or `sql` is enabled, because compose pulls and the pageserver start take longer.
- **Sidecars** (`house`). `stack start` spawns `loams-fabric house --single-node …` in the stack's process group, after the engine is ready. It logs to `logs/house.log` and writes `house.pid`. `stack stop` stops the sidecar first, then the engine (SIGTERM, grace, SIGKILL).
- **Role engines.** Stopping the engine is enough: `pg-control` and the `sqldb` runtime own their compose projects and stop them on shutdown (PG2 Task 56, SQ1 Task 20). `stack delete` also runs `compose down -v` for the stack's project names, with `--yes`, so a crashed engine leaves no containers behind. `--keep-data` keeps the volumes.
- **Desktop.** Under R9, `stack create` refuses a `--storage` or data path inside the daemon's data directory.

Tests: CLI1 Task 5's tests, plus:
- `ready_waits_for_engine_services`
- `sidecar_started_after_engine_and_stopped_first`
- `sidecar_crash_reports_unhealthy`
- `delete_downs_compose_projects`
- `stack_refuses_desktop_data_dir`
- e2e `it_stack_live_graph_lifecycle`

Commit `feat(cli): supervisor with sidecars and role engines`.

### Task 8: Container companions and `tikv`

**Files:** `src/companions/{mod.rs,runtime.rs,tikv.rs,images.rs}`, `release/companions.toml`, `tests/companions.rs`.

**Interfaces:**
- `ContainerRuntime::detect`. `LOAMS_CONTAINER_RUNTIME` overrides it. Otherwise it tries `docker compose`, `podman compose` and `docker-compose`, in the order PG2 Task 10 uses. `companion_runtime_missing` (exit 6) when none is found.
- `release/companions.toml`. Each image has a digest, and the TiKV and PD tags come from `deploy/tikv/compose.yaml` (`v8.5.8`).
- An image pull is a download, so it needs `--allow-download` or a prompt, and otherwise exits 3 with `download_consent_required` (§30 §6.4). Images are always referenced by digest.
- The `tikv` companion runs as compose project `loams-<stack>-tikv` with PD on the stack's +79 port and its data under `stacks/<name>/companions/tikv`. Its lifecycle follows the stack's. It sets `--sqldb-pd`, plus `--meta` or `--live-store tikv://…` only when `--metastore tikv://local` or `--live-store tikv` is asked for.
- `rustfs` (D299) stays deferred. `--object-store` already takes `s3://…` for a bucket the user runs.

Tests:
- `runtime_detection_order`
- `pull_requires_consent`
- `images_referenced_by_digest_only`
- `tikv_compose_golden`
- `tikv_follows_stack_lifecycle` (a fake runner)
- e2e `it_tikv_companion_pd_healthy` (`#[ignore]`, `products-e2e`)

Commit `feat(cli): container companions and tikv`.

### Task 9: Product bootstrap and `.env.loams` v2

**Files:** CLI1 Task 6's files, plus `src/bootstrap.rs`, `tests/golden/env/*.env`.

**Interfaces:** CLI1 Task 6, with:
- **Bootstrap** after a stack is ready, through the public API and idempotently. Each step carries a fixed `idempotency_key` derived as UUIDv5 of `(stack, step)`, so a re-run returns the first result:
  - `postgres`: `CreateProject("default")` if absent, then wait for `ready`. `CreateRole("app")` once; its password goes into `stacks/<name>/secrets/postgres-app` (0600). Then `GetConnectionInfo` → `DATABASE_URL`.
  - `sql`: `CreateDatabase("app")`, then `CreateRole("app")`, in the same way → `LOAMS_SQL_URL`.
  - `live`: `CreateApp("dev")` if LV1 Task 13's directory exists, otherwise the default app → `LOAMS_LIVE_APP`.
- The variables of the registry v2 table. `env export` re-reads the stored secrets, and never calls `reset-password`.
- `env_export` from MCP redacts `DATABASE_URL` and `LOAMS_SQL_URL` as whole values (`"<redacted: in .env.loams>"`).

Tests: CLI1 Task 6's tests, plus:
- `bootstrap_is_idempotent`
- `bootstrap_password_only_in_env_file`
- `database_url_only_from_postgres`
- `env_golden_per_engine_set`
- `mcp_env_export_redacts_connection_urls`

Commit `feat(cli): product bootstrap and env v2`.

### Task 10: `storage inspect`, `storage prepare` and NVMe stacks

**Files, interfaces and tests:** CLI1 Task 7, verbatim. Commit `feat(cli): storage inspect and prepare`. **CL1b exit:** `loams stack create --name dev --engines live,graph,postgres --allow-download` reaches `running`, with a working `.env.loams`, on the `products-e2e` runner. Every Task 5–10 test passes.

## CL1c — The product command groups (Tasks 11–16)

### Task 11: Shared product plumbing: selection, connection strings, reveal and `connect`

**Files:** `src/products/{mod.rs,select.rs}`, `src/{exec.rs,reveal.rs,connstr.rs}`, `tests/exec.rs`.

**Interfaces:**
- `Selection` resolution (Shared contracts) for each selection flag, with `selection_required` listing candidates.
- `connstr.rs`: the three formats. A secret is a `Secret` until it reaches the writer.
- `reveal.rs`: R7.
- `exec.rs::launch`:
  - `psql`: the password goes in a `PGPASSFILE` written 0600 under `$LOAMS_HOME/run/`, plus `PGSSLMODE=verify-full` and `PGSSLROOTCERT`;
  - `mysql`: `--defaults-extra-file` written 0600, holding `[client] password=…`, plus `--ssl-mode=VERIFY_IDENTITY --ssl-ca`;
  - `clickhouse client`: `--password` is never used; a `--config-file` written 0600 instead.
  - The file is removed when the child exits, and on SIGINT, SIGTERM and panic, through a guard. The child inherits the TTY, and its exit code becomes the CLI's.
  - `client_tool_missing` (exit 6) names the package to install.

Tests:
- `connect_never_puts_password_in_argv`: captures the spawned argv through the fake spawner.
- `client_file_is_0600_and_removed`
- `client_file_removed_on_sigint`
- `exit_code_is_the_childs`
- `reveal_requires_tty`
- `selection_required_lists_candidates`
- `connstr_formats_golden`

Commit `feat(cli): product selection, connection strings and connect`.

### Task 12: `loams live`

**Files:** `src/products/live.rs`, `tests/live.rs`, `tests/fakes/live.rs`, `crates/loams/tests/cli/products_live.rs`, `docs/guides/cli/live.md`.

**Interfaces** (each row is one subcommand → RPC):

| Command | RPC | Notes |
|---|---|---|
| `apps create\|describe\|list\|update\|delete` | `LiveAdminService.{CreateApp,GetApp,ListApps,UpdateApp,DeleteApp}` (LV1 Task 13) | `delete` needs `--yes` |
| `deploy --bundle --schema [--app] [--message] [--dry-run]` | `LiveAdminService.Deploy` with `expected_active` from `ListDeployments` | One key per invocation. A conflict is exit 5 with `details.active`. The CLI does no bundling: it reads `file.js` as bytes (LV1 Task 37's contract) |
| `deployments list\|describe` | `ListDeployments`, `GetDeployment` | |
| `rollback [--to ID] --yes` | `ActivateDeployment` | Without `--to`, the previous deployment |
| `logs [--follow] [--since] [--function]` | `TailLogs` (stream) | `jsonl` when following |
| `query\|mutate FN --args JSON` | `LiveService.{Query,Mutate}` | `mutate` prints `commit_ts` |
| `watch FN --args JSON` | `LiveService.Watch` + `ModifyQuerySet` | A stream. Each transition is one item |
| `indexes status` | `GetIndexStatus` | |
| `backups create\|list`, `restore --to-time --yes` | LV1 Tasks 29–30's RPCs (names per Task 0) | `restore` is destructive and waits on its operation |

The `loams-live-app` header comes from `--app`, then the profile's `live.app`, then `LOAMS_LIVE_APP`.

Tests:
- `deploy_sends_expected_active_and_reports_conflict`
- `deploy_retry_is_one_deployment`
- `rollback_without_to_activates_previous`
- `rollback_requires_yes`
- `watch_jsonl_one_line_per_transition`
- `logs_follow_ends_cleanly_on_sigint`
- `live_json_output_matches_schema`
- e2e `live_deploy_cli_roundtrip` (LV1 Task 37's named test, against `loams dev`)

Commit `feat(cli): loams live`.

### Task 13: `loams pg`

**Files:** `src/products/pg.rs`, `tests/pg.rs`, `tests/fakes/postgres.rs`, `crates/loams/tests/cli/products_pg.rs`, `docs/guides/cli/postgres.md`.

**Interfaces:**
- `pg up [--stack]`: if the stack exists, enable the `postgres` engine (`stack.toml` rewrite plus restart, with consent); otherwise `stack create --engines native,postgres`. Then bootstrap (Task 9). `pg down` stops the stack's Postgres by restarting the stack without the engine, and keeps its data.
- Resource commands, one per `PostgresService` RPC:
  - `projects create|describe|list|update|delete|upgrade` → `CreateProject` … `UpgradeProject`;
  - `branches create|describe|list|update|delete|restore|set-default` → `CreateBranch` (`--parent`, `--at-lsn`, `--at-time`), `RestoreBranch`, `SetDefaultBranch`;
  - `endpoints create|describe|list|update|start|suspend|restart|delete`;
  - `roles create|list|reset-password|delete`;
  - `databases create|list|delete`.
  Mutations returning an `Operation` wait (Task 4).
- `roles create` and `roles reset-password` follow R7: the password is shown once on a TTY, written to `--write-env`, or both. The JSON never carries it.
- `connection-string` uses `GetConnectionInfo`, plus the stored role secret (local) or `IssueConnectCredential` (remote, short-lived, never stored).
- `connect` is the same, then `exec::launch(Psql)`. `--read-only` uses the `__ro` routed name.
- `describe` on a branch shows `wal` heads (`commit_lsn`, `flush_lsn`, `remote_consistent_lsn`, `backup_lsn`) as `X/Y` strings.

Tests:
- `delete_project_requires_yes`
- `branch_create_retry_same_operation`
- `branch_at_time_and_lsn_are_exclusive` (exit 2)
- `create_role_json_has_no_password`
- `connect_uses_issued_credential_remote`: the credential is not stored.
- `cli_connect_issues_credential_and_execs_psql` (PG2 Task 57's named test, here, with the fake exec)
- `read_only_uses_ro_routed_name`
- `pg_up_enables_engine_with_consent`
- `pg_json_output_matches_schema`
- e2e `it_pg_up_branch_connect` (`#[ignore]`, after PG2 Task 56: `pg up`, `branches create`, `connect -- -c 'select 1'`)

Commit `feat(cli): loams pg`.

### Task 14: `loams sql`

**Files:** `src/products/sql.rs`, `tests/sql.rs`, `tests/fakes/sqldb.rs`, `crates/loams/tests/cli/products_sql.rs`, `docs/guides/cli/sql.md`.

**Interfaces:**
- `databases create|describe|list|update|delete|suspend|resume` → `DatabaseService`. `describe` shows `stage` and `engine_version`.
- `branches create [--parent] [--at-time]|describe|list|delete` → `BranchService`. `--at-time` maps to `point_ts`. Before calling, the CLI checks `GetRestoreWindow` and refuses a time outside `gc_window_start…now` locally, naming the window. The server stays the authority.
- `roles create|list|reset-password|delete` → `RoleService` (`RotateRolePassword`).
- `credentials create [--ttl]` → `CreateEphemeralCredential`. It is written to `--write-env` or revealed under R7.
- `backups window|list|export --to URL` → `BackupService`. `export` waits on its operation.
- `connection-string` and `connect`, with `exec::launch(Mysql)`.

Tests:
- `delete_database_requires_yes`
- `suspend_resume_wait_for_stage`
- `branch_time_outside_window_refused_locally`
- `ephemeral_credential_json_redacted`
- `reset_password_maps_to_rotate`
- `sql_json_output_matches_schema`
- e2e `it_sql_create_connect` (`#[ignore]`, after SQ1 Task 20, `LOAMS_IT_SQLDB=1`)

Commit `feat(cli): loams sql`.

### Task 15: `loams graph`

**Files:** `src/products/graph.rs`, `tests/graph.rs`, `tests/fakes/graph.rs`, `crates/loams/tests/cli/products_graph.rs`, `docs/guides/cli/graph.md`.

**Interfaces:**
- `create|describe|list|update|delete|schema|info` → `GraphAdminService.{CreateGraph,GetGraph,ListGraphs,UpdateGraph,DeleteGraph,GetSchema,GetEngineInfo}`.
- `query`:
  - `Execute` by default, or `ExecuteStream` with `--stream`;
  - `--param K=V`: typed parameters, where a value parses as JSON when it can and as a string otherwise (GR1's typed values);
  - `--file` reads GQL from a file, or from stdin with `-`;
  - the table renders typed values, and JSON keeps the typed form.
- `explain` → `Explain`.
- `export --to` and `import --from` → `ExportGraph` and `ImportGraph`. `restore --to-time --yes` → `RestoreGraph`. Each waits on its operation.
- Without the `graph` feature in the server, every command exits 6 with `feature_not_in_variant` (Q675's variant in the hint).

Tests:
- `delete_graph_requires_yes`
- `param_typing`
- `query_from_stdin`
- `stream_jsonl_rows`
- `graph_reasons_map` (`gql_syntax_error` → 2, `graph_not_found` → 4, `graph_write_conflict` → 5)
- `graph_not_in_variant_is_exit_6`
- `graph_json_output_matches_schema`
- e2e `it_graph_create_query` (against `loams dev` built with `graph`)

Commit `feat(cli): loams graph`.

### Task 16: `loams house`

**Files:** `src/products/{house.rs,house_export.rs}`, `tests/house.rs`, `tests/fakes/house.rs`, `crates/loams/tests/cli/products_house.rs`, `docs/guides/cli/house.md`.

**Interfaces:**
- `install [--version] [--allow-download]` downloads `loams-fabric`, `loams-house-worker` and `libchdb` for the platform, by digest, from the House manifest. It is the same signed manifest and verification path as the desktop's "Install the analytics engine" (HS1 Task 28) and CLI2's `self-update`. The files go to `$LOAMS_HOME/variants/<version>/house/`. Windows gets exit 6 (`platform_unsupported`, "House is remote-only on Windows", §49). `uninstall` removes them, and refuses while a stack uses them.
- `query` → `ExecuteQuery`:
  - `JSON_ROWS` for `table`, `json` and `jsonl` output;
  - `ARROW_IPC` with `--out-format arrow|parquet` (parquet through `arrow-ipc` → `parquet`, feature `house-export`);
  - `csv` from `JSON_ROWS`;
  - `--read-only` sets `read_only`. `--setting` and `--param` fill the maps;
  - progress goes to stderr, and the summary is the final item.
  The query id is printed on stderr first, so another terminal can `cancel` it.
- `explain`, `cancel` → `ExplainQuery` and `CancelQuery`.
- `databases list`, `tables list|describe|preview`, `history list|describe`, `pipes list` → the matching RPCs.
- `restore-table … --yes` → `RestoreTable`, then wait.
- `connect` → `exec::launch(ClickhouseClient)` against the HTTP or native port. Remote connections use the token from Task 17 as the password, through the 0600 config file.
- `house_*` reasons map: `house_syntax_error` → 2, `house_unknown_table` → 4, `house_read_only` → 5, `house_timeout` → 7, `house_quota_exceeded` → 5, `house_not_configured` → 6 (hint `loams house install`, or enable the `house` engine). `ErrorInfo.metadata.clickhouse_code` is carried in `details`.

Tests:
- `query_json_and_arrow_agree` (the same rows through both formats)
- `parquet_export_roundtrip`
- `max_rows_truncation_reported`
- `restore_table_requires_yes`
- `install_verifies_digest_and_signature` (a tampered file gives exit 9)
- `install_refused_on_windows`
- `query_id_printed_before_rows`
- `house_json_output_matches_schema`
- e2e `it_house_install_query_numbers` (`#[ignore]`, after HS1 Task 7: `stack create --engines house`, `house query 'select sum(number) from numbers(10)'`)

Commit `feat(cli): loams house`. **CL1c exit:** each product's fake-service suite passes, and each e2e passes once its product dependency has merged.

## CL1d — Identity, keys and agent tokens (Tasks 17–19)

### Task 17: `login`, `logout`, `whoami` and credentials

**Files:** `src/auth/{mod.rs,login.rs,credentials.rs,refresh.rs}`, `tests/auth.rs`, `tests/fakes/auth.rs`, `docs/guides/cli/auth.md`. `login.rs` is MT1 Task 5's file, and MT1 owns the protocol. CL1 owns the command, storage and refresh.

**Interfaces:**
- `loams login [--issuer URL] [--no-browser]`:
  - the device-code flow against the issuer's `loams-cli` public client (MT1 Ruling 1);
  - then the RFC 8693 exchange at Loams' token endpoint (MT1 Ruling 4), with `audience` = the profile's environment;
  - access and refresh tokens are stored in `credentials.toml` (0600) as `Secret`.
  The browser opens only when `--no-browser` is absent and a display is present; the verification URL and code always print on stderr.
- `refresh.rs` refreshes the access token 60 s before expiry and on one `unauthenticated` reply, once.
- `logout` revokes the refresh token (if the server supports revocation) and deletes the profile's entry.
- `whoami` → `InstanceService.WhoAmI`.
- Before the server advertises auth, `login` exits 6 (`auth_not_available`, §30 §15).
- When the host resolves into `100.64.0.0/10` and the connection fails, the hint "is your tailnet connected?" is added (§30 §15, §43).

Tests:
- `device_flow_happy_path`
- `device_flow_expired_code_exit_8`
- `tokens_stored_0600_and_redacted_in_debug`
- `refresh_before_expiry`
- `single_retry_after_unauthenticated`
- `logout_removes_profile_credentials`
- `login_without_auth_server_is_exit_6`
- `tailnet_hint_on_cgnat_host`
- `cli_login_end_to_end` (MT1's named test; compose with Authentik, `#[ignore]`)

Commit `feat(cli): login, logout, whoami`.

### Task 18: `loams keys`

**Files:** `src/auth/keys.rs`, `tests/keys.rs`.

**Interfaces:** §30 §15 verbatim. `keys create [--stack] [--env] [--name] [--expires 90d] [--write-env PATH]`, `list`, `rotate KEY_ID`, `revoke KEY_ID --yes`, against §19's key API (the RPC names as built, recorded by Task 0). The token is printed once on a TTY, or written to `--write-env` as `LOAMS_KEY_ID` and `LOAMS_API_KEY`. A local stack's bootstrap key goes to `stacks/<name>/secrets/` (0600). Keys for remote endpoints are never stored.

Tests:
- `key_token_printed_once_on_tty_only`
- `write_env_holds_whole_token` (D65's `loams_<key_id>_<secret>`)
- `revoke_requires_yes`
- `remote_keys_never_stored`
- `keys_json_output_matches_schema`

Commit `feat(cli): api keys`.

### Task 19: Agent tokens for `mcp serve`, and the remote-profile policy

**Files:** `src/auth/agent.rs`, `src/mcp/install.rs` (agent registration), `tests/auth.rs`.

**Interfaces:**
- `mcp install`, with a signed-in profile, registers an agent principal owned by the user (§19 §5.1). The policy is `collections:read`, `collections:write`, `query` and `mcp:tools`, plus the product scopes the person picks (`live:read`, `pg:connect`, `sql:connect`, `graph:read`, `house:read`). None is selected by default.
- `mcp serve` exchanges the user's credential for a vended agent token (§19 §5.2 flow 3; TTL 15 min) for each call it makes. The user's credential never leaves the process.
- A remote profile with credentials refuses `http://` (`insecure_endpoint`). A cloud endpoint is an ordinary profile URL: there is no `stack create --target cloud`, because cloud resources are created through the product groups against the same APIs (answers Q291's CLI half).

Tests:
- `agent_token_vended_per_call_and_never_returned`
- `agent_cannot_request_admin_scopes`
- `product_scopes_opt_in`
- `remote_http_with_credentials_refused`

Commit `feat(cli): agent principals and vended tokens`. **CL1d exit:** `loams login` against the MT1 compose stack, then `loams pg projects list --endpoint https://…`, works with the token refreshed. Every Task 17–19 test passes.

## CL1e — MCP, docs and the canary (Tasks 20–23)

### Task 20: `mcp serve` over stdio

**Files:** CLI1 Task 9's files.

**Interfaces:** CLI1 Task 9, with:
- `stack_create`'s `engines` enum is the registry v2 list. `postgres`, `sql`, `tikv` and `house` need `allow_download: true` (R8).
- `get_sdk_snippet`'s `engine` enum gains `live`, `postgres`, `sql`, `graph` and `house`. Its `task` enum gains `query`, `mutate`, `watch`, `branch` and `connect` where the snippets exist (Task 22).
- `stack_status` reports product endpoints and variable names, never values.
- No other tool is added (R8).

Tests: CLI1 Task 9's tests, plus:
- `no_destructive_product_tool`: the tool list against a deny list of product verbs.
- `stack_create_product_engines_need_allow_download`
- `tool_list_is_fixed`: the same list with and without stacks or products.

Commit `feat(cli): stdio mcp server`.

### Task 21: `mcp install` and `mcp uninstall`

**Files, interfaces and tests:** CLI1 Task 10, verbatim (plus Task 19's agent registration when signed in). Commit `feat(cli): mcp install`.

### Task 22: The embedded docs and the product snippets

**Files:** CLI1 Task 8's files, plus `docs/snippets/{live,postgres,sql,graph,house}/{python,typescript,shell}/…` and `pg-wire` (renamed from `pg`).

**Interfaces:** CLI1 Task 8, with the product snippets:
- `live`: `connect`, `query`, `mutate`, `watch`;
- `postgres`: `connect` (driver from `DATABASE_URL`), `branch` (`shell` through the CLI);
- `sql`: `connect`;
- `graph`: `connect`, `query`;
- `house`: `connect`, `query`.
Each reads its endpoint from `.env.loams` variables and never inlines a value. The `snippets` CI job starts a stack with the snippet's engine. Rows whose engine is unavailable in CI's variant are skipped, and the job prints them.

Tests: CLI1 Task 8's tests, plus `snippet_front_matter_names_env_vars_in_registry` and `no_snippet_inlines_a_secret_or_port`.

Commit `docs(cli): embedded docs and product snippets`.

### Task 23: The secret canary across products

**Files:** `crates/loams-cli/tests/canary.rs`, `crates/loams/tests/cli/mcp.rs` (extended).

**Interfaces:** CLI1 Task 9's canary, widened. The fakes return the canary `loams_cnry_9f2b…` as:
- every password (`CreateRole`, `ResetRolePassword`, `RotateRolePassword`);
- every credential (`IssueConnectCredential`, `CreateEphemeralCredential`);
- every login token and every API key.
Every command then runs in `json`, `jsonl`, `text` and non-TTY `table` modes, and every MCP tool runs with valid and invalid arguments. The test scans stdout, stderr, `server.log`, `mcp.log`, the spawned argv and the `-vv` request dumps.

Tests:
- `canary_never_appears_across_products`
- `canary_only_in_env_file_and_client_file`: positive control.
- `panic_message_has_no_canary`: a forced panic in a product handler.

Commit `test(cli): secret canary across products`. **CL1e exit:** Tasks 20–23 pass. The canary runs in CI on every PR that touches `crates/loams-cli/**`.

## CL1f — Release, compatibility and gates (Tasks 24–27)

### Task 24: Variants and release, amended for the products

**Files:** `release/variants.toml` (CLI2 Task 2's file), CLI2's plan (an amendment note under its "Rulings made during execution"), `release/house-manifest` signing steps (with CLI2 Task 4).

**Interfaces:**
- The variant matrix (§30 §9.2, amended):
  - `cli`: `--no-default-features` (D297), plus the `loams-proto/client` and `house-export` features of `loams-cli`;
  - `standard`: `es, flight, hnsw, qdrant, mcp, pgwire, durable, live`, plus `graph` if Q675 says so;
  - `full`: `standard` plus `tikv, durable-tikv, live-tikv, stream-grpc, mysql-wire, jobs, graph, sqldb, postgres`;
  - never: `failpoints`, `cluster-tests`, `durable-mysql`.
- House is **not a variant**. `loams-fabric`, `loams-house-worker` and `libchdb` ship as a separate signed artifact set per target, listed in `loams-release.json` under `"components": [{"name": "house", …}]`. Both `loams house install` and the desktop read that list.
- The CLI2 variant guard also checks that `full` contains every in-binary product feature.

Tests:
- `variant_guard_rejects_failpoints` (CLI2's)
- `full_contains_every_product_feature`
- `cli_variant_has_no_server_crates` (the `cargo tree` of the `cli` build)
- `release_manifest_lists_house_components`

Commit `release(cli): variants and house components for the products`.

### Task 25: Version skew, `loams doctor` and unstable packages

**Files:** `src/doctor.rs`, `tests/doctor.rs`, `src/version.rs` (extended).

**Interfaces:**
- `loams version --output json` gains `api_versions` (the packages this CLI was generated against) and `output_schema` (1, or 2 if Q-CL1-3's `jsonl` is ruled a breaking change).
- **Skew policy.** The CLI talks to servers at its minor version or one below (N/N−1, as §10 §7). An older server prints a stderr notice. A product command whose package is missing from the server's `api_versions` exits 6 (`service_not_served`).
- `loams doctor [--stack]` checks, read-only:
  - the binary and variant;
  - `LOAMS_HOME` permissions (0700, secrets 0600);
  - the update cache;
  - each stack's state, ports and CA expiry;
  - the container runtime;
  - the client tools (`psql`, `mysql`, `clickhouse`);
  - the endpoint's `GetInstance` and the skew;
  - the signed-in state.
  It prints a check list (`ok`, `warn`, `fail`) and exits 0, or 5 when any check fails.
- R10's unstable notice.

Tests:
- `skew_older_server_warns`
- `missing_package_is_service_not_served`
- `doctor_flags_world_readable_secret`
- `doctor_json_output_matches_schema`
- `unstable_notice_stderr_only`

Commit `feat(cli): doctor and version skew`.

### Task 26: The end-to-end matrix and the production gates

**Files:** `.github/workflows/cli.yml` (the jobs `cli-unit`, `cli-schemas`, `cli-tree-guard`, `cli-nvme`, `snippets`, `products-e2e` and `cli-variant`), `scripts/ci/no-metering.sh`, `crates/loams/tests/cli/*.rs`.

**Interfaces:**
- `products-e2e` runs every `#[ignore]` product e2e whose dependency has merged, one at a time, on the compose runner. It is marked required per product once that product's dependency merges.
- `cli-variant` builds the `cli` variant and runs the fake-service product suites against it.
- `no-metering.sh` covers `crates/loams-cli`.
- **The performance budgets** (measured on the CI runner, recorded in the PR):
  - `loams --help` under 50 ms;
  - `loams instance describe` against a local stack under 150 ms;
  - `loams pg branches list` (100 branches, fake) under 300 ms;
  - `cli` variant archive under 30 MB.

Tests:
- `cli_variant_builds_without_server`
- `no_metering_guard_covers_cli` (a fixture field `billable_rows` fails it)
- the budget checks as `#[ignore]` benches, run in `cli.yml` with thresholds

Commit `ci(cli): e2e matrix and gates`.

### Task 27: `init`, the guides, design amendments and plan status

**Files:**
- CLI1 Task 11's files;
- `docs/guides/cli.md` and `docs/guides/cli/*.md`;
- `docs/design/30-loams-cli.md`: §5 tree, §8.2 registry v2, D283 (R3), D299 (R4), §19 roadmap (CL1 row);
- `docs/design/13-decision-log.md`: the Q-CL1 answers;
- `docs/plans/README.md`: the CL1 row; CLI1 marked "Absorbed into CL1"; CLI3 replaced by CL1d;
- the CLI2 amendment note;
- PG2 Task 57 and LV1 Task 37: notes that their CLI halves moved here (R6).

**Interfaces:** CLI1 Task 11's `init`, plus a product picker. The person chooses products, and `init` maps them to engines (`live` and `graph` default on in `standard`; `postgres`, `sql` and `house` need downloads). Each guide command is run by `scripts/docs/check-cli-guides.sh` against `loams dev` in CI.

Tests: CLI1 Task 11's tests, plus `init_product_picker_maps_to_engines` and `guides_commands_execute` (CI).

Commit `docs(cli): guides, design amendments and status`.

---

## Exit criteria for production

- [ ] **Contract:** output formats, the error object with `reason`, exit codes, `jsonl` (as ruled), schema snapshots, stdout purity: Tasks 1, 3, 4.
- [ ] **Local stacks** run every product on Linux x86_64 and aarch64 and on macOS aarch64. House is excluded on Windows, and `cli` on Windows is remote-only. Ready means each product's service is served: Tasks 6–9.
- [ ] **Product groups:** every public RPC of `loams.live.v1`, `loams.postgres.v1`, `loams.sqldb.v1`, `loams.graph.v1` and `loams.house.v1` that a person or script needs is reachable from a command, and the mapping tables are complete: Tasks 12–16.
- [ ] **Safety:** confirmation on every destructive command, idempotent retries, no product mutation through MCP: Tasks 3, 12–16, 20.
- [ ] **Secrets:** the canary is clean across products, modes and MCP. No secret in argv. Client files are 0600 and removed: Tasks 9, 11, 23.
- [ ] **Identity:** login with refresh, keys, vended agent tokens, HTTPS-only credentials: Tasks 17–19.
- [ ] **Release:** the variant matrix with product features, the House components, the signed manifest, the `cli` variant free of server crates: Task 24 (with CLI2).
- [ ] **Operations:** `doctor`, the skew policy, the unstable notices: Task 25.
- [ ] **Gates:** the e2e matrix is green for every merged product, the budgets are met, `no-metering` covers the CLI: Task 26.
- [ ] **Docs:** the guides run in CI, the snippets are tested, §30 is amended, the plans index is updated: Tasks 22, 27.

## Self-review

- **Spec coverage.**

  | Source | Task(s) |
  |---|---|
  | §30 §4 (crate), §5 (tree), §6 (output) | 1, 4 |
  | §30 §7 (home), §15 (keys, login) | 2, 17, 18, 19 |
  | §30 §8 (stacks), §9.3 (variant pick) | 6, 7, 24 |
  | §30 §10 (NVMe) | 5, 10 |
  | §30 §11 (secrets) | 9, 11, 23 |
  | §30 §12 (MCP) | 20, 21 |
  | §30 §13 (docs) | 22 |
  | §30 §16 (companions) | 8 (and R4 for `postgres`) |
  | §30 §17 (distribution) | CLI2, amended by 24 |
  | §44 (API rules, catalogue, streams) | 3, 4, 25 |
  | §45 Live | 6, 9, 12 |
  | §46 Postgres (D719) | 6, 9, 13 |
  | §47 SQL on TiDB | 6, 8, 9, 14 |
  | §48 Graph | 6, 15 |
  | §49 House | 6, 7, 16, 24 |
  | §50 desktop daemon | 3 (R9), 7 |

- **Types.** `ApiClient`, `Endpoint`, `MutationCtx`, `await_operation`, `StreamRender`, `ClientLaunch`, `Secret`, the error mapping and the registry v2 are defined once, in Shared contracts.
- **Review Focus.** Items 1–8 each name owning tests (Tasks 1, 3, 4, 6, 7, 9, 11–16, 20, 23, 26).
- **Decisions the owner must make before the tasks that need them:**
  - Q-CL1-1 and Q-CL1-2 (names), before Task 6;
  - Q-CL1-3 (`jsonl`), before Task 1;
  - Q-CL1-5 (`postgres` engine), before Task 6;
  - Q-CL1-4 (`cluster enrol`), before MT4's CLI work;
  - Q675 (graph in `standard`), before Task 24.

## Open questions

| # | Question | Proposal | Owner | Needed by |
|---|---|---|---|---|
| Q-CL1-1 | §30's engine `pg` (the analytics wire) and the product group `pg` (Loams Postgres) share a name. Rename the wire engines to `pg-wire` and `mysql-wire`, and their variables to `LOAMS_PG_WIRE_URL` and `LOAMS_MYSQL_WIRE_URL`? | Yes (R2). Nothing is published, and PG2 Task 57 already took `loams pg` | Founder | Task 6 |
| Q-CL1-2 | The Loams SQL group name: `sql`, `mysql` or `tidb`? `sql` also reads as Flight SQL or the MCP `sql` data tool. And should `sql` ever set `DATABASE_URL`? | `loams sql`, after the product name. `DATABASE_URL` only on request (`env export --database-url sql`) | Founder | Task 6 |
| Q-CL1-3 | Amend D283 with a fourth format, `jsonl`, for streams, and buffer streams in `json` mode up to `--max-rows`? Is it additive (`output_schema` stays 1)? | Yes, additive (R3) | Eng | Task 1 |
| Q-CL1-4 | §41's `loams cluster enrol` collides with the server command `loams cluster`. Rename it to `loams byoc enrol`? | `loams byoc enrol\|list\|revoke` (R1) | Founder | MT4's CLI work |
| Q-CL1-5 | D299's `postgres` companion (a `postgres:17` container) versus Loams Postgres single-node (D719, pageserver plus `loams-wal` plus PgDog, about 2–3 GB of RAM): make the engine Loams Postgres only, or keep `postgres:17` as `--postgres-mode plain` for small machines? Which variant carries the `postgres` feature? | Loams Postgres only, in `full` (R4). No plain fallback, so the desktop and CLI behave the same | Founder | Task 6 |
| Q-CL1-6 | Default ports for the new engines: sql gate 13307, PgDog 15433, House 18123, 19000 and 18125, TiKV PD 12379. The SQ1 desktop uses 3306 and 19379 | Keep these for CLI stacks. The desktop keeps its own | Eng | Task 6 |
| Q-CL1-7 | At most one `postgres` stack and one `sql` stack per `LOAMS_HOME` in CL1, because of PG2's fixed compute port range and SQ1's single local runtime? | Yes. Lift it when PG2's compose runtime takes a per-stack range | Eng | Task 6 |
| Q-CL1-8 | Extract the Rust SDK's transport, retry and idempotency layer into a shared crate (`loams-client-core`) used by the CLI and `sdks/rust`, or keep the CLI's own thin layer checked by the SDK fixtures? | Own layer now (R5). Revisit at SDK2's 1.0 | Eng | Task 3 |
| Q-CL1-9 | The Live CLI split: the Rust `loams live` has no bundler, `dev` watch loop or `codegen` (they stay in `@loams/live-cli`). Is that the intended boundary? | Yes (§45 §13, R6) | Founder | Task 12 |
| Q-CL1-10 | House binaries for `loams house install`: the same signed release manifest as the `loams` binary (a `components` list), or the desktop's own manifest? Who holds the key (Q282)? | One manifest, `components`, the same minisign key | Founder | Task 16, Task 24 |
| Q-CL1-11 | Q675: is `graph` in `standard` at GA? It decides whether `loams graph` works against the default install's local stack | Follow Q675's default (`standard` if the size gate passes) | Founder | Task 24 |
| Q-CL1-12 | Should the stdio MCP server ever offer product mutations (for example, `pg_branch_create` for an agent's preview branch), given D289? | Not in CL1 (R8). Revisit with MCP `input_required` support | Founder | Task 20 |
| Q-CL1-13 | May the CLI drive the desktop's engine (`--endpoint desktop`) through DD1's adoption record, read-only and without the daemon's token? | Yes, read-only discovery. Stacks never share the desktop's data directory (R9) | Eng | Task 3 |

## Rulings made during execution

(Task 0 and later tasks append here.)
