# Route map: every HTTP route and its Connect RPC

Status: **API1 Task 0** (2026-10-02), reconciled with `dev` at the start of the API1 plan ([plan](../plans/2026-10-02-api1-unified-connect.md), design [§44](../design/44-unified-api-and-sdks.md) §5).

This page lists every route the `loams` binary serves over plain HTTP, and every path in the console contract `api/console/openapi.json`, with the RPC that replaces it, the internal call the handler makes (the call the RPC handler makes too: logic moves, it is not rewritten), and the tests that pin its behaviour today. The test `route_map_covers_every_route` (`crates/loams/tests/route_map.rs`) reads the routers' source and the contract and fails if a route is missing from this page, if this page lists a route that no longer exists, or if the number of `/v1` rows here is not the number of `/v1` method+path pairs the routers serve.

Format rules the test relies on: each route is one table row whose first cell is one HTTP method and whose second cell is the path in backticks, written as axum writes it (`{param}`). Rows whose first cell is not a method are ignored.

Kinds: **app** is application API, replaced by the RPC and deleted in Task 9; **protocol** is a protocol endpoint that stays HTTP (D602); **internal** is node-to-node, moved to `loams.internal.v1` on the cluster listener (Task 8); **raft** is the metastore's Raft transport (see ruling 0.3).

## Toolchain (recorded for Task 1)

| Component | Version | Where |
|---|---|---|
| `connectrpc` (connect-rust) | 0.9.1 | workspace `Cargo.toml`, `Cargo.lock` |
| `connectrpc-build` / `connectrpc-codegen` | 0.9.0 / 0.9.0 | build dependency of the proto crates |
| `buffa` / `buffa-types` / `buffa-codegen` | 0.9.2 / 0.9.2 / 0.9.2 | messages and well-known types (JSON feature) |
| `connectrpc-health`, `connectrpc-reflection` | 0.9.0 (crates.io) | `grpc.health.v1`, `grpc.reflection.v1`, added in Task 1 |
| `axum` | 0.8.9 | the one HTTP router; connect-rust mounts as an axum service |
| `tonic` | 0.14 | the tonic-era `loams-stream-grpc` (moves to connect-rust in Task 6, D128) and the Qdrant and Flight gRPC compat servers (stay tonic: compat) |

Existing Connect precedent: `loams-apps-mock` (AP0) and `loams-live-proto` (R1) generate buffa messages and connect-rust service traits from `proto/` in `build.rs` with the system `protoc`; the mock serves them with `Router::into_axum_service()` as an axum fallback.

## Native REST (`crates/loams/src/api/mod.rs`, `collections.rs`, `query.rs`, `sql.rs`, `events.rs`, `hot.rs`)

Served by `api::router` on `--listen` (gateway role). Tests are in `crates/loams/tests/it/` unless noted.

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| GET | `/health` | protocol | kept; also `grpc.health.v1.Health/Check` | none (200) | `it/http.rs` (startup helpers poll it) |
| GET | `/ready` | protocol | kept; also `grpc.health.v1.Health/Check` | `MetaStore::is_ready` | `it/http.rs`, `cluster.rs` |
| POST | `/durable/` | protocol | kept (B12): the Resonate protocol envelope, the loopback durable listener's `POST /` behind the port's authentication | `loams_durable::DurableGateway::handle` | `api::durable` unit tests, `durable.rs::durable_api_is_served_on_the_main_port_too` |
| POST | `/v1/namespaces` | app | `loams.collection.v1.NamespaceService/CreateNamespace` | `MetaStore::create_namespace` | `http.rs::namespaces_and_streams_are_created_once` |
| POST | `/v1/namespaces/{ns}/streams` | app | `loams.stream.v1.StreamService/CreateStream` | `MetaStore::create_stream` | `http.rs::namespaces_and_streams_are_created_once` |
| GET | `/v1/namespaces/{ns}/streams` | app | `loams.stream.v1.StreamService/ListStreams` (served since SDKP P0c, paged) | `MetaStore::streams` | `http.rs::list_streams_sorted`, `unknown_namespace_404`; `connect_streams.rs::describe_and_list_streams_paginate` |
| GET | `/v1/namespaces/{ns}/streams/{stream}` | app | `loams.stream.v1.StreamService/DescribeStream` | `MetaStore::stream` | `http.rs::namespaces_and_streams_are_created_once` |
| POST | `/v1/namespaces/{ns}/streams/{stream}/partitions/{partition}/records` | app | `loams.stream.v1.StreamService/Produce` | `api::produce_records` (`LogWriter::append`) | `http.rs::produced_records_are_fetched_back`, `acknowledged_records_survive_a_restart`, `a_record_for_the_wrong_partition_of_an_implicit_stream_is_rejected` |
| GET | `/v1/namespaces/{ns}/streams/{stream}/partitions/{partition}/records` | app | `loams.stream.v1.StreamService/Fetch` | `LogReader::fetch` (long-poll on the offset watch) | `http.rs::produced_records_are_fetched_back`, `a_long_poll_fetch_wakes_on_produce`, `out_of_range_offsets_get_416_with_both_bounds` |
| POST | `/v1/namespaces/{ns}/streams/{stream}/events` | app | `loams.stream.v1.StreamService/ProduceCloudEvents` | `events::ingest` (dedup ledger, `LogWriter::append`) | `it/events.rs` (10 tests), `stream_grpc_events.rs::cloudevents_over_grpc_share_the_ledger_with_http` |
| GET | `/v1/namespaces/{ns}/streams/{stream}/partitions/{partition}/events` | app | `loams.stream.v1.StreamService/FetchCloudEvents` | `LogReader::fetch` and the envelope synthesis | `events.rs::a_record_that_is_not_an_event_is_read_as_a_synthesized_envelope`, `binary_mode_maps_headers_and_serves_binary_mode_back` |
| POST | `/v1/namespaces/{ns}/links` | app | `loams.link.v1.LinkService/CreateLink` | `MetaStore::create_link` | `http.rs::a_link_is_created_once_and_sums_its_stream` |
| GET | `/v1/namespaces/{ns}/links` | app | `loams.link.v1.LinkService/ListLinks` (not yet in the proto; native REST until the links Connect service exists, AP1e Task 19, D672) | `MetaStore::links`, `MetaStore::streams`, `LinkApplySource::unregistered`, `TargetRegistry` | `http.rs::list_links_with_status`, `unknown_namespace_404` |
| GET | `/v1/namespaces/{ns}/links/{link}` | app | `loams.link.v1.LinkService/DescribeLink` | `MetaStore::link`, `MetaStore::stream_state` (lag), `CounterTable::load`, `TargetRegistry`, `LinkApplySource::unregistered` (status) | `http.rs::a_link_is_created_once_and_sums_its_stream`, `the_link_endpoint_shows_version_and_applied_for_collections`, `link_describe_has_lag` |
| POST | `/v1/namespaces/{ns}/collections` | app | `loams.collection.v1.CollectionService/CreateCollection` | `CollectionService::create_collection` | `native_collections.rs::collection_routes_speak_the_documented_json` |
| GET | `/v1/namespaces/{ns}/collections` | app | `loams.collection.v1.CollectionService/ListCollections` | `CollectionService::list_collections` | `native_collections.rs::collection_routes_speak_the_documented_json` |
| GET | `/v1/namespaces/{ns}/collections/{c}` | app | `loams.collection.v1.CollectionService/GetCollection` | `CollectionService::get_collection`, `hot::hot_status_value` | `native_collections.rs::collection_routes_speak_the_documented_json`, `hot_http.rs::get_collection_reports_hot_status_per_structure` |
| DELETE | `/v1/namespaces/{ns}/collections/{c}` | app | `loams.collection.v1.CollectionService/DropCollection` | `CollectionService::drop_collection` | `native_collections.rs::collection_routes_speak_the_documented_json` |
| POST | `/v1/namespaces/{ns}/collections/{c}/fields` | app | `loams.collection.v1.CollectionService/AddFields` | `CollectionService::add_fields` | `native_collections.rs::collection_routes_speak_the_documented_json` |
| GET | `/v1/namespaces/{ns}/collections/{c}/versions` | app | `loams.collection.v1.CollectionService/ListVersions` | `CollectionService::versions` | `native_collections.rs::collection_routes_speak_the_documented_json` |
| POST | `/v1/namespaces/{ns}/collections/{c}/scan` | app | `loams.collection.v1.CollectionService/Scan` | `CollectionService::scan_plan` | `native_scan.rs::scan_route_speaks_the_documented_json` |
| POST | `/v1/namespaces/{ns}/aliases` | app | `loams.collection.v1.CollectionService/UpdateAliases` | `CollectionService::update_aliases` | `native_collections.rs::collection_routes_speak_the_documented_json`, `hot_http.rs::hot_routes_accept_aliases` |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents` | app | `loams.collection.v1.DocumentService/WriteDocuments` | `CollectionService::write` | `native_collections.rs::write_get_scroll_count_round_trip_over_http`, `a_rejected_atomic_write_is_400_with_the_op_index`, `write_responses_carry_the_consistency_token_header`, `backpressure.rs` (4 tests) |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents/get` | app | `loams.collection.v1.DocumentService/GetDocuments` | `CollectionService::get_with_token` | `native_collections.rs::write_get_scroll_count_round_trip_over_http` |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents/scroll` | app | `loams.collection.v1.DocumentService/ScrollDocuments` | `CollectionService::scroll_with_token` | `native_collections.rs::write_get_scroll_count_round_trip_over_http` |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents/count` | app | `loams.collection.v1.DocumentService/CountDocuments` | `CollectionService::count_with_token` | `native_collections.rs::write_get_scroll_count_round_trip_over_http` |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents/delete_by_filter` | app | `loams.collection.v1.DocumentService/DeleteByFilter` | `CollectionService::delete_by_filter` | `filter_write_http.rs::delete_by_filter_route_speaks_the_documented_json`, `over_the_limit_is_400_with_matched_and_limit`, `the_token_header_covers_the_filter_write` |
| POST | `/v1/namespaces/{ns}/collections/{c}/documents/patch_by_filter` | app | `loams.collection.v1.DocumentService/PatchByFilter` | `CollectionService::patch_by_filter` | `filter_write_http.rs::patch_by_filter_route_applies_the_patch` |
| PUT | `/v1/namespaces/{ns}/collections/{c}/hot` | app | `loams.collection.v1.CollectionService/SetHot` | `MetaStore::set_collection_hot`, `hot::local_status` | `hot_http.rs::put_hot_sets_the_catalog_and_returns_status`, `put_hot_on_a_missing_collection_is_404`, `a_malformed_hot_body_is_400`, `hot_routes_accept_aliases` |
| POST | `/v1/namespaces/{ns}/collections/{c}/warm` | app | `loams.collection.v1.CollectionService/WarmCollection` | `hot::warm_local` or the owner's `/internal/v1/hot/warm` | `hot_http.rs::warm_prefetches_an_unpinned_collection`, `loams_warm_posts_the_warm_request` |
| POST | `/v1/namespaces/{ns}/query` | app | `loams.collection.v1.QueryService/Search` | `CollectionService::search` | `native_query.rs` (6 tests), `hot_http.rs::the_hot_off_header_gives_identical_results_and_reports_none`, `connect_query.rs` (10 tests) |
| POST | `/v1/namespaces/{ns}/sql` | app | `loams.sql.v1.SqlService/Query` | `CollectionService::sql_context_with`, `loams_query::sql::run_read_only` | `native_sql.rs::sql_over_http_returns_columns_and_rows`, `sql_ddl_is_400`; `connect_sql.rs::sql_over_http_returns_columns_and_rows_rpc`, `sql_ddl_is_400_rpc` |

Router-level behaviour pinned by `http.rs::framework_rejections_use_the_json_error_body` (the `no_route` fallback and `method_not_allowed`) is replaced by connect-rust's error envelope for RPC paths; the fallback stays for unknown paths.

As built in Task 4 (2026-10-07): none of these rows is gone. Every route above still answers as it did, **and** the same port now serves the Connect protocol, gRPC and gRPC-Web beside them, plus `grpc.health.v1` and (in `loams dev`) `grpc.reflection.v1`. The RPC paths are registered as their own axum routes rather than as the router's fallback, precisely so the `no_route` fallback keeps its JSON body for a path neither API knows (ruling 1.1 of the [plan](../plans/2026-10-02-api1-unified-connect.md)).

Three of the rows above now have their RPC: `loams.collection.v1.CollectionService` and `NamespaceService` (Task 2), `DocumentService` (Task 3) and `QueryService/Search` (Task 4). `InstanceService/GetInstance` answers, and its sibling `WhoAmI` answers `unimplemented` with reason `not_implemented` because there is no authentication on this port yet. The one catalogue package whose engine is not in this build, `loams.live.v1`, answers `unimplemented` with reason `feature_not_in_variant`. Tasks 5–8 add one RPC per remaining row above and Task 9 deletes the rows.

`/loams.house.v1.*` is not a route of this server: a middleware outside the router forwards every such path to the Loams House front named by `--house-endpoint` (HS1 Task 7, `crates/loams/src/api/house_proxy.rs`), or answers `house_not_configured` without one, so it has no row here.

**The Connect router carries its own `HotLayer`**, so an RPC answer names the hot structures it used in `loams-hot-used` and an unusable `Loams-Hot` is refused with a Connect error and its registry reason rather than the REST JSON body. It is layered on the connect router alone — not merged into the one the REST routes live in — because the layer picks a refusal's shape from the content type and `application/json` is both the Connect protocol's and the native REST one, so inside the connect router there is no ambiguity and outside it there is. `Loams-Hot` is the only request header on this surface: it says "may this read use a hot structure", which has no message-level meaning, and a call's consistency and pinning still travel in its message (Ruling 11, design §44 §7.4). See `crates/loams/src/api/connect_hot.rs`.

## Internal node routes (`crates/loams/src/api/internal.rs`, `loams_hot::READS_PATH`)

Merged outside `HotLayer` on every node (`api::router` and `api::internal_router`). Unauthenticated today (M1 overview §6.9).

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| POST | `/internal/v1/reads/{op}` | internal | `loams.internal.v1.ForwardService/Read` | `loams_hot::serve_forwarded` | `cluster.rs` (forwarded reads), `loams-hot` `remote` tests |
| GET | `/internal/v1/node/stats` | internal | `loams.internal.v1.NodeService/GetNodeStats` | `ForwardStats::snapshot`, `HotTierImpl::counters`, `NodeInfo::meta` | `cluster.rs` |
| POST | `/internal/v1/hot/status` | internal | `loams.internal.v1.HotService/GetHotStatus` | `hot::local_status` | `cluster.rs`, `hot_http.rs` (owner path) |
| POST | `/internal/v1/hot/warm` | internal | `loams.internal.v1.HotService/Warm` | `hot::warm_local` | `cluster.rs`, `hot_http.rs` (owner path) |
| POST | `/internal/v1/graph/{method}` | internal | `loams.internal.v1.GraphForwardService/Forward` (GR1 Task 12; moves with API1 Task 8) | `GraphAdmin::serve_forwarded` (feature `graph`) | `loams-graph` `ownership` tests |
| POST | `/internal/v1/git/commit` | internal | `loams.internal.v1.GitForwardService/Commit` (GT1 Task 14; moves with API1 Task 8) | `api::git_internal::commit` (a postcard `ForwardedTxn` to the sequencer owner) | `git/repos.rs::forwarded_commit_route_commits_here` |
| POST | `/internal/v1/meter/partitions/{partition}/records` | internal | none: the meter's service ingest (D2206, ME1; the node agent's forwarder), feature `meter` | `api::meter::ingest` (each event to its subject's `_system/_meter` partition) | `api::meter` unit tests, the meter-agent e2e ingest test |
| POST | `/internal/v1/meter/counted` | internal | none: a counter read forwarded to the partition's owner (D2208), feature `meter` | `api::meter::counted` | `api::meter` unit tests |

## Cluster bootstrap (`crates/loams/src/server.rs`)

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| GET | `/health` | protocol | kept | none (200, before the late router is ready) | `cluster.rs` |

## Metastore Raft transport (`crates/loams-meta/src/rpc.rs`)

Served on the cluster listener before the API router (`meta_rpc::router`). See ruling 0.3.

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| POST | `/internal/v1/raft/append` | raft | `loams.internal.v1.RaftService/AppendEntries` | `MetaNode` openraft `append_entries` | `loams-meta` network tests, `cluster.rs` |
| POST | `/internal/v1/raft/vote` | raft | `loams.internal.v1.RaftService/Vote` | openraft `vote` | `loams-meta` network tests |
| POST | `/internal/v1/raft/pre-vote` | raft | `loams.internal.v1.RaftService/PreVote` | openraft pre-vote | `loams-meta` network tests |
| POST | `/internal/v1/raft/snapshot` | raft | `loams.internal.v1.RaftService/InstallSnapshot` | openraft `install_full_snapshot` | `loams-meta` network tests |
| POST | `/internal/v1/meta/write` | raft | `loams.internal.v1.MetaService/Write` | `MetaNode::client_write` | `loams-meta` network tests, `cluster.rs` |
| POST | `/internal/v1/meta/read-index` | raft | `loams.internal.v1.MetaService/ReadIndex` | `MetaNode::read_index` | `loams-meta` network tests |
| POST | `/internal/v1/meta/join` | raft | `loams.internal.v1.MetaService/Join` | `MetaNode::join` | `cluster.rs` |
| POST | `/internal/v1/meta/leave` | raft | `loams.internal.v1.MetaService/Leave` | `MetaNode::leave` | `cluster.rs` |
| POST | `/internal/v1/meta/status` | raft | `loams.internal.v1.MetaService/Status` | `MetaNode` metrics | `cluster.rs` |

## Loams Git listener (`crates/loams/src/api/git_auth.rs`, `--git-listen`)

Served on `--git-listen` (feature `git`, GT1 Task 16; loopback until MT1, D111): authentication by git token, the namespace from the token, and one bodiless `404` for anything outside it. An admitted request for an existing repository answers `501` until Task 46 adds the protocol routes listed under Git Smart HTTP below.

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| GET | `/git/{ns}/{repo}` | protocol | none (stays HTTP, D602) | `git_auth::serve` | `git.rs` `http::` |
| POST | `/git/{ns}/{repo}` | protocol | none (stays HTTP, D602) | `git_auth::serve` | `git.rs` `http::` |
| GET | `/git/{ns}/{repo}/{*rest}` | protocol | none (stays HTTP, D602) | `git_auth::serve` | `git.rs` `http::` |
| POST | `/git/{ns}/{repo}/{*rest}` | protocol | none (stays HTTP, D602) | `git_auth::serve` | `git.rs` `http::` |

## Console contract (`api/console/openapi.json`, served by `loams-apps-mock` from `loams-console-mock`'s routes)

The real server does not serve `/api/v1` yet (the gateway work is AP4/MT1); only `loams-apps-mock` does, from the contract. `loams-console-mock` keeps the contract and the seed and its `routes()` list, and `loams-apps-mock::console` mounts that list over axum on the same listener as the app protos, so the console, the desktop, the phone apps and the SDKs all point at one address. Tests: `crates/loams-console-mock/tests/contract.rs` (`every_operation_has_a_mock_and_every_mock_an_operation`, `every_mocked_body_matches_its_response_schema`, `the_server_answers_from_the_seed`) and the console's `web/apps/console` tests over `openapi-fetch`. Services are in `loams.admin.v1` unless noted.

**On Connect (API2 Tasks 6 and 7, mock half).** Every `app` row below is also served over Connect by `loams-apps-mock` (`src/admin/`), from the same resolved seed as the REST route, as the RPC in its Target RPC column; `loams-console-mock`'s `CONNECT_RPCS` is the same table as code, and `tests/contract.rs` (`connect_rpcs_cover_the_contract_and_agree_with_the_route_map`) holds it to this page and to the contract. Two RPCs answer no REST route: `AuthService/ListProviders` (the `sign_in` part of `GET /api/v1/instance`) and `AuditService/WatchAuditEvents`. The tests are `crates/loams-apps-mock/tests/console_rpc.rs`: each console route test ported with `_rpc`, `mock_rest_and_connect_answer_alike`, `create_key_returns_secret_once` and `watch_audit_resumes_from_cursor`. The server does not serve `loams.auth.v1` or `loams.admin.v1` yet: `crates/loams/src/api/connect_admin.rs` is the server half of Task 7. The REST routes stay until API2 Task 11.

| Method | Route | Kind | Target RPC | Internal call | Existing tests |
|---|---|---|---|---|---|
| GET | `/api/v1/instance` | app | `loams.instance.v1.InstanceService/GetInstance` (exists, AP0) | console mock seed | console mock contract tests |
| POST | `/api/v1/setup` | app | `loams.auth.v1.AuthService/CompleteSetup` | console mock seed | console mock contract tests |
| GET | `/api/v1/session` | app | `loams.auth.v1.AuthService/GetSession` | console mock seed | console mock contract tests |
| POST | `/api/v1/session` | app | `loams.auth.v1.AuthService/CreateSession` | console mock seed | console mock contract tests |
| DELETE | `/api/v1/session` | app | `loams.auth.v1.AuthService/DeleteSession` | console mock seed | console mock contract tests |
| GET | `/api/v1/auth/oidc/{provider}/start` | protocol | kept (browser redirect, D602); `loams.auth.v1.AuthService/ListProviders` lists providers | console mock seed | console mock contract tests |
| GET | `/api/v1/org` | app | `OrgService/GetOrg` | console mock seed | console mock contract tests |
| PATCH | `/api/v1/org` | app | `OrgService/UpdateOrg` | console mock seed | console mock contract tests |
| GET | `/api/v1/org/members` | app | `OrgService/ListMembers` | console mock seed | console mock contract tests |
| DELETE | `/api/v1/org/members/{user}` | app | `OrgService/RemoveMember` | console mock seed | console mock contract tests |
| GET | `/api/v1/org/invitations` | app | `OrgService/ListInvitations` | console mock seed | console mock contract tests |
| POST | `/api/v1/org/invitations` | app | `OrgService/CreateInvitation` | console mock seed | console mock contract tests |
| GET | `/api/v1/teams` | app | `OrgService/ListTeams` | console mock seed | console mock contract tests |
| POST | `/api/v1/teams` | app | `OrgService/CreateTeam` | console mock seed | console mock contract tests |
| GET | `/api/v1/teams/{team}` | app | `OrgService/GetTeam` | console mock seed | console mock contract tests |
| POST | `/api/v1/teams/{team}/members` | app | `OrgService/AddTeamMember` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects` | app | `ProjectService/ListProjects` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects` | app | `ProjectService/CreateProject` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}` | app | `ProjectService/GetProject` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/access` | app | `ProjectService/ListAccess` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects/{project}/access` | app | `ProjectService/GrantAccess` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/environments` | app | `ProjectService/ListEnvironments` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects/{project}/environments` | app | `ProjectService/CreateEnvironment` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/environments/{environment}` | app | `ProjectService/GetEnvironment` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/environments/{environment}/usage` | app | `ProjectService/GetEnvironmentActivity` (§44's `GetEnvironmentUsage`, renamed: API2 ruling 6/7.4) | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/agents` | app | `AgentService/ListAgents` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects/{project}/agents` | app | `AgentService/CreateAgent` | console mock seed | console mock contract tests |
| GET | `/api/v1/agents/{agent}` | app | `AgentService/GetAgent` | console mock seed | console mock contract tests |
| PATCH | `/api/v1/agents/{agent}` | app | `AgentService/UpdateAgent` | console mock seed | console mock contract tests |
| POST | `/api/v1/agents/{agent}/suspend` | app | `AgentService/SuspendAgent` | console mock seed | console mock contract tests |
| POST | `/api/v1/agents/{agent}/resume` | app | `AgentService/ResumeAgent` | console mock seed | console mock contract tests |
| GET | `/api/v1/agents/{agent}/tokens` | app | `AgentService/ListAgentTokens` | console mock seed | console mock contract tests |
| GET | `/api/v1/agents/{agent}/trust-policies` | app | `AgentService/ListTrustPolicies` | console mock seed | console mock contract tests |
| POST | `/api/v1/agents/{agent}/trust-policies` | app | `AgentService/CreateTrustPolicy` | console mock seed | console mock contract tests |
| DELETE | `/api/v1/tokens/{jti}` | app | `KeyService/RevokeToken` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/service-accounts` | app | `AgentService/ListServiceAccounts` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects/{project}/service-accounts` | app | `AgentService/CreateServiceAccount` | console mock seed | console mock contract tests |
| GET | `/api/v1/projects/{project}/environments/{environment}/keys` | app | `KeyService/ListKeys` | console mock seed | console mock contract tests |
| POST | `/api/v1/projects/{project}/environments/{environment}/keys` | app | `KeyService/CreateKey` | console mock seed | console mock contract tests |
| DELETE | `/api/v1/keys/{key}` | app | `KeyService/RevokeKey` | console mock seed | console mock contract tests |
| GET | `/api/v1/audit` | app | `AuditService/ListAuditEvents` (and the `WatchAuditEvents` stream) | console mock seed | console mock contract tests |
| POST | `/api/v1/oauth/token` | protocol | kept (RFC 6749/8693, D602) | console mock seed | console mock contract tests |
| GET | `/api/v1/oauth/authorize` | protocol | kept (RFC 6749/7636, D602) | console mock seed | console mock contract tests |
| POST | `/api/v1/oauth/consent` | app | `loams.auth.v1.AuthService/DecideConsent` | console mock seed | console mock contract tests |
| GET | `/.well-known/oauth-protected-resource` | protocol | kept (RFC 9728, D602) | console mock seed | console mock contract tests |
| GET | `/.well-known/oauth-authorization-server` | protocol | kept (RFC 8414, D602) | console mock seed | console mock contract tests |
| GET | `/.well-known/jwks.json` | protocol | kept (RFC 7517, D602) | console mock seed | console mock contract tests |

## Planned: `loams.postgres.v1` console REST (PG2 Task 1; the RPCs are served from PG2 Task 9)

Design §46 §4.2's console REST for Loams Postgres. Since PG2 Task 9 the `pg-control` role (`loams pg-control`, feature `postgres`) serves `loams.postgres.v1.PostgresService` over Connect, gRPC and gRPC-Web on its own API listener, but **no REST route**: API2 (D602) removes application REST in favour of the RPCs, so these routes stay planned (PG2 ruling R9.4). The rows put the route first, which `route_map_covers_every_route` skips, and `every_rpc_has_a_planned_console_route` (`crates/loams-proto/tests/postgres.rs`) checks that every row names an RPC of the service and every RPC has a row.

§46 §4.2 writes the pattern `/v1/namespaces/{ns}/postgres/projects[/{project}[/branches[/{branch}]|/endpoints[/{endpoint}][:start|:suspend|:restart]|/roles|/databases]]`. Rows marked "beyond §46 §4.2's pattern" are the RPCs it does not route, given AIP-136 custom methods (`:verb`) or a sub-resource in the same style (PG2 Task 1 ruling R1.2). Roles and databases are per branch (§46 §3) but sit under the project, as §4.2 writes them, with the branch in the body or the query, defaulting to the project's default branch. `Idempotency-Key` carries `idempotency_key` (§44 §5.2). `{project}`, `{branch}` and `{endpoint}` are always the ids (`prj-`, `br-`, `ep-` + ULID), never names: ids never change and names can (PG2 Task 1 ruling R1.7). `{role}` and `{database}` are names, the only identity those have. A `PATCH` takes the fields it changes; an empty change is `invalid_argument`.

| Route | Method | Target RPC | Notes |
|---|---|---|---|
| `/v1/namespaces/{ns}/postgres/projects` | POST | `loams.postgres.v1.PostgresService/CreateProject` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects` | GET | `loams.postgres.v1.PostgresService/ListProjects` | `page_size`, `page_token` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}` | GET | `loams.postgres.v1.PostgresService/GetProject` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}` | PATCH | `loams.postgres.v1.PostgresService/UpdateProject` | `update_mask` from the body's fields |
| `/v1/namespaces/{ns}/postgres/projects/{project}` | DELETE | `loams.postgres.v1.PostgresService/DeleteProject` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}:upgrade` | POST | `loams.postgres.v1.PostgresService/UpgradeProject` | beyond §46 §4.2's pattern (PG2 Task 51) |
| `/v1/namespaces/{ns}/postgres/projects/{project}:watch` | GET | `loams.postgres.v1.PostgresService/WatchProject` | server stream; beyond §46 §4.2's pattern |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches` | POST | `loams.postgres.v1.PostgresService/CreateBranch` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches` | GET | `loams.postgres.v1.PostgresService/ListBranches` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches/{branch}` | GET | `loams.postgres.v1.PostgresService/GetBranch` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches/{branch}` | PATCH | `loams.postgres.v1.PostgresService/UpdateBranch` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches/{branch}` | DELETE | `loams.postgres.v1.PostgresService/DeleteBranch` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches/{branch}:restore` | POST | `loams.postgres.v1.PostgresService/RestoreBranch` | returns an `Operation`; beyond §46 §4.2's pattern |
| `/v1/namespaces/{ns}/postgres/projects/{project}/branches/{branch}:set_default` | POST | `loams.postgres.v1.PostgresService/SetDefaultBranch` | beyond §46 §4.2's pattern |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints` | POST | `loams.postgres.v1.PostgresService/CreateEndpoint` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints` | GET | `loams.postgres.v1.PostgresService/ListEndpoints` | `branch` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}` | GET | `loams.postgres.v1.PostgresService/GetEndpoint` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}` | PATCH | `loams.postgres.v1.PostgresService/UpdateEndpoint` |  |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}` | DELETE | `loams.postgres.v1.PostgresService/DeleteEndpoint` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}:start` | POST | `loams.postgres.v1.PostgresService/StartEndpoint` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}:suspend` | POST | `loams.postgres.v1.PostgresService/SuspendEndpoint` | returns an `Operation` |
| `/v1/namespaces/{ns}/postgres/projects/{project}/endpoints/{endpoint}:restart` | POST | `loams.postgres.v1.PostgresService/RestartEndpoint` | returns an `Operation`; `promote` in the body |
| `/v1/namespaces/{ns}/postgres/projects/{project}/roles` | POST | `loams.postgres.v1.PostgresService/CreateRole` | `branch` in the body (default: the default branch); answers the password once |
| `/v1/namespaces/{ns}/postgres/projects/{project}/roles` | GET | `loams.postgres.v1.PostgresService/ListRoles` | `branch` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}/roles/{role}:reset_password` | POST | `loams.postgres.v1.PostgresService/ResetRolePassword` | answers the password once; beyond §46 §4.2's pattern |
| `/v1/namespaces/{ns}/postgres/projects/{project}/roles/{role}` | DELETE | `loams.postgres.v1.PostgresService/DeleteRole` | `branch` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}/databases` | POST | `loams.postgres.v1.PostgresService/CreateDatabase` | `branch` in the body |
| `/v1/namespaces/{ns}/postgres/projects/{project}/databases` | GET | `loams.postgres.v1.PostgresService/ListDatabases` | `branch` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}/databases/{database}` | DELETE | `loams.postgres.v1.PostgresService/DeleteDatabase` | `branch` query |
| `/v1/namespaces/{ns}/postgres/projects/{project}/connection_info` | GET | `loams.postgres.v1.PostgresService/GetConnectionInfo` | `branch`, `endpoint`, `role`, `database`, `read_only` query; never a password; beyond §46 §4.2's pattern |
| `/v1/namespaces/{ns}/postgres/projects/{project}:issue_credential` | POST | `loams.postgres.v1.PostgresService/IssueConnectCredential` | answers the credential once; beyond §46 §4.2's pattern |

## Planned: `loams.repos.v1` and Loams Git's HTTP routes (GT1 Task 12)

Loams Git (design §36, §19.10). With the `loams` feature `git`, the main port serves `loams.repos.v1.RepoService` (and, from GT1 Task 18, `ObjectService`, and from Task 25, `WorkspaceService`, which have no REST rows yet: they are client data paths, and the console's workspace routes are planned with its Git pages) over Connect, gRPC and gRPC-Web, behind the engine's authentication (`LOAMS_AUTH_MODE`), from GT1 Task 13; without it every RPC answers `feature_not_in_variant`. As for `loams.postgres.v1`, **no REST route** is served (API2, D602): the console REST rows below stay planned. `every_rpc_has_a_planned_console_route` (`crates/loams-proto/tests/repos.rs`) checks that every row names an RPC of the service and every RPC has a row. `{repo}` is the repository's name, which the server resolves to its immutable `repo_id`; `Idempotency-Key` carries `idempotency_key` and `Loams-Consistency-Token` carries `consistency_token` (§44 §5.2). The rows put the route first, which `route_map_covers_every_route` skips.

| Route | Method | Target RPC | Notes |
|---|---|---|---|
| `/v1/namespaces/{ns}/repos` | POST | `loams.repos.v1.RepoService/CreateRepo` |  |
| `/v1/namespaces/{ns}/repos` | GET | `loams.repos.v1.RepoService/ListRepos` | `page_size`, `page_token` query |
| `/v1/namespaces/{ns}/repos/{repo}` | GET | `loams.repos.v1.RepoService/GetRepo` |  |
| `/v1/namespaces/{ns}/repos/{repo}` | DELETE | `loams.repos.v1.RepoService/DeleteRepo` | returns an `Operation` |
| `/v1/namespaces/{ns}/repos/{repo}:fork` | POST | `loams.repos.v1.RepoService/ForkRepo` | the new name in the body |
| `/v1/namespaces/{ns}/repos/{repo}/config` | PATCH | `loams.repos.v1.RepoService/UpdateRepoConfig` | `update_mask` from the body's fields |
| `/v1/namespaces/{ns}/repos/{repo}/refs` | GET | `loams.repos.v1.RepoService/ListRefs` | `prefix`, `page_size`, `page_token` query |
| `/v1/namespaces/{ns}/repos/{repo}:compact` | POST | `loams.repos.v1.RepoService/CompactRepo` | returns an `Operation` |
| `/v1/namespaces/{ns}/repos/{repo}:watch` | GET | `loams.repos.v1.RepoService/WatchRepo` | server stream |

Git Smart HTTP (§36 §6.1, D602, D603) is a **protocol** surface and stays HTTP. It is served on `--git-listen` (loopback until MT1) from GT1 Task 46, with `info/refs` and `git-upload-pack` from Task 46 to Task 49 and `git-receive-pack` from Task 50:

| Route | Method | Kind | Notes |
|---|---|---|---|
| `/git/{ns}/{repo}.git/info/refs` | GET | protocol | `?service=git-upload-pack` or `git-receive-pack` |
| `/git/{ns}/{repo}.git/git-upload-pack` | POST | protocol | v2 (`Git-Protocol: version=2`), and v0/v1 for libgit2 (Task 52, R0.10) |
| `/git/{ns}/{repo}.git/git-receive-pack` | POST | protocol | v0/v1 receive-pack |
| `/git/{ns}/{repo}.git/loams/objects/{oid}` | GET | object | one object in git's loose format, `Cache-Control: private, max-age=31536000, immutable`; `?ws=<ws_id>` reads a workspace's packs too (GT1 Task 18, §36 §19.5.1) |

Until Task 46 the listener serves only the authenticated catch-all routes listed under "Loams Git listener" above.

## Planned: `loams.memory.v1` (MM1 Task 1; the RPCs are served from MM1 Task 11)

Loams Memory (design §52 §9.1, D884). With the `loams` feature `memory` (MM1 Task 11), the main port serves `loams.memory.v1.MemoryAdminService` and `MemoryService` over Connect, gRPC and gRPC-Web, behind the engine's authentication; an RPC whose MM1 task has not landed answers `unimplemented` with `memory_not_yet_available`, and without the feature every RPC answers `feature_not_in_variant` (the package's `CATALOGUE` row and that stub land together in Task 11, MM1 ruling R1.8). It is a pure-RPC package: §52 names no REST surface and API2 (D602) removes application REST, so the rows below are the Connect paths, not console REST routes (MM1 ruling R1.5). They put the path first, which `route_map_covers_every_route` skips; `every_rpc_has_a_planned_route` (`crates/loams-proto/tests/memory.rs`) checks that every row names an RPC of the two services and every RPC has a row. The MVP subset (the plan's "Shared contracts", marked MVP) is served by the end of MM1a. `CodeMemoryService` joins this section with `code.proto` (MM1 Task 27).

| Path | Method | Target RPC | Notes |
|---|---|---|---|
| `/loams.memory.v1.MemoryAdminService/CreateStore` | POST | `loams.memory.v1.MemoryAdminService/CreateStore` | MVP: MM1 Task 3 |
| `/loams.memory.v1.MemoryAdminService/GetStore` | POST | `loams.memory.v1.MemoryAdminService/GetStore` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/ListStores` | POST | `loams.memory.v1.MemoryAdminService/ListStores` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/UpdateStore` | POST | `loams.memory.v1.MemoryAdminService/UpdateStore` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/DeleteStore` | POST | `loams.memory.v1.MemoryAdminService/DeleteStore` | returns an `Operation`; Task 23 (owner ruling, R1.7) |
| `/loams.memory.v1.MemoryAdminService/CreateSpace` | POST | `loams.memory.v1.MemoryAdminService/CreateSpace` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/GetSpace` | POST | `loams.memory.v1.MemoryAdminService/GetSpace` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/ListSpaces` | POST | `loams.memory.v1.MemoryAdminService/ListSpaces` | MVP: Task 3 |
| `/loams.memory.v1.MemoryAdminService/UpdateSpace` | POST | `loams.memory.v1.MemoryAdminService/UpdateSpace` | `name` and `ttl` with Task 3; an `isolation` move returns `memory.space.move`, Task 52 (owner ruling, R1.7) |
| `/loams.memory.v1.MemoryAdminService/ExportSpace` | POST | `loams.memory.v1.MemoryAdminService/ExportSpace` | returns an `Operation`; Task 25 |
| `/loams.memory.v1.MemoryAdminService/ImportSpace` | POST | `loams.memory.v1.MemoryAdminService/ImportSpace` | returns an `Operation`; Task 25 |
| `/loams.memory.v1.MemoryAdminService/ConfigureMirror` | POST | `loams.memory.v1.MemoryAdminService/ConfigureMirror` | Task 26 (deferred past GA, Q785/Q786) |
| `/loams.memory.v1.MemoryAdminService/GetErasure` | POST | `loams.memory.v1.MemoryAdminService/GetErasure` | MVP: Task 9 (logical forget); the purge's progress from Task 23 |
| `/loams.memory.v1.MemoryService/Add` | POST | `loams.memory.v1.MemoryService/Add` | MVP: Task 5; `Idempotency-Key` carries `idempotency_key` |
| `/loams.memory.v1.MemoryService/AddBatch` | POST | `loams.memory.v1.MemoryService/AddBatch` | MVP: Task 5 |
| `/loams.memory.v1.MemoryService/Search` | POST | `loams.memory.v1.MemoryService/Search` | MVP: Task 8 (hybrid); expand from Task 15 |
| `/loams.memory.v1.MemoryService/GetProfile` | POST | `loams.memory.v1.MemoryService/GetProfile` | MVP: Task 10 |
| `/loams.memory.v1.MemoryService/GetFact` | POST | `loams.memory.v1.MemoryService/GetFact` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/ListFacts` | POST | `loams.memory.v1.MemoryService/ListFacts` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/UpdateFact` | POST | `loams.memory.v1.MemoryService/UpdateFact` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/InvalidateFact` | POST | `loams.memory.v1.MemoryService/InvalidateFact` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/PinFact` | POST | `loams.memory.v1.MemoryService/PinFact` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/History` | POST | `loams.memory.v1.MemoryService/History` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/GetEntity` | POST | `loams.memory.v1.MemoryService/GetEntity` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/ListEntities` | POST | `loams.memory.v1.MemoryService/ListEntities` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/ListEpisodes` | POST | `loams.memory.v1.MemoryService/ListEpisodes` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/RetryExtraction` | POST | `loams.memory.v1.MemoryService/RetryExtraction` | MVP: Task 9 |
| `/loams.memory.v1.MemoryService/Forget` | POST | `loams.memory.v1.MemoryService/Forget` | MVP: Task 9 (logical); physical purge Task 23 |
| `/loams.memory.v1.MemoryService/GetBlock` | POST | `loams.memory.v1.MemoryService/GetBlock` | MVP: Task 10 |
| `/loams.memory.v1.MemoryService/ListBlocks` | POST | `loams.memory.v1.MemoryService/ListBlocks` | MVP: Task 10 |
| `/loams.memory.v1.MemoryService/UpdateBlock` | POST | `loams.memory.v1.MemoryService/UpdateBlock` | MVP: Task 10 |
| `/loams.memory.v1.MemoryService/Consolidate` | POST | `loams.memory.v1.MemoryService/Consolidate` | returns an `Operation`; Task 18 |
| `/loams.memory.v1.MemoryService/ListDreamRuns` | POST | `loams.memory.v1.MemoryService/ListDreamRuns` | Task 18 |
| `/loams.memory.v1.MemoryService/RevertDreamRun` | POST | `loams.memory.v1.MemoryService/RevertDreamRun` | returns an `Operation`; Task 18 |
| `/loams.memory.v1.MemoryService/WatchSpace` | POST | `loams.memory.v1.MemoryService/WatchSpace` | server stream; Task 24 |

## Out of scope: compatibility surfaces (D603)

Not mapped and not changed: the Qdrant REST and gRPC gateway (`loams-qdrant`), the Elasticsearch gateway (`loams-es`), Flight SQL (`server.rs`, `--flight-sql-listen`), Postgres wire (`pg`), MySQL wire (`mysql_wire`), the Resonate HTTP protocol (`loams-durable`), and the tonic `loams.stream.v1` listener for Dapr adapters (`loams-stream-grpc`, `--stream-grpc-listen`), which Task 6 moves onto connect-rust without changing its wire contract.

## Rulings made in this reconciliation

| # | Ruling | Why |
|---|---|---|
| 0.1 | §44 §3 counts 22 native routes; the as-built router has 27 `/v1` method/path pairs: the 25 `api::router` registers itself, plus `hot` and `warm`, which `hot::routes()` registers separately and `api::router` merges (so they are counted here too, not added on top). `/health`, `/ready` and the `/internal/*` routes are not app routes and are not in the 27. The table above is authoritative, and `route_map_covers_every_route` asserts the count so it cannot drift. AP1e Task 19 (D672, 2026-10-08) added `GET /v1/namespaces/{ns}/streams` and `GET /v1/namespaces/{ns}/links`, making it 29 (27 from `api::router`); they are native REST until the streams and links Connect services exist. | Counted from `api::router` and `hot::routes` |
| 0.2 | The console contract is served only by the mocks (`loams-apps-mock` mounting `loams-console-mock`'s routes); there is no `/api/v1` handler in the server to port. Task 7 builds `loams.admin.v1`/`loams.auth.v1` against the mock's seed (as the plan says: "tests are the console mock's") and the console moves to Connect in Task 9. | `grep -r /api/v1 crates` finds only the mock |
| 0.3 | The metastore Raft transport (`/internal/v1/raft/*`, `/internal/v1/meta/*`) is node-to-node HTTP like the other `/internal` routes, so it is mapped to `loams.internal.v1`. Task 8 moves the four `api/internal.rs` routes first; the Raft transport moves in the same task only if `loams-meta`'s `HttpTransport` can switch without changing openraft's wire types, else it is a follow-up issue (it is never on the public port and never in an SDK, D607). | §44 §5.1 row `/internal/*`; keep Task 8 one PR |
| 0.4 | Hot and warm become `CollectionService/SetHot` and `WarmCollection`, as §44 §5.1; the native `/v1/namespaces/{ns}/collections/{c}/hot` GET does not exist (the hot status is part of `GetCollection`). | As built |
