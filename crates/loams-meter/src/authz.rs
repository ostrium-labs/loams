//! Who may change the meter (design §63 §19 point 5, D1133; design 76's
//! model in `deploy/openfga/core.fga`).
//!
//! | Relation (`can_*`) | Object | Who holds it |
//! |---|---|---|
//! | `can_manage_meters` | `platform:default` | `meter_admin`: meters, plans, grants, customers |
//! | `can_manage_billing` | `org:<org>` | `billing_admin`: spend controls, alerts, the payment link |
//! | `can_view_billing` | `org:<org>` | `billing_viewer`, and every `meter_admin` |
//!
//! The model's type restrictions let only people (`user:`) hold
//! `meter_admin` and `billing_admin`. This module refuses an agent (or a
//! person acting through an agent, RFC 8693 `act`) before it asks, so an
//! agent can never write a grant even under a model that someone loosened
//! (test `agent_cannot_write_grants`). `SetPaymentState` is not a relation
//! at all: only the billing exporter's service identity may call it.

use std::sync::Arc;

use loams_authz::{Authorizer, CheckRequest, Consistency, ObjectRef, Subject, SubjectKind};

use crate::error::{MeterError, Result};

/// The platform object the meter-admin relation is on.
pub const PLATFORM: &str = "default";

pub const CAN_MANAGE_METERS: &str = "can_manage_meters";
pub const CAN_MANAGE_BILLING: &str = "can_manage_billing";
pub const CAN_VIEW_BILLING: &str = "can_view_billing";

/// Who is calling a meter service, as the role's transport layer puts it in
/// the request's extensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// A person, an agent or a service account, from its token or key.
    Principal(Subject),
    /// A Loams service identity (the cluster's mTLS identity), such as
    /// `meter-export` or a product role holding leases.
    Service(String),
}

impl Caller {
    /// The actor an audit event names.
    pub fn actor(&self) -> String {
        match self {
            Caller::Principal(s) => s.fga_user(),
            Caller::Service(id) => format!("service:{id}"),
        }
    }

    /// An agent, or anyone acting through one.
    pub fn is_agent(&self) -> bool {
        match self {
            Caller::Principal(s) => {
                s.kind == SubjectKind::Agent || s.act.iter().any(|a| a.kind == SubjectKind::Agent)
            }
            Caller::Service(_) => false,
        }
    }
}

/// The meter's authorization checks.
#[derive(Clone)]
pub struct MeterAuthz {
    authz: Arc<dyn Authorizer>,
    /// The billing exporter's service identity: the only caller of
    /// `SetPaymentState`.
    billing_service: String,
}

impl std::fmt::Debug for MeterAuthz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeterAuthz")
            .field("billing_service", &self.billing_service)
            .finish_non_exhaustive()
    }
}

impl MeterAuthz {
    pub fn new(authz: Arc<dyn Authorizer>, billing_service: impl Into<String>) -> Self {
        MeterAuthz {
            authz,
            billing_service: billing_service.into(),
        }
    }

    async fn check(&self, subject: &Subject, relation: &str, object: ObjectRef) -> Result<bool> {
        let req = CheckRequest::new(subject.clone(), relation, object)
            .consistency(Consistency::HigherConsistency);
        match self.authz.check(req).await {
            Ok(d) => Ok(d.allowed),
            // An authorizer that cannot answer is a deny, never an allow.
            Err(e) => Err(MeterError::Unavailable(format!("authorizer: {e}"))),
        }
    }

    fn principal(caller: &Caller) -> Result<&Subject> {
        match caller {
            Caller::Principal(s) => Ok(s),
            Caller::Service(id) => Err(MeterError::denied(
                "service_not_allowed",
                format!("service identity {id} may not call this"),
            )),
        }
    }

    fn refuse_agents(caller: &Caller, what: &str) -> Result<()> {
        if caller.is_agent() {
            return Err(MeterError::denied(
                "agent_not_allowed",
                format!("agents can never {what} (meter_admin and billing_admin are people only)"),
            ));
        }
        Ok(())
    }

    /// Meters, plans, grants, entitlements and customers (`meter_admin`).
    pub async fn require_meter_admin(&self, caller: &Caller) -> Result<()> {
        Self::refuse_agents(caller, "change meters, plans, grants or customers")?;
        let s = Self::principal(caller)?;
        if self
            .check(s, CAN_MANAGE_METERS, ObjectRef::new("platform", PLATFORM))
            .await?
        {
            Ok(())
        } else {
            Err(MeterError::denied(
                "meter_admin_required",
                "this needs meter_admin on the platform",
            ))
        }
    }

    /// Whether the caller is a meter admin (no error).
    pub async fn is_meter_admin(&self, caller: &Caller) -> Result<bool> {
        if caller.is_agent() {
            return Ok(false);
        }
        match caller {
            Caller::Principal(s) => {
                self.check(s, CAN_MANAGE_METERS, ObjectRef::new("platform", PLATFORM))
                    .await
            }
            Caller::Service(_) => Ok(false),
        }
    }

    /// Spend controls, alerts and the payment link of `org`
    /// (`billing_admin`).
    pub async fn require_billing_admin(&self, caller: &Caller, org: &str) -> Result<()> {
        Self::refuse_agents(caller, "change spend controls or billing alerts")?;
        let s = Self::principal(caller)?;
        if self
            .check(s, CAN_MANAGE_BILLING, ObjectRef::new("org", org))
            .await?
        {
            Ok(())
        } else {
            Err(MeterError::denied(
                "billing_admin_required",
                format!("this needs billing_admin on org {org}"),
            ))
        }
    }

    /// Reading `org`'s usage, entitlements and customers (`billing_viewer`,
    /// or `meter_admin`).
    pub async fn require_billing_view(&self, caller: &Caller, org: &str) -> Result<()> {
        let s = Self::principal(caller)?;
        if self
            .check(s, CAN_VIEW_BILLING, ObjectRef::new("org", org))
            .await?
        {
            Ok(())
        } else {
            Err(MeterError::denied(
                "billing_viewer_required",
                format!("this needs billing_viewer on org {org}"),
            ))
        }
    }

    /// `SetPaymentState`: the billing exporter's service identity only.
    pub fn require_billing_service(&self, caller: &Caller) -> Result<()> {
        match caller {
            Caller::Service(id) if *id == self.billing_service => Ok(()),
            _ => Err(MeterError::denied(
                "billing_service_only",
                "only the billing exporter's service identity may set the payment state",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_authz::{BuiltinAuthorizer, Tuple};

    fn authz() -> MeterAuthz {
        let fga = BuiltinAuthorizer::new();
        fga.write(&[
            Tuple::new("platform:default", "platform", "org:acme"),
            Tuple::new("user:mia", "meter_admin", "platform:default"),
            Tuple::new("user:bea", "billing_admin", "org:acme"),
            Tuple::new("user:bill", "billing_viewer", "org:acme"),
        ])
        .expect("tuples");
        MeterAuthz::new(Arc::new(fga), "meter-export")
    }

    #[tokio::test]
    async fn meter_admins_are_people_and_agents_never_pass() {
        let a = authz();
        let mia = Caller::Principal(Subject::user("mia"));
        a.require_meter_admin(&mia).await.expect("meter admin");
        let bill = Caller::Principal(Subject::user("bill"));
        assert_eq!(
            a.require_meter_admin(&bill)
                .await
                .expect_err("viewer")
                .reason(),
            "meter_admin_required"
        );
        let agent = Caller::Principal(Subject::agent("agent-1"));
        assert_eq!(
            a.require_meter_admin(&agent)
                .await
                .expect_err("agent")
                .reason(),
            "agent_not_allowed"
        );
        // A meter admin acting through an agent is refused too.
        let mut via = Subject::user("mia");
        via.act.push(Subject::agent("agent-1"));
        assert_eq!(
            a.require_meter_admin(&Caller::Principal(via))
                .await
                .expect_err("via agent")
                .reason(),
            "agent_not_allowed"
        );
    }

    #[tokio::test]
    async fn billing_relations_follow_the_org() {
        let a = authz();
        let bea = Caller::Principal(Subject::user("bea"));
        a.require_billing_admin(&bea, "acme")
            .await
            .expect("billing admin");
        a.require_billing_view(&bea, "acme")
            .await
            .expect("admin views");
        let bill = Caller::Principal(Subject::user("bill"));
        a.require_billing_view(&bill, "acme").await.expect("viewer");
        assert!(a.require_billing_admin(&bill, "acme").await.is_err());
        assert!(a.require_billing_view(&bill, "other").await.is_err());
        // A meter admin reads every org's billing.
        let mia = Caller::Principal(Subject::user("mia"));
        a.require_billing_view(&mia, "acme")
            .await
            .expect("meter admin views");
    }

    #[test]
    fn only_the_billing_service_sets_payment_state() {
        let a = authz();
        a.require_billing_service(&Caller::Service("meter-export".into()))
            .expect("exporter");
        assert!(
            a.require_billing_service(&Caller::Service("house".into()))
                .is_err()
        );
        assert!(
            a.require_billing_service(&Caller::Principal(Subject::user("mia")))
                .is_err()
        );
    }
}
