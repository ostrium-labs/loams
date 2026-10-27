//! Fixtures shared by the meter role's integration tests: one meter
//! (`house_cpu_ms`, SUM of `$.cpu_ms`) feeding one feature (`house_cpu`),
//! one 30-day period, an in-memory stream partition and House.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use loams_meter::owner::{
    CounterKey, FeatureIndex, HouseTotals, OwnerError, PartitionReader, Periods,
};
use loams_meter_core::{
    Aggregation, Amount, Event, Feature, FeatureKey, JsonPath, Meter, Period, Slug, SubjectKey,
    Unit, UtcNanos,
};

pub const TYPE: &str = "io.loams.dev.usage.house.cpu.v1";
pub const PERIOD: Period = Period {
    start: UtcNanos(0),
    end: UtcNanos(30 * UtcNanos::DAY),
};

pub fn index() -> FeatureIndex {
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
pub struct OnePeriod;

impl Periods for OnePeriod {
    fn period(&self, _: &SubjectKey, _: &FeatureKey, at: UtcNanos) -> Option<Period> {
        PERIOD.contains(at).then_some(PERIOD)
    }
}

pub fn event(id: &str, subject: &str, cpu_ms: i64) -> Event {
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

pub fn key(subject: &str) -> CounterKey {
    CounterKey {
        subject: SubjectKey::new(subject).expect("subject"),
        feature: FeatureKey::new("house_cpu").expect("feature"),
        period_start: PERIOD.start,
    }
}

/// One partition of the stream, in memory.
#[derive(Debug, Default)]
pub struct Stream(pub Mutex<Vec<(u64, Event)>>);

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
pub struct House {
    pub events: Vec<(u64, Event)>,
    pub committed: u64,
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
