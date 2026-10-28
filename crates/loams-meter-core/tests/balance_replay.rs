//! The balance engine against a brute-force replay (ME1 Task 9, D2166).
//!
//! The reference here shares no code with `balance::balance`: it walks time
//! one minute at a time, finds resets by arithmetic on a fixed period
//! length, and burns usage one unit at a time from whichever live grant
//! comes first in burn order. Every generated history is on the minute grid
//! with whole-unit amounts, so both must agree exactly.

use loams_meter_core::balance::{UsageSegment, balance};
use loams_meter_core::{
    Amount, AsOf, Balance, Currency, Entitlement, EntitlementId, EntitlementKind,
    EntitlementSource, Expiry, ExpiryUnit, FeatureKey, Grant, GrantId, GrantKind, Interval, Mode,
    Money, Period, Rollover, SubjectKey, UsagePeriod, UtcNanos,
};
use proptest::prelude::*;

const MIN: i64 = UtcNanos::MINUTE;
const HOUR: i64 = UtcNanos::HOUR;
const DAY: i64 = UtcNanos::DAY;
/// Two days of minutes.
const HORIZON_MIN: i64 = 2 * 24 * 60;

#[derive(Clone, Debug)]
struct GrantSpec {
    eff_min: i64,
    created_back_min: i64,
    amount: i64,
    priority: u8,
    kind: u8,
    expiry_hours: Option<u16>,
    void_after_min: Option<i64>,
    rollover: Option<(i64, i64)>,
    recurrence: Option<bool>, // Some(true) hourly, Some(false) daily
}

#[derive(Clone, Debug)]
struct Case {
    daily: bool,
    created_min: i64,
    mode: u8,
    price_micros: i64,
    cap_micros: i64,
    issue: Option<i64>,
    preserve: bool,
    grants: Vec<GrantSpec>,
    segments: Vec<(i64, i64)>,
    at_min: i64,
}

fn grant_spec() -> impl Strategy<Value = GrantSpec> {
    (
        0..HORIZON_MIN,
        0..120i64,
        1..40i64,
        1..=3u8,
        0..3u8,
        proptest::option::of(1..30u16),
        proptest::option::of(0..HORIZON_MIN),
        proptest::option::of((0..5i64, 0..30i64)),
        proptest::option::of(any::<bool>()),
    )
        .prop_map(
            |(
                eff_min,
                created_back_min,
                amount,
                priority,
                kind,
                expiry_hours,
                void_after_min,
                rollover,
                recurrence,
            )| {
                GrantSpec {
                    eff_min,
                    created_back_min,
                    amount,
                    priority,
                    kind,
                    expiry_hours,
                    void_after_min,
                    rollover: rollover.map(|(min, extra)| (min, min + extra)),
                    recurrence,
                }
            },
        )
}

fn case() -> impl Strategy<Value = Case> {
    (
        any::<bool>(),
        0..HORIZON_MIN,
        0..3u8,
        1..3_000_000i64,
        1..100_000_000i64,
        proptest::option::of(0..50i64),
        any::<bool>(),
        proptest::collection::vec(grant_spec(), 0..5),
        proptest::collection::vec((0..HORIZON_MIN, 0..30i64), 0..15),
        0..=HORIZON_MIN,
    )
        .prop_map(
            |(
                daily,
                created_min,
                mode,
                price_micros,
                cap_micros,
                issue,
                preserve,
                grants,
                segments,
                at_min,
            )| Case {
                daily,
                created_min,
                mode,
                price_micros,
                cap_micros,
                issue,
                preserve,
                grants,
                segments,
                at_min,
            },
        )
}

fn id<T>(r: Result<T, loams_meter_core::IdError>) -> T {
    r.expect("valid id")
}

fn mode_of(c: &Case) -> Mode {
    match c.mode {
        0 => Mode::HardLimit,
        1 => Mode::SoftLimit,
        _ => Mode::OverageAllowed {
            overage_price: Money::new(c.price_micros, Currency::USD),
            spend_cap: Money::new(c.cap_micros, Currency::USD),
        },
    }
}

fn period_len(c: &Case) -> i64 {
    if c.daily { DAY } else { HOUR }
}

fn entitlement(c: &Case) -> Entitlement {
    Entitlement {
        id: id(EntitlementId::new("e1")),
        subject: id(SubjectKey::new("org_1")),
        feature: id(FeatureKey::new("house_cpu")),
        kind: EntitlementKind::Metered {
            usage_period: UsagePeriod {
                interval: if c.daily {
                    Interval::Day
                } else {
                    Interval::Hour
                },
                anchor: UtcNanos(0),
            },
            mode: mode_of(c),
            issue_after_reset: c.issue.map(Amount::from_units),
            preserve_overage_at_reset: c.preserve,
        },
        created_at: UtcNanos(c.created_min * MIN),
        source: EntitlementSource::Manual { by: "prop".into() },
    }
}

fn grants(c: &Case) -> Vec<Grant> {
    c.grants
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let eff = g.eff_min * MIN;
            Grant {
                id: id(GrantId::new(format!("g{i}"))),
                entitlement: id(EntitlementId::new("e1")),
                amount: Amount::from_units(g.amount),
                priority: g.priority,
                effective_at: UtcNanos(eff),
                expires: g.expiry_hours.map(|count| Expiry {
                    duration: ExpiryUnit::Hour,
                    count,
                }),
                rollover: g
                    .rollover
                    .map_or(Rollover::default(), |(min, max)| Rollover {
                        min: Amount::from_units(min),
                        max: Amount::from_units(max),
                    }),
                recurrence: g.recurrence.map(|hourly| UsagePeriod {
                    interval: if hourly {
                        Interval::Hour
                    } else {
                        Interval::Day
                    },
                    anchor: UtcNanos(eff),
                }),
                kind: match g.kind {
                    0 => GrantKind::Promotional,
                    1 => GrantKind::Included,
                    _ => GrantKind::Prepaid {
                        paid: Money::new(1_000_000, Currency::USD),
                    },
                },
                voided_at: g.void_after_min.map(|m| UtcNanos(eff + m * MIN)),
                created_at: UtcNanos(eff - g.created_back_min * MIN),
            }
        })
        .collect()
}

fn segments(c: &Case) -> Vec<UsageSegment> {
    c.segments
        .iter()
        .map(|&(start, amount)| UsageSegment {
            start: UtcNanos(start * MIN),
            end: UtcNanos(start * MIN + MIN),
            amount: Amount::from_units(amount),
        })
        .collect()
}

/// One grant in the reference, in whole units.
#[derive(Clone, Debug)]
struct RefGrant {
    id: String,
    priority: u8,
    expires: Option<i64>,
    rank: u8,
    created: i64,
    eff: i64,
    void: Option<i64>,
    rollover: (i64, i64),
    amount: i64,
    recurrence_len: Option<i64>,
    prepaid: bool,
    active: bool,
    rem: i64,
}

impl RefGrant {
    fn live(&self, t: i64) -> bool {
        self.active
            && self.eff <= t
            && self.expires.is_none_or(|e| t < e)
            && self.void.is_none_or(|v| t < v)
    }

    fn key(&self) -> (u8, i64, u8, i64, String) {
        (
            self.priority,
            self.expires.unwrap_or(i64::MAX),
            self.rank,
            self.created,
            self.id.clone(),
        )
    }
}

#[derive(Default)]
struct RefPeriod {
    usage: i64,
    overage: i64,
    total: i64,
    funded: u32,
    funded_prepaid: u32,
}

impl RefPeriod {
    fn fund(&mut self, amount: i64, prepaid: bool) {
        self.total += amount;
        if amount > 0 {
            self.funded += 1;
            if prepaid {
                self.funded_prepaid += 1;
            }
        }
    }
}

/// Burns `units` one at a time; returns what was not covered.
fn burn_units(gs: &mut [RefGrant], t: i64, units: i64) -> i64 {
    let mut uncovered = 0;
    for _ in 0..units {
        let best = (0..gs.len())
            .filter(|&i| gs[i].live(t) && gs[i].rem > 0)
            .min_by_key(|&i| gs[i].key());
        match best {
            Some(i) => gs[i].rem -= 1,
            None => uncovered += 1,
        }
    }
    uncovered
}

fn reference(c: &Case) -> Balance {
    let len = period_len(c);
    let at = c.at_min * MIN;
    let mut gs: Vec<RefGrant> = c
        .grants
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let eff = g.eff_min * MIN;
            RefGrant {
                id: format!("g{i}"),
                priority: g.priority,
                expires: g.expiry_hours.map(|h| eff + i64::from(h) * HOUR),
                rank: g.kind.min(2),
                created: eff - g.created_back_min * MIN,
                eff,
                void: g.void_after_min.map(|m| eff + m * MIN),
                rollover: g.rollover.unwrap_or((0, 0)),
                amount: g.amount,
                recurrence_len: g.recurrence.map(|hourly| if hourly { HOUR } else { DAY }),
                prepaid: g.kind >= 2,
                active: false,
                rem: 0,
            }
        })
        .collect();
    let user_grants = gs.len();
    let earliest = c
        .grants
        .iter()
        .map(|g| g.eff_min * MIN)
        .chain(c.segments.iter().map(|s| s.0 * MIN))
        .chain([c.created_min * MIN, at])
        .min()
        .unwrap_or(at);
    let first_reset = earliest.div_euclid(len) * len;
    let carry_overage = c.preserve && c.mode == 1;

    let mut p = RefPeriod::default();
    let mut t = first_reset;
    while t <= at {
        let is_reset = t.rem_euclid(len) == 0;
        let mut carry = 0;
        if is_reset {
            carry = p.overage;
            p = RefPeriod::default();
            for g in gs.iter_mut().filter(|g| g.active) {
                if g.live(t) {
                    let (min, max) = g.rollover;
                    g.rem = g.rem.max(min).min(max);
                    p.fund(g.rem, g.prepaid);
                } else {
                    g.rem = 0;
                }
            }
            if let Some(issue) = c.issue {
                gs.push(RefGrant {
                    id: format!("issue/{}", t.div_euclid(len)),
                    priority: 1,
                    expires: None,
                    rank: 1,
                    created: t,
                    eff: t,
                    void: None,
                    rollover: (0, 0),
                    amount: issue,
                    recurrence_len: None,
                    prepaid: false,
                    active: true,
                    rem: issue,
                });
                p.fund(issue, false);
            }
        }
        for g in gs[..user_grants].iter_mut() {
            if g.eff == t {
                g.active = true;
                if g.live(t) {
                    g.rem = g.amount;
                    p.fund(g.amount, g.prepaid);
                }
            }
        }
        for g in gs[..user_grants].iter_mut() {
            if let Some(rlen) = g.recurrence_len
                && t > g.eff
                && (t - g.eff).rem_euclid(rlen) == 0
                && g.live(t)
            {
                let added = g.amount - g.rem;
                g.rem = g.amount;
                p.fund(added, g.prepaid);
            }
        }
        if is_reset && carry_overage && carry > 0 {
            p.overage += burn_units(&mut gs, t, carry);
        }
        for &(start, amount) in &c.segments {
            if start * MIN == t {
                p.usage += amount;
                p.overage += burn_units(&mut gs, t, amount);
            }
        }
        t += MIN;
    }

    let mut live: Vec<&RefGrant> = gs.iter().filter(|g| g.live(at) && g.rem > 0).collect();
    live.sort_by_key(|g| g.key());
    let bal: i64 = live.iter().map(|g| g.rem).sum();
    let prepaid_only = p.funded > 0 && p.funded == p.funded_prepaid;
    let has_access = if bal <= 0 && prepaid_only {
        false
    } else {
        match c.mode {
            0 => bal > 0,
            1 => true,
            // price × overage units, in micros (whole units, so exact).
            _ => c.price_micros * p.overage < c.cap_micros,
        }
    };
    let n_at = at.div_euclid(len);
    Balance {
        entitlement: id(EntitlementId::new("e1")),
        period: Period {
            start: UtcNanos(n_at * len),
            end: UtcNanos(n_at * len + len),
        },
        has_access,
        balance: Amount::from_units(bal),
        usage: Amount::from_units(p.usage),
        overage: Amount::from_units(p.overage),
        total_available: Amount::from_units(p.total),
        grants: live
            .iter()
            .map(|g| (id(GrantId::new(g.id.clone())), Amount::from_units(g.rem)))
            .collect(),
        prepaid_only,
        as_of: AsOf::Replay,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn engine_matches_brute_force_replay(c in case()) {
        let got = balance(
            &entitlement(&c),
            &grants(&c),
            &segments(&c),
            UtcNanos(c.at_min * MIN),
            AsOf::Replay,
        )
        .expect("balance");
        prop_assert_eq!(got, reference(&c));
    }

    /// Conservation within one period with no grant changes after the first
    /// instant: balance + (usage − overage) = total available.
    #[test]
    fn usage_is_conserved_without_expiry(amounts in proptest::collection::vec(0..20i64, 0..20), grant in 1..200i64) {
        let c = Case {
            daily: true,
            created_min: 0,
            mode: 1,
            price_micros: 1,
            cap_micros: 1,
            issue: None,
            preserve: false,
            grants: vec![GrantSpec {
                eff_min: 0,
                created_back_min: 0,
                amount: grant,
                priority: 1,
                kind: 1,
                expiry_hours: None,
                void_after_min: None,
                rollover: None,
                recurrence: None,
            }],
            segments: amounts.iter().enumerate().map(|(i, &a)| (i as i64 + 1, a)).collect(),
            at_min: 600,
        };
        let b = balance(&entitlement(&c), &grants(&c), &segments(&c), UtcNanos(600 * MIN), AsOf::Replay)
            .expect("balance");
        prop_assert_eq!(b.balance + (b.usage - b.overage), b.total_available);
        prop_assert!(!b.balance.is_negative());
    }
}
