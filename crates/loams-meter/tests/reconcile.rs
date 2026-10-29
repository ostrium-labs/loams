//! Reconciliation (ME1 Task 13): the counters are reset to the House's
//! deduplicated totals at the pipe's committed offset plus the tail after
//! it, so each event counts once.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use common::{House, OnePeriod, Stream, event, index, key};
use loams_meter::owner::{BUCKET_SPAN, CounterKey, CounterOwners, OwnershipChange};
use loams_meter::reconcile::{Allowances, reconcile};
use loams_meter_core::{Amount, Event};

/// Every counter is allowed 1,000 units.
#[derive(Debug)]
struct Thousand;

impl Allowances for Thousand {
    fn allowance(&self, _: &CounterKey) -> Option<Amount> {
        Some(Amount::from_units(1_000))
    }
}

/// 200 events, one every 1,000 offsets (so across four buckets), three
/// subjects, plus a retried append of `e10` 50,500 offsets later.
fn stream() -> Vec<(u64, Event)> {
    let mut out: Vec<(u64, Event)> = (0..200u64)
        .map(|i| {
            let subject = ["org_a", "org_b", "org_c"][(i % 3) as usize];
            (
                i * 1_000,
                event(&format!("e{i}"), subject, (i as i64 % 7) + 1),
            )
        })
        .collect();
    let dup = out[10].1.clone();
    out.push((60_500, dup));
    out.sort_by_key(|(o, _)| *o);
    out
}

fn exact(events: &[(u64, Event)]) -> BTreeMap<String, Amount> {
    let mut seen = HashSet::new();
    let mut sums = BTreeMap::new();
    for (_, e) in events {
        if seen.insert(e.dedup_key()) {
            let cpu = e.data["cpu_ms"].as_i64().expect("cpu");
            *sums
                .entry(e.subject.as_str().to_string())
                .or_insert(Amount::ZERO) += Amount::from_units(cpu);
        }
    }
    sums
}

#[tokio::test]
async fn reconcile_splits_at_pipe_offset() {
    let events = stream();
    let reader = Stream(Mutex::new(events.clone()));
    // Rebuilt when the House had committed offset 50,000: `e10` (offset
    // 10,000) is on the House side and its retry (60,500) in the tail, out
    // of dedup layer 2's sight, so it is counted twice.
    let house = House {
        events: events.clone(),
        committed: 50_000,
    };
    let mut owners = CounterOwners::new();
    owners
        .on_change(
            OwnershipChange::Gained {
                partition: 0,
                epoch: 1,
            },
            &house,
        )
        .await
        .expect("rebuild");
    owners
        .catch_up(0, &reader, &index(), &OnePeriod)
        .await
        .expect("tail");
    let want = exact(&events);
    let doubled = owners.get(0).expect("owned").counted(&key("org_b"));
    assert_eq!(doubled, want["org_b"] + Amount::from_units(4), "e10 twice");

    // The House has moved on to 100,500, in the middle of bucket 1: the
    // split replays 100,501..=131,071 and adds the buckets after it.
    let house = House {
        events: events.clone(),
        committed: 100_500,
    };
    const { assert!(100_500 > BUCKET_SPAN && 100_500 < 2 * BUCKET_SPAN) };
    let counters = owners.get_mut(0).expect("owned");
    let report = reconcile(counters, &house, &reader, &index(), &OnePeriod, &Thousand)
        .await
        .expect("reconcile");
    assert_eq!(report.split, Some((1, 100_500)));
    assert_eq!(report.replayed, 31, "offsets 101,000..=131,000");
    for (subject, sum) in &want {
        assert_eq!(counters.counted(&key(subject)), *sum, "{subject}");
    }
    // Only org_b drifted, by e10's 4 units: under 1% of 1,000.
    assert_eq!(report.drifts.len(), 1);
    let d = &report.drifts[0];
    assert_eq!(d.key, key("org_b"));
    assert_eq!(d.amount(), Amount::from_units(4));
    assert_eq!(report.alerts().count(), 0);

    // A second run at the same split changes nothing.
    let report = reconcile(counters, &house, &reader, &index(), &OnePeriod, &Thousand)
        .await
        .expect("again");
    assert!(report.drifts.is_empty(), "{:?}", report.drifts);
    for (subject, sum) in &want {
        assert_eq!(counters.counted(&key(subject)), *sum, "{subject}");
    }
}

#[tokio::test]
async fn a_house_without_a_commit_changes_nothing() {
    #[derive(Debug)]
    struct Empty;
    #[async_trait::async_trait]
    impl loams_meter::owner::HouseTotals for Empty {
        async fn committed(
            &self,
            _: u32,
        ) -> Result<Option<(u64, i64)>, loams_meter::owner::OwnerError> {
            Ok(None)
        }
        async fn totals(
            &self,
            _: u32,
            _: u64,
            _: i64,
        ) -> Result<Vec<(CounterKey, Amount)>, loams_meter::owner::OwnerError> {
            Ok(Vec::new())
        }
    }
    let events = stream();
    let reader = Stream(Mutex::new(events));
    let mut owners = CounterOwners::new();
    owners
        .on_change(
            OwnershipChange::Gained {
                partition: 0,
                epoch: 1,
            },
            &Empty,
        )
        .await
        .expect("rebuild");
    owners
        .catch_up(0, &reader, &index(), &OnePeriod)
        .await
        .expect("read");
    let before = owners.get(0).expect("owned").counted(&key("org_a"));
    let report = reconcile(
        owners.get_mut(0).expect("owned"),
        &Empty,
        &reader,
        &index(),
        &OnePeriod,
        &Thousand,
    )
    .await
    .expect("reconcile");
    assert_eq!(report.split, None);
    assert_eq!(owners.get(0).expect("owned").counted(&key("org_a")), before);
}
