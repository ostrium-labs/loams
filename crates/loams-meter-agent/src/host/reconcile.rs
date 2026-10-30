//! Per-window reconciliation of host reports against the wasmtime host's
//! cgroup total (platform doc 06 PD63, §3.3; design §63 §19, PD67).
//!
//! The T1 host is one shared cgroup (`loams.slice/wasm-host.scope`), so its
//! `cpu.stat` says how much CPU all tenants used together, and the host
//! reports say how it splits. The cgroup is authoritative for the total; the
//! reports only split it:
//!
//! * reported sum within the tolerance below the total: each tenant gets
//!   what the host reported, and the rest is the host's overhead, metered
//!   under `_platform` and never billed;
//! * reported sum above the total, or short of it by more than the
//!   tolerance: the window is flagged and the cgroup total is apportioned by
//!   the reported shares;
//! * a window that overlaps a `Gap` is `incomplete`: the total is
//!   apportioned by the shares of the last complete window, never billed as
//!   zero;
//! * no reports and no earlier shares: all of it is overhead.
//!
//! Every split floors, so **the tenants' sum never exceeds the cgroup
//! total**; the remainder goes to `_platform`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::proto::Invocation;

/// Who one share of the host's CPU belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TenantKey {
    /// The org.
    pub org: String,
    /// The namespace.
    pub namespace: String,
    /// The function.
    pub function: String,
}

impl TenantKey {
    /// The key of `inv`.
    pub fn of(inv: &Invocation) -> Self {
        Self {
            org: inv.org.clone(),
            namespace: inv.namespace.clone(),
            function: inv.function.clone(),
        }
    }
}

/// How a window was split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitKind {
    /// The reports were within tolerance; tenants got what was reported.
    Reported,
    /// The reports exceeded the total or fell short beyond the tolerance;
    /// the total was apportioned by the reported shares.
    Scaled,
    /// The window overlapped a gap; the total was apportioned by the last
    /// complete window's shares.
    PreviousShares,
    /// Nothing to split by; all of it is overhead.
    Overhead,
}

impl SplitKind {
    /// Whether the tenants' amounts are estimates rather than reports.
    pub fn estimated(self) -> bool {
        !matches!(self, SplitKind::Reported)
    }
}

/// A window's split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// CPU microseconds per tenant. Their sum is at most the total.
    pub tenants: BTreeMap<TenantKey, u64>,
    /// What is left: the host's overhead, metered under `_platform`.
    pub overhead: u64,
    /// How it was split.
    pub kind: SplitKind,
}

impl Split {
    /// The tenants' sum.
    pub fn tenant_total(&self) -> u64 {
        self.tenants.values().sum()
    }
}

/// Host-reported CPU since the last reconciliation, and the shares of the
/// last complete window. Saved with the agent's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLedger {
    /// Reported CPU microseconds per tenant since the last reconciliation.
    #[serde(with = "pairs", default)]
    pub pending: BTreeMap<TenantKey, u64>,
    /// Whether a gap was recorded since the last reconciliation.
    pub incomplete: bool,
    /// The last complete window's reported shares.
    #[serde(with = "pairs", default)]
    pub last_shares: BTreeMap<TenantKey, u64>,
}

/// A map keyed by [`TenantKey`] as a list of pairs: JSON object keys must be
/// strings.
mod pairs {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serializer};

    use super::TenantKey;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<TenantKey, u64>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<TenantKey, u64>, D::Error> {
        let pairs: Vec<(TenantKey, u64)> = Vec::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}

/// The tolerance in basis points: a reported sum this far below the total
/// is the host's overhead, not an error (2%, doc 06 §4's exit gate).
pub const DEFAULT_TOLERANCE_BP: u64 = 200;

impl HostLedger {
    /// Adds a report's invocations.
    pub fn add(&mut self, invocations: &[Invocation]) {
        for inv in invocations {
            let entry = self.pending.entry(TenantKey::of(inv)).or_default();
            *entry = entry.saturating_add(inv.cpu_usec);
        }
    }

    /// Adds `cpu_usec` reported for `tenant` (crash replay).
    pub fn add_cpu(&mut self, tenant: TenantKey, cpu_usec: u64) {
        let entry = self.pending.entry(tenant).or_default();
        *entry = entry.saturating_add(cpu_usec);
    }

    /// Records a gap: this window is incomplete.
    pub fn gap(&mut self) {
        self.incomplete = true;
    }

    /// Splits `total` (the host cgroup's CPU over the window) and starts the
    /// next window.
    pub fn reconcile(&mut self, total: u64, tolerance_bp: u64) -> Split {
        let pending = std::mem::take(&mut self.pending);
        let incomplete = std::mem::take(&mut self.incomplete);
        let split = split(total, &pending, incomplete, &self.last_shares, tolerance_bp);
        if !incomplete && !pending.is_empty() {
            self.last_shares = pending;
        }
        split
    }
}

/// Splits `total` by `reported`, under the rules in the module docs.
pub fn split(
    total: u64,
    reported: &BTreeMap<TenantKey, u64>,
    incomplete: bool,
    last_shares: &BTreeMap<TenantKey, u64>,
    tolerance_bp: u64,
) -> Split {
    let sum: u128 = reported.values().map(|v| u128::from(*v)).sum();
    let (tenants, kind) = if incomplete {
        if last_shares.is_empty() {
            (apportion(total, reported), SplitKind::Scaled)
        } else {
            (apportion(total, last_shares), SplitKind::PreviousShares)
        }
    } else if sum == 0 {
        (BTreeMap::new(), SplitKind::Overhead)
    } else {
        let floor = u128::from(total) * u128::from(10_000u64.saturating_sub(tolerance_bp)) / 10_000;
        if sum <= u128::from(total) && sum >= floor {
            (reported.clone(), SplitKind::Reported)
        } else {
            (apportion(total, reported), SplitKind::Scaled)
        }
    };
    let tenant_total: u64 = tenants.values().sum();
    let kind = if tenants.is_empty() {
        SplitKind::Overhead
    } else {
        kind
    };
    Split {
        overhead: total - tenant_total,
        tenants,
        kind,
    }
}

/// `total` apportioned by `shares`, each part floored, zero parts dropped.
fn apportion(total: u64, shares: &BTreeMap<TenantKey, u64>) -> BTreeMap<TenantKey, u64> {
    let sum: u128 = shares.values().map(|v| u128::from(*v)).sum();
    if sum == 0 {
        return BTreeMap::new();
    }
    shares
        .iter()
        .filter_map(|(k, v)| {
            let part = u128::from(total) * u128::from(*v) / sum;
            let part = u64::try_from(part).unwrap_or(total);
            (part > 0).then(|| (k.clone(), part))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(org: &str) -> TenantKey {
        TenantKey {
            org: org.into(),
            namespace: "ns".into(),
            function: "f".into(),
        }
    }

    fn inv(org: &str, cpu: u64) -> Invocation {
        Invocation {
            org: org.into(),
            namespace: "ns".into(),
            function: "f".into(),
            cpu_usec: cpu,
            ..Invocation::default()
        }
    }

    #[test]
    fn host_report_split_never_exceeds_cgroup_total() {
        // A deterministic sweep: totals and reported vectors, over, under
        // and within the tolerance, with and without gaps.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..5_000 {
            let mut ledger = HostLedger::default();
            if round % 3 == 0 {
                ledger.add(&[
                    inv("org_prev", next() % 10_000 + 1),
                    inv("org_b", next() % 500),
                ]);
                ledger.reconcile(next() % 20_000, DEFAULT_TOLERANCE_BP);
            }
            let n = (next() % 6) as usize;
            let invs: Vec<Invocation> = (0..n)
                .map(|i| inv(&format!("org_{i}"), next() % 1_000_000))
                .collect();
            ledger.add(&invs);
            if round % 7 == 0 {
                ledger.gap();
            }
            let total = match round % 4 {
                0 => 0,
                1 => next() % 1_000,
                _ => next() % 5_000_000,
            };
            let split = ledger.reconcile(total, DEFAULT_TOLERANCE_BP);
            assert!(
                split.tenant_total() <= total,
                "round {round}: {} > {total} ({split:?})",
                split.tenant_total()
            );
            assert_eq!(
                split.tenant_total() + split.overhead,
                total,
                "round {round}"
            );
        }
        // Extremes.
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", u64::MAX), inv("org_b", u64::MAX)]);
        let split = ledger.reconcile(u64::MAX, DEFAULT_TOLERANCE_BP);
        let _ = split.tenant_total(); // must not overflow
    }

    #[test]
    fn within_tolerance_bills_reports_and_overhead_is_platform() {
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", 600), inv("org_b", 390)]);
        let split = ledger.reconcile(1_000, DEFAULT_TOLERANCE_BP);
        assert_eq!(split.kind, SplitKind::Reported);
        assert_eq!(split.tenants[&key("org_a")], 600);
        assert_eq!(split.tenants[&key("org_b")], 390);
        assert_eq!(split.overhead, 10);
    }

    #[test]
    fn over_reporting_is_scaled_to_the_total() {
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", 1_500), inv("org_b", 500)]);
        let split = ledger.reconcile(1_000, DEFAULT_TOLERANCE_BP);
        assert_eq!(split.kind, SplitKind::Scaled);
        assert_eq!(split.tenants[&key("org_a")], 750);
        assert_eq!(split.tenants[&key("org_b")], 250);
        assert_eq!(split.overhead, 0);
    }

    #[test]
    fn under_reporting_beyond_tolerance_apportions_the_total() {
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", 300), inv("org_b", 100)]);
        let split = ledger.reconcile(1_000, DEFAULT_TOLERANCE_BP);
        assert_eq!(split.kind, SplitKind::Scaled);
        assert_eq!(split.tenants[&key("org_a")], 750);
        assert_eq!(split.tenants[&key("org_b")], 250);
    }

    #[test]
    fn gap_window_uses_last_complete_shares_never_zero() {
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", 800), inv("org_b", 200)]);
        ledger.reconcile(1_000, DEFAULT_TOLERANCE_BP);
        // The next window lost its reports.
        ledger.gap();
        let split = ledger.reconcile(2_000, DEFAULT_TOLERANCE_BP);
        assert_eq!(split.kind, SplitKind::PreviousShares);
        assert_eq!(split.tenants[&key("org_a")], 1_600);
        assert_eq!(split.tenants[&key("org_b")], 400);
        assert!(split.kind.estimated());
    }

    #[test]
    fn ledger_round_trips_through_json() {
        let mut ledger = HostLedger::default();
        ledger.add(&[inv("org_a", 10)]);
        ledger.gap();
        let json = serde_json::to_string(&ledger).unwrap();
        let back: HostLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ledger);
    }

    #[test]
    fn no_reports_is_overhead() {
        let mut ledger = HostLedger::default();
        let split = ledger.reconcile(1_000, DEFAULT_TOLERANCE_BP);
        assert_eq!(split.kind, SplitKind::Overhead);
        assert_eq!(split.overhead, 1_000);
    }
}
