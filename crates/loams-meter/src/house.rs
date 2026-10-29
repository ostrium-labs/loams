//! The role's House side (design §63 §7.5, §8; ME1 role wiring, part 2b),
//! over ME-house's `loams-meter-house`:
//!
//! - [`HouseSchedule`] applies the `_meter` schema at start (retrying until
//!   the House answers), then every [`CLOSE_EVERY`] runs the close job over
//!   every meter and, after it, the period close over every metered
//!   entitlement. Both are idempotent and safe on every instance at once.
//! - [`HouseUsage`] answers "has this meter any events" from
//!   `_meter.events`, which is what lets a plan apply change a meter nobody
//!   has used yet.
//! - [`RoleHouse`] is the House the role was given, or none: without one,
//!   every House call answers `unavailable`, and the role still serves
//!   everything else.
//! - [`query_router`] is OpenMeter's `GET /api/v1/meters/{slug}/query` over
//!   [`MeterQueries`], so it has the same authorization as `QueryMeter`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use loams_meter_core::{EntitlementKind, Interval, Slug, UtcNanos};
pub use loams_meter_house::HttpHouse;
use loams_meter_house::period::{feature_def, periods_for};
use loams_meter_house::query::{meter_def, openmeter_json, parse_query};
use loams_meter_house::sql::{self, MeterDef};
use loams_meter_house::{CloseJob, HouseError, HouseSql, PeriodCloser, PeriodSpec, ensure_schema};
use tokio::sync::Notify;

use crate::authz::Caller;
use crate::customers::Clock;
use crate::error::{MeterError, Result};
use crate::plans::MeterUsage;
use crate::queries::MeterQueries;
use crate::store::{MeterStore, Records};

/// How often the close job runs (§63 §8.2).
pub const CLOSE_EVERY: Duration = Duration::from_secs(60);

/// How long to wait before trying the schema again when the House did not
/// answer.
pub const SCHEMA_RETRY: Duration = Duration::from_secs(10);

/// The House the role was configured with, if any.
#[derive(Clone, Debug)]
pub enum RoleHouse {
    Http(HttpHouse),
    /// No House configured: every call is `unavailable`.
    Absent,
}

impl HouseSql for RoleHouse {
    async fn query(&self, sql: &str) -> std::result::Result<String, HouseError> {
        match self {
            RoleHouse::Http(h) => h.query(sql).await,
            RoleHouse::Absent => Err(HouseError::Transport(
                "the meter has no House configured (--meter-house-url)".into(),
            )),
        }
    }
}

impl RoleHouse {
    pub fn is_configured(&self) -> bool {
        matches!(self, RoleHouse::Http(_))
    }
}

/// Whether a meter has events, from `_meter.events`. A House that does
/// not answer is an error, which a plan apply turns into a refusal: never a
/// guess that the meter is unused.
#[derive(Clone)]
pub struct HouseUsage<H> {
    pub house: Arc<H>,
    pub store: Arc<dyn MeterStore>,
}

impl<H> fmt::Debug for HouseUsage<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HouseUsage").finish_non_exhaustive()
    }
}

/// The SQL that answers whether `meter` has any event.
pub fn has_events_sql(meter: &MeterDef) -> String {
    format!(
        "SELECT 1 FROM {}.events WHERE {} LIMIT 1",
        sql::DATABASE,
        sql::events_filter(meter)
    )
}

#[async_trait]
impl<H: HouseSql + 'static> MeterUsage for HouseUsage<H> {
    async fn has_events(&self, slug: &Slug) -> Result<bool> {
        let Some(meter) = self.store.meter(slug).await? else {
            return Ok(false);
        };
        let answer = self
            .house
            .query(&has_events_sql(&meter_def(&meter)))
            .await
            .map_err(|e| {
                MeterError::Unavailable(format!(
                    "cannot tell whether meter {} has events: House: {e}",
                    slug.as_str()
                ))
            })?;
        Ok(!answer.trim().is_empty())
    }
}

/// The SQL for `feature`'s usage by `subject` in `[start, end)` (Unix
/// seconds) from `_meter.events FINAL`: what a balance falls back to when
/// no counter answers.
pub fn usage_sql(subject: &str, feature: &sql::FeatureDef, start: i64, end: i64) -> String {
    format!(
        "SELECT {} FROM {}.events FINAL WHERE subject = {} AND {} AND time >= {} AND time < {}{} \
         FORMAT TabSeparated",
        sql::raw_agg_expr(&feature.meter),
        sql::DATABASE,
        sql::quote(subject),
        sql::events_filter(&feature.meter),
        sql::at64(start),
        sql::at64(end),
        sql::event_filter(feature),
    )
}

/// Period-to-date usage from the House ([`crate::run::UsageSoFar`]).
#[derive(Clone)]
pub struct HouseUsageSoFar<H> {
    pub house: Arc<H>,
    pub store: Arc<dyn MeterStore>,
}

impl<H> fmt::Debug for HouseUsageSoFar<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HouseUsageSoFar").finish_non_exhaustive()
    }
}

#[async_trait]
impl<H: HouseSql + 'static> crate::run::UsageSoFar for HouseUsageSoFar<H> {
    async fn usage(
        &self,
        e: &loams_meter_core::Entitlement,
        start: UtcNanos,
        at: UtcNanos,
    ) -> Result<loams_meter_core::Amount> {
        let unavailable = |what: String| MeterError::Unavailable(what);
        let feature = self
            .store
            .feature(&e.feature)
            .await?
            .ok_or_else(|| unavailable(format!("feature {} is gone", e.feature.as_str())))?;
        let slug = feature
            .meter
            .clone()
            .ok_or_else(|| unavailable(format!("feature {} has no meter", e.feature.as_str())))?;
        let meter = self
            .store
            .meter(&slug)
            .await?
            .ok_or_else(|| unavailable(format!("meter {} is gone", slug.as_str())))?;
        let secs = |t: UtcNanos| t.0.div_euclid(UtcNanos::SECOND);
        let answer = self
            .house
            .query(&usage_sql(
                e.subject.as_str(),
                &feature_def(&feature, &meter),
                secs(start),
                secs(at) + 1,
            ))
            .await
            .map_err(|err| unavailable(format!("House: {err}")))?;
        let text = answer.trim();
        if text.is_empty() || text == "\\N" {
            return Ok(loams_meter_core::Amount::ZERO);
        }
        loams_meter_core::Amount::parse(text)
            .map_err(|err| unavailable(format!("House answered {text:?}: {err}")))
    }
}

/// The periods to look at for an entitlement's usage period: the current
/// one and enough earlier ones to reach one that closed (two for daily and
/// longer periods; four for hourly ones, whose 2-hour grace spans two).
fn periods_back(interval: Interval) -> i64 {
    match interval {
        Interval::Hour => 4,
        _ => 2,
    }
}

/// Every meter, as the close job takes it.
pub fn meter_defs(records: &Records) -> Vec<MeterDef> {
    records.meters.iter().map(meter_def).collect()
}

/// Every metered entitlement's (subject, feature, periods), as the period
/// close takes them.
pub fn period_specs(records: &Records, now: UtcNanos) -> Vec<PeriodSpec> {
    let mut out = Vec::new();
    for e in &records.entitlements {
        let EntitlementKind::Metered { usage_period, .. } = &e.kind else {
            continue;
        };
        let Some(feature) = records
            .features
            .iter()
            .find(|f| f.key == e.feature && !f.archived)
        else {
            continue;
        };
        let Some(meter) = feature
            .meter
            .as_ref()
            .and_then(|slug| records.meters.iter().find(|m| &m.slug == slug))
        else {
            continue;
        };
        out.push(PeriodSpec {
            subject: e.subject.as_str().to_string(),
            feature: feature_def(feature, meter),
            periods: periods_for(usage_period, now, periods_back(usage_period.interval)),
        });
    }
    out
}

/// The schema at start, then the close job and the period close.
pub struct HouseSchedule<H> {
    pub house: Arc<H>,
    pub store: Arc<dyn MeterStore>,
    pub close: CloseJob,
    pub period: PeriodCloser,
    pub clock: Clock,
}

impl<H> fmt::Debug for HouseSchedule<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HouseSchedule").finish_non_exhaustive()
    }
}

impl<H: HouseSql> HouseSchedule<H> {
    pub fn new(house: Arc<H>, store: Arc<dyn MeterStore>, clock: Clock) -> Self {
        HouseSchedule {
            house,
            store,
            close: CloseJob::default(),
            period: PeriodCloser::default(),
            clock,
        }
    }

    fn now_ms(&self) -> i64 {
        (self.clock)().0.div_euclid(1_000_000)
    }

    /// One lap: the close job over every meter, then the period close over
    /// every metered entitlement. Answers how many meters and periods
    /// failed (each is logged; the next lap retries it).
    pub async fn lap(&self) -> Result<(usize, usize)> {
        let records = self.store.records().await?;
        let now = (self.clock)();
        let mut failed = (0, 0);
        for (meter, result) in self
            .close
            .run_once(&*self.house, &meter_defs(&records), self.now_ms())
            .await
        {
            if let Err(err) = result {
                failed.0 += 1;
                tracing::warn!(meter, %err, "the meter close job failed for a meter");
            }
        }
        for (subject, feature, result) in self
            .period
            .run_once(&*self.house, &period_specs(&records, now), self.now_ms())
            .await
        {
            if let Err(err) = result {
                failed.1 += 1;
                tracing::warn!(subject, feature, %err, "the meter period close failed");
            }
        }
        Ok(failed)
    }

    /// Applies the schema, retrying until the House answers, then runs a
    /// lap every [`CLOSE_EVERY`] until `stop` is notified.
    pub async fn run(self, stop: Arc<Notify>) {
        loop {
            match ensure_schema(&*self.house).await {
                Ok(()) => break,
                Err(err) => {
                    tracing::warn!(%err, "the meter's House schema could not be applied; retrying")
                }
            }
            tokio::select! {
                () = stop.notified() => return,
                () = tokio::time::sleep(SCHEMA_RETRY) => {}
            }
        }
        tracing::info!("the meter's House schema is applied");
        loop {
            if let Err(err) = self.lap().await {
                tracing::warn!(%err, "the meter's close lap could not read its records");
            }
            tokio::select! {
                () = stop.notified() => return,
                () = tokio::time::sleep(CLOSE_EVERY) => {}
            }
        }
    }
}

/// OpenMeter's `queryMeter` (`GET /api/v1/meters/{slug}/query`), answered
/// by [`MeterQueries`]: a meter admin may query every subject, anyone else
/// only the subjects of orgs they may view billing of. The caller is the
/// [`Caller`] the role's authentication put in the request's extensions.
pub fn query_router<H: HouseSql + 'static>(queries: MeterQueries<H>) -> Router {
    Router::new()
        .route(
            "/api/v1/meters/{slug}/query",
            get(
                |State(queries): State<MeterQueries<H>>,
                 caller: Option<Extension<Caller>>,
                 Path(slug): Path<String>,
                 RawQuery(raw): RawQuery| async move {
                    let instance = format!("/api/v1/meters/{slug}/query");
                    let result = async {
                        let caller = caller
                            .map(|Extension(c)| c)
                            .ok_or(MeterError::Unauthenticated)?;
                        let query = parse_query(raw.as_deref().unwrap_or(""))
                            .map_err(|e| MeterError::invalid("query", e.to_string()))?;
                        queries.query(&caller, &slug, query).await
                    }
                    .await;
                    match result {
                        Ok(a) => (
                            StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            openmeter_json(&a.query, &a.group_by, &a.answer).to_string(),
                        )
                            .into_response(),
                        Err(e) => crate::openmeter::problem(&e, &instance),
                    }
                },
            ),
        )
        .with_state(queries)
}
