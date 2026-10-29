//! Counter owners (ME1 Task 12): the rebuild from the House and the stream
//! tail counts every event once, and a lost partition's counters go away.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use loams_meter::owner::{
    Applied, CounterKey, CounterOwners, FeatureIndex, HouseTotals, OwnerError, OwnershipChange,
    PartitionCounters, PartitionReader, Periods,
};
use loams_meter_core::{
    Aggregation, Amount, Event, Feature, FeatureKey, JsonPath, Meter, Period, Slug, SubjectKey,
    Unit, UtcNanos,
};

const TYPE: &str = "io.loams.dev.usage.house.cpu.v1";
const PERIOD: Period = Period {
    start: UtcNanos(0),
    end: UtcNanos(30 * UtcNanos::DAY),
};

fn index() -> FeatureIndex {
    let meter = Meter {
        slug: Slug::new("house_cpu_ms").expect("slug"),
        event_type: TYPE.into(),
        aggregation: Aggregation::Sum,
        value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: BTreeMap::new(),
        event_from: None,
    };
    let feature = Feature {
        key: FeatureKey::new("house_cpu").expect("feature"),
        meter: Some(meter.slug.clone()),
        filter: BTreeMap::new(),
        unit: Unit::Ms,
        archived: false,
    };
    FeatureIndex::new(&[meter], &[feature])
}

/// Every subject is entitled, one 30-day period from 0.
#[derive(Debug)]
struct OnePeriod;

impl Periods for OnePeriod {
    fn period(&self, _: &SubjectKey, _: &FeatureKey, at: UtcNanos) -> Option<Period> {
        PERIOD.contains(at).then_some(PERIOD)
    }
}

fn event(id: &str, subject: &str, cpu_ms: i64) -> Event {
    let mut data = serde_json::Map::new();
    data.insert("cpu_ms".into(), cpu_ms.into());
    Event {
        id: id.into(),
        source: "/house/node-1".into(),
        r#type: TYPE.into(),
        subject: SubjectKey::new(subject).expect("subject"),
        time: UtcNanos(UtcNanos::HOUR),
        tenant: None,
        lease: None,
        data,
    }
}

fn key(subject: &str) -> CounterKey {
    CounterKey {
        subject: SubjectKey::new(subject).expect("subject"),
        feature: FeatureKey::new("house_cpu").expect("feature"),
        period_start: PERIOD.start,
    }
}

/// One partition of the stream, in memory.
#[derive(Debug, Default)]
struct Stream(Mutex<Vec<(u64, Event)>>);

#[async_trait]
impl PartitionReader for Stream {
    async fn read(
        &self,
        _partition: u32,
        from: u64,
        max: usize,
    ) -> Result<Vec<(u64, Event)>, OwnerError> {
        Ok(self
            .0
            .lock()
            .expect("lock")
            .iter()
            .filter(|(o, _)| *o >= from)
            .take(max)
            .cloned()
            .collect())
    }
}

/// The House: `_meter.events FINAL` (deduplicated by event id and source)
/// over the stream's events at offsets `<= committed`.
#[derive(Debug)]
struct House {
    events: Vec<(u64, Event)>,
    committed: u64,
}

#[async_trait]
impl HouseTotals for House {
    async fn committed(&self, _partition: u32) -> Result<Option<(u64, i64)>, OwnerError> {
        Ok(Some((self.committed, 1)))
    }

    async fn totals(
        &self,
        _partition: u32,
        committed: u64,
        _snapshot: i64,
    ) -> Result<Vec<(CounterKey, Amount)>, OwnerError> {
        let mut seen = HashSet::new();
        let mut totals: BTreeMap<CounterKey, Amount> = BTreeMap::new();
        let index = index();
        for (offset, e) in &self.events {
            if *offset > committed || !seen.insert(e.dedup_key()) {
                continue;
            }
            for (_, amount) in index.amounts(e) {
                *totals
                    .entry(key(e.subject.as_str()))
                    .or_insert(Amount::ZERO) += amount;
            }
        }
        Ok(totals.into_iter().collect())
    }
}

/// A stream with retried appends (each duplicate right after its
/// original), three subjects, 300 events.
fn stream() -> Vec<(u64, Event)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for i in 0..300 {
        let subject = ["org_a", "org_b", "org_c"][i % 3];
        let e = event(&format!("e{i}"), subject, (i as i64 % 7) + 1);
        out.push((offset, e.clone()));
        offset += 1;
        if i % 17 == 0 {
            out.push((offset, e));
            offset += 1;
        }
    }
    out
}

fn expected(events: &[(u64, Event)]) -> BTreeMap<String, Amount> {
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
async fn owner_rebuild_from_house_and_tail() {
    let events = stream();
    // A committed offset that does not split a retried pair.
    let committed = events
        .windows(2)
        .find(|w| w[0].0 > 150 && w[0].1.id != w[1].1.id)
        .map(|w| w[0].0)
        .expect("a split point");
    let house = House {
        events: events.clone(),
        committed,
    };
    let reader = Stream(Mutex::new(events.clone()));
    let mut owners = CounterOwners::new();
    owners
        .on_change(
            OwnershipChange::Gained {
                partition: 3,
                epoch: 7,
            },
            &house,
        )
        .await
        .expect("rebuild");
    assert_eq!(owners.get(3).expect("owned").next_offset(), committed + 1);
    let read = owners
        .catch_up(3, &reader, &index(), &OnePeriod)
        .await
        .expect("tail");
    assert_eq!(read as u64, events.len() as u64 - committed - 1);

    let counters = owners.get(3).expect("owned");
    assert_eq!(counters.epoch(), 7);
    for (subject, sum) in expected(&events) {
        assert_eq!(counters.counted(&key(&subject)), sum, "{subject}");
    }
    assert!(counters.duplicates() > 0, "the tail had retried appends");

    // Reading again changes nothing: every offset was applied.
    owners
        .catch_up(3, &reader, &index(), &OnePeriod)
        .await
        .expect("again");
    for (subject, sum) in expected(&events) {
        assert_eq!(owners.get(3).expect("owned").counted(&key(&subject)), sum);
    }

    // Losing the partition at that epoch drops the counters; a stale loss
    // (an older epoch) does not.
    owners
        .on_change(
            OwnershipChange::Lost {
                partition: 3,
                epoch: 6,
            },
            &house,
        )
        .await
        .expect("stale");
    assert!(owners.get(3).is_some());
    owners
        .on_change(
            OwnershipChange::Lost {
                partition: 3,
                epoch: 7,
            },
            &house,
        )
        .await
        .expect("lost");
    assert!(owners.get(3).is_none());
}

#[test]
fn replayed_offsets_and_duplicates_are_not_counted() {
    let mut c = PartitionCounters::rebuild(0, 1, Vec::new(), Some(9));
    let e = event("x", "org_a", 5);
    assert_eq!(c.apply(9, &e, &index(), &OnePeriod), Applied::Replayed);
    assert!(matches!(
        c.apply(10, &e, &index(), &OnePeriod),
        Applied::Counted { .. }
    ));
    assert_eq!(c.apply(11, &e, &index(), &OnePeriod), Applied::Duplicate);
    assert_eq!(c.counted(&key("org_a")), Amount::from_units(5));
    // The bucket holds it for reconciliation's tail.
    assert_eq!(c.tail(&key("org_a"), 0).whole, Amount::ZERO);
    assert_eq!(
        c.tail(&key("org_a"), 0).replay,
        Some(1..=loams_meter::owner::BUCKET_SPAN - 1)
    );
}

#[test]
fn events_outside_any_entitlement_feed_no_counter() {
    #[derive(Debug)]
    struct NoPeriods;
    impl Periods for NoPeriods {
        fn period(&self, _: &SubjectKey, _: &FeatureKey, _: UtcNanos) -> Option<Period> {
            None
        }
    }
    let mut c = PartitionCounters::rebuild(0, 1, Vec::new(), None);
    let applied = c.apply(0, &event("y", "org_a", 5), &index(), &NoPeriods);
    assert_eq!(
        applied,
        Applied::Counted {
            amounts: vec![],
            lease: None
        }
    );
    assert_eq!(c.counted(&key("org_a")), Amount::ZERO);
}
