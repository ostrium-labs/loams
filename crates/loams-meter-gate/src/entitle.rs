//! What a product holds to ask the gate (design §63 §10.2; ME1 Tasks 20–23):
//! a [`Gate`] and the [`SubjectResolver`] that turns the product's namespace
//! (and org, when it keeps one) into the billing subject. The default is
//! [`NoGate`]: every check allows, and the product behaves as before (D202,
//! D551).
//!
//! Feature keys of Loams' own plans are in [`features`].

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::adapters::{DeploymentOrg, SubjectResolver};
use crate::gate::{Amount, Decision, Gate, LeaseId, NoGate, Refusal, RefusalReason, Reservation};

/// Feature keys the call sites check (§63 §9.5, §10.2).
pub mod features {
    /// House CPU, ms (`check` with the 1-unit floor before a statement).
    pub const HOUSE_CPU: &str = "house_cpu";
    /// House bytes scanned (reads only).
    pub const HOUSE_SCAN: &str = "house_scan";
    /// Postgres compute, compute-unit ms; `check(0)` asks for access at all.
    pub const PG_COMPUTE: &str = "pg_compute";
    /// Stream ingest, logical bytes per produce batch.
    pub const STREAM_INGEST_BYTES: &str = "stream_ingest_bytes";
    /// Git push, bytes received per push.
    pub const GIT_PUSH_BYTES: &str = "git_push_bytes";
    /// Gateway HTTP API requests.
    pub const API_REQUESTS: &str = "api_requests";
    /// LLM output tokens (`reserve(max_tokens)`, settled with the provider's count).
    pub const LLM_OUTPUT_TOKENS: &str = "llm_output_tokens";
    /// LLM input tokens (estimated prompt tokens).
    pub const LLM_INPUT_TOKENS: &str = "llm_input_tokens";
    /// MCP `tools/call` invocations.
    pub const MCP_CALLS: &str = "mcp_calls";
}

/// A gate plus how to find a namespace's billing subject.
#[derive(Clone)]
pub struct Entitlements {
    gate: Arc<dyn Gate>,
    resolver: Arc<dyn SubjectResolver>,
    unattributed: Arc<AtomicU64>,
}

impl Default for Entitlements {
    /// [`NoGate`]: everything is allowed.
    fn default() -> Self {
        Self::new(Arc::new(NoGate), Arc::new(DeploymentOrg::default()))
    }
}

impl fmt::Debug for Entitlements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entitlements")
            .field("gate", &self.gate)
            .field("resolver", &self.resolver)
            .finish()
    }
}

impl Entitlements {
    pub fn new(gate: Arc<dyn Gate>, resolver: Arc<dyn SubjectResolver>) -> Self {
        Self {
            gate,
            resolver,
            unattributed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Checks with no gate (the default).
    pub fn none() -> Self {
        Self::default()
    }

    /// Checks for namespaces no subject could be resolved for. They are
    /// allowed: a namespace outside any billed org is not the meter's to
    /// stop (the deployment's org settles who is billed).
    pub fn unattributed(&self) -> u64 {
        self.unattributed.load(Ordering::Relaxed)
    }

    fn subject(&self, org: Option<&str>, namespace: &str) -> Option<String> {
        let s = self.resolver.resolve(org, namespace).map(|a| a.subject);
        if s.is_none() {
            self.unattributed.fetch_add(1, Ordering::Relaxed);
        }
        s
    }

    /// May `cost` of `feature` be spent for `namespace` now?
    pub fn check(
        &self,
        org: Option<&str>,
        namespace: &str,
        feature: &str,
        cost: Amount,
    ) -> Result<LeaseId, Refusal> {
        match self.subject(org, namespace) {
            Some(subject) => self.gate.check(&subject, feature, cost).into_result(),
            None => Ok(LeaseId::NONE),
        }
    }

    /// Holds `estimate` of `feature` for `namespace`.
    pub fn reserve(
        &self,
        org: Option<&str>,
        namespace: &str,
        feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal> {
        match self.subject(org, namespace) {
            Some(subject) => self.gate.reserve(&subject, feature, estimate),
            None => NoGate.reserve("", feature, estimate),
        }
    }
}

/// Refuses one feature for one subject and allows everything else (tests
/// of products' call sites and their error mapping).
#[derive(Clone, Debug)]
pub struct DenyGate {
    pub subject: String,
    pub feature: String,
    pub reason: RefusalReason,
}

impl DenyGate {
    pub fn new(subject: &str, feature: &str, reason: RefusalReason) -> Self {
        Self {
            subject: subject.to_owned(),
            feature: feature.to_owned(),
            reason,
        }
    }
}

impl Gate for DenyGate {
    fn check(&self, subject: &str, feature: &str, _cost: Amount) -> Decision {
        if subject == self.subject && feature == self.feature {
            Decision::Refuse(Refusal::new(self.reason, feature))
        } else {
            Decision::Allow {
                lease: LeaseId::NONE,
            }
        }
    }

    fn reserve(
        &self,
        subject: &str,
        feature: &str,
        estimate: Amount,
    ) -> Result<Reservation, Refusal> {
        self.check(subject, feature, estimate).into_result()?;
        NoGate.reserve(subject, feature, estimate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allows_everything() {
        let e = Entitlements::default();
        assert!(e.check(None, "7", features::HOUSE_CPU, 1).is_ok());
        assert_eq!(e.unattributed(), 1);
    }

    #[test]
    fn subject_comes_from_the_resolver() {
        let e = Entitlements::new(
            Arc::new(DenyGate::new(
                "org_42",
                features::HOUSE_CPU,
                RefusalReason::EntitlementExhausted,
            )),
            Arc::new(DeploymentOrg::new("42")),
        );
        let r = e.check(None, "7", features::HOUSE_CPU, 1).unwrap_err();
        assert_eq!(r.feature, "house_cpu");
        assert!(e.check(None, "7", features::HOUSE_SCAN, 1).is_ok());
        assert!(e.check(Some("9"), "7", features::HOUSE_CPU, 1).is_ok());
    }
}
