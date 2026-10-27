//! GlitchTip: `/api/0/`, a bearer token, bare-array lists whose pagination is all
//! in the `X-Hits`, `X-Max-Hits` and `Link` headers.

use serde_json::{Map, Value};

use super::{Connection, ok_status};
use crate::client::{Auth, Client, Query, encode_segment, json_of};
use crate::error::OpError;
use crate::json::{Obj, arr, get, num_of, rec, str_of};
use crate::params::{Params, clamp};

/// GlitchTip's sort fields; a leading `-` sorts descending.
const SORTS: &[&str] = &[
    "last_seen",
    "-last_seen",
    "first_seen",
    "-first_seen",
    "count",
    "-count",
    "priority",
    "-priority",
];

/// The innermost frames kept, and the longest stack (Studio's "Fix this error").
const MAX_FRAMES: usize = 40;
const MAX_STACK: usize = 4000;

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "issues" => {
            let p = Params::strict(params, &["orgSlug", "limit", "sort"])?;
            p.required_str("orgSlug", 1, 100)?;
            p.int_min1("limit")?;
            p.one_of("sort", SORTS)?;
            Ok(p)
        }
        "latestEvent" => {
            let p = Params::strict(params, &["issueId"])?;
            let id = p.matching("issueId", |s| {
                (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
            })?;
            if id.is_none() {
                return Err(OpError::param("issueId: Required"));
            }
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    Client::new(
        &conn.url,
        Auth::Bearer(conn.value("token")),
        conn.upstream.clone(),
    )
}

/// A list endpoint: a bare array, with its pagination headers present.
async fn list(c: &Client, suffix: &str, query: &Query<'_>) -> Result<Vec<Value>, OpError> {
    let path = format!("/api/0/{suffix}");
    let response = c.get_raw(&path, query).await?;
    let body = json_of(&path, &response)?;
    if !body.is_array() {
        return Err(OpError::http(
            response.status,
            "expected a bare JSON array from a list endpoint",
        ));
    }
    let missing: Vec<&str> = ["X-Hits", "X-Max-Hits", "Link"]
        .into_iter()
        .filter(|h| response.header(h).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(OpError::other(format!(
            "glitchtip: response is missing pagination header(s) {}",
            missing.join(", ")
        )));
    }
    Ok(arr(Some(&body)))
}

/// The exception of a Sentry-format event as plain text, innermost frame first.
fn stack_of(event: &Value) -> Option<String> {
    let entries = arr(get(event, "entries"));
    let exc = entries
        .iter()
        .find(|e| get(e, "type").and_then(Value::as_str) == Some("exception"))?;
    let data = get(exc, "data").map(rec).unwrap_or_default();
    let mut values = arr(data.get("values"));
    if values.is_empty() {
        return None;
    }
    values.reverse();
    let mut lines: Vec<String> = Vec::new();
    for v in &values {
        let head = [str_of(get(v, "type")), str_of(get(v, "value"))]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(": ");
        if !head.is_empty() {
            lines.push(head);
        }
        let stack = get(v, "stacktrace").map(rec).unwrap_or_default();
        let mut frames = arr(stack.get("frames"));
        frames.reverse();
        for f in frames.iter().take(MAX_FRAMES) {
            let file = str_of(get(f, "filename"))
                .or_else(|| str_of(get(f, "module")))
                .or_else(|| str_of(get(f, "absPath")))
                .unwrap_or_else(|| "?".into());
            let line = num_of(get(f, "lineNo")).or_else(|| num_of(get(f, "lineno")));
            let func = str_of(get(f, "function")).unwrap_or_else(|| "?".into());
            match line {
                Some(n) => lines.push(format!(
                    "  at {func} ({file}:{})",
                    crate::json::js_string(&n)
                )),
                None => lines.push(format!("  at {func} ({file})")),
            }
        }
    }
    let text = lines.join("\n");
    if text.is_empty() {
        None
    } else {
        Some(crate::json::slice_chars(&text, MAX_STACK))
    }
}

/// An event's release: `release` (a string or `{version}`), else its `release` tag.
fn release_of(event: &Value) -> Option<String> {
    let r = get(event, "release");
    if let Some(direct) = str_of(r).or_else(|| r.and_then(|r| str_of(get(r, "version")))) {
        return Some(direct);
    }
    for t in arr(get(event, "tags")) {
        if get(&t, "key").and_then(Value::as_str) == Some("release") {
            return str_of(get(&t, "value"));
        }
        if let Some(release) = str_of(get(&t, "release")) {
            return Some(release);
        }
    }
    None
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "organizations" => {
            let orgs = list(&c, "organizations/", &vec![]).await?;
            Ok(Value::Array(
                orgs.iter()
                    .map(|o| {
                        Obj::new()
                            .opt("slug", str_of(get(o, "slug")))
                            .opt("name", str_of(get(o, "name")))
                            .done()
                    })
                    .collect(),
            ))
        }
        "issues" => {
            let org = p.required_str("orgSlug", 1, 100)?;
            let q: Query<'_> = vec![
                ("query", Some("is:unresolved".into())),
                ("sort", p.one_of("sort", SORTS)?),
                ("limit", Some(clamp(p.int_min1("limit")?, 20).to_string())),
            ];
            let issues = list(
                &c,
                &format!("organizations/{}/issues/", encode_segment(&org)),
                &q,
            )
            .await?;
            Ok(Value::Array(
                issues
                    .iter()
                    .filter(|i| match get(i, "status") {
                        None => true,
                        Some(s) => s.as_str() == Some("unresolved"),
                    })
                    .map(|i| {
                        let count = str_of(get(i, "count"))
                            .map(Value::String)
                            .or_else(|| num_of(get(i, "count")).map(Value::Number));
                        Obj::new()
                            .opt("id", str_of(get(i, "id")))
                            .opt("title", str_of(get(i, "title")))
                            .opt("level", str_of(get(i, "level")))
                            .opt("count", count)
                            .opt("lastSeen", str_of(get(i, "lastSeen")))
                            .opt("permalink", str_of(get(i, "permalink")))
                            .done()
                    })
                    .collect(),
            ))
        }
        "latestEvent" => {
            let id = p
                .matching("issueId", |s| s.bytes().all(|b| b.is_ascii_digit()))?
                .unwrap_or_default();
            let path = format!("/api/0/issues/{}/events/latest/", encode_segment(&id));
            let event = c.get(&path, &vec![]).await?;
            let event = if event.is_object() {
                event
            } else {
                Value::Object(Map::new())
            };
            Ok(Obj::new()
                .opt("release", release_of(&event))
                .opt("culprit", str_of(get(&event, "culprit")))
                .opt("stack", stack_of(&event))
                .done())
        }
        "health" => {
            let root = c.get("/api/0/", &vec![]).await?;
            // `auth` is null when the route answered without a usable token.
            match get(&root, "auth") {
                None | Some(Value::Null) => Err(OpError::http(401, "not authenticated")),
                Some(_) => Ok(ok_status()),
            }
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}
