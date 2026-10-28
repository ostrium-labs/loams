//! The policy table and payment states (design §63 §13, D1128; ME1 Task 19).
//!
//! A pure table: what happens at a zero balance, at the spend cap, when the
//! counter owner cannot be reached, and whether overage may be turned on,
//! for each [`Funding`] a customer can be in. The `meter` role applies it when
//! it grants leases (a lease's `fail_open` budget, `counting` and `overage`
//! flags); the gate applies the unreachable-owner row itself
//! ([`crate::gate::LeaseGate`]), from the last grant or a [`FailOpenLookup`].
//! Defaults are the table of §63 §13; a plan may override them.

use std::fmt;

use crate::gate::{Amount, RefusalReason};

/// A customer's payment state (§63 §13): set by the billing exporter from
/// provider webhooks, or by an operator for contract customers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaymentState {
    /// No payment method: the free tier.
    #[default]
    None,
    /// A card on file.
    Card,
    /// Invoiced under a contract.
    Invoice,
}

impl PaymentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Card => "card",
            Self::Invoice => "invoice",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "card" => Some(Self::Card),
            "invoice" => Some(Self::Invoice),
            _ => None,
        }
    }
}

/// The columns of §63 §13's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Funding {
    /// No payment method.
    Free,
    /// A card, no overage opt-in.
    Paid,
    /// A card, overage opted in with a spend cap.
    PaidOverage,
    /// Funded only by prepaid grants.
    PrepaidOnly { auto_top_up: bool },
    /// Invoiced under a contract.
    Contract,
}

impl Funding {
    /// The column a customer is in. Prepaid-only wins over the payment state
    /// (prepaid credits stop at zero, §63 §9.4); overage counts only with a
    /// card.
    pub fn of(
        payment: PaymentState,
        overage_opt_in: bool,
        prepaid_only: bool,
        auto_top_up: bool,
    ) -> Funding {
        if prepaid_only {
            return Funding::PrepaidOnly { auto_top_up };
        }
        match payment {
            PaymentState::None => Funding::Free,
            PaymentState::Card if overage_opt_in => Funding::PaidOverage,
            PaymentState::Card => Funding::Paid,
            PaymentState::Invoice => Funding::Contract,
        }
    }
}

/// What happens when the balance reaches zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtZero {
    /// New work is refused for this reason.
    HardStop(RefusalReason),
    /// Allowed; usage past zero is billed as overage up to the spend cap.
    Overage,
    /// Allowed and notified; overage is not billed (a contract's default).
    SoftLimit,
}

/// What a gate does with no lease when the owner misses its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhenUnreachable {
    /// Refuse with `meter_unavailable` and `Retry-After: 1`.
    FailClosed,
    /// Allow up to `budget` units, counted locally and reconciled later.
    FailOpen { budget: Amount },
}

/// Why overage cannot be turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverageError {
    /// No card on file (`payment_method_required`).
    PaymentMethodRequired,
    /// A spend cap is required and none (or zero) was given.
    SpendCapRequired,
    /// Only a billing admin may opt in (§63 §19).
    NotBillingAdmin,
    /// The operator sets contract terms; prepaid needs a card first.
    NotSelfServe,
}

impl OverageError {
    /// The refusal reason surfaced to the console and APIs.
    pub fn reason(self) -> &'static str {
        match self {
            Self::PaymentMethodRequired => "payment_method_required",
            Self::SpendCapRequired => "spend_cap_required",
            Self::NotBillingAdmin => "permission_denied",
            Self::NotSelfServe => "operator_managed",
        }
    }
}

impl fmt::Display for OverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for OverageError {}

/// A usage threshold that emits a `meter.threshold` event (§63 §15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threshold {
    /// Percent of the period's total available.
    Allowance(u8),
    /// Percent of the spend cap.
    SpendCap(u8),
    /// Prepaid credit left, in percent.
    CreditsLeft(u8),
}

/// §63 §13's table with its defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyTable {
    /// A contract's behaviour at zero (default: soft limit).
    pub contract_at_zero: AtZero,
    /// The default spend cap as a multiple of the plan's included value
    /// (Q961: 2×).
    pub default_cap_multiple: u32,
    /// Allowance thresholds in percent (80, 100).
    pub allowance_thresholds: Vec<u8>,
}

impl Default for PolicyTable {
    fn default() -> Self {
        Self {
            contract_at_zero: AtZero::SoftLimit,
            default_cap_multiple: 2,
            allowance_thresholds: vec![80, 100],
        }
    }
}

impl PolicyTable {
    /// The "balance reaches 0" row.
    pub fn at_zero(&self, f: Funding) -> AtZero {
        match f {
            Funding::Free | Funding::Paid => AtZero::HardStop(RefusalReason::EntitlementExhausted),
            Funding::PaidOverage => AtZero::Overage,
            Funding::PrepaidOnly { auto_top_up: true } => AtZero::Overage,
            Funding::PrepaidOnly { auto_top_up: false } => {
                AtZero::HardStop(RefusalReason::EntitlementExhausted)
            }
            Funding::Contract => self.contract_at_zero,
        }
    }

    /// The "overage reaches the spend cap" row: `Some` is a hard stop; a
    /// contract only alerts the operator.
    pub fn at_spend_cap(&self, f: Funding) -> Option<RefusalReason> {
        match f {
            Funding::PaidOverage | Funding::PrepaidOnly { auto_top_up: true } => {
                Some(RefusalReason::SpendCapReached)
            }
            _ => None,
        }
    }

    /// The "counter owner unreachable past the deadline, no lease" row.
    /// `cap_headroom` is what is left under the spend cap, in units;
    /// `included` is one period's included amount, the budget when a paid
    /// customer has no cap.
    pub fn when_unreachable(
        &self,
        f: Funding,
        cap_headroom: Option<Amount>,
        included: Amount,
    ) -> WhenUnreachable {
        match f {
            Funding::Free | Funding::PrepaidOnly { .. } => WhenUnreachable::FailClosed,
            Funding::Paid | Funding::PaidOverage => WhenUnreachable::FailOpen {
                budget: cap_headroom.unwrap_or(included),
            },
            Funding::Contract => WhenUnreachable::FailOpen {
                budget: cap_headroom.unwrap_or(Amount::MAX),
            },
        }
    }

    /// The "turn on overage" row: a card, a billing admin and a positive cap.
    pub fn enable_overage(
        &self,
        f: Funding,
        by_billing_admin: bool,
        spend_cap_micros: Option<i64>,
    ) -> Result<(), OverageError> {
        match f {
            Funding::Free => return Err(OverageError::PaymentMethodRequired),
            Funding::Contract => return Err(OverageError::NotSelfServe),
            Funding::PrepaidOnly { .. } | Funding::Paid | Funding::PaidOverage => {}
        }
        if !by_billing_admin {
            return Err(OverageError::NotBillingAdmin);
        }
        match spend_cap_micros {
            Some(c) if c > 0 => Ok(()),
            _ => Err(OverageError::SpendCapRequired),
        }
    }

    /// The cap proposed when a customer opts in: the plan's included value
    /// times [`PolicyTable::default_cap_multiple`].
    pub fn default_spend_cap_micros(&self, included_value_micros: i64) -> i64 {
        included_value_micros.saturating_mul(i64::from(self.default_cap_multiple))
    }

    /// The thresholds that emit events for this funding (§63 §13 last row).
    pub fn thresholds(&self, f: Funding) -> Vec<Threshold> {
        let mut out: Vec<Threshold> = self
            .allowance_thresholds
            .iter()
            .map(|p| Threshold::Allowance(*p))
            .collect();
        match f {
            Funding::Paid | Funding::PaidOverage => {
                out.extend([Threshold::SpendCap(80), Threshold::SpendCap(100)]);
            }
            Funding::PrepaidOnly { .. } => out.push(Threshold::CreditsLeft(20)),
            Funding::Free | Funding::Contract => {}
        }
        out
    }
}

/// The fail-open budget for a (subject, feature) the gate holds no lease
/// for, from the last policy the meter role pushed. The default
/// ([`FailClosedLookup`]) fails every unknown subject closed, which is the
/// free tier's row.
pub trait FailOpenLookup: Send + Sync + fmt::Debug {
    fn fail_open_budget(&self, subject: &str, feature: &str) -> Amount;
}

/// Every subject fails closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedLookup;

impl FailOpenLookup for FailClosedLookup {
    fn fail_open_budget(&self, _subject: &str, _feature: &str) -> Amount {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funding_columns() {
        use PaymentState as P;
        assert_eq!(Funding::of(P::None, false, false, false), Funding::Free);
        assert_eq!(Funding::of(P::None, true, false, false), Funding::Free);
        assert_eq!(Funding::of(P::Card, false, false, false), Funding::Paid);
        assert_eq!(
            Funding::of(P::Card, true, false, false),
            Funding::PaidOverage
        );
        assert_eq!(
            Funding::of(P::Invoice, false, false, false),
            Funding::Contract
        );
        assert_eq!(
            Funding::of(P::Card, true, true, false),
            Funding::PrepaidOnly { auto_top_up: false }
        );
    }

    #[test]
    fn balance_zero_row() {
        let t = PolicyTable::default();
        let stop = AtZero::HardStop(RefusalReason::EntitlementExhausted);
        assert_eq!(t.at_zero(Funding::Free), stop);
        assert_eq!(t.at_zero(Funding::Paid), stop);
        assert_eq!(t.at_zero(Funding::PaidOverage), AtZero::Overage);
        assert_eq!(t.at_zero(Funding::PrepaidOnly { auto_top_up: false }), stop);
        assert_eq!(t.at_zero(Funding::Contract), AtZero::SoftLimit);
    }

    #[test]
    fn spend_cap_row() {
        let t = PolicyTable::default();
        assert_eq!(
            t.at_spend_cap(Funding::PaidOverage),
            Some(RefusalReason::SpendCapReached)
        );
        assert_eq!(t.at_spend_cap(Funding::Contract), None);
        assert_eq!(t.at_spend_cap(Funding::Free), None);
    }

    #[test]
    fn free_fails_closed_when_owner_unreachable() {
        let t = PolicyTable::default();
        assert_eq!(
            t.when_unreachable(Funding::Free, None, 1_000),
            WhenUnreachable::FailClosed
        );
        assert_eq!(
            t.when_unreachable(Funding::PrepaidOnly { auto_top_up: true }, Some(5), 1_000),
            WhenUnreachable::FailClosed
        );
    }

    #[test]
    fn paid_fail_open_bounded_by_cap() {
        let t = PolicyTable::default();
        assert_eq!(
            t.when_unreachable(Funding::PaidOverage, Some(250), 1_000),
            WhenUnreachable::FailOpen { budget: 250 }
        );
        // No cap: one period's included amount.
        assert_eq!(
            t.when_unreachable(Funding::Paid, None, 1_000),
            WhenUnreachable::FailOpen { budget: 1_000 }
        );
    }

    #[test]
    fn overage_requires_card_and_cap() {
        let t = PolicyTable::default();
        assert_eq!(
            t.enable_overage(Funding::Free, true, Some(10)),
            Err(OverageError::PaymentMethodRequired)
        );
        assert_eq!(
            t.enable_overage(Funding::Paid, true, None),
            Err(OverageError::SpendCapRequired)
        );
        assert_eq!(
            t.enable_overage(Funding::Paid, true, Some(0)),
            Err(OverageError::SpendCapRequired)
        );
        assert_eq!(
            t.enable_overage(Funding::Paid, false, Some(10)),
            Err(OverageError::NotBillingAdmin)
        );
        assert_eq!(t.enable_overage(Funding::Paid, true, Some(10)), Ok(()));
        assert_eq!(
            t.enable_overage(Funding::Contract, true, Some(10)),
            Err(OverageError::NotSelfServe)
        );
        assert_eq!(t.default_spend_cap_micros(5_000_000), 10_000_000);
    }

    #[test]
    fn thresholds_per_funding() {
        let t = PolicyTable::default();
        assert_eq!(
            t.thresholds(Funding::Free),
            [Threshold::Allowance(80), Threshold::Allowance(100)]
        );
        assert!(
            t.thresholds(Funding::PaidOverage)
                .contains(&Threshold::SpendCap(100))
        );
        assert!(
            t.thresholds(Funding::PrepaidOnly { auto_top_up: false })
                .contains(&Threshold::CreditsLeft(20))
        );
    }

    #[test]
    fn payment_state_round_trips() {
        for p in [
            PaymentState::None,
            PaymentState::Card,
            PaymentState::Invoice,
        ] {
            assert_eq!(PaymentState::parse(p.as_str()), Some(p));
        }
    }
}
