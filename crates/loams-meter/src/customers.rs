//! `CustomerService` (design §63 §9.1, §13, §17.1; ME1 Task 10): customers,
//! their subjects, payment state and spend controls.

use std::sync::Arc;

use loams_meter_core::{Currency, Money, SubjectKey, UtcNanos};

use crate::audit::{AuditEvent, AuditSink};
use crate::authz::{Caller, MeterAuthz};
use crate::error::{MeterError, Result};
use crate::store::{Customer, MeterStore, PaymentState, SpendControls};

/// The most customers one page answers.
pub const MAX_PAGE: usize = 500;

/// A clock for record times.
pub type Clock = Arc<dyn Fn() -> UtcNanos + Send + Sync>;

/// A new customer.
#[derive(Clone, Debug, Default)]
pub struct NewCustomer {
    pub org: String,
    pub display_name: String,
    pub subjects: Vec<String>,
    pub currency: String,
    pub stripe_customer_id: Option<String>,
    pub idempotency_key: Option<String>,
}

/// The fields `UpdateCustomer` may change, by mask path.
#[derive(Clone, Debug, Default)]
pub struct CustomerChanges {
    pub display_name: Option<String>,
    pub subjects: Option<Vec<String>>,
    pub stripe_customer_id: Option<Option<String>>,
}

/// The service.
#[derive(Clone)]
pub struct Customers {
    store: Arc<dyn MeterStore>,
    authz: MeterAuthz,
    audit: Arc<dyn AuditSink>,
    clock: Clock,
}

impl std::fmt::Debug for Customers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Customers").finish_non_exhaustive()
    }
}

/// A new record id (a ULID).
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string().to_ascii_lowercase()
}

fn subjects(raw: &[String]) -> Result<Vec<SubjectKey>> {
    let mut out: Vec<SubjectKey> = Vec::with_capacity(raw.len());
    for s in raw {
        let key = SubjectKey::new(s.clone())
            .map_err(|e| MeterError::invalid("subjects", e.to_string()))?;
        if !out.contains(&key) {
            out.push(key);
        }
    }
    Ok(out)
}

fn org_name(org: &str) -> Result<()> {
    if org.is_empty() || org.len() > 128 || org.chars().any(char::is_control) || org.contains(':') {
        return Err(MeterError::invalid("org", "an org id is required"));
    }
    Ok(())
}

impl Customers {
    pub fn new(
        store: Arc<dyn MeterStore>,
        authz: MeterAuthz,
        audit: Arc<dyn AuditSink>,
        clock: Clock,
    ) -> Self {
        Customers {
            store,
            authz,
            audit,
            clock,
        }
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: &str,
        c: &Customer,
        reason: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        self.audit
            .record(AuditEvent {
                action: action.to_string(),
                actor: caller.actor(),
                object: format!("customer:{}", c.id),
                org: Some(c.org.clone()),
                reason: reason.to_string(),
                details,
                time: (self.clock)(),
            })
            .await
    }

    async fn load(&self, id: &str) -> Result<Customer> {
        self.store
            .customer(id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", id))
    }

    /// `CreateCustomer` (`meter_admin`).
    pub async fn create(&self, caller: &Caller, new: NewCustomer) -> Result<Customer> {
        self.authz.require_meter_admin(caller).await?;
        org_name(&new.org)?;
        let currency = Currency::parse(if new.currency.is_empty() {
            "USD"
        } else {
            &new.currency
        })
        .map_err(|e| MeterError::invalid("currency", e.to_string()))?;
        let now = (self.clock)();
        let c = Customer {
            id: new_id(),
            org: new.org,
            display_name: new.display_name,
            subjects: subjects(&new.subjects)?,
            payment_state: PaymentState::None,
            spend_controls: SpendControls::default(),
            stripe_customer_id: new.stripe_customer_id.filter(|s| !s.is_empty()),
            currency,
            plan: None,
            created_at: now,
            updated_at: now,
            version: 1,
        };
        let created = self
            .store
            .create_customer(c.clone(), new.idempotency_key.as_deref())
            .await?;
        if created.id == c.id {
            self.audit(
                caller,
                "meter.customer.create",
                &created,
                "",
                serde_json::json!({ "subjects": created.subjects }),
            )
            .await?;
        }
        Ok(created)
    }

    /// `GetCustomer` (`meter_admin`, or `billing_viewer` of its org).
    pub async fn get(&self, caller: &Caller, id: &str) -> Result<Customer> {
        let c = self.load(id).await?;
        self.authz.require_billing_view(caller, &c.org).await?;
        Ok(c)
    }

    /// `ListCustomers`: an org's (its billing viewers) or all (meter admins).
    pub async fn list(
        &self,
        caller: &Caller,
        org: Option<&str>,
        page_token: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<Customer>, Option<String>)> {
        match org {
            Some(o) => self.authz.require_billing_view(caller, o).await?,
            None => self.authz.require_meter_admin(caller).await?,
        }
        let limit = page_size.clamp(1, MAX_PAGE);
        let page = self.store.list_customers(org, page_token, limit).await?;
        let next = (page.len() == limit)
            .then(|| page.last().map(|c| c.id.clone()))
            .flatten();
        Ok((page, next))
    }

    /// `UpdateCustomer` (`meter_admin`).
    pub async fn update(
        &self,
        caller: &Caller,
        id: &str,
        changes: CustomerChanges,
        reason: &str,
    ) -> Result<Customer> {
        self.authz.require_meter_admin(caller).await?;
        let mut c = self.load(id).await?;
        if let Some(name) = changes.display_name {
            c.display_name = name;
        }
        if let Some(s) = &changes.subjects {
            c.subjects = subjects(s)?;
        }
        if let Some(stripe) = changes.stripe_customer_id {
            c.stripe_customer_id = stripe.filter(|s| !s.is_empty());
        }
        c.updated_at = (self.clock)();
        let updated = self.store.update_customer(c).await?;
        self.audit(
            caller,
            "meter.customer.update",
            &updated,
            reason,
            serde_json::json!({
                "subjects": updated.subjects,
                "stripe_customer_id": updated.stripe_customer_id,
            }),
        )
        .await?;
        Ok(updated)
    }

    /// `SetPaymentState`. The billing exporter's service identity sets
    /// `none` or `card` from the provider; it never changes an operator's
    /// `invoice` (that call answers the customer unchanged). A meter admin
    /// sets `invoice` for contract customers, and may take it off.
    pub async fn set_payment_state(
        &self,
        caller: &Caller,
        id: Option<&str>,
        stripe_customer_id: Option<&str>,
        state: PaymentState,
        reason: &str,
    ) -> Result<Customer> {
        let from_service = self.authz.require_billing_service(caller).is_ok();
        if from_service {
            if state == PaymentState::Invoice {
                return Err(MeterError::denied(
                    "operator_only",
                    "only an operator sets the invoice payment state",
                ));
            }
        } else {
            // An operator (meter admin) puts contract customers on invoice
            // or takes them off it; a card comes from the provider only.
            self.authz.require_meter_admin(caller).await?;
            if state == PaymentState::Card {
                return Err(MeterError::denied(
                    "billing_service_only",
                    "a card on file is set by the billing provider, not by hand",
                ));
            }
        }
        let mut c = match (
            id.filter(|s| !s.is_empty()),
            stripe_customer_id.filter(|s| !s.is_empty()),
        ) {
            (Some(id), _) => self.load(id).await?,
            (None, Some(stripe)) => self
                .store
                .customer_by_stripe(stripe)
                .await?
                .ok_or_else(|| MeterError::not_found("stripe customer", stripe))?,
            (None, None) => {
                return Err(MeterError::invalid(
                    "id",
                    "name the customer by id or by stripe_customer_id",
                ));
            }
        };
        if from_service && c.payment_state == PaymentState::Invoice {
            return Ok(c);
        }
        if c.payment_state == state {
            return Ok(c);
        }
        let before = c.payment_state;
        c.payment_state = state;
        // Without a payment method, overage switches off (§63 §13): the cap
        // stays, so turning it back on needs only the card.
        if state == PaymentState::None {
            c.spend_controls.overage_enabled = false;
            c.spend_controls.auto_top_up = false;
        }
        c.updated_at = (self.clock)();
        let updated = self.store.update_customer(c).await?;
        self.audit(
            caller,
            "meter.customer.payment_state",
            &updated,
            reason,
            serde_json::json!({ "from": before, "to": state }),
        )
        .await?;
        Ok(updated)
    }

    /// `SetSpendControls` (`billing_admin` of the customer's org; §63 §13):
    /// overage needs a card on file (or a contract) and a spend cap in the
    /// customer's currency; auto top-up needs a card and both amounts.
    pub async fn set_spend_controls(
        &self,
        caller: &Caller,
        id: &str,
        controls: SpendControls,
        reason: &str,
    ) -> Result<Customer> {
        let mut c = self.load(id).await?;
        self.authz.require_billing_admin(caller, &c.org).await?;
        validate_controls(&c, &controls)?;
        let before = c.spend_controls.clone();
        c.spend_controls = controls;
        c.updated_at = (self.clock)();
        let updated = self.store.update_customer(c).await?;
        self.audit(
            caller,
            "meter.customer.spend_controls",
            &updated,
            reason,
            serde_json::json!({ "from": before, "to": updated.spend_controls }),
        )
        .await?;
        Ok(updated)
    }
}

fn positive_in(m: &Option<Money>, currency: Currency, field: &str) -> Result<Money> {
    let Some(m) = m else {
        return Err(MeterError::precondition(
            "spend_cap_required",
            format!("{field} is required"),
        ));
    };
    if m.currency != currency {
        return Err(MeterError::invalid(
            field,
            format!("must be in the customer's currency, {}", currency.as_str()),
        ));
    }
    if m.micros <= 0 {
        return Err(MeterError::invalid(field, "must be positive"));
    }
    Ok(*m)
}

/// §63 §13's rules for spend controls.
pub fn validate_controls(c: &Customer, controls: &SpendControls) -> Result<()> {
    let has_payment = matches!(c.payment_state, PaymentState::Card | PaymentState::Invoice);
    if controls.overage_enabled {
        if !has_payment {
            return Err(MeterError::Refused {
                reason: loams_meter_core::RefusalReason::PaymentMethodRequired,
                message: "turning on overage needs a payment method on file".into(),
            });
        }
        positive_in(&controls.spend_cap, c.currency, "spend_cap")?;
    } else if controls.spend_cap.is_some() {
        positive_in(&controls.spend_cap, c.currency, "spend_cap")?;
    }
    if controls.auto_top_up {
        if c.payment_state != PaymentState::Card {
            return Err(MeterError::Refused {
                reason: loams_meter_core::RefusalReason::PaymentMethodRequired,
                message: "auto top-up needs a card on file".into(),
            });
        }
        positive_in(&controls.top_up_amount, c.currency, "top_up_amount")?;
        positive_in(&controls.top_up_threshold, c.currency, "top_up_threshold")?;
    }
    Ok(())
}
