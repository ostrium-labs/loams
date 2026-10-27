//! Forgejo: `/api/v1`, `Authorization: token <pat>`.

use serde_json::Value;

use super::{Connection, ok_status};
use crate::client::{Auth, Client, Query, encode_segment};
use crate::error::OpError;
use crate::json::{Obj, arr, get, js_string, num_of, rec, str_of};
use crate::params::{Params, clamp};

pub(super) fn check(op: &str, params: &Value) -> Result<Params, OpError> {
    match op {
        "repos" => {
            let p = Params::strict(params, &["q", "page", "limit"])?;
            p.str_max("q", 200)?;
            p.int_min1("page")?;
            p.int_min1("limit")?;
            Ok(p)
        }
        "issues" => {
            let p = Params::strict(params, &["q", "page", "limit", "type", "owner", "repo"])?;
            p.str_max("q", 200)?;
            p.int_min1("page")?;
            p.int_min1("limit")?;
            let kind = p.one_of("type", &["issues", "pulls"])?;
            let owner = p.str_range("owner", 1, 100)?;
            let repo = p.str_range("repo", 1, 100)?;
            if kind.as_deref() == Some("pulls") && (owner.is_none() || repo.is_none()) {
                return Err(OpError::param(
                    "params: owner and repo are required for pulls",
                ));
            }
            Ok(p)
        }
        _ => Params::strict(params, &[]),
    }
}

fn client(conn: &Connection) -> Client {
    let base = format!("{}/api/v1", conn.url.trim_end_matches('/'));
    Client::new(
        &base,
        Auth::Raw(format!("token {}", conn.value("token"))),
        conn.upstream.clone(),
    )
}

fn require_array(value: Option<&Value>, what: &str) -> Result<Vec<Value>, OpError> {
    match value {
        Some(Value::Array(_)) => Ok(arr(value)),
        _ => Err(OpError::other(format!(
            "{what}: expected a bare JSON array — Forgejo list endpoints do not wrap their results"
        ))),
    }
}

/// An issue or pull request, as `issueDto` projects it.
fn issue_dto(i: &Value) -> Value {
    let id = str_of(get(i, "identifier"))
        .or_else(|| str_of(get(i, "id")))
        .unwrap_or_else(|| {
            num_of(get(i, "number"))
                .map(|n| js_string(&n))
                .unwrap_or_else(|| str_of(get(i, "number")).unwrap_or_default())
        });
    let head = get(i, "head").map(rec).and_then(|h| str_of(h.get("ref")));
    Obj::new()
        .put("id", id)
        .opt("title", str_of(get(i, "title")))
        .opt(
            "state",
            str_of(get(i, "state")).or_else(|| str_of(get(i, "status"))),
        )
        .opt(
            "url",
            str_of(get(i, "html_url")).or_else(|| str_of(get(i, "permalink"))),
        )
        .opt(
            "updatedAt",
            str_of(get(i, "updated_at")).or_else(|| str_of(get(i, "updatedAt"))),
        )
        .opt("head", head)
        .done()
}

pub(super) async fn run(op: &str, p: &Params, conn: &Connection) -> Result<Value, OpError> {
    let c = client(conn);
    match op {
        "repos" => {
            let limit = clamp(p.int_min1("limit")?, 20);
            let q: Query<'_> = vec![
                ("q", Some(p.str_max("q", 200)?.unwrap_or_default())),
                ("page", Some(p.int_min1("page")?.unwrap_or(1).to_string())),
                ("limit", Some(limit.to_string())),
            ];
            let body = c.get("/repos/search", &q).await?;
            let repos = require_array(get(&body, "data"), "GET /repos/search")?;
            Ok(Value::Array(
                repos
                    .iter()
                    .map(|r| {
                        Obj::new()
                            .opt("fullName", str_of(get(r, "full_name")))
                            .opt("description", str_of(get(r, "description")))
                            .opt("stars", num_of(get(r, "stars_count")))
                            .opt("forks", num_of(get(r, "forks_count")))
                            .opt("openIssues", num_of(get(r, "open_issues_count")))
                            .opt("updatedAt", str_of(get(r, "updated_at")))
                            .opt("url", str_of(get(r, "html_url")))
                            .done()
                    })
                    .collect(),
            ))
        }
        "issues" => {
            let limit = clamp(p.int_min1("limit")?, 20).to_string();
            let page = p.int_min1("page")?.unwrap_or(1).to_string();
            let body = if p.one_of("type", &["issues", "pulls"])?.as_deref() == Some("pulls") {
                let owner = p.str_range("owner", 1, 100)?.unwrap_or_default();
                let repo = p.str_range("repo", 1, 100)?.unwrap_or_default();
                let path = format!(
                    "/repos/{}/{}/pulls",
                    encode_segment(&owner),
                    encode_segment(&repo)
                );
                let q: Query<'_> = vec![
                    ("state", Some("open".into())),
                    ("page", Some(page)),
                    ("limit", Some(limit)),
                ];
                c.get(&path, &q).await?
            } else {
                let q: Query<'_> = vec![
                    ("q", Some(p.str_max("q", 200)?.unwrap_or_default())),
                    ("type", Some("issues".into())),
                    ("page", Some(page)),
                    ("limit", Some(limit)),
                ];
                c.get("/repos/issues/search", &q).await?
            };
            let items = require_array(Some(&body), "GET list")?;
            Ok(Value::Array(items.iter().map(issue_dto).collect()))
        }
        "version" => {
            let body = c.get("/version", &vec![]).await?;
            Ok(Obj::new()
                .opt("version", str_of(get(&body, "version")))
                .done())
        }
        "health" => {
            c.get("/version", &vec![]).await?;
            Ok(ok_status())
        }
        _ => Err(OpError::unsupported(format!(
            "{op} is not supported by this adapter"
        ))),
    }
}
