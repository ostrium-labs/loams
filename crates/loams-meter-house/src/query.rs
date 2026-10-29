//! `QueryMeter` and OpenMeter's query path (design §63 §8.5, §17.2; ME1
//! Task 8): `GET /api/v1/meters/{slug}/query`.
//!
//! The parameters are OpenMeter's: `from`, `to` (RFC 3339), `windowSize`
//! (`MINUTE`, `HOUR`, `DAY`, `MONTH`; none means one window over the range),
//! `windowTimeZone` (only `UTC`, ruling T8-2), `subject` (repeated),
//! `groupBy` (repeated; `subject` or a meter dimension) and
//! `filterGroupBy[<dimension>]=<value>`. The answer is OpenMeter's
//! `MeterQueryResult` plus `as_of` (the latest closed minute) and, per row,
//! `provisional` (the window reaches past `as_of`, so its value may still
//! change).
//!
//! The work is [`sql::QueryRun`]; this module parses, drives it and shapes
//! the answer. The `meter` role mounts [`openmeter_router`] on its listener
//! behind its own authentication and authorization (`meter_admin`, or
//! `billing_viewer` for the caller's own subjects, §63 §17.1).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use loams_meter_core::{Amount, Meter, UtcNanos, WindowSize};
use serde_json::{Value, json};

use crate::close::{CloseError, drive};
use crate::house::HouseSql;
use crate::sql::{QueryAnswer, QueryRun, QuerySpec, QueryWindow};

/// A meter query, with ME-core's types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterQuery {
    pub from: UtcNanos,
    pub to: UtcNanos,
    /// `None`: one window over `[from, to)`.
    pub window_size: Option<WindowSize>,
    pub subjects: Vec<String>,
    /// `subject` and/or meter dimensions.
    pub group_by: Vec<String>,
    /// `filterGroupBy[dimension]`: the dimension is one of the values.
    pub filter_group_by: BTreeMap<String, Vec<String>>,
}

/// Why a query was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The parameters are wrong (HTTP 400).
    #[error("{0}")]
    BadRequest(String),
    /// No such meter (HTTP 404).
    #[error("meter {0} not found")]
    NotFound(String),
    #[error(transparent)]
    House(#[from] CloseError),
}

/// The std-only mirror of `meter` the House SQL takes.
pub fn meter_def(meter: &Meter) -> crate::sql::MeterDef {
    use crate::sql::Aggregation as A;
    use loams_meter_core::Aggregation as C;
    crate::sql::MeterDef {
        slug: meter.slug.as_str().to_string(),
        event_type: meter.event_type.clone(),
        aggregation: match meter.aggregation {
            C::Sum => A::Sum,
            C::Count => A::Count,
            C::UniqueCount => A::UniqueCount,
            C::Avg => A::Avg,
            C::Min => A::Min,
            C::Max => A::Max,
            C::Latest => A::Latest,
        },
        value_property: meter
            .value_property
            .as_ref()
            .map(|p| p.as_str().to_string()),
        group_by: meter
            .group_by
            .iter()
            .map(|(name, path)| (name.clone(), path.as_str().to_string()))
            .collect(),
        event_from: meter.event_from.map(|t| t.0.div_euclid(UtcNanos::SECOND)),
    }
}

fn bad(message: impl Into<String>) -> QueryError {
    QueryError::BadRequest(message.into())
}

/// OpenMeter's query parameters (the raw query string: `subject` and
/// `groupBy` repeat, `filterGroupBy` is a deep object).
pub fn parse_query(raw: &str) -> Result<MeterQuery, QueryError> {
    let mut from = None;
    let mut to = None;
    let mut window_size = None;
    let mut subjects = Vec::new();
    let mut group_by = Vec::new();
    let mut filter_group_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        let value = value.into_owned();
        match key.as_ref() {
            "from" => from = Some(time(&value, "from")?),
            "to" => to = Some(time(&value, "to")?),
            "windowSize" => {
                window_size = Some(match value.as_str() {
                    "MINUTE" => WindowSize::Minute,
                    "HOUR" => WindowSize::Hour,
                    "DAY" => WindowSize::Day,
                    "MONTH" => WindowSize::Month,
                    other => {
                        return Err(bad(format!(
                            "windowSize is MINUTE, HOUR, DAY or MONTH, not {other:?}"
                        )));
                    }
                });
            }
            "windowTimeZone" => {
                if value != "UTC" && value != "Etc/UTC" {
                    return Err(bad(format!(
                        "windowTimeZone {value:?} is not supported: windows are UTC"
                    )));
                }
            }
            "subject" => subjects.push(value),
            "groupBy" => group_by.push(value),
            other => {
                let Some(dimension) = other
                    .strip_prefix("filterGroupBy[")
                    .and_then(|k| k.strip_suffix(']'))
                else {
                    return Err(bad(format!("unknown parameter {other:?}")));
                };
                filter_group_by
                    .entry(dimension.to_string())
                    .or_default()
                    .push(value);
            }
        }
    }
    let to = to.unwrap_or_else(|| UtcNanos(now_nanos()));
    let from = from.ok_or_else(|| bad("from is required"))?;
    if from >= to {
        return Err(bad("from must be before to"));
    }
    Ok(MeterQuery {
        from,
        to,
        window_size,
        subjects,
        group_by,
        filter_group_by,
    })
}

fn time(text: &str, name: &str) -> Result<UtcNanos, QueryError> {
    UtcNanos::from_rfc3339(text).map_err(|e| bad(format!("{name}: {e}")))
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The SQL's query for `meter`: dimensions checked against the meter.
pub fn query_spec(meter: &Meter, query: &MeterQuery) -> Result<QuerySpec, QueryError> {
    let is_dimension = |d: &str| meter.group_by.contains_key(d);
    let mut dims = Vec::new();
    let mut by_subject = !query.subjects.is_empty();
    for g in &query.group_by {
        if g == "subject" {
            by_subject = true;
        } else if is_dimension(g) {
            if !dims.contains(g) {
                dims.push(g.clone());
            }
        } else {
            return Err(bad(format!(
                "groupBy {g:?} is not a dimension of meter {}",
                meter.slug.as_str()
            )));
        }
    }
    let mut filters = Vec::new();
    for (dim, values) in &query.filter_group_by {
        if !is_dimension(dim) {
            return Err(bad(format!(
                "filterGroupBy[{dim}] is not a dimension of meter {}",
                meter.slug.as_str()
            )));
        }
        filters.push((dim.clone(), values.clone()));
    }
    let secs = |t: UtcNanos| t.0.div_euclid(UtcNanos::SECOND);
    Ok(QuerySpec {
        from: secs(query.from),
        // An instant inside a second still includes that second's events.
        to: secs(UtcNanos(query.to.0 + UtcNanos::SECOND - 1)),
        window: match query.window_size {
            Some(WindowSize::Minute) => QueryWindow::Minute,
            Some(WindowSize::Hour) => QueryWindow::Hour,
            Some(WindowSize::Day) => QueryWindow::Day,
            Some(WindowSize::Month) => QueryWindow::Month,
            None => QueryWindow::Whole,
        },
        subjects: query.subjects.clone(),
        by_subject,
        group_by: dims,
        filters,
    })
}

/// `MeterService.QueryMeter`: runs `query` on `meter` against the House.
pub async fn query_meter<H: HouseSql>(
    house: &H,
    meter: &Meter,
    query: &MeterQuery,
) -> Result<QueryAnswer, QueryError> {
    let spec = query_spec(meter, query)?;
    let mut run = QueryRun::new(meter_def(meter), spec);
    drive(house, meter.slug.as_str(), |answer| run.next(answer)).await?;
    Ok(run.into_answer())
}

fn rfc3339(secs: i64) -> String {
    UtcNanos(secs.saturating_mul(UtcNanos::SECOND)).to_rfc3339()
}

/// The answer as OpenMeter's `MeterQueryResult`, plus `as_of` and per-row
/// `provisional`.
pub fn openmeter_json(query: &MeterQuery, spec_dims: &[String], answer: &QueryAnswer) -> Value {
    let data: Vec<Value> = answer
        .rows
        .iter()
        .map(|row| {
            let group: serde_json::Map<String, Value> = spec_dims
                .iter()
                .zip(&row.group)
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            let value = Amount::from_raw(row.value);
            json!({
                "value": amount_number(value),
                "windowStart": rfc3339(row.window_start),
                "windowEnd": rfc3339(row.window_end),
                "subject": row.subject,
                "groupBy": group,
                "provisional": row.provisional,
            })
        })
        .collect();
    json!({
        "from": query.from.to_rfc3339(),
        "to": query.to.to_rfc3339(),
        "windowSize": query.window_size.map(|w| match w {
            WindowSize::Minute => "MINUTE",
            WindowSize::Hour => "HOUR",
            WindowSize::Day => "DAY",
            WindowSize::Month => "MONTH",
        }),
        "as_of": answer.as_of.map(rfc3339),
        "data": data,
    })
}

/// OpenMeter's `value` is a JSON number: the exact decimal when it has one,
/// else the nearest float.
fn amount_number(value: Amount) -> Value {
    let raw = value.raw();
    if raw % 1_000_000_000 == 0
        && let Ok(units) = i64::try_from(raw / 1_000_000_000)
    {
        return Value::from(units);
    }
    let float = raw as f64 / 1e9;
    serde_json::Number::from_f64(float).map_or(Value::Null, Value::Number)
}

/// Where the router finds meters by slug (ME-svc's meter store).
pub trait MeterCatalog: Send + Sync {
    fn meter(&self, slug: &str) -> Option<Meter>;
}

/// The router's state.
pub struct QueryState<H, C> {
    pub house: H,
    pub meters: C,
}

impl<H, C> std::fmt::Debug for QueryState<H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryState").finish_non_exhaustive()
    }
}

fn error_response(err: &QueryError) -> Response {
    let (status, title) = match err {
        QueryError::BadRequest(_) => (StatusCode::BAD_REQUEST, "Bad Request"),
        QueryError::NotFound(_) => (StatusCode::NOT_FOUND, "Not Found"),
        QueryError::House(_) => (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable"),
    };
    let body = json!({
        "type": "about:blank",
        "title": title,
        "status": status.as_u16(),
        "detail": err.to_string(),
    });
    (
        status,
        [("content-type", "application/problem+json")],
        body.to_string(),
    )
        .into_response()
}

/// Answers `GET /api/v1/meters/{slug}/query` for `slug` with the raw query
/// string `raw`.
pub async fn handle_query<H: HouseSql, C: MeterCatalog>(
    state: &QueryState<H, C>,
    slug: &str,
    raw: &str,
) -> Response {
    let result = async {
        let meter = state
            .meters
            .meter(slug)
            .ok_or_else(|| QueryError::NotFound(slug.to_string()))?;
        let query = parse_query(raw)?;
        let spec = query_spec(&meter, &query)?;
        let answer = query_meter(&state.house, &meter, &query).await?;
        Ok::<_, QueryError>(openmeter_json(&query, &spec.group_by, &answer))
    }
    .await;
    match result {
        Ok(body) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            body.to_string(),
        )
            .into_response(),
        Err(err) => error_response(&err),
    }
}

/// `GET /api/v1/meters/{slug}/query` (OpenMeter's `queryMeter`), for the
/// `meter` role to mount behind its authentication and authorization.
pub fn openmeter_router<H, C>(state: Arc<QueryState<H, C>>) -> Router
where
    H: HouseSql + 'static,
    C: MeterCatalog + 'static,
{
    Router::new()
        .route(
            "/api/v1/meters/{slug}/query",
            get(
                |State(state): State<Arc<QueryState<H, C>>>,
                 Path(slug): Path<String>,
                 RawQuery(raw): RawQuery| async move {
                    handle_query(&state, &slug, raw.as_deref().unwrap_or("")).await
                },
            ),
        )
        .with_state(state)
}
