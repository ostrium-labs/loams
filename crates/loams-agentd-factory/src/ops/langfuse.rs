//! Langfuse: `/api/public`, HTTP Basic with the public and secret keys.
//!
//! Recent traces read `/v2/observations` (Langfuse 4, or 3.x on its v4 data model)
//! and fall back to the legacy `/traces` list on a 404 (owner ruling, 2026-10-18).
//! Daily usage reads `/metrics/daily`, gone in Langfuse 4, and then `/v2/metrics`.

use serde_json::Value;

use super::{Connection, OpContext, ok_status};
use crate::client::{Auth, Client, Query};
use crate::error::OpError;
use crate::json::{Obj, arr, get, num_of, numbers, str_of};
use crate::params::{Params, clamp};

const PREFIX: &str = "/api/public";

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "traces" => {
            let p = Params::strict(params, &["limit", "fromStartTime"])?;
            p.int_min1("limit")?;
            p.str_max("fromStartTime", 40)?;
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    Client::new(
        &conn.url,
        Auth::Basic {
            username: conn.value("publicKey"),
            password: conn.value("secretKey"),
        },
        conn.upstream.clone(),
    )
}

/// Strict ISO 8601 with an offset (`2026-01-01T00:00:00Z`), as the adapter requires.
fn strict_iso(value: &str, name: &str) -> Result<String, OpError> {
    let b = value.as_bytes();
    let ok = b.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && (b.ends_with(b"Z") || {
            let tail = &value[value.len().saturating_sub(6)..];
            (tail.starts_with('+') || tail.starts_with('-')) && tail.as_bytes()[3] == b':'
        });
    if ok {
        Ok(value.to_owned())
    } else {
        Err(OpError::other(format!(
            "langfuse: {name} must be an ISO 8601 timestamp with an offset; got {value}"
        )))
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time in milliseconds.
fn iso_seconds(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn data_of(body: &Value) -> Vec<Value> {
    arr(get(body, "data"))
}

pub(super) async fn run(
    op: &str,
    p: &Params,
    conn: &Connection,
    ctx: OpContext,
) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "traces" => {
            let limit = clamp(p.int_min1("limit")?, 20);
            let from = match p.str_max("fromStartTime", 40)? {
                Some(t) => Some(strict_iso(&t, "fromStartTime")?),
                None => None,
            };
            let q: Query<'_> = vec![
                ("limit", Some(limit.to_string())),
                ("isRootObservation", Some("true".into())),
                ("fromStartTime", from.clone()),
            ];
            match c.get(&format!("{PREFIX}/v2/observations"), &q).await {
                Ok(body) => {
                    return Ok(Value::Array(
                        data_of(&body)
                            .iter()
                            .map(|o| {
                                Obj::new()
                                    .opt("id", str_of(get(o, "id")))
                                    .opt("traceId", str_of(get(o, "traceId")))
                                    .opt("name", str_of(get(o, "name")))
                                    .opt("startTime", str_of(get(o, "startTime")))
                                    .opt("level", str_of(get(o, "level")))
                                    .opt(
                                        "totalCost",
                                        num_of(get(o, "totalCost"))
                                            .or_else(|| num_of(get(o, "calculatedTotalCost"))),
                                    )
                                    .done()
                            })
                            .collect(),
                    ));
                }
                Err(e) if e.is_not_found() => {}
                Err(e) => return Err(e),
            }
            // A self-hosted 3.x off the v4 data model: its legacy trace list.
            let q: Query<'_> = vec![
                ("limit", Some(limit.to_string())),
                ("page", None),
                ("fromTimestamp", from),
            ];
            match c.get(&format!("{PREFIX}/traces"), &q).await {
                Ok(body) => Ok(Value::Array(
                    data_of(&body)
                        .iter()
                        .map(|t| {
                            Obj::new()
                                .opt("id", str_of(get(t, "id")))
                                .opt("traceId", str_of(get(t, "id")))
                                .opt("name", str_of(get(t, "name")))
                                .opt("startTime", str_of(get(t, "timestamp")))
                                .opt("totalCost", num_of(get(t, "totalCost")))
                                .done()
                        })
                        .collect(),
                )),
                Err(e) if e.is_not_found() => Err(OpError::unsupported(
                    "This Langfuse serves neither its observations API nor its trace list. Open the app to see its traces.",
                )),
                Err(e) => Err(e),
            }
        }
        "daily" => {
            match c.get(&format!("{PREFIX}/metrics/daily"), &vec![]).await {
                Ok(body) => {
                    return Ok(Obj::new()
                        .put("available", true)
                        .put(
                            "days",
                            Value::Array(
                                data_of(&body)
                                    .iter()
                                    .take(120)
                                    .map(|d| {
                                        Obj::new()
                                            .opt("date", str_of(get(d, "date")))
                                            .extend(numbers(Some(d)))
                                            .done()
                                    })
                                    .collect(),
                            ),
                        )
                        .done());
                }
                Err(e) if e.is_not_found() => {}
                Err(e) => return Err(e),
            }
            let to = iso_seconds(ctx.now_ms);
            let from = iso_seconds(ctx.now_ms - 30 * 86_400_000);
            // On the wire each metric is { measure, aggregation }; key order as the
            // TypeScript adapter sends it.
            let query = format!(
                r#"{{"view":"observations","metrics":[{{"measure":"count","aggregation":"count"}},{{"measure":"totalCost","aggregation":"sum"}}],"timeDimension":{{"granularity":"day"}},"fromTimestamp":"{from}","toTimestamp":"{to}"}}"#
            );
            match c
                .get(
                    &format!("{PREFIX}/v2/metrics"),
                    &vec![("query", Some(query))],
                )
                .await
            {
                Ok(body) => Ok(Obj::new()
                    .put("available", true)
                    .put(
                        "days",
                        Value::Array(
                            data_of(&body)
                                .iter()
                                .take(120)
                                .map(|d| {
                                    let n = numbers(Some(d));
                                    Obj::new()
                                        .opt("date", str_of(get(d, "time_dimension")))
                                        .opt("observations", n.get("count_count").cloned())
                                        .opt("totalCost", n.get("sum_totalCost").cloned())
                                        .done()
                                })
                                .collect(),
                        ),
                    )
                    .done()),
                Err(e) if e.is_not_found() => Ok(Obj::new()
                    .put("available", false)
                    .put("days", Value::Array(Vec::new()))
                    .done()),
                Err(e) => Err(e),
            }
        }
        "health" => {
            // `status()` never throws; its failure is rethrown with its status (or 0).
            if let Err(e) = c
                .with_auth(Auth::None)
                .get(&format!("{PREFIX}/health"), &vec![])
                .await
            {
                let status = e.status.unwrap_or(0);
                return Err(OpError {
                    status: Some(status),
                    ..e
                });
            }
            match c
                .get(
                    &format!("{PREFIX}/v2/observations"),
                    &vec![("limit", Some("1".into()))],
                )
                .await
            {
                Ok(_) => Ok(ok_status()),
                Err(e) if e.is_not_found() => Ok(ok_status()),
                Err(e) => Err(e),
            }
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_seconds_is_utc_without_millis() {
        assert_eq!(iso_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_seconds(1_792_411_200_000), "2026-10-19T12:00:00Z");
        assert_eq!(iso_seconds(1_792_411_200_999), "2026-10-19T12:00:00Z");
    }

    #[test]
    fn strict_iso_wants_an_offset() {
        assert!(strict_iso("2026-01-01T00:00:00Z", "x").is_ok());
        assert!(strict_iso("2026-01-01T00:00:00+05:30", "x").is_ok());
        assert!(strict_iso("1767225600000", "x").is_err());
        assert!(strict_iso("2026-01-01", "x").is_err());
    }
}
