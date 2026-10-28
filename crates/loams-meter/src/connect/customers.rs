//! `loams.meter.v1.CustomerService` over [`Customers`].

use std::sync::Arc;

use buffa::enumeration::EnumValue;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_proto::loams::meter::v1 as pb;
use loams_proto::loams::meter::v1::{CustomerService, CustomerServiceExt};

use super::{IntoConnect, caller, money_in, money_out, nonempty, page_size, timestamp, to_connect};
use crate::customers::{CustomerChanges, Customers, NewCustomer};
use crate::error::MeterError;
use crate::store::{Customer, PaymentState, SpendControls};

#[derive(Debug)]
struct Adapter(Customers);

pub fn register(router: connectrpc::Router, customers: Customers) -> connectrpc::Router {
    CustomerServiceExt::register(Arc::new(Adapter(customers)), router)
}

fn state_out(s: PaymentState) -> pb::PaymentState {
    match s {
        PaymentState::None => pb::PaymentState::PAYMENT_STATE_NONE,
        PaymentState::Card => pb::PaymentState::PAYMENT_STATE_CARD,
        PaymentState::Invoice => pb::PaymentState::PAYMENT_STATE_INVOICE,
    }
}

fn state_in(s: &EnumValue<pb::PaymentState>) -> Result<PaymentState, MeterError> {
    match s.as_known() {
        Some(pb::PaymentState::PAYMENT_STATE_NONE) => Ok(PaymentState::None),
        Some(pb::PaymentState::PAYMENT_STATE_CARD) => Ok(PaymentState::Card),
        Some(pb::PaymentState::PAYMENT_STATE_INVOICE) => Ok(PaymentState::Invoice),
        _ => Err(MeterError::invalid(
            "payment_state",
            "a payment state is required",
        )),
    }
}

fn controls_out(c: &SpendControls) -> pb::SpendControls {
    pb::SpendControls {
        overage_enabled: c.overage_enabled,
        spend_cap: c.spend_cap.as_ref().map(money_out).into(),
        auto_top_up: c.auto_top_up,
        top_up_amount: c.top_up_amount.as_ref().map(money_out).into(),
        top_up_threshold: c.top_up_threshold.as_ref().map(money_out).into(),
        ..Default::default()
    }
}

fn controls_in(c: &pb::SpendControls) -> Result<SpendControls, MeterError> {
    Ok(SpendControls {
        overage_enabled: c.overage_enabled,
        spend_cap: c
            .spend_cap
            .as_option()
            .map(|m| money_in(m, "spend_controls.spend_cap"))
            .transpose()?,
        auto_top_up: c.auto_top_up,
        top_up_amount: c
            .top_up_amount
            .as_option()
            .map(|m| money_in(m, "spend_controls.top_up_amount"))
            .transpose()?,
        top_up_threshold: c
            .top_up_threshold
            .as_option()
            .map(|m| money_in(m, "spend_controls.top_up_threshold"))
            .transpose()?,
    })
}

/// A customer as the API answers it.
pub fn customer_out(c: &Customer) -> pb::Customer {
    pb::Customer {
        id: c.id.clone(),
        org: c.org.clone(),
        display_name: c.display_name.clone(),
        subjects: c.subjects.iter().map(|s| s.as_str().to_string()).collect(),
        payment_state: EnumValue::Known(state_out(c.payment_state)),
        spend_controls: controls_out(&c.spend_controls).into(),
        stripe_customer_id: c.stripe_customer_id.clone().unwrap_or_default(),
        currency: c.currency.as_str().to_string(),
        plan: c
            .plan
            .as_ref()
            .map(|(p, _)| p.as_str().to_string())
            .unwrap_or_default(),
        plan_version: c.plan.as_ref().map_or(0, |(_, v)| *v),
        create_time: timestamp(c.created_at).into(),
        update_time: timestamp(c.updated_at).into(),
        ..Default::default()
    }
}

impl CustomerService for Adapter {
    async fn create_customer(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateCustomerRequest>,
    ) -> ServiceResult<pb::CreateCustomerResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let c = self
            .0
            .create(
                &caller,
                NewCustomer {
                    org: r.org,
                    display_name: r.display_name,
                    subjects: r.subjects,
                    currency: r.currency,
                    stripe_customer_id: nonempty(&r.stripe_customer_id).map(str::to_string),
                    idempotency_key: nonempty(&r.idempotency_key).map(str::to_string),
                },
            )
            .await
            .connect()?;
        Response::ok(pb::CreateCustomerResponse {
            customer: customer_out(&c).into(),
            ..Default::default()
        })
    }

    async fn get_customer(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetCustomerRequest>,
    ) -> ServiceResult<pb::GetCustomerResponse> {
        let caller = caller(&ctx)?;
        let c = self.0.get(&caller, request.id).await.connect()?;
        Response::ok(pb::GetCustomerResponse {
            customer: customer_out(&c).into(),
            ..Default::default()
        })
    }

    async fn list_customers(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListCustomersRequest>,
    ) -> ServiceResult<pb::ListCustomersResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (page, next) = self
            .0
            .list(
                &caller,
                nonempty(&r.org),
                nonempty(&r.page_token),
                page_size(r.page_size),
            )
            .await
            .connect()?;
        Response::ok(pb::ListCustomersResponse {
            customers: page.iter().map(customer_out).collect(),
            next_page_token: next.unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn update_customer(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateCustomerRequest>,
    ) -> ServiceResult<pb::UpdateCustomerResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let given = r.customer.as_option().cloned().unwrap_or_default();
        let paths: Vec<String> = r
            .update_mask
            .as_option()
            .map(|m| m.paths.clone())
            .unwrap_or_default();
        if paths.is_empty() {
            return Err(to_connect(MeterError::invalid(
                "update_mask",
                "name the fields to change",
            )));
        }
        let mut changes = CustomerChanges::default();
        for path in &paths {
            match path.as_str() {
                "display_name" | "displayName" => {
                    changes.display_name = Some(given.display_name.clone());
                }
                "subjects" => changes.subjects = Some(given.subjects.clone()),
                "stripe_customer_id" | "stripeCustomerId" => {
                    changes.stripe_customer_id =
                        Some(nonempty(&given.stripe_customer_id).map(str::to_string));
                }
                other => {
                    return Err(to_connect(MeterError::invalid(
                        "update_mask",
                        format!("{other} cannot be changed here"),
                    )));
                }
            }
        }
        let c = self
            .0
            .update(&caller, &r.id, changes, &r.reason)
            .await
            .connect()?;
        Response::ok(pb::UpdateCustomerResponse {
            customer: customer_out(&c).into(),
            ..Default::default()
        })
    }

    async fn set_payment_state(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetPaymentStateRequest>,
    ) -> ServiceResult<pb::SetPaymentStateResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let state = state_in(&r.payment_state).connect()?;
        let c = self
            .0
            .set_payment_state(
                &caller,
                nonempty(&r.id),
                nonempty(&r.stripe_customer_id),
                state,
                &r.reason,
            )
            .await
            .connect()?;
        Response::ok(pb::SetPaymentStateResponse {
            customer: customer_out(&c).into(),
            ..Default::default()
        })
    }

    async fn set_spend_controls(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetSpendControlsRequest>,
    ) -> ServiceResult<pb::SetSpendControlsResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let controls =
            controls_in(&r.spend_controls.as_option().cloned().unwrap_or_default()).connect()?;
        let c = self
            .0
            .set_spend_controls(&caller, &r.id, controls, &r.reason)
            .await
            .connect()?;
        Response::ok(pb::SetSpendControlsResponse {
            customer: customer_out(&c).into(),
            ..Default::default()
        })
    }
}
