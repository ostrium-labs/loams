//! The Connect API on the main port (design §44 §4, D600; API1 Task 1).
//!
//! One port serves the Connect protocol, gRPC and gRPC-Web at once:
//! connect-rust answers all three from one handler, so a `curl` POST of JSON,
//! a `grpcurl` call and a browser's gRPC-Web fetch are the same RPC. This
//! module owns that wiring and nothing else:
//!
//! - the **catalogue** (`CATALOGUE`): every public `loams.*.v1` package that
//!   exists, its services, and whether this binary serves them;
//! - [`InstanceService`], which an SDK calls first, before sign-in, to
//!   feature-detect from `GetInstance.services[]`;
//! - the stub an **unavailable** package answers with: `unimplemented` with
//!   the reason `feature_not_in_variant` and the variant in `metadata`;
//! - `grpc.health.v1`, and `grpc.reflection.v1` where reflection is on.
//!
//! The native REST routes in [`super`] are **not** replaced here. Tasks 2–8
//! add one RPC per route and Task 9 deletes the route, so [`super::router`]
//! serves both for now. The Connect paths are registered as their own axum
//! routes rather than as the router's fallback, precisely so that the native
//! `no_route` fallback stays the answer for a path neither API knows
//! (`it/http.rs::framework_rejections_use_the_json_error_body`).

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement.
#![allow(refining_impl_trait)]

use std::sync::Arc;
use std::sync::LazyLock;

use axum::Router as AxumRouter;
use buffa::enumeration::EnumValue;
use connectrpc::{
    ConnectError, ErrorCode, ErrorDetail, RequestContext, Response, Router, ServiceRequest,
    ServiceResult,
};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::instance::v1::{
    Edition, GetInstanceRequest, GetInstanceResponse, InstanceService, InstanceServiceExt,
    Principal as PrincipalMsg, PrincipalKind as PrincipalKindMsg, ServiceStatus, SignInKind,
    SignInMethod, WhoAmIRequest, WhoAmIResponse,
};
use ulid::Ulid;

use super::AppState;

/// The build variant this binary is (§30 §9, D286): `full` when an engine
/// only `full` carries is compiled in, `standard` otherwise. The variant
/// never changes the wire contract; it only decides which catalogue packages
/// answer `feature_not_in_variant` (§44 §4). CLI2 Task 3 replaces this with
/// the release variant file; until then the cargo features are the variant.
pub const VARIANT: &str = if cfg!(any(
    feature = "tikv",
    feature = "mysql-wire",
    feature = "stream-grpc",
    feature = "graph"
)) {
    "full"
} else {
    "standard"
};

/// What this binary calls itself in `GetInstance.name`. The auth plan (MT)
/// makes it configurable per instance; until then it is the product name.
const INSTANCE_NAME: &str = "Loams";

/// One package of the API catalogue (design §44 §4, D600).
struct Package {
    /// The proto package, for example `loams.collection.v1`.
    package: &'static str,
    /// Its services, fully qualified.
    services: &'static [&'static str],
    /// Whether this binary serves it. False means every one of its RPCs
    /// answers `unimplemented` with reason `feature_not_in_variant`.
    available: bool,
    /// Whether its wire contract may still change (`ModuleOptions.unstable`,
    /// §44 §10.3): SDKs mark the module experimental and `buf breaking`
    /// skips it.
    unstable: bool,
}

/// Every public package of the API, and what this binary does with it.
///
/// API1 Task 1 starts the list with the two packages whose protos exist and
/// whose availability is decided: the one this binary serves and the one it
/// does not. Tasks 2–8 append `loams.collection.v1`, `loams.sql.v1`,
/// `loams.link.v1`, `loams.stream.v1`, `loams.admin.v1`, `loams.auth.v1`, and
/// the cluster-listener-only `loams.internal.v1`, flipping each row's
/// `available` as its handler lands. A package with no proto yet has no row, so
/// `GetInstance.services[]` never advertises a contract that does not exist.
///
/// Every package here must be one §44 enumerates; `route_map.rs` reads §44 and
/// the protos and fails on a row that is not.
const CATALOGUE: &[Package] = &[
    Package {
        package: "loams.instance.v1",
        services: &["loams.instance.v1.InstanceService"],
        available: true,
        unstable: false,
    },
    Package {
        // Namespaces, collections, their schema, aliases, versions, scan plans
        // and the hot tier (design §44 §5.1; API1 Task 2), plus documents — the
        // atomic write, the get, the scroll, the count and the two filter
        // writes (API1 Task 3), plus the search IR — `QueryService/Search`,
        // §8's `Query` in the same `New protos:` sentence (API1 Task 4). One
        // package for all of it is what §44 §7.2's
        // module catalogue says (`loams.collections`, `loams.documents` ⇢
        // `loams.collection.v1` `CollectionService`, `DocumentService`) and
        // what §8's `New protos:` sentence enumerates as Namespace, Collection,
        // Document and Query; `DocumentService` was briefly a package of its
        // own, which no design section mentions.
        //
        // The REST routes this replaces stay until Task 9, so both surfaces
        // answer today, and `WriteDocuments` is the first RPC here that needs an
        // `idempotency_key` (plan ruling 2.4).
        package: "loams.collection.v1",
        services: &[
            "loams.collection.v1.NamespaceService",
            "loams.collection.v1.CollectionService",
            "loams.collection.v1.DocumentService",
            "loams.collection.v1.QueryService",
        ],
        available: true,
        unstable: false,
    },
    Package {
        // One read-only SQL statement over a namespace's collections, with
        // `$n` parameters, as rows or as Arrow IPC (API2 Task 1, SDKP P0b,
        // `super::connect_sql`). Flight SQL is unchanged.
        package: "loams.sql.v1",
        services: &["loams.sql.v1.SqlService"],
        available: true,
        unstable: false,
    },
    Package {
        // Streams: create, describe and list a namespace's logs; produce and
        // fetch records and CloudEvents, with an idempotent produce and a long
        // poll (API2 Task 2, SDKP P0c, `super::connect_streams`). The tonic
        // listener of `--stream-grpc-listen` keeps serving its subset.
        package: "loams.stream.v1",
        services: &["loams.stream.v1.StreamService"],
        available: true,
        unstable: false,
    },
    Package {
        // Sign-in providers, first-run setup, console sessions and OAuth
        // consent (API2 Task 6, `super::connect_auth`), over the principal
        // store in the metastore (`crate::auth::directory`).
        package: "loams.auth.v1",
        services: &["loams.auth.v1.AuthService"],
        available: true,
        unstable: false,
    },
    Package {
        // The org, projects, agents, API keys and the audit log (API2 Task 7,
        // `super::connect_admin`), each RPC checked by the `Authorizer`.
        package: "loams.admin.v1",
        services: &[
            "loams.admin.v1.OrgService",
            "loams.admin.v1.ProjectService",
            "loams.admin.v1.AgentService",
            "loams.admin.v1.KeyService",
            "loams.admin.v1.AuditService",
        ],
        available: true,
        unstable: false,
    },
    Package {
        // Loams Live (design §45, D688; LV1 plan Task 12): served with the
        // `live` cargo feature while Live runs (dev and standalone, unless
        // `--no-live`), by the running Live server's own services
        // (`super::live`); otherwise every RPC refuses with
        // `feature_not_in_variant`. Unstable until LV1 Task 40.
        package: "loams.live.v1",
        services: &[
            "loams.live.v1.LiveService",
            "loams.live.v1.LiveAdminService",
        ],
        available: cfg!(feature = "live"),
        unstable: true,
    },
    Package {
        // Loams Graph (GR1 Task 5, design §48 §8, D741): served with the `graph` cargo feature
        // (the `full` variant) on dev and standalone; otherwise every RPC answers
        // `feature_not_in_variant` (`super::graph`). Unstable until GR1 Task 39.
        package: "loams.graph.v1",
        services: &[
            "loams.graph.v1.GraphAdminService",
            "loams.graph.v1.GraphService",
        ],
        available: cfg!(feature = "graph"),
        unstable: true,
    },
    Package {
        // Loams House (HS1 Task 7, design §49 §18, D778): served by a separate
        // `loams-fabric house` front and proxied by this port
        // (`super::house_proxy`), so `available` here only says "this binary can
        // proxy it": the row is available while `[house] endpoint` is configured
        // **and** its health check passes ([`Served`]). Unstable until HS1 Task 38.
        package: "loams.house.v1",
        services: &["loams.house.v1.HouseService"],
        available: true,
        unstable: true,
    },
    Package {
        // Loams Git (GT1 Task 12, design §36 §19.10, D834, ruling T0-10): served with the `git`
        // cargo feature (`RepoService` from GT1 Task 13, `ObjectService` from Task 18,
        // `WorkspaceService` from Task 25); otherwise every RPC answers `feature_not_in_variant`
        // (`super::repos`, `super::objects`, `super::workspaces`). `EnvService` joins this row
        // when GT1 Task 61 gives it RPCs. Unstable until GT1 GA.
        package: "loams.repos.v1",
        services: &[
            "loams.repos.v1.RepoService",
            "loams.repos.v1.ObjectService",
            "loams.repos.v1.WorkspaceService",
        ],
        available: super::repos::SERVED,
        unstable: true,
    },
    Package {
        // Loams Meter (design §63 §17.1, D1131; ME1 role wiring): customers,
        // entitlements and grants, plans, notifications. Served with the
        // `loams` feature `meter` on a gateway node; `LeaseService` is internal
        // (service identities, the cluster listener) and is not listed.
        // Unstable until ME1's exit.
        package: "loams.meter.v1",
        services: &[
            "loams.meter.v1.CustomerService",
            "loams.meter.v1.EntitlementService",
            "loams.meter.v1.PlanService",
            "loams.meter.v1.NotificationService",
            "loams.meter.v1.MeterService",
        ],
        available: super::meter::SERVED,
        unstable: true,
    },
];

/// What this running server serves beyond the build's catalogue: the graph
/// runtime, Loams Live, and a healthy House front behind the proxy.
#[derive(Clone, Copy, Debug)]
struct Served {
    /// The graph runtime is on (not `--no-graph`, not a cluster node).
    graph: bool,
    /// Loams Live runs and this router serves it (`super::live`).
    live: bool,
    /// `[house] endpoint` is configured and answered its last health check.
    house: bool,
}

/// Whether a catalogue package is served by this running server: the build's `available`, and
/// for `loams.graph.v1` also whether the graph runtime is on (`--no-graph`, cluster nodes), for
/// `loams.live.v1` whether Live runs and this router serves it, for `loams.house.v1` whether the
/// House front is configured and healthy.
fn is_available(entry: &Package, served: Served) -> bool {
    entry.available
        && match entry.package {
            "loams.graph.v1" => served.graph,
            "loams.live.v1" => served.live,
            super::house_proxy::PACKAGE => served.house,
            _ => true,
        }
}

/// The product features compiled into this binary, as `GetInstance.features`
/// announces them (`feature.<name>`; UP1 Task 16, design §54 §13.1): a
/// console feature-detects a product the build left out instead of calling
/// it. Developer and test features (`failpoints`, `cluster-tests`) are not
/// products and are not listed.
const PRODUCT_FEATURES: &[(&str, bool)] = &[
    ("es", cfg!(feature = "es")),
    ("flight", cfg!(feature = "flight")),
    ("qdrant", cfg!(feature = "qdrant")),
    ("hnsw", cfg!(feature = "hnsw")),
    ("live", cfg!(feature = "live")),
    ("live-tikv", cfg!(feature = "live-tikv")),
    ("tikv", cfg!(feature = "tikv")),
    ("durable", cfg!(feature = "durable")),
    ("durable-mysql", cfg!(feature = "durable-mysql")),
    ("durable-tikv", cfg!(feature = "durable-tikv")),
    ("graph", cfg!(feature = "graph")),
    ("git", cfg!(feature = "git")),
    ("postgres", cfg!(feature = "postgres")),
    ("pgwire", cfg!(feature = "pgwire")),
    ("mysql-wire", cfg!(feature = "mysql-wire")),
    ("stream-grpc", cfg!(feature = "stream-grpc")),
    ("sqldb", cfg!(feature = "sqldb")),
    ("registry", cfg!(feature = "registry")),
    ("buildcache", cfg!(feature = "buildcache")),
    ("meter", cfg!(feature = "meter")),
    (
        "meter-export-webhook",
        cfg!(feature = "meter-export-webhook"),
    ),
    ("meter-export-stripe", cfg!(feature = "meter-export-stripe")),
    ("meter-export-lago", cfg!(feature = "meter-export-lago")),
    (
        "meter-export-openmeter",
        cfg!(feature = "meter-export-openmeter"),
    ),
    ("knowledge", cfg!(feature = "knowledge")),
    // Resumable transfers and job runs (design 85; preview, on by default).
    ("transfer", cfg!(feature = "transfer")),
    // The cluster CA and `loams ca` (design 83; off by default).
    ("transport", cfg!(feature = "transport")),
    // The managed engines' CLIs (design 81; preview).
    ("flink", cfg!(feature = "flink")),
    ("spark", cfg!(feature = "spark")),
];

/// `GetInstance.features`: every product feature of the build
/// (`feature.<name>`) and every role this server runs (`role.graph`,
/// `role.live`, `role.house`).
fn features(served: Served) -> impl Iterator<Item = (String, bool)> {
    PRODUCT_FEATURES
        .iter()
        .map(|(name, on)| (format!("feature.{name}"), *on))
        .chain([
            ("role.graph".to_owned(), served.graph),
            ("role.live".to_owned(), served.live),
            ("role.house".to_owned(), served.house),
        ])
}

/// `GetInstance.min_app_versions`: the oldest app per platform this release
/// supports, from its entry in the release train (`release/upgrade-path.toml`,
/// `[release.min_app_versions]`; design §54 §13.1).
fn min_app_versions(
    path: &loams_migrate::path::UpgradePath,
    version: &str,
) -> impl Iterator<Item = (String, String)> {
    path.release(version)
        .map(|r| r.min_app_versions.clone())
        .unwrap_or_default()
        .into_iter()
}

/// The catalogue as `GetInstance.services[]`.
fn statuses(served: Served) -> Vec<ServiceStatus> {
    CATALOGUE
        .iter()
        .map(|entry| ServiceStatus {
            package: entry.package.to_owned(),
            version: "v1".to_owned(),
            available: is_available(entry, served),
            services: entry
                .services
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            unstable: entry.unstable,
            ..Default::default()
        })
        .collect()
}

/// The services a `grpc.health.v1` probe may ask about. The whole-process
/// entry (the empty name) is pre-registered by `connectrpc-health` and is not
/// repeated here. The registration is static, so `loams.house.v1`, whose
/// health is another process's and changes while this one runs, is never in
/// it: the catalogue's `available` is the place to ask.
fn served_services(graph_served: bool, live_served: bool) -> Vec<&'static str> {
    let served = Served {
        graph: graph_served,
        live: live_served,
        house: false,
    };
    CATALOGUE
        .iter()
        .filter(|entry| is_available(entry, served))
        .flat_map(|entry| entry.services.iter().copied())
        .collect()
}

/// This process's instance id: a ULID generated when the server starts. The
/// auth plan (MT) persists one id per install and serves it here; until then
/// it identifies the running process.
static INSTANCE_ID: LazyLock<String> = LazyLock::new(|| Ulid::generate().to_string());

/// `loams.instance.v1.InstanceService` for the OSS server.
#[derive(Debug)]
struct Instance {
    /// Whether `loams.graph.v1` is served by this running server.
    graph_served: bool,
    /// Whether `loams.live.v1` is served by this router.
    live_served: bool,
    /// The House proxy, whose health decides `loams.house.v1`'s row.
    house: Option<Arc<super::house_proxy::HouseProxy>>,
    /// The principal store, which says whether first-run setup is due.
    principals: crate::auth::Principals,
}

impl Instance {
    /// What is served right now (the House's health changes while the server runs).
    fn served(&self) -> Served {
        Served {
            graph: self.graph_served,
            live: self.live_served,
            house: self.house.as_ref().is_some_and(|house| house.healthy()),
        }
    }

    /// Whether the console should open on first-run setup: there is no org
    /// yet and `AuthService/CompleteSetup` can run (authentication is off, or
    /// `LOAMS_SETUP_TOKEN` is set). With setup closed it stays false, so the
    /// console does not open on a page nobody can complete.
    async fn setup_required(&self) -> bool {
        !matches!(self.principals.setup, crate::auth::SetupGate::Closed)
            && self
                .principals
                .directory
                .read()
                .await
                .is_ok_and(|doc| doc.org.is_none())
    }
}

impl InstanceService for Instance {
    /// No credentials: an app calls this before sign-in.
    async fn get_instance(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetInstanceRequest>,
    ) -> ServiceResult<GetInstanceResponse> {
        let served = self.served();
        Response::ok(GetInstanceResponse {
            instance_id: INSTANCE_ID.clone(),
            name: INSTANCE_NAME.to_owned(),
            // This binary is the open-source edition; `EDITION_CLOUD` and
            // `EDITION_BYOC` are the hosted gateway's.
            edition: EnumValue::Known(Edition::EDITION_OSS),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            // Only what is served, which is what the console needs to build an
            // `rpc.<service>` (AP1a Ruling 6); `services` is the long form.
            api_versions: CATALOGUE
                .iter()
                .filter(|entry| is_available(entry, served))
                .map(|entry| entry.package.to_owned())
                .collect(),
            sign_in_methods: vec![SignInMethod {
                kind: EnumValue::Known(SignInKind::SIGN_IN_KIND_NONE),
                display_name: "No sign-in".to_owned(),
                ..Default::default()
            }],
            services: statuses(served),
            setup_required: self.setup_required().await,
            features: features(served).collect(),
            min_app_versions: min_app_versions(
                loams_migrate::path::UpgradePath::compiled(),
                env!("CARGO_PKG_VERSION"),
            )
            .collect(),
            // Empty until their plans land, not because they are off: `issuer`
            // and `jwks_uri` with the auth plan (MT), `tls_pins` and `push`
            // with the phone plans (§37 §7), `key_rotation` with signing-key
            // rotation.
            ..Default::default()
        })
    }

    /// The principal the auth layer ([`crate::auth`]) admitted. Without one
    /// (a router served without the layer) the refusal names the RPC, so a
    /// caller can tell it apart from a token that was rejected.
    async fn who_am_i(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, WhoAmIRequest>,
    ) -> ServiceResult<WhoAmIResponse> {
        use crate::auth::PrincipalKind as Kind;
        let Some(principal) = ctx.extensions().get::<crate::auth::Principal>() else {
            return Err(not_implemented("loams.instance.v1.InstanceService/WhoAmI"));
        };
        let kind = match principal.kind {
            Kind::User | Kind::Session | Kind::Tailscale => PrincipalKindMsg::PRINCIPAL_KIND_USER,
            Kind::ApiKey => PrincipalKindMsg::PRINCIPAL_KIND_SERVICE_ACCOUNT,
            Kind::DevLoopback => PrincipalKindMsg::PRINCIPAL_KIND_UNSPECIFIED,
        };
        Response::ok(WhoAmIResponse {
            principal: buffa::MessageField::some(PrincipalMsg {
                id: principal.subject.clone(),
                kind: EnumValue::Known(kind),
                display_name: principal.display_name.clone(),
                email: principal.email.clone().unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

/// A failed RPC: the Connect `code`, and one `loams.errors.v1.ErrorInfo` in
/// the details carrying the stable `reason` callers branch on plus the
/// `metadata` that goes with it (design §44 §7.4, D611). `metadata` never
/// holds a secret. Every reason this module raises is registered in
/// `docs/api/reasons.md`.
///
/// The metadata values are **owned** because they are not always literals: an
/// `ApiError`'s structured fields are stringified on the way in (they are
/// numbers in the REST body), so a caller building one has to own the text.
pub(crate) fn refuse(
    code: ErrorCode,
    reason: &str,
    message: impl Into<String>,
    metadata: &[(&str, String)],
) -> ConnectError {
    let info = ErrorInfo {
        reason: reason.to_owned(),
        metadata: metadata
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
        ..Default::default()
    };
    ConnectError::new(code, message).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &info,
    ))
}

/// What every RPC of a catalogue package this binary does not serve answers
/// (design §44 §4, D600): `unimplemented`, the reason
/// `feature_not_in_variant`, and the variant that was asked for, so a caller
/// can tell "not in this build" from "not written yet" (`not_implemented`).
pub(super) fn not_in_variant(rpc: &str) -> ConnectError {
    refuse(
        ErrorCode::Unimplemented,
        "feature_not_in_variant",
        format!("{rpc} is not in the {VARIANT} variant"),
        &[("variant", VARIANT.to_owned())],
    )
}

/// What an RPC whose service exists in the protos but is not implemented by
/// this binary yet answers (AP0's `not_implemented`).
pub(super) fn not_implemented(rpc: &str) -> ConnectError {
    refuse(
        ErrorCode::Unimplemented,
        "not_implemented",
        format!("{rpc} is not implemented by this build yet"),
        &[],
    )
}

/// The reflection services, which list themselves (as grpc-go's do).
const REFLECTION_SERVICES: [&str; 2] = [
    "grpc.reflection.v1.ServerReflection",
    "grpc.reflection.v1alpha.ServerReflection",
];

/// Installs `grpc.reflection.v1` on `rpc`. `ListServices` lists only what
/// this process answers (PG2 R1.4): every service with a route on `rpc`
/// (served, or a stub that refuses with a typed reason; health among them),
/// the services in `also` that a layer outside `rpc` answers, and
/// reflection itself. A package that is only in `loams-proto`'s descriptor
/// set, such as one a different role serves, is never listed, though its
/// files stay resolvable.
///
/// Reflection is a convenience, so a descriptor set that does not parse
/// costs reflection and nothing else: it is logged and the port still
/// serves.
pub(crate) fn install_reflection(rpc: Router, also: &[&str]) -> Router {
    let mut services: Vec<String> = Vec::new();
    let answered = rpc
        .methods()
        .filter_map(|path| path.split_once('/').map(|(service, _)| service))
        .chain(also.iter().copied())
        .chain(REFLECTION_SERVICES);
    for service in answered {
        if !services.iter().any(|s| s == service) {
            services.push(service.to_owned());
        }
    }
    match connectrpc_reflection::Reflector::from_descriptor_set_bytes(
        loams_proto::FILE_DESCRIPTOR_SET,
    ) {
        Ok(reflector) => connectrpc_reflection::install(rpc, reflector.with_services(services)),
        Err(err) => {
            tracing::warn!(%err, "gRPC reflection is off: the descriptor set does not parse");
            rpc
        }
    }
}

/// The Connect routes of the main port, as axum routes.
///
/// Every RPC path of the connect router becomes its own axum route. The
/// alternative — one catch-all at the router's fallback — would take the
/// native API's `404` and `405` away, which Task 9's `no_native_rest_route_
/// remains` still expects from the paths that survive it. A Connect path is
/// `/<package>.<Service>/<Method>` and every native path is under `/v1`,
/// `/internal`, `/health` or `/ready`, so the two sets cannot collide.
pub(crate) fn routes(state: &AppState, hot_default: bool) -> AxumRouter {
    let graph_served = super::graph::served(state);
    let live_served = super::live::served(state);
    let mut rpc = Router::new();
    rpc = Arc::new(Instance {
        graph_served,
        live_served,
        house: state.house.clone(),
        principals: state.principals.clone(),
    })
    .register(rpc);
    rpc = super::live::register(rpc, state);
    rpc = super::graph::register(rpc, state);
    rpc = super::repos::register(rpc, state);
    rpc = super::objects::register(rpc, state);
    rpc = super::workspaces::register(rpc, state);
    rpc = super::connect_collections::register(rpc, state);
    rpc = super::connect_documents::register(rpc, state);
    rpc = super::connect_query::register(rpc, state);
    rpc = super::connect_sql::register(rpc, state);
    rpc = super::connect_streams::register(rpc, state);
    rpc = super::connect_auth::register(rpc, &state.principals);
    rpc = super::connect_admin::register(rpc, &state.principals);
    rpc = super::meter::register(rpc, state);
    let (rpc, _health) =
        connectrpc_health::install_static(rpc, served_services(graph_served, live_served));
    let rpc = if state.reflection {
        // `loams.house.v1` is answered by the House layer around these
        // routes (`super::house_proxy`), not by a route of `rpc`.
        let layered: Vec<&str> = CATALOGUE
            .iter()
            .filter(|entry| entry.package == super::house_proxy::PACKAGE)
            .flat_map(|entry| entry.services.iter().copied())
            .collect();
        install_reflection(rpc, &layered)
    } else {
        rpc
    };
    // Before it is consumed: the paths are what the axum router keys on.
    let paths: Vec<String> = rpc.methods().map(|path| format!("/{path}")).collect();
    let service = rpc.into_axum_service();
    let mut axum = AxumRouter::new();
    for path in paths {
        axum = axum.route_service(&path, service.clone());
    }
    // Loams Meter's OpenMeter-compatible HTTP routes (`/api/v1/meters/…`,
    // `/api/v1/subjects/…`), inside the meter's caller layer below.
    #[cfg(feature = "meter")]
    if let Some(meter) = &state.meter {
        axum = axum.merge(meter.http_routes());
    }
    // Live's admin service records who deployed: the principal the port's
    // authentication layer (outside these routes) admitted.
    #[cfg(feature = "live")]
    if live_served {
        axum = axum.layer(axum::middleware::from_fn(super::live::caller));
    }
    // The meter's own view of the caller (operator or authorizer subject),
    // from the principal the port's authentication layer admitted.
    #[cfg(feature = "meter")]
    if state.meter.is_some() {
        axum = axum.layer(axum::middleware::from_fn_with_state(
            state.principals.clone(),
            super::meter::caller,
        ));
    }
    // Inside `HotLayer`, and with this router's own Connect-shaped refusal of an
    // unusable `Loams-Hot` in front of it (`super::connect_hot`). Applied here
    // rather than by merging these routes into the router the REST routes live
    // in, because the layer decides a refusal's shape from the content type and
    // `application/json` is both the Connect protocol's and the REST one: inside
    // this router there is no ambiguity, outside it there is.
    super::connect_hot::layer(axum, hot_default)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// UP1 Task 16: the release train's `[release.min_app_versions]` is what
    /// `GetInstance` serves, for this binary's release only.
    #[test]
    fn min_app_versions_filled_from_manifest() {
        let path = loams_migrate::path::UpgradePath::parse(
            r#"
[[release]]
version = "0.1.0-preview.3"
from = "0.1.0-preview.2"
[release.min_app_versions]
desktop = "0.1.0-preview.2"
ios = "0.1.0"

[[release]]
version = "0.1.0-preview.2"
from = "fresh"
"#,
        )
        .expect("parses");
        let served: BTreeMap<String, String> = min_app_versions(&path, "0.1.0-preview.3").collect();
        assert_eq!(
            served,
            BTreeMap::from([
                ("desktop".to_owned(), "0.1.0-preview.2".to_owned()),
                ("ios".to_owned(), "0.1.0".to_owned()),
            ])
        );
        assert_eq!(min_app_versions(&path, "0.1.0-preview.2").count(), 0);
        assert_eq!(min_app_versions(&path, "9.9.9").count(), 0);
        // This binary's own entry, from the compiled train.
        let compiled = loams_migrate::path::UpgradePath::compiled();
        let own: BTreeMap<String, String> =
            min_app_versions(compiled, env!("CARGO_PKG_VERSION")).collect();
        assert_eq!(
            own,
            compiled
                .release(env!("CARGO_PKG_VERSION"))
                .map(|r| r.min_app_versions.clone())
                .unwrap_or_default()
        );
    }

    /// UP1 Task 16: one `feature.<name>` per compiled product feature, with
    /// its state in this build, and one `role.<name>` per role.
    #[test]
    fn get_instance_reports_features() {
        let served = Served {
            graph: true,
            live: false,
            house: true,
        };
        let map: BTreeMap<String, bool> = features(served).collect();
        assert_eq!(map["feature.es"], cfg!(feature = "es"));
        assert_eq!(map["feature.durable"], cfg!(feature = "durable"));
        assert_eq!(map["feature.postgres"], cfg!(feature = "postgres"));
        assert!(map["role.graph"]);
        assert!(!map["role.live"]);
        assert!(map["role.house"]);
        assert_eq!(map.len(), PRODUCT_FEATURES.len() + 3);
        // Every product feature of Cargo.toml is listed; test-only ones are not.
        let manifest = include_str!("../../Cargo.toml");
        let section = manifest
            .split("\n[features]\n")
            .nth(1)
            .and_then(|s| s.split("\n[").next())
            .expect("[features]");
        for line in section.lines() {
            let Some((name, _)) = line
                .split_once(" = ")
                .filter(|(name, _)| !name.starts_with('#') && !name.contains(' '))
            else {
                continue;
            };
            if [
                "default",
                "failpoints",
                "cluster-tests",
                "postgres-kubernetes",
            ]
            .contains(&name)
            {
                continue;
            }
            assert!(
                map.contains_key(&format!("feature.{name}")),
                "feature {name} is not announced"
            );
        }
    }
}
