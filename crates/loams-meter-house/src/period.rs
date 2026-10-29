//! Period totals and period close (design §63 §8.4; ME1 Task 7).
//!
//! `_meter.period_totals` holds, per (subject, feature, period), the value
//! that rates the period. The `meter` role runs [`PeriodCloser::run_once`]
//! after each close-job lap with every metered entitlement's subject,
//! feature and periods (the entitlement's usage period with the customer's
//! own anchor, as OpenMeter's `usagePeriod.anchor`). A period is
//! provisional until 2 hours after its end, then closed and handed to the
//! exporters (§16); a late event that re-closes one of its windows closes
//! it again with a newer version. The work is [`sql::PeriodRun`], a sans-I/O
//! machine; this module drives it.

use crate::close::{CloseError, drive};
use crate::house::HouseSql;
use crate::sql::{self, FeatureDef, PeriodConfig, PeriodRun, PeriodSummary};

/// One (subject, feature) and the periods to look at, `[start, end)` in Unix
/// seconds: the current period and enough earlier ones to reach one that
/// closed (two for monthly periods; four for hourly ones, whose 2-hour grace
/// spans two periods). [`periods_back`] makes the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodSpec {
    pub subject: String,
    pub feature: FeatureDef,
    pub periods: Vec<(i64, i64)>,
}

/// The periods `index - back ..= index` of a usage period, given its
/// `period(n)` and the index holding now, oldest first.
pub fn periods_back(index: i64, back: i64, period: impl Fn(i64) -> (i64, i64)) -> Vec<(i64, i64)> {
    (index - back.max(0)..=index).map(period).collect()
}

/// The period close of §63 §8.4.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeriodCloser {
    pub config: PeriodConfig,
}

impl PeriodCloser {
    /// One run for one (subject, feature) at `now_ms` (Unix milliseconds;
    /// also the run's version).
    pub async fn close<H: HouseSql>(
        &self,
        house: &H,
        spec: &PeriodSpec,
        now_ms: i64,
    ) -> Result<PeriodSummary, CloseError> {
        let mut run = PeriodRun::new(
            spec.subject.clone(),
            spec.feature.clone(),
            spec.periods.clone(),
            now_ms,
            self.config,
        );
        drive(house, &spec.feature.meter.slug, |answer| run.next(answer)).await?;
        Ok(run.summary().clone())
    }

    /// One run over every spec; one failing does not stop the others.
    pub async fn run_once<H: HouseSql>(
        &self,
        house: &H,
        specs: &[PeriodSpec],
        now_ms: i64,
    ) -> Vec<(String, String, Result<PeriodSummary, CloseError>)> {
        let mut out = Vec::with_capacity(specs.len());
        for spec in specs {
            let result = self.close(house, spec, now_ms).await;
            out.push((spec.subject.clone(), spec.feature.key.clone(), result));
        }
        out
    }
}

/// The closed totals of `subject` (every feature) for the period starting at
/// `start` (Unix seconds), for the exporters: `feature\tvalue\tclose_run`.
pub fn closed_totals_sql(subject: &str, start: i64) -> String {
    format!(
        "SELECT feature, value, close_run FROM {}.period_totals FINAL \
         WHERE subject = {} AND period_start = {} AND closed = 1 ORDER BY feature",
        sql::DATABASE,
        sql::quote(subject),
        sql::at(start)
    )
}

/// The std-only mirror of `feature` (counting `meter`) the period SQL takes.
/// A `ByteHours` feature sums the meter's hourly values (§63 §8.3).
pub fn feature_def(
    feature: &loams_meter_core::Feature,
    meter: &loams_meter_core::Meter,
) -> FeatureDef {
    FeatureDef {
        key: feature.key.as_str().to_string(),
        meter: crate::query::meter_def(meter),
        filter: feature
            .filter
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        byte_hours: feature.unit == loams_meter_core::Unit::ByteHours,
    }
}

/// The periods of `usage` to look at `now`, oldest first, as `[start, end)`
/// Unix seconds: the current one and `back` earlier ones (with the
/// customer's anchor, calendar months clamped as ME-core's `UsagePeriod`
/// does).
pub fn periods_for(
    usage: &loams_meter_core::UsagePeriod,
    now: loams_meter_core::UtcNanos,
    back: i64,
) -> Vec<(i64, i64)> {
    let secs = |t: loams_meter_core::UtcNanos| t.0.div_euclid(loams_meter_core::UtcNanos::SECOND);
    periods_back(usage.index_at(now), back, |n| {
        let p = usage.period(n);
        (secs(p.start), secs(p.end))
    })
}
