//! The service implementations. Each is a thin layer over [`Store`]; the
//! decision rules live in [`crate::acceptance`].

// The generated traits return `impl Encodable<_>`; these impls name the
// concrete message type, which is the intended refinement.
#![allow(refining_impl_trait)]

mod approvals;
mod devices;
mod watch;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buffa::MessageField;
use connectrpc::{
    ConnectError, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use rand::Rng as _;

use crate::auth::caller;
use crate::not_implemented;
use crate::oauth::AUTHENTIK;
use crate::proto::loams::approvals::v1::{Approval, ApprovalPolicy, ApprovalState, Risk, StepUp};
use crate::proto::loams::devices::v1::NotificationCategory;
use crate::proto::loams::instance::v1::{
    DeviceRef, GetInstanceRequest, GetInstanceResponse, InstanceService, PrincipalKind,
    WhoAmIRequest, WhoAmIResponse,
};
use crate::proto::loams::notifications::v1::__buffa::oneof::notification::Ref;
use crate::proto::loams::notifications::v1::Notification;

use crate::proto::loams::notifications::v1::__buffa::oneof::watch_notifications_response::Event as NotificationEvent;
use crate::proto::loams::notifications::v1::{
    ListNotificationsRequest, ListNotificationsResponse, MarkReadRequest, MarkReadResponse,
    NotificationService, NotificationSnapshot, WatchNotificationsRequest,
    WatchNotificationsResponse,
};
use crate::proto::loams::operations::v1::__buffa::oneof::watch_operations_response::Event as OperationEvent;
use crate::proto::loams::operations::v1::{
    CancelOperationRequest, CancelOperationResponse, GetOperationRequest, GetOperationResponse,
    ListOperationsRequest, ListOperationsResponse, Operation, OperationSnapshot, OperationState,
    OperationsService, WatchOperationsRequest, WatchOperationsResponse,
};
use crate::seed::ts;
use crate::store::Store;

pub(crate) use approvals::Approvals;
pub(crate) use devices::Devices;

/// Creates a pending approval, its awaiting operation and its notification, as
/// an agent asking for something would, and returns the approval's id.
///
/// The mock's test control `POST /mock/approvals` uses this so a phone can be
/// made to receive a real approval through the same state the RPCs read.
pub(crate) fn new_demo_approval(store: &Arc<Store>) -> Option<String> {
    let seed = &store.seed;
    let requester = seed
        .principals
        .iter()
        .find(|p| p.kind.as_known() == Some(PrincipalKind::PRINCIPAL_KIND_USER))
        .cloned()?;
    let agent = seed
        .principals
        .iter()
        .find(|p| p.kind.as_known() == Some(PrincipalKind::PRINCIPAL_KIND_AGENT))
        .cloned()
        .unwrap_or_else(|| requester.clone());
    let environment = seed.environments.first()?;
    let now = SystemTime::now();
    let mut rng = rand::rng();
    let id = format!("apr_{}", ulid::Ulid::from(rng.random::<u128>()));
    let operation_id = format!("op_{}", &id[4..]);
    let summary = format!("{}: review the seeded namespace", requester.display_name);
    let approval = Approval {
        id: id.clone(),
        revision: 1,
        operation_id: operation_id.clone(),
        promise_id: format!("prm_{}", &id[4..]),
        kind: "namespace.erasure".into(),
        environment: MessageField::some(environment.clone()),
        requested_by: MessageField::some(agent.clone()),
        actor_chain: vec![agent, requester.clone()],
        summary: summary.clone(),
        detail_lines: vec![
            "Created by POST /mock/approvals.".into(),
            format!(
                "Requested at epoch {}",
                now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
            ),
        ],
        target: [("namespace".to_owned(), environment.namespace.clone())]
            .into_iter()
            .collect(),
        risk: Risk::RISK_HIGH.into(),
        policy: MessageField::some(ApprovalPolicy {
            required_approvals: 1,
            step_up: StepUp::STEP_UP_DEVICE.into(),
            ..Default::default()
        }),
        state: ApprovalState::APPROVAL_STATE_PENDING.into(),
        created_at: ts(now),
        expires_at: ts(now + Duration::from_secs(72 * 3600)),
        ..Default::default()
    };
    let operation = Operation {
        id: operation_id,
        kind: "namespace.erasure".into(),
        namespace: environment.namespace.clone(),
        state: OperationState::OPERATION_STATE_AWAITING_APPROVAL.into(),
        approval_id: id.clone(),
        created_at: ts(now),
        updated_at: ts(now),
        ..Default::default()
    };
    let notification = Notification {
        id: format!("01J9ZN{}", &id[4..]),
        category: NotificationCategory::NOTIFICATION_CATEGORY_APPROVALS.into(),
        cloudevent_type: "io.loams.dev.approval.requested.v1".into(),
        subject: id.clone(),
        title: "Approval needed".into(),
        body: summary,
        r#ref: Some(Ref::ApprovalId(id.clone())),
        environment: environment.id.clone(),
        created_at: ts(now),
        ..Default::default()
    };
    let mut state = store.lock();
    state.operations.insert(operation.id.clone(), operation);
    state
        .notifications
        .insert(notification.id.clone(), notification);
    let change = state.record(approval);
    drop(state);
    // Broadcast after the lock, as the RPC paths do.
    let _ = store.changes.send(change);
    Some(id)
}

/// `loams.instance.v1.InstanceService`.
pub(crate) struct Instance(pub(crate) Arc<Store>);

impl InstanceService for Instance {
    async fn get_instance(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, GetInstanceRequest>,
    ) -> ServiceResult<GetInstanceResponse> {
        let mut instance = self.0.seed.instance.clone();
        // The seed carries `.invalid` placeholders. A client has to be able to
        // follow these, so point them at the mock it is talking to: the address
        // it dialled, or `--public-url` when the phone reaches it by another name.
        if let Some(base) = crate::oauth::public_base(&self.0, &ctx) {
            instance.issuer = base.clone();
            instance.jwks_uri = format!("{base}/.well-known/jwks.json");
            for method in &mut instance.sign_in_methods {
                method.issuer = format!("{base}{AUTHENTIK}");
            }
        }
        Response::ok(instance)
    }

    async fn who_am_i(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, WhoAmIRequest>,
    ) -> ServiceResult<WhoAmIResponse> {
        let seed = &self.0.seed;
        let me = caller(&self.0, &ctx)?;
        let mut actor_chain = Vec::new();
        let mut principal = me.principal.clone();
        if let Some(user) = seed
            .acts_for(&me.principal.id)
            .and_then(|u| seed.principal(u))
        {
            actor_chain.push(me.principal.clone());
            principal = user;
        }
        // The device the token was issued for, when it was one this mock issued;
        // otherwise any of the *token owner's* devices, so a hand-written
        // `mock-access-<principal>` token still names one. Filtering on the owner
        // matters: devices live in one flat map, so ignoring it would let a
        // principal be attributed whichever device happened to sort first.
        let device = {
            let state = self.0.lock();
            let owner = me.principal.id.clone();
            let id = me.device_id.clone().or_else(|| {
                state
                    .devices
                    .iter()
                    .find(|(_, (device_owner, device))| {
                        device_owner == &owner && !device.revoked_at.is_set()
                    })
                    .map(|(id, _)| id.clone())
            });
            id.and_then(|id| {
                state.devices.get(&id).map(|(_, device)| DeviceRef {
                    id: device.id.clone(),
                    name: device.name.clone(),
                    ..Default::default()
                })
            })
        };
        Response::ok(WhoAmIResponse {
            principal: principal.into(),
            actor_chain,
            org: seed.org.clone().into(),
            environments: seed.environments.clone(),
            device: device.map(Into::into).unwrap_or_default(),
            authenticated_at: ts(me.authenticated_at),
            ..Default::default()
        })
    }
}

/// `loams.operations.v1.OperationsService`.
pub(crate) struct Operations(pub(crate) Arc<Store>);

impl OperationsService for Operations {
    async fn get_operation(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetOperationRequest>,
    ) -> ServiceResult<GetOperationResponse> {
        caller(&self.0, &ctx)?;
        let id = request.operation_id.to_owned();
        let operation = self.0.lock().operations.get(&id).cloned();
        let operation =
            operation.ok_or_else(|| ConnectError::not_found(format!("no operation `{id}`")))?;
        Response::ok(GetOperationResponse {
            operation: operation.into(),
            ..Default::default()
        })
    }

    async fn list_operations(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListOperationsRequest>,
    ) -> ServiceResult<ListOperationsResponse> {
        caller(&self.0, &ctx)?;
        let request = request.to_owned_message();
        let operations = self
            .0
            .lock()
            .operations
            .values()
            .filter(|o| request.namespace.is_empty() || o.namespace == request.namespace)
            .filter(|o| request.states.is_empty() || request.states.contains(&o.state))
            .cloned()
            .collect();
        Response::ok(ListOperationsResponse {
            operations,
            ..Default::default()
        })
    }

    async fn watch_operations(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, WatchOperationsRequest>,
    ) -> ServiceResult<ServiceStream<WatchOperationsResponse>> {
        caller(&self.0, &ctx)?;
        let operations = self.0.lock().operations.values().cloned().collect();
        // Changes to operations are not simulated yet (scenarios, AP0 Ruling 9):
        // the stream is a snapshot followed by heartbeats.
        let first = WatchOperationsResponse {
            event: Some(OperationEvent::Snapshot(Box::new(OperationSnapshot {
                operations,
                ..Default::default()
            }))),
            cursor: "c0".into(),
            ..Default::default()
        };
        Response::stream_ok(watch::snapshot_then_heartbeats(
            first,
            self.0.heartbeat,
            || WatchOperationsResponse {
                event: Some(OperationEvent::Heartbeat(Box::default())),
                cursor: "c0".into(),
                ..Default::default()
            },
        ))
    }

    async fn cancel_operation(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, CancelOperationRequest>,
    ) -> ServiceResult<CancelOperationResponse> {
        caller(&self.0, &ctx)?;
        Err(not_implemented("CancelOperation"))
    }
}

/// `loams.notifications.v1.NotificationService`.
pub(crate) struct Notifications(pub(crate) Arc<Store>);

impl NotificationService for Notifications {
    async fn list_notifications(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListNotificationsRequest>,
    ) -> ServiceResult<ListNotificationsResponse> {
        caller(&self.0, &ctx)?;
        let unread_only = request.unread_only;
        let notifications = self
            .0
            .lock()
            .notifications
            .values()
            .rev()
            .filter(|n| !unread_only || !n.read_at.is_set())
            .cloned()
            .collect();
        Response::ok(ListNotificationsResponse {
            notifications,
            ..Default::default()
        })
    }

    async fn watch_notifications(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, WatchNotificationsRequest>,
    ) -> ServiceResult<ServiceStream<WatchNotificationsResponse>> {
        caller(&self.0, &ctx)?;
        let notifications = self
            .0
            .lock()
            .notifications
            .values()
            .filter(|n| !n.read_at.is_set())
            .cloned()
            .collect();
        let first = WatchNotificationsResponse {
            event: Some(NotificationEvent::Snapshot(Box::new(
                NotificationSnapshot {
                    notifications,
                    ..Default::default()
                },
            ))),
            cursor: "c0".into(),
            ..Default::default()
        };
        Response::stream_ok(watch::snapshot_then_heartbeats(
            first,
            self.0.heartbeat,
            || WatchNotificationsResponse {
                event: Some(NotificationEvent::Heartbeat(Box::default())),
                cursor: "c0".into(),
                ..Default::default()
            },
        ))
    }

    async fn mark_read(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, MarkReadRequest>,
    ) -> ServiceResult<MarkReadResponse> {
        caller(&self.0, &ctx)?;
        let request = request.to_owned_message();
        let now = ts(std::time::SystemTime::now());
        let mut state = self.0.lock();
        let mut marked = 0u32;
        for n in state.notifications.values_mut() {
            if !n.read_at.is_set() && (request.all || request.notification_ids.contains(&n.id)) {
                n.read_at = now.clone();
                marked += 1;
            }
        }
        Response::ok(MarkReadResponse {
            marked,
            ..Default::default()
        })
    }
}
