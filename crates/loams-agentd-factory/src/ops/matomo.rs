//! Matomo 5: `GET /index.php?module=API&method=…&format=JSON`, the token as a bearer
//! header, and an error inside a 200 body (`{"result":"error","message":…}`).

use serde_json::{Map, Value};

use super::{Connection, need, ok_status};
use crate::client::{Auth, Client, Query, json_of};
use crate::error::OpError;
use crate::json::{Obj, get, num_of, numbers, str_of};
use crate::params::{Params, clamp};

const PERIODS: &[&str] = &["day", "week", "month", "year"];
/// `API_datatable_default_limit`.
const DEFAULT_ROW_LIMIT: i64 = 100;

fn site_ok(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `today|yesterday|lastN|YYYY-MM-DD[,YYYY-MM-DD]` (the range only when `range`).
fn date_ok(s: &str, range: bool) -> bool {
    let day = |d: &str| {
        let b = d.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    };
    if s == "today" || s == "yesterday" {
        return true;
    }
    if let Some(n) = s.strip_prefix("last") {
        return (1..=3).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit());
    }
    match s.split_once(',') {
        Some((a, b)) if range => day(a) && day(b),
        Some(_) => false,
        None => day(s),
    }
}

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "visits" => {
            let p = Params::strict(params, &["idSite", "period", "date"])?;
            p.matching("idSite", site_ok)?;
            p.one_of("period", PERIODS)?;
            p.matching("date", |d| date_ok(d, true))?;
            Ok(p)
        }
        "pages" => {
            let p = Params::strict(params, &["idSite", "period", "date", "limit"])?;
            p.matching("idSite", site_ok)?;
            p.one_of("period", PERIODS)?;
            p.matching("date", |d| date_ok(d, false))?;
            p.int_min1("limit")?;
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    Client::new(
        &conn.url,
        Auth::Bearer(conn.value("apiToken")),
        conn.upstream.clone(),
    )
}

fn error_message(body: &Value) -> Option<String> {
    let map = body.as_object()?;
    if map.get("result").and_then(Value::as_str) != Some("error") {
        return None;
    }
    Some(
        map.get("message")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .unwrap_or("(Matomo returned result=error with no message)")
            .to_owned(),
    )
}

/// `_request`: the fixed params first, then the method's.
async fn request(
    c: &Client,
    method: &str,
    params: Vec<(&'static str, Option<String>)>,
    row_limit: i64,
) -> Result<Value, OpError> {
    let mut q: Query<'_> = vec![
        ("module", Some("API".into())),
        ("method", Some(method.into())),
        ("format", Some("JSON".into())),
        ("format_metrics", Some("0".into())),
        ("filter_limit", Some(row_limit.to_string())),
    ];
    q.extend(params);
    let response = c.get_raw("/index.php", &q).await?;
    let body = match json_of("/index.php", &response) {
        Ok(body) => body,
        Err(err) => {
            let parsed: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
            if let Some(message) = error_message(&parsed) {
                return Err(OpError {
                    status: Some(response.status),
                    ..OpError::matomo(format!("matomo: {method} failed: {message}"))
                });
            }
            return Err(err);
        }
    };
    if let Some(message) = error_message(&body) {
        return Err(OpError::matomo(format!(
            "matomo: {method} failed: {message}"
        )));
    }
    Ok(body)
}

/// A period label: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optionally with a time.
fn period_key(key: &str) -> bool {
    let (date, time) = match key.find([' ', 'T']) {
        Some(at) => (&key[..at], Some(&key[at + 1..])),
        None => (key, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    let date_ok = !parts.is_empty()
        && parts.len() <= 3
        && parts[0].len() == 4
        && parts[1..].iter().all(|p| p.len() == 2)
        && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()));
    let time_ok = time.is_none_or(|t| {
        let t: Vec<&str> = t.split(':').collect();
        (2..=3).contains(&t.len())
            && t.iter()
                .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))
    });
    date_ok && time_ok
}

/// `normaliseMatomoResponse`: any of Matomo's five shapes as rows.
fn rows_of(input: &Value) -> Vec<Value> {
    let with = |row: &Value, date: Option<&str>, site: Option<&str>| {
        let mut map = match row {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };
        if let Some(date) = date {
            map.insert("date".into(), Value::String(date.into()));
        }
        if let Some(site) = site {
            map.insert("siteId".into(), Value::String(site.into()));
        }
        Value::Object(map)
    };
    match input {
        Value::Array(items) => {
            if items.iter().all(Value::is_object) {
                items.iter().map(|r| with(r, None, None)).collect()
            } else {
                Vec::new()
            }
        }
        Value::Object(map) => {
            let nested: Vec<(&String, &Map<String, Value>)> = map
                .iter()
                .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
                .collect();
            if nested.is_empty() {
                return if map.is_empty() {
                    Vec::new()
                } else {
                    vec![with(input, None, None)]
                };
            }
            let second = nested.iter().any(|(_, v)| v.values().any(Value::is_object));
            if !second && map.keys().all(|k| period_key(k)) {
                let mut dates: Vec<&String> = map.keys().collect();
                dates.sort();
                return dates
                    .into_iter()
                    .map(|d| {
                        with(
                            map.get(d).filter(|v| v.is_object()).unwrap_or(&Value::Null),
                            Some(d),
                            None,
                        )
                    })
                    .collect();
            }
            let mut sites: Vec<&String> = map.keys().collect();
            sites.sort();
            let mut rows = Vec::new();
            for site in sites {
                let by_date = map
                    .get(site)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let is_date_map = !by_date.is_empty()
                    && by_date.keys().all(|k| period_key(k))
                    && by_date.values().all(Value::is_object);
                if !is_date_map {
                    rows.push(with(&Value::Object(by_date), None, Some(site)));
                    continue;
                }
                let mut dates: Vec<&String> = by_date.keys().collect();
                dates.sort();
                for d in dates {
                    rows.push(with(&by_date[d], Some(d), Some(site)));
                }
            }
            rows
        }
        _ => Vec::new(),
    }
}

fn site(p: &Params, conn: &Connection) -> Result<String, OpError> {
    need(
        p.matching("idSite", site_ok)?
            .or_else(|| conn.field("idSite")),
        "idSite",
    )
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "visits" => {
            let id = site(p, conn)?;
            let body = request(
                &c,
                "VisitsSummary.get",
                vec![
                    ("idSite", Some(id)),
                    (
                        "period",
                        Some(p.one_of("period", PERIODS)?.unwrap_or_else(|| "day".into())),
                    ),
                    (
                        "date",
                        Some(
                            p.matching("date", |d| date_ok(d, true))?
                                .unwrap_or_else(|| "last7".into()),
                        ),
                    ),
                ],
                DEFAULT_ROW_LIMIT,
            )
            .await?;
            Ok(Value::Array(
                rows_of(&body)
                    .iter()
                    .map(|row| {
                        Obj::new()
                            .opt(
                                "date",
                                str_of(get(row, "date")).or_else(|| str_of(get(row, "label"))),
                            )
                            .extend(numbers(Some(row)))
                            .done()
                    })
                    .collect(),
            ))
        }
        "pages" => {
            let id = site(p, conn)?;
            let limit = clamp(p.int_min1("limit")?, 10);
            let body = request(
                &c,
                "Actions.getPageUrls",
                vec![
                    ("idSite", Some(id)),
                    (
                        "period",
                        Some(p.one_of("period", PERIODS)?.unwrap_or_else(|| "day".into())),
                    ),
                    (
                        "date",
                        Some(
                            p.matching("date", |d| date_ok(d, false))?
                                .unwrap_or_else(|| "today".into()),
                        ),
                    ),
                ],
                limit,
            )
            .await?;
            Ok(Value::Array(
                rows_of(&body)
                    .iter()
                    .map(|row| {
                        Obj::new()
                            .opt("label", str_of(get(row, "label")))
                            .opt("hits", num_of(get(row, "nb_hits")))
                            .opt("visits", num_of(get(row, "nb_visits")))
                            .done()
                    })
                    .collect(),
            ))
        }
        "health" => {
            let body = request(&c, "API.getMatomoVersion", vec![], DEFAULT_ROW_LIMIT).await?;
            // A bare scalar, or the JSON renderer's {"value": …}.
            let scalar = match &body {
                Value::Object(m) if m.len() == 1 && m.contains_key("value") => {
                    !matches!(m["value"], Value::Object(_) | Value::Array(_))
                }
                Value::Object(_) | Value::Array(_) => false,
                _ => true,
            };
            if !scalar {
                return Err(OpError::matomo(
                    "matomo: API.getMatomoVersion did not return a scalar",
                ));
            }
            Ok(ok_status())
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_of_reads_matomo_shapes() {
        assert_eq!(
            rows_of(&json!({"2026-10-18": {"nb_visits": 3}, "2026-10-19": []})),
            vec![
                json!({"nb_visits": 3, "date": "2026-10-18"}),
                json!({"date": "2026-10-19"}),
            ]
        );
        assert_eq!(
            rows_of(&json!([{"label": "/a"}])),
            vec![json!({"label": "/a"})]
        );
        assert_eq!(rows_of(&json!({})), Vec::<Value>::new());
        assert_eq!(
            rows_of(&json!({"nb_visits": 1})),
            vec![json!({"nb_visits": 1})]
        );
        assert!(period_key("2026-10-18") && period_key("2026") && !period_key("site1"));
        assert!(date_ok("last7", true) && date_ok("2026-01-01,2026-01-31", true));
        assert!(!date_ok("2026-01-01,2026-01-31", false));
    }
}
