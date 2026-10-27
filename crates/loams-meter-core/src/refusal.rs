//! Why work is refused (design §63 §10.1, §17.4). Shared by the gate, the
//! lease service and the APIs so every surface names the same reason.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The balance is zero on a hard limit (or prepaid credit is spent).
    EntitlementExhausted,
    /// Overage needs a payment method on file.
    PaymentMethodRequired,
    /// Overage reached the customer's spend cap.
    SpendCapReached,
    /// The subject has no entitlement to the feature.
    NotEntitled,
    /// No lease and the counter owner did not answer within the deadline;
    /// free and prepaid tenants fail closed (§13).
    MeterUnavailable,
}

impl RefusalReason {
    /// The `ErrorInfo.reason` string (§17.4).
    pub fn reason(self) -> &'static str {
        match self {
            Self::EntitlementExhausted => "entitlement_exhausted",
            Self::PaymentMethodRequired => "payment_method_required",
            Self::SpendCapReached => "spend_cap_reached",
            Self::NotEntitled => "not_entitled",
            Self::MeterUnavailable => "meter_unavailable",
        }
    }

    /// The HTTP status on REST surfaces: 402 for allowance refusals (Q960),
    /// 503 when the meter is unavailable. 429 stays with rate limits.
    pub fn http_status(self) -> u16 {
        match self {
            Self::MeterUnavailable => 503,
            _ => 402,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_match_serde_and_design() {
        for r in [
            RefusalReason::EntitlementExhausted,
            RefusalReason::PaymentMethodRequired,
            RefusalReason::SpendCapReached,
            RefusalReason::NotEntitled,
            RefusalReason::MeterUnavailable,
        ] {
            let json = serde_json::to_value(r).expect("ser");
            assert_eq!(json, serde_json::Value::from(r.reason()));
        }
        assert_eq!(RefusalReason::EntitlementExhausted.http_status(), 402);
        assert_eq!(RefusalReason::MeterUnavailable.http_status(), 503);
    }
}
