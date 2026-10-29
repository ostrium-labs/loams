//! `loams.meter.v1.MeterService` over [`MeterQueries`]: `QueryMeter`, the
//! Connect face of the same query OpenMeter's
//! `GET /api/v1/meters/{slug}/query` runs.

use std::sync::Arc;

use buffa::enumeration::EnumValue;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_meter_core::{Amount, UtcNanos, WindowSize};
use loams_meter_house::HouseSql;
use loams_meter_house::query::MeterQuery;
use loams_proto::loams::meter::v1 as pb;
use loams_proto::loams::meter::v1::{MeterService, MeterServiceExt};

use super::{IntoConnect, caller, timestamp, utc};
use crate::error::MeterError;
use crate::queries::{MeterQueries, QueryAnswered};

struct Adapter<H>(MeterQueries<H>);

impl<H> std::fmt::Debug for Adapter<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Adapter").field(&self.0).finish()
    }
}

/// Registers `MeterService` on `router`. The `meter` role calls it beside
/// [`super::register`].
pub fn register<H: HouseSql + 'static>(
    router: connectrpc::Router,
    queries: MeterQueries<H>,
) -> connectrpc::Router {
    MeterServiceExt::register(Arc::new(Adapter(queries)), router)
}

fn window_in(w: &EnumValue<pb::WindowSize>) -> Result<Option<WindowSize>, MeterError> {
    match w.as_known() {
        Some(pb::WindowSize::WINDOW_SIZE_UNSPECIFIED) => Ok(None),
        Some(pb::WindowSize::WINDOW_SIZE_MINUTE) => Ok(Some(WindowSize::Minute)),
        Some(pb::WindowSize::WINDOW_SIZE_HOUR) => Ok(Some(WindowSize::Hour)),
        Some(pb::WindowSize::WINDOW_SIZE_DAY) => Ok(Some(WindowSize::Day)),
        Some(pb::WindowSize::WINDOW_SIZE_MONTH) => Ok(Some(WindowSize::Month)),
        None => Err(MeterError::invalid("window_size", "unknown window size")),
    }
}

fn window_out(w: Option<WindowSize>) -> pb::WindowSize {
    match w {
        None => pb::WindowSize::WINDOW_SIZE_UNSPECIFIED,
        Some(WindowSize::Minute) => pb::WindowSize::WINDOW_SIZE_MINUTE,
        Some(WindowSize::Hour) => pb::WindowSize::WINDOW_SIZE_HOUR,
        Some(WindowSize::Day) => pb::WindowSize::WINDOW_SIZE_DAY,
        Some(WindowSize::Month) => pb::WindowSize::WINDOW_SIZE_MONTH,
    }
}

fn now() -> UtcNanos {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    UtcNanos(nanos)
}

/// The request as the query the House code takes.
pub fn query_in(r: &pb::QueryMeterRequest) -> Result<MeterQuery, MeterError> {
    let from = r
        .from
        .as_option()
        .map(utc)
        .ok_or_else(|| MeterError::invalid("from", "from is required"))?;
    let to = r.to.as_option().map_or_else(now, utc);
    if !r.window_time_zone.is_empty()
        && r.window_time_zone != "UTC"
        && r.window_time_zone != "Etc/UTC"
    {
        return Err(MeterError::invalid(
            "window_time_zone",
            format!("{:?} is not supported: windows are UTC", r.window_time_zone),
        ));
    }
    let mut filter_group_by = std::collections::BTreeMap::<String, Vec<String>>::new();
    for f in &r.filters {
        if f.dimension.is_empty() {
            return Err(MeterError::invalid(
                "filters",
                "a filter names its dimension",
            ));
        }
        filter_group_by
            .entry(f.dimension.clone())
            .or_default()
            .extend(f.values.iter().cloned());
    }
    Ok(MeterQuery {
        from,
        to,
        window_size: window_in(&r.window_size)?,
        subjects: r.subjects.clone(),
        group_by: r.group_by.clone(),
        filter_group_by,
    })
}

/// The answer as the response.
pub fn response_out(a: &QueryAnswered) -> pb::QueryMeterResponse {
    let rows = a
        .answer
        .rows
        .iter()
        .map(|row| pb::MeterQueryRow {
            value: Amount::from_raw(row.value).to_string(),
            window_start: timestamp(UtcNanos::from_secs(row.window_start)).into(),
            window_end: timestamp(UtcNanos::from_secs(row.window_end)).into(),
            subject: row.subject.clone().unwrap_or_default(),
            group_by: a
                .group_by
                .iter()
                .zip(&row.group)
                .map(|(dimension, value)| pb::DimensionValue {
                    dimension: dimension.clone(),
                    value: value.clone(),
                    ..Default::default()
                })
                .collect(),
            provisional: row.provisional,
            ..Default::default()
        })
        .collect();
    pb::QueryMeterResponse {
        from: timestamp(a.query.from).into(),
        to: timestamp(a.query.to).into(),
        window_size: EnumValue::Known(window_out(a.query.window_size)),
        as_of: a
            .answer
            .as_of
            .map(|s| timestamp(UtcNanos::from_secs(s)))
            .into(),
        rows,
        ..Default::default()
    }
}

impl<H: HouseSql + 'static> MeterService for Adapter<H> {
    async fn query_meter(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::QueryMeterRequest>,
    ) -> ServiceResult<pb::QueryMeterResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let query = query_in(&r).connect()?;
        let answered = self.0.query(&caller, &r.meter, query).await.connect()?;
        Response::ok(response_out(&answered))
    }
}
