//! Zulip: `/api/v1`, HTTP Basic with the email and API key, and Zulip's
//! `{result, msg}` envelope on every answer.

use serde_json::{Value, json};

use super::{Connection, ok_status};
use crate::client::{Auth, Client, Query, json_of};
use crate::error::OpError;
use crate::json::{Obj, arr, get, num_of, slice_chars, str_of};
use crate::params::{Params, clamp};

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "messages" => {
            let p = Params::strict(params, &["channel", "limit"])?;
            p.required_str("channel", 1, 100)?;
            p.int_min1("limit")?;
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    Client::new(
        &conn.url,
        Auth::Basic {
            username: conn.value("email"),
            password: conn.value("apiKey"),
        },
        conn.upstream.clone(),
    )
}

/// `_call`: GET, then Zulip's envelope. An error envelope on a non-2xx answer is
/// Zulip's message without the status, as the TypeScript adapter throws it.
async fn call(c: &Client, path: &str, query: &Query<'_>) -> Result<Value, OpError> {
    let target = format!("/api/v1{path}");
    let response = c.get_raw(&target, query).await?;
    let body = match json_of(&target, &response) {
        Ok(body) => body,
        Err(err) => {
            if response.status != 0
                && let Ok(Value::Object(envelope)) = serde_json::from_slice::<Value>(&response.body)
                && envelope.get("result").and_then(Value::as_str) == Some("error")
            {
                let msg = envelope
                    .get("msg")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .map_or_else(|| err.message.clone(), str::to_owned);
                return Err(OpError::other(msg));
            }
            return Err(err);
        }
    };
    let Value::Object(raw) = &body else {
        return Err(OpError::other(format!(
            "{path}: response was not a Zulip envelope"
        )));
    };
    let msg = raw.get("msg").and_then(Value::as_str).unwrap_or("");
    match raw.get("result").and_then(Value::as_str) {
        Some("success") => Ok(body),
        Some("error") => Err(OpError::other(if msg.is_empty() {
            format!("{path} failed")
        } else {
            msg.to_owned()
        })),
        _ => Err(OpError::other(format!("{path}: unexpected result"))),
    }
}

fn require_array(value: Option<&Value>, what: &str) -> Result<Vec<Value>, OpError> {
    match value {
        Some(Value::Array(_)) => Ok(arr(value)),
        _ => Err(OpError::other(format!("{what}: expected an array"))),
    }
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "streams" => {
            let body = call(&c, "/streams", &vec![("include_default", None)]).await?;
            let streams = require_array(get(&body, "streams"), "GET /streams")?;
            Ok(Value::Array(
                streams
                    .iter()
                    .take(200)
                    .map(|s| {
                        Obj::new()
                            .opt("id", num_of(get(s, "stream_id")))
                            .opt("name", str_of(get(s, "name")))
                            .opt("description", str_of(get(s, "description")))
                            .put("private", get(s, "invite_only") == Some(&Value::Bool(true)))
                            .done()
                    })
                    .collect(),
            ))
        }
        "messages" => {
            let channel = p.required_str("channel", 1, 100)?;
            let narrow = json!([{ "operator": "channel", "operand": channel }]).to_string();
            let q: Query<'_> = vec![
                ("narrow", Some(narrow)),
                ("anchor", Some("newest".into())),
                (
                    "num_before",
                    Some(clamp(p.int_min1("limit")?, 20).to_string()),
                ),
                ("num_after", Some("0".into())),
            ];
            let body = call(&c, "/messages", &q).await?;
            let messages = require_array(get(&body, "messages"), "GET /messages")?;
            Ok(Value::Array(
                messages
                    .iter()
                    .map(|m| {
                        Obj::new()
                            .opt("id", num_of(get(m, "id")))
                            .opt("sender", str_of(get(m, "sender_full_name")))
                            .opt("timestamp", num_of(get(m, "timestamp")))
                            .opt(
                                "topic",
                                str_of(get(m, "subject")).or_else(|| str_of(get(m, "topic"))),
                            )
                            .put(
                                "content",
                                slice_chars(&str_of(get(m, "content")).unwrap_or_default(), 2000),
                            )
                            .done()
                    })
                    .collect(),
            ))
        }
        "server" => {
            let me = call(&c, "/users/me", &vec![]).await?;
            Ok(Obj::new()
                .opt("name", str_of(get(&me, "full_name")))
                .opt("email", str_of(get(&me, "email")))
                .done())
        }
        "health" => {
            call(&c, "/users/me", &vec![]).await?;
            Ok(ok_status())
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}
