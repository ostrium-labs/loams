//! `loams.meter.v1.PlanService` over [`Plans`].

use std::sync::Arc;

use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_proto::loams::meter::v1 as pb;
use loams_proto::loams::meter::v1::{PlanService, PlanServiceExt};

use super::customers::customer_out;
use super::{IntoConnect, caller, nonempty, timestamp};
use crate::plans::Plans;
use crate::store::PlanRecord;

#[derive(Debug)]
struct Adapter(Plans);

pub fn register(router: connectrpc::Router, plans: Plans) -> connectrpc::Router {
    PlanServiceExt::register(Arc::new(Adapter(plans)), router)
}

fn plan_out(p: &PlanRecord) -> pb::Plan {
    let features = crate::plan_yaml::parse(&p.yaml)
        .map(|parsed| {
            parsed
                .templates
                .iter()
                .map(|t| t.feature.to_string())
                .collect()
        })
        .unwrap_or_default();
    pb::Plan {
        name: p.name.to_string(),
        version: p.version,
        yaml: p.yaml.clone(),
        features,
        digest: p.digest.clone(),
        create_time: timestamp(p.created_at).into(),
        ..Default::default()
    }
}

impl PlanService for Adapter {
    async fn apply_plan(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ApplyPlanRequest>,
    ) -> ServiceResult<pb::ApplyPlanResponse> {
        let caller = caller(&ctx)?;
        let applied = self
            .0
            .apply(&caller, request.yaml, request.reason)
            .await
            .connect()?;
        Response::ok(pb::ApplyPlanResponse {
            plan: plan_out(&applied.plan).into(),
            changed: applied.changed,
            ..Default::default()
        })
    }

    async fn list_plans(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListPlansRequest>,
    ) -> ServiceResult<pb::ListPlansResponse> {
        let caller = caller(&ctx)?;
        let plans = self
            .0
            .list(&caller, nonempty(request.name), request.all_versions)
            .await
            .connect()?;
        Response::ok(pb::ListPlansResponse {
            plans: plans.iter().map(plan_out).collect(),
            ..Default::default()
        })
    }

    async fn assign_plan(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::AssignPlanRequest>,
    ) -> ServiceResult<pb::AssignPlanResponse> {
        let caller = caller(&ctx)?;
        let (customer, changed) = self
            .0
            .assign(
                &caller,
                request.customer_id,
                request.plan,
                request.version,
                request.reason,
            )
            .await
            .connect()?;
        Response::ok(pb::AssignPlanResponse {
            customer: customer_out(&customer).into(),
            entitlements_changed: changed,
            ..Default::default()
        })
    }

    async fn unassign_plan(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UnassignPlanRequest>,
    ) -> ServiceResult<pb::UnassignPlanResponse> {
        let caller = caller(&ctx)?;
        let customer = self
            .0
            .unassign(&caller, request.customer_id, request.reason)
            .await
            .connect()?;
        Response::ok(pb::UnassignPlanResponse {
            customer: customer_out(&customer).into(),
            ..Default::default()
        })
    }
}
