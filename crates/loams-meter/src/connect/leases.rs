//! `loams.meter.internal.v1.LeaseService` over [`LeaseServer`].

use std::sync::Arc;

use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_meter_core::{Amount, FeatureKey, LeaseId, LeaseReport, SubjectKey};
use loams_proto::loams::meter::internal::v1 as pb;
use loams_proto::loams::meter::internal::v1::{LeaseService, LeaseServiceExt};

use super::{IntoConnect, amount_in, caller, timestamp};
use crate::error::MeterError;
use crate::lease_service::LeaseServer;

#[derive(Debug)]
struct Adapter(Arc<LeaseServer>);

pub fn register(router: connectrpc::Router, server: Arc<LeaseServer>) -> connectrpc::Router {
    LeaseServiceExt::register(Arc::new(Adapter(server)), router)
}

fn ids(subject: &str, feature: &str) -> Result<(SubjectKey, FeatureKey), MeterError> {
    Ok((
        SubjectKey::new(subject).map_err(|e| MeterError::invalid("subject", e.to_string()))?,
        FeatureKey::new(feature).map_err(|e| MeterError::invalid("feature", e.to_string()))?,
    ))
}

fn lease_id(s: &str, field: &str) -> Result<LeaseId, MeterError> {
    LeaseId::new(s).map_err(|e| MeterError::invalid(field, e.to_string()))
}

fn report_in(r: &pb::LeaseReport) -> Result<LeaseReport, MeterError> {
    Ok(LeaseReport {
        lease: lease_id(&r.lease_id, "held.lease_id")?,
        granted: amount_in(&r.granted, "held.granted")?.unwrap_or(Amount::ZERO),
        spent: amount_in(&r.spent, "held.spent")?.unwrap_or(Amount::ZERO),
    })
}

impl LeaseService for Adapter {
    async fn acquire_lease(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::AcquireLeaseRequest>,
    ) -> ServiceResult<pb::AcquireLeaseResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (subject, feature) = ids(&r.subject, &r.feature).connect()?;
        let want = amount_in(&r.want, "want")
            .connect()?
            .unwrap_or(Amount::ZERO);
        let held = r
            .held
            .iter()
            .map(report_in)
            .collect::<Result<Vec<_>, _>>()
            .connect()?;
        let g = self
            .0
            .acquire(&caller, subject, feature, r.node, want, held, r.known_epoch)
            .await
            .connect()?;
        let grant = g.grant;
        Response::ok(pb::AcquireLeaseResponse {
            lease: pb::Lease {
                lease_id: grant.id.map(|i| i.to_string()).unwrap_or_default(),
                amount: grant.amount.to_string(),
                has_access: grant.has_access,
                expire_time: timestamp(grant.expires_at).into(),
                refresh_time: timestamp(grant.refresh_after).into(),
                owner_epoch: grant.owner_epoch,
                partition: g.partition,
                balance_hint: grant.balance_hint.to_string(),
                exact: grant.exact,
                refusal_reason: grant
                    .refusal
                    .map(|r| r.reason().to_string())
                    .unwrap_or_default(),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        })
    }

    async fn return_lease(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ReturnLeaseRequest>,
    ) -> ServiceResult<pb::ReturnLeaseResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (subject, feature) = ids(&r.subject, &r.feature).connect()?;
        let lease = lease_id(&r.lease_id, "lease_id").connect()?;
        let spent = amount_in(&r.spent, "spent")
            .connect()?
            .unwrap_or(Amount::ZERO);
        self.0
            .return_lease(&caller, &subject, &feature, &lease, spent, r.owner_epoch)
            .connect()?;
        Response::ok(pb::ReturnLeaseResponse::default())
    }
}
