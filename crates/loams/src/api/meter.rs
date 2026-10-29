//! Loams Meter's public services on the main port (design §63 §17.1; ME1
//! role wiring, part 1), behind the `loams` feature `meter`.
//!
//! - With the feature, a gateway node serves `CustomerService`,
//!   `EntitlementService`, `PlanService` and `NotificationService` from
//!   `loams_meter::role::MeterRole`, over records kept as versioned
//!   documents in the metastore and the bucket (as the principal directory
//!   is). Audit events go to the org's audit log (`loams.admin.v1`).
//! - [`caller`] gives the meter its `Caller` from the principal the port's
//!   authentication admitted: the install's operator, or a subject the
//!   authorizer answers for.
//! - Without it, every RPC of the package answers `unimplemented` with
//!   `feature_not_in_variant`, as every unserved package does.

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement.
#![allow(refining_impl_trait)]

use connectrpc::{ConnectError, RequestContext, Router, ServiceRequest, ServiceResult};
use loams_proto::loams::meter::v1 as pb;

use super::AppState;

/// The package (`connect.rs`'s catalogue row).
pub const PACKAGE: &str = "loams.meter.v1";

/// The meter's House user when `LOAMS_METER_HOUSE_USER` is not set.
pub const DEFAULT_HOUSE_USER: &str = "meter";

/// The `meter` role's settings (environment only for now).
#[derive(Clone, Debug, Default)]
pub struct MeterConfig {
    /// Where the meter's House is (design §63 §7.5): the ClickHouse HTTP
    /// interface (`LOAMS_METER_HOUSE_URL`, `http://house:8123`), signed in
    /// to as the meter's own House user, which the operator maps to the
    /// system namespace. `None`: no House side.
    pub house_url: Option<String>,
    /// `LOAMS_METER_HOUSE_USER` ([`DEFAULT_HOUSE_USER`]).
    pub house_user: String,
    /// `LOAMS_METER_HOUSE_PASSWORD_FILE`: the user's password, read at start.
    pub house_password_file: Option<std::path::PathBuf>,
    /// `LOAMS_METER_SECRETS_KEY_FILE`: the age identity webhook secrets are
    /// sealed to (D2205), 0600, created on first use. Default
    /// `<data_dir>/meter/secrets.key`. Every gateway node of a cluster needs
    /// the same one.
    pub secrets_key_file: Option<std::path::PathBuf>,
}

/// Whether this build serves the meter.
pub const SERVED: bool = cfg!(feature = "meter");

/// Registers the meter's public services, or the stubs that refuse them.
pub(super) fn register(router: Router, state: &AppState) -> Router {
    #[cfg(feature = "meter")]
    if let Some(meter) = &state.meter {
        return meter.register(router);
    }
    let _ = state;
    let absent = std::sync::Arc::new(MeterAbsent);
    let router = pb::CustomerServiceExt::register(std::sync::Arc::clone(&absent), router);
    let router = pb::EntitlementServiceExt::register(std::sync::Arc::clone(&absent), router);
    let router = pb::PlanServiceExt::register(std::sync::Arc::clone(&absent), router);
    let router = pb::MeterServiceExt::register(std::sync::Arc::clone(&absent), router);
    pb::NotificationServiceExt::register(absent, router)
}

fn absent(rpc: &str) -> ConnectError {
    let rpc = format!("{PACKAGE}.{rpc}");
    if SERVED {
        super::connect::refuse(
            connectrpc::ErrorCode::Unavailable,
            "meter_not_served",
            format!("{rpc}: this node does not serve the meter (it is not a gateway node)"),
            &[],
        )
    } else {
        super::connect::not_in_variant(&rpc)
    }
}

/// The meter's services where this node does not serve them.
#[derive(Debug)]
struct MeterAbsent;

impl pb::CustomerService for MeterAbsent {
    async fn create_customer(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateCustomerRequest>,
    ) -> ServiceResult<pb::CreateCustomerResponse> {
        Err(absent("CustomerService/CreateCustomer"))
    }

    async fn get_customer(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetCustomerRequest>,
    ) -> ServiceResult<pb::GetCustomerResponse> {
        Err(absent("CustomerService/GetCustomer"))
    }

    async fn list_customers(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListCustomersRequest>,
    ) -> ServiceResult<pb::ListCustomersResponse> {
        Err(absent("CustomerService/ListCustomers"))
    }

    async fn update_customer(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UpdateCustomerRequest>,
    ) -> ServiceResult<pb::UpdateCustomerResponse> {
        Err(absent("CustomerService/UpdateCustomer"))
    }

    async fn set_payment_state(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::SetPaymentStateRequest>,
    ) -> ServiceResult<pb::SetPaymentStateResponse> {
        Err(absent("CustomerService/SetPaymentState"))
    }

    async fn set_spend_controls(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::SetSpendControlsRequest>,
    ) -> ServiceResult<pb::SetSpendControlsResponse> {
        Err(absent("CustomerService/SetSpendControls"))
    }
}

impl pb::EntitlementService for MeterAbsent {
    async fn create_entitlement(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateEntitlementRequest>,
    ) -> ServiceResult<pb::CreateEntitlementResponse> {
        Err(absent("EntitlementService/CreateEntitlement"))
    }

    async fn list_entitlements(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListEntitlementsRequest>,
    ) -> ServiceResult<pb::ListEntitlementsResponse> {
        Err(absent("EntitlementService/ListEntitlements"))
    }

    async fn get_entitlement_value(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetEntitlementValueRequest>,
    ) -> ServiceResult<pb::GetEntitlementValueResponse> {
        Err(absent("EntitlementService/GetEntitlementValue"))
    }

    async fn check_access(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CheckAccessRequest>,
    ) -> ServiceResult<pb::CheckAccessResponse> {
        Err(absent("EntitlementService/CheckAccess"))
    }

    async fn create_grant(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateGrantRequest>,
    ) -> ServiceResult<pb::CreateGrantResponse> {
        Err(absent("EntitlementService/CreateGrant"))
    }

    async fn void_grant(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::VoidGrantRequest>,
    ) -> ServiceResult<pb::VoidGrantResponse> {
        Err(absent("EntitlementService/VoidGrant"))
    }

    async fn list_grants(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListGrantsRequest>,
    ) -> ServiceResult<pb::ListGrantsResponse> {
        Err(absent("EntitlementService/ListGrants"))
    }

    async fn reset_entitlement(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ResetEntitlementRequest>,
    ) -> ServiceResult<pb::ResetEntitlementResponse> {
        Err(absent("EntitlementService/ResetEntitlement"))
    }

    async fn get_balance_history(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetBalanceHistoryRequest>,
    ) -> ServiceResult<pb::GetBalanceHistoryResponse> {
        Err(absent("EntitlementService/GetBalanceHistory"))
    }
}

impl pb::PlanService for MeterAbsent {
    async fn apply_plan(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ApplyPlanRequest>,
    ) -> ServiceResult<pb::ApplyPlanResponse> {
        Err(absent("PlanService/ApplyPlan"))
    }

    async fn list_plans(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListPlansRequest>,
    ) -> ServiceResult<pb::ListPlansResponse> {
        Err(absent("PlanService/ListPlans"))
    }

    async fn assign_plan(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::AssignPlanRequest>,
    ) -> ServiceResult<pb::AssignPlanResponse> {
        Err(absent("PlanService/AssignPlan"))
    }

    async fn unassign_plan(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UnassignPlanRequest>,
    ) -> ServiceResult<pb::UnassignPlanResponse> {
        Err(absent("PlanService/UnassignPlan"))
    }
}

impl pb::MeterService for MeterAbsent {
    async fn query_meter(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::QueryMeterRequest>,
    ) -> ServiceResult<pb::QueryMeterResponse> {
        Err(absent("MeterService/QueryMeter"))
    }
}

impl pb::NotificationService for MeterAbsent {
    async fn create_rule(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateRuleRequest>,
    ) -> ServiceResult<pb::CreateRuleResponse> {
        Err(absent("NotificationService/CreateRule"))
    }

    async fn list_rules(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListRulesRequest>,
    ) -> ServiceResult<pb::ListRulesResponse> {
        Err(absent("NotificationService/ListRules"))
    }

    async fn create_webhook(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateWebhookRequest>,
    ) -> ServiceResult<pb::CreateWebhookResponse> {
        Err(absent("NotificationService/CreateWebhook"))
    }

    async fn rotate_webhook_secret(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::RotateWebhookSecretRequest>,
    ) -> ServiceResult<pb::RotateWebhookSecretResponse> {
        Err(absent("NotificationService/RotateWebhookSecret"))
    }

    async fn list_deliveries(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListDeliveriesRequest>,
    ) -> ServiceResult<pb::ListDeliveriesResponse> {
        Err(absent("NotificationService/ListDeliveries"))
    }
}

#[cfg(feature = "meter")]
pub use served::{MeterRuntime, build, caller};

#[cfg(feature = "meter")]
mod served {
    use std::sync::Arc;

    use async_trait::async_trait;
    use bytes::Bytes;
    use loams_common::meta::MetaStore;
    use loams_meter::audit::{AuditEvent, AuditSink};
    use loams_meter::authz::Caller;
    use loams_meter::error::{MeterError, Result as MeterResult};
    use loams_meter::house::{HouseUsage, HouseUsageSoFar, RoleHouse};
    use loams_meter::lease_service::LeaseServer;
    use loams_meter::notify::MeterEvent;
    use loams_meter::notify::notifier::DocNotifications;
    use loams_meter::notify::sealing::AgeSealer;
    use loams_meter::notify::webhook::HttpTransport;
    use loams_meter::owner::{METER_PARTITIONS, MetaLeases, OwnerError, PartitionReader};
    use loams_meter::plans::MeterUsage;
    use loams_meter::role::{MeterRole, NoHouseYet, RoleParts};
    use loams_meter::run::{
        CLUSTER_SERVICE, CatalogInputs, CounterRuntime, CounterValues, Counters, RuntimeParts,
        StreamOnly,
    };
    use loams_meter::run::{CounterPeer, holder_name};
    use loams_meter::run::{MeterEventHandler, MeterEventSink, own_event, to_cloudevent};
    use loams_meter::store::{DocStore, MeterStore, VersionedDoc};
    use loams_meter_core::{AllocatorConfig, Event, Origin, UtcNanos};
    use loams_proto::loams::admin::v1 as admin;
    use loams_store::Store;

    use crate::auth::Principals;
    use crate::auth::authz::{Standing, standing};
    use crate::auth::directory::{Directory, DirectoryStore, MetaDirectoryStore, Unavailable};

    /// The metastore pointers and bucket prefixes of the meter's two
    /// documents (beside the principal directory's).
    const RECORDS_POINTER: &str = "meter-records";
    const RECORDS_PREFIX: &str = "system/meter/records/";
    const NOTIFY_POINTER: &str = "meter-notifications";
    const NOTIFY_PREFIX: &str = "system/meter/notifications/";

    /// A meter document kept the way the principal directory is.
    #[derive(Debug)]
    struct MeterDoc(MetaDirectoryStore);

    fn unavailable(e: Unavailable) -> MeterError {
        MeterError::Unavailable(e.0)
    }

    #[async_trait]
    impl VersionedDoc for MeterDoc {
        async fn load(&self) -> MeterResult<Option<(u64, Vec<u8>)>> {
            Ok(self
                .0
                .load()
                .await
                .map_err(unavailable)?
                .map(|(v, b)| (v, b.to_vec())))
        }

        async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> MeterResult<Option<u64>> {
            self.0
                .save(expected, Bytes::from(doc))
                .await
                .map_err(unavailable)
        }
    }

    /// Meter audit events in the org's audit log (`AuditService`), so the
    /// console's audit page shows them. The admin event has no reason field:
    /// the reason is the target's name.
    #[derive(Debug)]
    struct OrgAudit(Arc<Directory>);

    fn actor_ref(actor: &str) -> admin::SubjectRef {
        let (kind, id) = actor.split_once(':').unwrap_or(("user", actor));
        let kind = match kind {
            "user" => admin::SubjectKind::SUBJECT_KIND_USER,
            "agent" => admin::SubjectKind::SUBJECT_KIND_AGENT,
            "service_account" => admin::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT,
            _ => admin::SubjectKind::SUBJECT_KIND_SYSTEM,
        };
        admin::SubjectRef {
            kind: kind.into(),
            id: id.to_owned(),
            name: actor.to_owned(),
            ..Default::default()
        }
    }

    #[async_trait]
    impl AuditSink for OrgAudit {
        async fn record(&self, event: AuditEvent) -> MeterResult<()> {
            let (kind, id) = event
                .object
                .split_once(':')
                .unwrap_or(("meter", event.object.as_str()));
            let record = admin::AuditEvent {
                actor: actor_ref(&event.actor).into(),
                action: event.action.clone(),
                target: admin::ResourceRef {
                    kind: kind.to_owned(),
                    id: id.to_owned(),
                    name: event.reason.clone(),
                    ..Default::default()
                }
                .into(),
                outcome: admin::AuditOutcome::AUDIT_OUTCOME_SUCCESS.into(),
                ..Default::default()
            };
            self.0
                .update(|doc| {
                    doc.record(record.clone());
                    Ok::<(), Unavailable>(())
                })
                .await
                .map_err(unavailable)
        }
    }

    fn now() -> UtcNanos {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        UtcNanos(i64::try_from(ns).unwrap_or(i64::MAX))
    }

    /// `_system/_meter` read in-process by the counter owners (§63 §11.3).
    /// Records that fail the usage profile are skipped (and logged); a read
    /// below the stream's retention starts at its log start, since what
    /// retention dropped is gone.
    struct MeterStream {
        meta: Arc<dyn MetaStore>,
        reader: loams_log::LogReader,
        id: tokio::sync::OnceCell<loams_common::StreamId>,
        missing_logged: std::sync::atomic::AtomicBool,
    }

    impl std::fmt::Debug for MeterStream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MeterStream").finish_non_exhaustive()
        }
    }

    /// Bytes one read asks for.
    const READ_BYTES: usize = 1024 * 1024;

    impl MeterStream {
        async fn id(&self) -> Result<Option<loams_common::StreamId>, OwnerError> {
            if let Some(id) = self.id.get() {
                return Ok(Some(*id));
            }
            use loams_common::meta::{Consistency, METER_STREAM, SYSTEM_NAMESPACE};
            let stream_err = |e: loams_common::meta::MetaError| OwnerError::Stream(e.to_string());
            let Some(ns) = self
                .meta
                .namespace_by_name(Consistency::Local, SYSTEM_NAMESPACE)
                .await
                .map_err(stream_err)?
            else {
                return Ok(None);
            };
            let Some(stream) = self
                .meta
                .stream_by_name(Consistency::Local, ns.id, METER_STREAM)
                .await
                .map_err(stream_err)?
            else {
                return Ok(None);
            };
            let _ = self.id.set(stream.id);
            Ok(Some(stream.id))
        }
    }

    #[async_trait]
    impl PartitionReader for MeterStream {
        async fn read(
            &self,
            partition: u32,
            from: u64,
            max: usize,
        ) -> Result<Vec<(u64, Event)>, OwnerError> {
            use loams_cloudevents::record::{RecordContext, from_record};
            use loams_common::meta::{METER_STREAM, SYSTEM_NAMESPACE};
            let Some(id) = self.id().await? else {
                if !self
                    .missing_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    tracing::warn!(
                        "the meter stream _system/_meter does not exist yet \
                         (`loams migrate run` creates it); nothing is counted"
                    );
                }
                return Ok(Vec::new());
            };
            let mut offset = from;
            loop {
                let request = loams_log::FetchRequest {
                    stream: id,
                    partition,
                    offset,
                    max_bytes: READ_BYTES,
                    max_wait: std::time::Duration::ZERO,
                };
                let response = match self.reader.fetch(request).await {
                    Ok(r) => r,
                    Err(loams_log::LogError::OffsetOutOfRange {
                        log_start_offset, ..
                    }) if offset < log_start_offset => {
                        tracing::warn!(
                            partition,
                            from = offset,
                            log_start_offset,
                            "meter stream records past retention were never counted"
                        );
                        offset = log_start_offset;
                        continue;
                    }
                    Err(e) => return Err(OwnerError::Stream(e.to_string())),
                };
                if response.records.is_empty() {
                    return Ok(Vec::new());
                }
                let mut out = Vec::new();
                for r in &response.records {
                    let ce = from_record(
                        &r.record,
                        RecordContext {
                            namespace: SYSTEM_NAMESPACE,
                            stream: METER_STREAM,
                            stream_id: id.0,
                            partition,
                            offset: r.offset,
                        },
                    );
                    if let Some(e) = own_event(&ce) {
                        out.push((r.offset, e));
                        continue;
                    }
                    match Event::from_cloudevent(&ce, Origin::Service) {
                        Ok(e) => out.push((r.offset, e)),
                        Err(err) => tracing::warn!(
                            partition,
                            offset = r.offset,
                            %err,
                            "a meter stream record is not a usage event; skipped"
                        ),
                    }
                    if out.len() >= max {
                        break;
                    }
                }
                // A batch of records none of which counts: read on, or the
                // owner would read the same batch again forever.
                if out.is_empty() {
                    offset = response.next_offset;
                    continue;
                }
                return Ok(out);
            }
        }
    }

    /// Appends the meter's own events to `_system/_meter`, keyed by
    /// subject (so each lands in the partition of its subject's owner). No
    /// idempotency ledger: their ids are deterministic, and the counter
    /// owners' recent keys and the House's `FINAL` drop a re-emission.
    struct MeterAppender {
        meta: Arc<dyn MetaStore>,
        writer: loams_log::LogWriter,
        id: tokio::sync::OnceCell<(loams_common::StreamId, u32)>,
    }

    impl std::fmt::Debug for MeterAppender {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MeterAppender").finish_non_exhaustive()
        }
    }

    impl MeterAppender {
        async fn stream(&self) -> MeterResult<(loams_common::StreamId, u32)> {
            if let Some(found) = self.id.get() {
                return Ok(*found);
            }
            use loams_common::meta::{Consistency, METER_STREAM, SYSTEM_NAMESPACE};
            let unavailable = |e: String| MeterError::Unavailable(e);
            let ns = self
                .meta
                .namespace_by_name(Consistency::Local, SYSTEM_NAMESPACE)
                .await
                .map_err(|e| unavailable(e.to_string()))?
                .ok_or_else(|| unavailable("the _system namespace does not exist yet".into()))?;
            let stream = self
                .meta
                .stream_by_name(Consistency::Local, ns.id, METER_STREAM)
                .await
                .map_err(|e| unavailable(e.to_string()))?
                .ok_or_else(|| {
                    unavailable("the _system/_meter stream does not exist yet".into())
                })?;
            let found = (stream.id, stream.partitions);
            let _ = self.id.set(found);
            Ok(found)
        }
    }

    impl MeterAppender {
        /// Appends `events`, each to its key's partition (the subject's).
        async fn append_cloudevents(
            &self,
            events: &[loams_cloudevents::CloudEvent],
        ) -> MeterResult<usize> {
            let (id, partitions) = self.stream().await?;
            let mut batches: std::collections::BTreeMap<u32, Vec<loams_log::Record>> =
                std::collections::BTreeMap::new();
            for ce in events {
                let p = loams_cloudevents::partition::partition_of(ce, partitions);
                batches
                    .entry(p)
                    .or_default()
                    .push(loams_cloudevents::record::to_record(ce));
            }
            self.writer
                .append_many(id, batches.into_iter().collect())
                .await
                .map(|_| events.len())
                .map_err(|e| MeterError::Unavailable(format!("appending meter events: {e}")))
        }
    }

    #[async_trait]
    impl MeterEventSink for MeterAppender {
        async fn append(&self, events: Vec<MeterEvent>) -> MeterResult<()> {
            let events = events
                .iter()
                .map(to_cloudevent)
                .collect::<MeterResult<Vec<_>>>()?;
            self.append_cloudevents(&events).await.map(|_| ())
        }
    }

    /// The service ingest path into `_system/_meter` (ME1 part 3): outside
    /// HTTP produces to the reserved `_system` namespace are refused, so the
    /// node agent's forwarder (and any other Loams service) appends here.
    ///
    /// `POST /internal/v1/meter/partitions/{p}/records` takes the native
    /// produce body (`{"records": [{key, value, headers, timestamp_ms}]}`,
    /// base64, D270's binary-mode layout), so the forwarder only changes its
    /// path. Every record must be a usage event (`Origin::Service`: Loams'
    /// own types allowed); one that is not refuses the whole request (400,
    /// with its index). The partition in the path is not trusted: each event
    /// goes to its subject's partition, where its counter owner reads. No
    /// idempotency ledger: a retried batch is appended again, and the
    /// counter owners' recent keys and the House's `FINAL` drop the copies
    /// (§63 §7.3).
    ///
    /// Callers: the cluster's service identity (the cluster listener), or
    /// the operator (the main port of a single node).
    pub const INGEST_PATH: &str = "/internal/v1/meter/partitions/{partition}/records";

    async fn ingest(
        axum::extract::State(appender): axum::extract::State<Arc<MeterAppender>>,
        caller: Option<axum::Extension<Caller>>,
        body: Bytes,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let admitted = matches!(
            caller.as_ref().map(|axum::Extension(c)| c),
            Some(Caller::Operator)
        ) || matches!(
            caller.as_ref().map(|axum::Extension(c)| c),
            Some(Caller::Service(id)) if id == CLUSTER_SERVICE
        );
        if !admitted {
            return crate::api::errors::ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "permission_denied",
                "only Loams services and the operator append to _system/_meter",
            )
            .into_response();
        }
        let records = match crate::api::decode_records(&body) {
            Ok(r) => r,
            Err(e) => return e.into_response(),
        };
        let mut events = Vec::with_capacity(records.len());
        for (i, record) in records.iter().enumerate() {
            let ce = loams_cloudevents::record::from_record(
                record,
                loams_cloudevents::record::RecordContext {
                    namespace: loams_common::meta::SYSTEM_NAMESPACE,
                    stream: loams_common::meta::METER_STREAM,
                    stream_id: 0,
                    partition: 0,
                    offset: 0,
                },
            );
            if let Err(err) = Event::from_cloudevent(&ce, Origin::Service) {
                return crate::api::errors::ApiError::invalid(format!(
                    "record {i} is not a usage event: {err}"
                ))
                .with("index", i)
                .into_response();
            }
            events.push(ce);
        }
        if events.is_empty() {
            return crate::api::errors::ApiError::invalid("a request carries at least one record")
                .into_response();
        }
        match appender.append_cloudevents(&events).await {
            Ok(n) => axum::Json(serde_json::json!({ "appended": n })).into_response(),
            Err(e) => crate::api::errors::ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                e.to_string(),
            )
            .into_response(),
        }
    }

    fn ingest_route(appender: Arc<MeterAppender>) -> axum::Router {
        axum::Router::new()
            .route(INGEST_PATH, axum::routing::post(ingest))
            .with_state(appender)
    }

    /// This node's meter: the role's services, the lease server and the
    /// counter runtime not yet started.
    pub struct MeterNode {
        pub role: Arc<MeterRole>,
        pub leases: Arc<LeaseServer>,
        runtime: CounterRuntime<MetaLeases>,
        clock: loams_meter::customers::Clock,
        appender: Arc<MeterAppender>,
        principals: Principals,
        counters: Arc<Counters>,
    }

    impl std::fmt::Debug for MeterNode {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MeterNode").finish_non_exhaustive()
        }
    }

    /// The running counter owners; [`MeterRuntime::stop`] gives the
    /// partitions back.
    #[derive(Debug)]
    pub struct MeterRuntime {
        stop: Arc<tokio::sync::Notify>,
        task: tokio::task::JoinHandle<()>,
        house_stop: Arc<tokio::sync::Notify>,
        house: Option<tokio::task::JoinHandle<()>>,
    }

    impl MeterRuntime {
        pub async fn stop(self) {
            self.house_stop.notify_one();
            if let Some(house) = self.house
                && let Err(err) = house.await
            {
                tracing::warn!(%err, "the meter's House schedule ended badly");
            }
            self.stop.notify_one();
            if let Err(err) = self.task.await {
                tracing::warn!(%err, "the meter's counter runtime ended badly");
            }
        }
    }

    impl MeterNode {
        /// Starts the counter runtime; answers the role, the runtime's handle,
        /// the internal routes for the cluster listener (`LeaseService`, the
        /// service ingest) and the main port's own (the ingest, for the
        /// operator of a single node).
        pub fn start(self) -> (Arc<MeterRole>, MeterRuntime, axum::Router, axum::Router) {
            let stop = Arc::new(tokio::sync::Notify::new());
            let task = tokio::spawn(self.runtime.run(stop.clone()));
            // The House schedule: schema at start, then the close job every
            // 60 s with the period close after it.
            let house_stop = Arc::new(tokio::sync::Notify::new());
            let house = self
                .role
                .house_schedule(self.clock.clone())
                .map(|schedule| tokio::spawn(schedule.run(house_stop.clone())));
            if house.is_none() {
                tracing::info!(
                    "Loams Meter runs without its House (no LOAMS_METER_HOUSE_URL): no close \
                     job, meter queries answer unavailable, every meter counts as used"
                );
            }
            (
                self.role,
                MeterRuntime {
                    stop,
                    task,
                    house_stop,
                    house,
                },
                internal_routes(self.leases, self.appender.clone(), self.counters.clone()),
                ingest_route(self.appender).layer(axum::middleware::from_fn_with_state(
                    self.principals,
                    caller,
                )),
            )
        }
    }

    /// Where a counter read forwarded to a partition's owner goes, on that
    /// owner's cluster listener.
    pub const COUNTED_PATH: &str = "/internal/v1/meter/counted";

    /// How long a forwarded counter read may take before the House answers
    /// instead.
    const PEER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    #[derive(serde::Serialize, serde::Deserialize)]
    struct CountedRequest {
        subject: String,
        feature: String,
        /// Unix nanoseconds.
        period_start: i64,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct CountedResponse {
        counted: String,
        owner_epoch: u64,
        offset: u64,
    }

    /// `POST /internal/v1/meter/counted`: one counter of a partition this
    /// node owns; 404 when it does not count it.
    fn counted_route(counters: Arc<Counters>) -> axum::Router {
        use axum::response::IntoResponse;
        axum::Router::new()
            .route(
                COUNTED_PATH,
                axum::routing::post(
                    |axum::extract::State(counters): axum::extract::State<Arc<Counters>>,
                     axum::Json(req): axum::Json<CountedRequest>| async move {
                        let (Ok(subject), Ok(feature)) = (
                            loams_meter_core::SubjectKey::new(req.subject),
                            loams_meter_core::FeatureKey::new(req.feature),
                        ) else {
                            return axum::http::StatusCode::BAD_REQUEST.into_response();
                        };
                        let key = loams_meter::owner::CounterKey {
                            subject,
                            feature,
                            period_start: UtcNanos(req.period_start),
                        };
                        match counters.counted(&key).await {
                            Some((
                                counted,
                                loams_meter_core::AsOf::Counter {
                                    owner_epoch,
                                    offset,
                                },
                            )) => axum::Json(CountedResponse {
                                counted: counted.to_string(),
                                owner_epoch,
                                offset,
                            })
                            .into_response(),
                            _ => axum::http::StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                ),
            )
            .with_state(counters)
    }

    /// Reads a counter on its owner over the owner's cluster listener.
    #[derive(Debug)]
    struct HttpPeer(reqwest::Client);

    impl HttpPeer {
        fn new() -> Result<Self, String> {
            reqwest::Client::builder()
                .connect_timeout(PEER_TIMEOUT)
                .timeout(PEER_TIMEOUT)
                .build()
                .map(HttpPeer)
                .map_err(|e| format!("the meter's peer client: {e}"))
        }
    }

    #[async_trait]
    impl CounterPeer for HttpPeer {
        async fn counted(
            &self,
            addr: &str,
            key: &loams_meter::owner::CounterKey,
        ) -> MeterResult<Option<(loams_meter_core::Amount, loams_meter_core::AsOf)>> {
            let unavailable = |e: String| MeterError::Unavailable(e);
            let response = self
                .0
                .post(format!("http://{addr}{COUNTED_PATH}"))
                .json(&CountedRequest {
                    subject: key.subject.to_string(),
                    feature: key.feature.to_string(),
                    period_start: key.period_start.0,
                })
                .send()
                .await
                .map_err(|e| unavailable(e.to_string()))?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(unavailable(format!(
                    "the owner answered {}",
                    response.status()
                )));
            }
            let body: CountedResponse = response
                .json()
                .await
                .map_err(|e| unavailable(e.to_string()))?;
            let counted = loams_meter_core::Amount::parse(&body.counted)
                .map_err(|e| unavailable(e.to_string()))?;
            Ok(Some((
                counted,
                loams_meter_core::AsOf::Counter {
                    owner_epoch: body.owner_epoch,
                    offset: body.offset,
                },
            )))
        }
    }

    /// `loams.meter.internal.v1.LeaseService` for the cluster listener. A
    /// caller there is a product role on another node: the cluster's service
    /// identity ([`CLUSTER_SERVICE`]), admitted by the network as every route
    /// of that listener is in M1.
    fn internal_routes(
        leases: Arc<LeaseServer>,
        appender: Arc<MeterAppender>,
        counters: Arc<Counters>,
    ) -> axum::Router {
        let rpc = loams_meter::connect::register_internal(connectrpc::Router::new(), leases);
        let paths: Vec<String> = rpc.methods().map(|path| format!("/{path}")).collect();
        let service = rpc.into_axum_service();
        let mut router = ingest_route(appender).merge(counted_route(counters));
        for path in paths {
            router = router.route_service(&path, service.clone());
        }
        router.layer(axum::middleware::from_fn(
            |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                request
                    .extensions_mut()
                    .insert(Caller::Service(CLUSTER_SERVICE.to_string()));
                next.run(request).await
            },
        ))
    }

    /// The meter of this node, over the metastore and the bucket: the
    /// role's services, and the counter owners over `_system/_meter`.
    pub fn build(
        meta: Arc<dyn MetaStore>,
        store: Store,
        reader: loams_log::LogReader,
        node_id: u64,
        principals: &Principals,
        writer: loams_log::LogWriter,
        config: &super::MeterConfig,
        data_dir: &std::path::Path,
        advertise: Option<String>,
    ) -> Result<MeterNode, String> {
        let key_file = config
            .secrets_key_file
            .clone()
            .unwrap_or_else(|| data_dir.join("meter").join("secrets.key"));
        let sealer = AgeSealer::from_key_file(&key_file).map_err(|e| e.to_string())?;
        let house = Arc::new(match &config.house_url {
            None => RoleHouse::Absent,
            Some(url) => {
                let password = match &config.house_password_file {
                    Some(path) => std::fs::read_to_string(path)
                        .map_err(|e| {
                            format!("LOAMS_METER_HOUSE_PASSWORD_FILE {}: {e}", path.display())
                        })?
                        .trim_end_matches(['\r', '\n'])
                        .to_string(),
                    None => String::new(),
                };
                RoleHouse::Http(
                    loams_meter::house::HttpHouse::new(
                        url.clone(),
                        config.house_user.clone(),
                        password,
                    )
                    .map_err(|e| e.to_string())?,
                )
            }
        });
        let records: Arc<dyn VersionedDoc> = Arc::new(MeterDoc(MetaDirectoryStore::with_key(
            meta.clone(),
            store.clone(),
            RECORDS_POINTER,
            RECORDS_PREFIX,
        )));
        let notes = MeterDoc(MetaDirectoryStore::with_key(
            meta.clone(),
            store,
            NOTIFY_POINTER,
            NOTIFY_PREFIX,
        ));
        let transport = HttpTransport::new().map_err(|e| e.to_string())?;
        let clock: loams_meter::customers::Clock = Arc::new(now);
        let counters = Counters::new(METER_PARTITIONS);
        let leases = Arc::new(LeaseServer::new(
            METER_PARTITIONS,
            AllocatorConfig::default(),
            Arc::new(CatalogInputs(counters.clone())),
            vec![CLUSTER_SERVICE.to_string()],
            clock.clone(),
        ));
        let appender = Arc::new(MeterAppender {
            meta: meta.clone(),
            writer,
            id: tokio::sync::OnceCell::new(),
        });
        let meter_store: Arc<dyn MeterStore> = Arc::new(DocStore::new(records.clone()));
        // Balances: this node's counters, else the owner's (over its cluster
        // listener), else the House.
        let mut values = CounterValues::new(counters.clone()).with_owners(
            Arc::new(MetaLeases(meta.clone())),
            Arc::new(HttpPeer::new()?),
        );
        if house.is_configured() {
            values = values.with_house(Arc::new(HouseUsageSoFar {
                house: house.clone(),
                store: meter_store.clone(),
            }));
        }
        let usage: Arc<dyn MeterUsage> = if house.is_configured() {
            Arc::new(HouseUsage {
                house: house.clone(),
                store: meter_store,
            })
        } else {
            Arc::new(NoHouseYet)
        };
        let role = Arc::new(MeterRole::new(RoleParts {
            records: records.clone(),
            notifications: Arc::new(DocNotifications::new(Arc::new(notes))),
            authorizer: principals.authz.clone(),
            audit: Arc::new(OrgAudit(principals.directory.clone())),
            transport: Arc::new(transport),
            // No deployment mailer or console channel is wired yet: webhooks
            // only.
            mailer: None,
            console: None,
            random: Arc::new(|| {
                let mut b = [0u8; 32];
                getrandom::fill(&mut b)
                    .unwrap_or_else(|e| panic!("OS randomness unavailable: {e}"));
                b
            }),
            clock: clock.clone(),
            values: Arc::new(values),
            usage,
            house,
            sealer: Arc::new(sealer),
        }));
        let runtime = CounterRuntime::new(RuntimeParts {
            leases: MetaLeases(meta.clone()),
            me: holder_name(
                node_id,
                &ulid::Ulid::generate().to_string(),
                advertise.as_deref(),
            ),
            reader: Arc::new(MeterStream {
                meta: meta.clone(),
                reader,
                id: tokio::sync::OnceCell::new(),
                missing_logged: std::sync::atomic::AtomicBool::new(false),
            }),
            // The House pipe's cursor is not readable over SQL (D2203): the
            // counters rebuild from the stream alone.
            house: Arc::new(StreamOnly),
            store: Arc::new(DocStore::new(records.clone())) as Arc<dyn MeterStore>,
            counters: counters.clone(),
            lease_server: leases.clone(),
            clock: clock.clone(),
            events: Some(appender.clone() as Arc<dyn MeterEventSink>),
            notifier: Some(Arc::new(role.notifier.clone()) as Arc<dyn MeterEventHandler>),
        });
        Ok(MeterNode {
            role,
            leases,
            runtime,
            clock,
            appender,
            principals: principals.clone(),
            counters,
        })
    }

    /// Gives a `/loams.meter.v1.*` call, and an OpenMeter-compatible
    /// `/api/v1/meters/*` or `/api/v1/subjects/*` one, its [`Caller`]: the operator's own
    /// credentials are [`Caller::Operator`]; anyone the org knows is the
    /// subject the authorizer answers for; anyone else has none and is
    /// refused as unauthenticated by the service.
    pub async fn caller(
        axum::extract::State(principals): axum::extract::State<Principals>,
        mut request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let path = request.uri().path();
        let meter_path = path.starts_with("/loams.meter.v1.")
            || path.starts_with("/internal/v1/meter/")
            || path.starts_with("/api/v1/meters/")
            || path.starts_with("/api/v1/subjects/");
        if meter_path && let Some(principal) = request.extensions().get::<crate::auth::Principal>()
        {
            match standing(&principals, principal).await {
                Ok(Standing::Operator) => {
                    request.extensions_mut().insert(Caller::Operator);
                }
                Ok(Standing::Subject(subject)) => {
                    request.extensions_mut().insert(Caller::Principal(subject));
                }
                Ok(Standing::Outside) => {}
                Err(err) => {
                    tracing::warn!(?err, "the meter could not resolve its caller");
                }
            }
        }
        next.run(request).await
    }
}
