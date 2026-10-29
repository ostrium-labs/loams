//! Audit events (design §63 §19 point 6, D100): every grant, plan
//! assignment, spend-control change, manual adjustment and payment-state
//! change is recorded with the principal and the reason. The role writes
//! them to `_audit`; [`MemAudit`] keeps them in memory for tests.

use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use loams_meter_core::UtcNanos;
use serde::{Deserialize, Serialize};

use crate::error::{MeterError, Result};

/// One audited change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// What happened, `meter.<object>.<verb>`: `meter.grant.create`,
    /// `meter.customer.payment_state`, …
    pub action: String,
    /// Who did it: `user:<id>`, `service_account:<id>` or `service:<id>`.
    pub actor: String,
    /// What it was done to: `customer:<id>`, `grant:<id>`, …
    pub object: String,
    /// The org the object belongs to, when it has one.
    pub org: Option<String>,
    /// The reason the caller gave (required for manual grants).
    pub reason: String,
    /// What changed, as JSON.
    pub details: serde_json::Value,
    pub time: UtcNanos,
}

/// Where audit events go. A failed write fails the change: an unaudited
/// grant must not happen.
#[async_trait]
pub trait AuditSink: Send + Sync + fmt::Debug {
    async fn record(&self, event: AuditEvent) -> Result<()>;
}

/// Audit events in memory (tests, and the desktop without `_audit`).
#[derive(Debug, Default)]
pub struct MemAudit {
    events: Mutex<Vec<AuditEvent>>,
}

impl MemAudit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event so far.
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl AuditSink for MemAudit {
    async fn record(&self, event: AuditEvent) -> Result<()> {
        if event.action.is_empty() || event.actor.is_empty() {
            return Err(MeterError::invalid(
                "audit",
                "an audit event needs an action and an actor",
            ));
        }
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
        Ok(())
    }
}
