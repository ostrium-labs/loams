// The generated service traits return `impl Encodable<_>`; these impls name the concrete message
// type, which is the intended refinement (as `connect.rs` does).
#![allow(refining_impl_trait)]

//! `loams.live.v1` on the main port (LV1 plan Task 12, design §45 §6, D688).
//!
//! With the `live` feature and Live running (dev and standalone), `LiveService` and
//! `LiveAdminService` are the running Live server's own services ([`loams_live::LiveRoutes`]):
//! the same sessions, engine and log ring as the Live listener. They sit behind the port's
//! authentication layer like every other service, and [`caller`] names the admitted principal
//! for the admin service, which records it as a deployment's `created_by`. Otherwise every RPC
//! answers `unimplemented` with the reason `feature_not_in_variant` ([`LiveAbsent`],
//! [`LiveAdminAbsent`]).
//!
//! Until LV1 Task 18 (TLS and the non-loopback rule), Live is registered only on a loopback
//! main port and on the `tailscale serve` target (itself loopback, reached over the tailnet with
//! the tailnet's TLS): the server leaves it out of a router it serves on any other address
//! (`Server::assemble`).

use std::sync::Arc;

use connectrpc::{RequestContext, Router, ServiceRequest, ServiceResult, ServiceStream};
use loams_live_proto::loams::live::v1::{
    ActivateDeploymentRequest, ActivateDeploymentResponse, AuthenticateRequest,
    AuthenticateResponse, CreateAppRequest, CreateAppResponse, DeleteAppRequest, DeleteAppResponse,
    DeployRequest, DeployResponse, GetAppRequest, GetAppResponse, GetDeploymentRequest,
    GetDeploymentResponse, GetIndexStatusRequest, GetIndexStatusResponse, ListAppsRequest,
    ListAppsResponse, ListDeploymentsRequest, ListDeploymentsResponse, LiveAdminService,
    LiveAdminServiceExt, LiveService, LiveServiceExt, ModifyQuerySetRequest,
    ModifyQuerySetResponse, MutateRequest, MutateResponse, QueryRequest, QueryResponse,
    TailLogsRequest, TailLogsResponse, Transition, UpdateAppRequest, UpdateAppResponse,
    WatchRequest,
};

use super::AppState;
use super::connect::not_in_variant;

/// Whether this router serves `loams.live.v1` (for `GetInstance.services[]` and health).
pub(crate) fn served(state: &AppState) -> bool {
    #[cfg(feature = "live")]
    {
        state.live.is_some()
    }
    #[cfg(not(feature = "live"))]
    {
        let _ = state;
        false
    }
}

/// Registers both Live services: the running server's, or the `feature_not_in_variant` stubs.
pub(super) fn register(router: Router, state: &AppState) -> Router {
    #[cfg(feature = "live")]
    if let Some(live) = &state.live {
        return live.register(router);
    }
    let _ = state;
    let router = Arc::new(LiveAbsent).register(router);
    Arc::new(LiveAdminAbsent).register(router)
}

/// Names the principal the authentication layer admitted for Loams Live's admin service (LV1
/// plan Task 10): its id, never its credential. On the main port it runs inside the port's
/// authentication layer; on Live's own listener inside the guard the server gives it.
#[cfg(feature = "live")]
pub(crate) async fn caller(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // LV1 plan Task 18: an origin the engine itself allows (the console's).
    if request
        .extensions()
        .get::<crate::auth::layer::EngineOrigin>()
        .is_some()
    {
        request.extensions_mut().insert(loams_live::ConsoleOrigin);
    }
    if let Some(principal) = request.extensions().get::<crate::auth::Principal>() {
        let caller = live_caller(principal);
        request.extensions_mut().insert(caller);
    } else if request
        .extensions()
        .get::<crate::auth::layer::LeftToApp>()
        .is_some()
    {
        // LV1 plan Task 16: no engine principal; Live identifies the app's
        // end user (or an anonymous caller) itself.
        request
            .extensions_mut()
            .insert(loams_live::auth::Caller::anonymous());
    }
    next.run(request).await
}

/// Closes the Live sessions of every principal the directory's audit
/// stream says lost its access (LV1 plan Task 17): a revoked API key
/// (`apikey:<id>`) or a removed member (their user id). Ends with the
/// stream.
#[cfg(feature = "live")]
pub(crate) async fn close_revoked(
    mut events: tokio::sync::broadcast::Receiver<crate::auth::directory::AuditRec>,
    live: loams_live::LiveRoutes,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match events.recv().await {
            Ok(rec) => {
                if let Some(principal) = revoked_principal(&rec.event) {
                    live.revoke(&principal);
                }
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(
                    missed,
                    "Live missed directory audit events; sessions of a revoked credential may stay open until they expire"
                );
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// The principal an audit event takes access from, if any.
#[cfg(feature = "live")]
fn revoked_principal(event: &loams_proto::loams::admin::v1::AuditEvent) -> Option<String> {
    let target = event.target.as_option()?;
    match event.action.as_str() {
        "api_key.revoke" => Some(format!("apikey:{}", target.id)),
        "member.remove" => Some(target.id.clone()),
        _ => None,
    }
}

/// The Live caller of an admitted principal (LV1 plan Task 14): its id,
/// its kind and what its credential reaches.
#[cfg(feature = "live")]
pub(crate) fn live_caller(principal: &crate::auth::Principal) -> loams_live::auth::Caller {
    use crate::auth::PrincipalKind;
    use loams_live::auth::{Caller, CallerKind};
    let kind = match principal.kind {
        PrincipalKind::User => CallerKind::User,
        PrincipalKind::Session => CallerKind::Session,
        PrincipalKind::ApiKey => CallerKind::ApiKey,
        PrincipalKind::Tailscale => CallerKind::Tailscale,
        PrincipalKind::DevLoopback => CallerKind::DevLoopback,
    };
    let grant = &principal.grant;
    Caller {
        org: grant.org.clone(),
        environment: grant.environment.clone(),
        scopes: grant.scopes.clone(),
        expires_at_ms: grant.expires_at_ms,
        ..Caller::new(principal.subject.clone(), kind)
    }
}

/// `loams.live.v1.LiveService` as this binary answers it: every RPC refuses
/// with `feature_not_in_variant`.
///
/// Registering the service rather than leaving its paths unrouted is what
/// makes the failure typed. A caller gets the Connect code, the stable reason
/// and the variant in `metadata` instead of a bare `404`, and
/// `GetInstance.services[]` can list the package at all (§44 §4). Without
/// Live running, the main port answers with this.
#[derive(Debug)]
struct LiveAbsent;

#[cfg(all(test, feature = "live"))]
mod tests {
    use super::*;
    use crate::auth::{Grant, Principal, PrincipalKind};
    use loams_live::auth::CallerKind;

    /// LV1 plan Task 17: a revoked key and a removed member close their
    /// sessions; other audit events do not.
    #[test]
    fn revocations_name_the_principal() {
        use loams_proto::loams::admin::v1::{AuditEvent, ResourceRef};
        let event = |action: &str, id: &str| AuditEvent {
            action: action.to_owned(),
            target: buffa::MessageField::some(ResourceRef {
                id: id.to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            revoked_principal(&event("api_key.revoke", "01")).as_deref(),
            Some("apikey:01")
        );
        assert_eq!(
            revoked_principal(&event("member.remove", "u1")).as_deref(),
            Some("u1")
        );
        assert_eq!(revoked_principal(&event("api_key.create", "01")), None);
    }

    /// LV1 plan Task 14: the caller carries the principal's kind and grant.
    #[test]
    fn the_live_caller_carries_the_grant() {
        let principal = Principal {
            subject: "apikey:01".to_owned(),
            kind: PrincipalKind::ApiKey,
            issuer: None,
            display_name: "ci".to_owned(),
            email: None,
            grant: Grant {
                org: Some("acme".to_owned()),
                environment: Some("shop/prod".to_owned()),
                scopes: Grant::scopes_of(["live:call"]),
                expires_at_ms: Some(42),
            },
        };
        let caller = live_caller(&principal);
        assert_eq!(caller.id, "apikey:01");
        assert_eq!(caller.kind, CallerKind::ApiKey);
        assert_eq!(caller.environment.as_deref(), Some("shop/prod"));
        assert!(caller.has_scope("live:call") && !caller.has_scope("live:deploy"));
        assert_eq!(caller.expires_at_ms, Some(42));
        let open = live_caller(&Principal {
            grant: Grant::default(),
            kind: PrincipalKind::DevLoopback,
            issuer: None,
            ..principal
        });
        assert!(open.has_scope("live:deploy") && open.environment.is_none());
    }
}

impl LiveService for LiveAbsent {
    async fn watch(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, WatchRequest>,
    ) -> ServiceResult<ServiceStream<Transition>> {
        Err(not_in_variant("loams.live.v1.LiveService/Watch"))
    }

    async fn modify_query_set(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ModifyQuerySetRequest>,
    ) -> ServiceResult<ModifyQuerySetResponse> {
        Err(not_in_variant("loams.live.v1.LiveService/ModifyQuerySet"))
    }

    async fn authenticate(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, AuthenticateRequest>,
    ) -> ServiceResult<AuthenticateResponse> {
        Err(not_in_variant("loams.live.v1.LiveService/Authenticate"))
    }

    async fn query(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, QueryRequest>,
    ) -> ServiceResult<QueryResponse> {
        Err(not_in_variant("loams.live.v1.LiveService/Query"))
    }

    async fn mutate(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, MutateRequest>,
    ) -> ServiceResult<MutateResponse> {
        Err(not_in_variant("loams.live.v1.LiveService/Mutate"))
    }
}

/// `loams.live.v1.LiveAdminService` (LV1 plan Task 10) as the main port
/// answers it without Live running, as [`LiveAbsent`] does `LiveService`.
#[derive(Debug)]
struct LiveAdminAbsent;

impl LiveAdminService for LiveAdminAbsent {
    async fn deploy(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, DeployRequest>,
    ) -> ServiceResult<DeployResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/Deploy"))
    }

    async fn list_deployments(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ListDeploymentsRequest>,
    ) -> ServiceResult<ListDeploymentsResponse> {
        Err(not_in_variant(
            "loams.live.v1.LiveAdminService/ListDeployments",
        ))
    }

    async fn get_deployment(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetDeploymentRequest>,
    ) -> ServiceResult<GetDeploymentResponse> {
        Err(not_in_variant(
            "loams.live.v1.LiveAdminService/GetDeployment",
        ))
    }

    async fn activate_deployment(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ActivateDeploymentRequest>,
    ) -> ServiceResult<ActivateDeploymentResponse> {
        Err(not_in_variant(
            "loams.live.v1.LiveAdminService/ActivateDeployment",
        ))
    }

    async fn get_index_status(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetIndexStatusRequest>,
    ) -> ServiceResult<GetIndexStatusResponse> {
        Err(not_in_variant(
            "loams.live.v1.LiveAdminService/GetIndexStatus",
        ))
    }

    async fn tail_logs(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, TailLogsRequest>,
    ) -> ServiceResult<ServiceStream<TailLogsResponse>> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/TailLogs"))
    }

    async fn create_app(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, CreateAppRequest>,
    ) -> ServiceResult<CreateAppResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/CreateApp"))
    }

    async fn list_apps(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ListAppsRequest>,
    ) -> ServiceResult<ListAppsResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/ListApps"))
    }

    async fn get_app(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetAppRequest>,
    ) -> ServiceResult<GetAppResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/GetApp"))
    }

    async fn update_app(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, UpdateAppRequest>,
    ) -> ServiceResult<UpdateAppResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/UpdateApp"))
    }

    async fn delete_app(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, DeleteAppRequest>,
    ) -> ServiceResult<DeleteAppResponse> {
        Err(not_in_variant("loams.live.v1.LiveAdminService/DeleteApp"))
    }
}
