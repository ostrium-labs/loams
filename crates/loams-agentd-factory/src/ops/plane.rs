//! Plane (ItsAPlan): absolute paths on the API's URL (no `/api/v1`), `x-api-key`,
//! and an `{error, code?}` body as the one error shape, even on a 2xx.

use serde_json::Value;

use super::{Connection, need, ok_status};
use crate::client::{Auth, Client, Query, encode_segment, json_of};
use crate::error::OpError;
use crate::json::{Obj, arr, get, numbers, str_of};
use crate::params::{Params, clamp};

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "stats" => {
            let p = Params::strict(params, &["projectKey"])?;
            p.str_range("projectKey", 1, 50)?;
            Ok(p)
        }
        "issues" => {
            let p = Params::strict(params, &["projectKey", "limit"])?;
            p.str_range("projectKey", 1, 50)?;
            p.int_min1("limit")?;
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    Client::new(
        &conn.url,
        Auth::Headers(vec![("x-api-key".into(), conn.value("apiKey"))]),
        conn.upstream.clone(),
    )
}

fn error_message(body: &Value) -> Option<String> {
    get(body, "error")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
}

/// `_call`: a 4xx's message keeps its status; a 5xx's message is generic and not
/// surfaced; an analytics 403/404 is a configuration problem; a 2xx `{error}` is an
/// error with status 200.
async fn call(
    c: &Client,
    path: &str,
    query: &Query<'_>,
    analytics: bool,
) -> Result<Value, OpError> {
    let response = c.get_raw(path, query).await?;
    let body = match json_of(path, &response) {
        Ok(body) => body,
        Err(err) => {
            let parsed: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
            let status = response.status;
            if let Some(message) = error_message(&parsed) {
                if status >= 500 {
                    return Err(OpError::http(
                        status,
                        format!(
                            "itsaplan: GET {path} failed with {status}. The server's error text on a 5xx is generic by design."
                        ),
                    ));
                }
                return Err(OpError::http(
                    status,
                    format!("itsaplan: GET {path}: {message}"),
                ));
            }
            if analytics && (status == 403 || status == 404) {
                return Err(OpError::http(
                    status,
                    format!(
                        "itsaplan: GET {path} returned {status}: the project's dashboards feature or the key's permission is missing"
                    ),
                ));
            }
            return Err(err);
        }
    };
    if let Some(message) = error_message(&body) {
        return Err(OpError::http(
            200,
            format!("itsaplan: GET {path}: {message}"),
        ));
    }
    Ok(body)
}

fn require_object(value: Value, what: &str) -> Result<Value, OpError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(OpError::http(200, format!("{what}: expected an object")))
    }
}

fn project(p: &Params, conn: &Connection) -> Result<String, OpError> {
    need(
        p.str_range("projectKey", 1, 50)?
            .or_else(|| conn.field("projectKey")),
        "projectKey",
    )
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "stats" => {
            let key = project(p, conn)?;
            let path = format!("/projects/{}/analytics/stats", encode_segment(&key));
            let body = require_object(
                call(&c, &path, &vec![], true).await?,
                "GET /analytics/stats",
            )?;
            Ok(Value::Object(numbers(Some(&body))))
        }
        "issues" => {
            let key = project(p, conn)?;
            let path = format!("/projects/{}/issues", encode_segment(&key));
            let q: Query<'_> = vec![("limit", Some(clamp(p.int_min1("limit")?, 20).to_string()))];
            let body = call(&c, &path, &q, false).await?;
            if !body.is_array() {
                return Err(OpError::http(
                    200,
                    "GET /projects/:projectKey/issues: expected a bare array",
                ));
            }
            Ok(Value::Array(
                arr(Some(&body))
                    .iter()
                    .map(|i| {
                        Obj::new()
                            .opt("id", str_of(get(i, "identifier")))
                            .opt("title", str_of(get(i, "title")))
                            .opt("priority", str_of(get(i, "priority")))
                            .opt("updatedAt", str_of(get(i, "updatedAt")))
                            .done()
                    })
                    .collect(),
            ))
        }
        "health" => {
            require_object(call(&c, "/", &vec![], false).await?, "/")?;
            let me = require_object(call(&c, "/me", &vec![], false).await?, "GET /me")?;
            if get(&me, "authenticated") == Some(&Value::Bool(false)) {
                return Err(OpError::http(401, "not authenticated"));
            }
            Ok(ok_status())
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}
