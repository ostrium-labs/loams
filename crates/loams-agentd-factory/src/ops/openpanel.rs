//! OpenPanel: the API under `/api`, the client id and secret as headers.

use serde_json::Value;

use super::{Connection, need, ok_status};
use crate::client::{Auth, Client, Query, encode_segment};
use crate::error::OpError;
use crate::json::{Obj, arr, get, num_of, numbers, str_of};
use crate::params::{Params, clamp};

const INTERVALS: &[&str] = &["minute", "hour", "day", "week", "month"];
/// The insights routes' own cap.
const INSIGHTS_LIMIT_MAX: i64 = 100;

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "insights" => {
            let p = Params::strict(params, &["projectId", "range", "interval", "limit"])?;
            p.str_range("projectId", 1, 100)?;
            p.str_max("range", 20)?;
            p.one_of("interval", INTERVALS)?;
            p.int_min1("limit")?;
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

/// Lower-case hex in 8-4-4-4-12 form, what OpenPanel checks a client id against.
fn client_id_ok(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| {
            p.len() == n
                && p.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn client(conn: &Connection) -> Result<Client, OpError> {
    let id = conn.value("clientId");
    if !client_id_ok(&id) {
        return Err(OpError::other(
            "openpanel: the client id is not a valid UUIDv4 (lowercase hex 8-4-4-4-12)",
        ));
    }
    let root = conn.url.trim_end_matches('/');
    let base = if root.ends_with("/api") {
        root.to_owned()
    } else {
        format!("{root}/api")
    };
    Ok(Client::new(
        &base,
        Auth::Headers(vec![
            ("openpanel-client-id".into(), id),
            ("openpanel-client-secret".into(), conn.value("clientSecret")),
        ]),
        conn.upstream.clone(),
    ))
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn)?;
    match op {
        "insights" => {
            let id = need(
                p.str_range("projectId", 1, 100)?
                    .or_else(|| conn.field("projectId")),
                "projectId",
            )?;
            let id = encode_segment(&id);
            // OpenPanel's own default window ends yesterday; the last 30 days include today.
            let range = p.str_max("range", 20)?.unwrap_or_else(|| "30d".into());
            let overview_q: Query<'_> = vec![
                ("range", Some(range.clone())),
                (
                    "interval",
                    Some(
                        p.one_of("interval", INTERVALS)?
                            .unwrap_or_else(|| "day".into()),
                    ),
                ),
            ];
            let limit = clamp(p.int_min1("limit")?, 10).min(INSIGHTS_LIMIT_MAX);
            let pages_q: Query<'_> =
                vec![("range", Some(range)), ("limit", Some(limit.to_string()))];
            let overview_path = format!("/insights/{id}/overview");
            let pages_path = format!("/insights/{id}/pages/top");
            let (ov, pages) = tokio::try_join!(
                c.get(&overview_path, &overview_q),
                c.get(&pages_path, &pages_q),
            )?;
            let raw_series = arr(get(&ov, "series"));
            let series: Vec<Value> = raw_series
                .iter()
                .take(400)
                .map(|s| {
                    Obj::new()
                        .extend(numbers(Some(s)))
                        .opt("date", str_of(get(s, "date")))
                        .done()
                })
                .collect();
            let top: Vec<Value> = arr(Some(&pages))
                .iter()
                .map(|r| {
                    Obj::new()
                        .opt("path", str_of(get(r, "path")))
                        .opt("sessions", num_of(get(r, "sessions")))
                        .opt("pageviews", num_of(get(r, "pageviews")))
                        .done()
                })
                .collect();
            Ok(Obj::new()
                .put("summary", Value::Object(numbers(get(&ov, "summary"))))
                .put("series", Value::Array(series))
                .put("topPages", Value::Array(top))
                .done())
        }
        "health" => {
            c.get("/healthcheck", &vec![]).await?;
            if let Some(id) = conn.field("projectId") {
                c.get(&format!("/insights/{}/live", encode_segment(&id)), &vec![])
                    .await?;
            }
            Ok(ok_status())
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}
