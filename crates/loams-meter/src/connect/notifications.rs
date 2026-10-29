//! `loams.meter.v1.NotificationService` over [`Notifier`].

use std::sync::Arc;

use buffa::enumeration::EnumValue;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_proto::loams::meter::v1 as pb;
use loams_proto::loams::meter::v1::{NotificationService, NotificationServiceExt};

use super::{IntoConnect, caller, nonempty, page_size, timestamp};
use crate::notify::{Channel, Delivery, DeliveryStatus, Endpoint, Notifier, Rule};

#[derive(Debug)]
struct Adapter(Notifier);

pub fn register(router: connectrpc::Router, notifier: Notifier) -> connectrpc::Router {
    NotificationServiceExt::register(Arc::new(Adapter(notifier)), router)
}

fn channel_out(c: Channel) -> pb::NotificationChannel {
    match c {
        Channel::Email => pb::NotificationChannel::NOTIFICATION_CHANNEL_EMAIL,
        Channel::Console => pb::NotificationChannel::NOTIFICATION_CHANNEL_CONSOLE,
    }
}

fn rule_out(r: &Rule) -> pb::Rule {
    pb::Rule {
        id: r.id.clone(),
        customer_id: r.customer_id.clone(),
        feature: r
            .feature
            .as_ref()
            .map(|f| f.to_string())
            .unwrap_or_default(),
        thresholds: r.thresholds.clone(),
        spend_cap: r.spend_cap,
        channels: r
            .channels
            .iter()
            .map(|c| EnumValue::Known(channel_out(*c)))
            .collect(),
        ..Default::default()
    }
}

fn webhook_out(e: &Endpoint) -> pb::Webhook {
    pb::Webhook {
        id: e.id.clone(),
        customer_id: e.customer_id.clone(),
        url: e.url.clone(),
        types: e.types.clone(),
        create_time: timestamp(e.created_at).into(),
        ..Default::default()
    }
}

fn delivery_out(d: &Delivery) -> pb::Delivery {
    pb::Delivery {
        webhook_id: d.endpoint_id.clone(),
        event_id: d.event_id.clone(),
        event_type: d.event_type.clone(),
        time: timestamp(d.at).into(),
        status: EnumValue::Known(match d.status {
            DeliveryStatus::Delivered => pb::DeliveryStatus::DELIVERY_STATUS_DELIVERED,
            DeliveryStatus::Retrying => pb::DeliveryStatus::DELIVERY_STATUS_RETRYING,
            DeliveryStatus::Failed => pb::DeliveryStatus::DELIVERY_STATUS_FAILED,
        }),
        attempt: d.attempt,
        last_outcome: d.last_outcome.clone(),
        next_attempt_time: d.next_attempt.map(timestamp).into(),
        ..Default::default()
    }
}

impl NotificationService for Adapter {
    async fn create_rule(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateRuleRequest>,
    ) -> ServiceResult<pb::CreateRuleResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let rule = r.rule.as_option().cloned().unwrap_or_default();
        let channels = rule
            .channels
            .iter()
            .filter_map(|c| match c.as_known() {
                Some(pb::NotificationChannel::NOTIFICATION_CHANNEL_EMAIL) => Some(Channel::Email),
                Some(pb::NotificationChannel::NOTIFICATION_CHANNEL_CONSOLE) => {
                    Some(Channel::Console)
                }
                _ => None,
            })
            .collect();
        let created = self
            .0
            .create_rule(
                &caller,
                &rule.customer_id,
                nonempty(&rule.feature),
                rule.thresholds.clone(),
                rule.spend_cap,
                channels,
            )
            .await
            .connect()?;
        Response::ok(pb::CreateRuleResponse {
            rule: rule_out(&created).into(),
            ..Default::default()
        })
    }

    async fn list_rules(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListRulesRequest>,
    ) -> ServiceResult<pb::ListRulesResponse> {
        let caller = caller(&ctx)?;
        let rules = self
            .0
            .list_rules(&caller, request.customer_id)
            .await
            .connect()?;
        Response::ok(pb::ListRulesResponse {
            rules: rules.iter().map(rule_out).collect(),
            ..Default::default()
        })
    }

    async fn create_webhook(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateWebhookRequest>,
    ) -> ServiceResult<pb::CreateWebhookResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (ep, secret) = self
            .0
            .create_webhook(&caller, &r.customer_id, &r.url, r.types)
            .await
            .connect()?;
        Response::ok(pb::CreateWebhookResponse {
            webhook: webhook_out(&ep).into(),
            secret,
            ..Default::default()
        })
    }

    async fn rotate_webhook_secret(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RotateWebhookSecretRequest>,
    ) -> ServiceResult<pb::RotateWebhookSecretResponse> {
        let caller = caller(&ctx)?;
        let (ep, secret) = self.0.rotate_secret(&caller, request.id).await.connect()?;
        Response::ok(pb::RotateWebhookSecretResponse {
            webhook: webhook_out(&ep).into(),
            secret,
            ..Default::default()
        })
    }

    async fn list_deliveries(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListDeliveriesRequest>,
    ) -> ServiceResult<pb::ListDeliveriesResponse> {
        let caller = caller(&ctx)?;
        let deliveries = self
            .0
            .list_deliveries(&caller, request.customer_id, page_size(request.page_size))
            .await
            .connect()?;
        Response::ok(pb::ListDeliveriesResponse {
            deliveries: deliveries.iter().map(delivery_out).collect(),
            ..Default::default()
        })
    }
}
