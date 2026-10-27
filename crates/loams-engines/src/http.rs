//! The namespace-scoped HTTP facade (D2015), the contract in `api/engines/openapi.json`.
//!
//! [`router`] serves the routes with no authorization of its own, for a host that already
//! runs IA2's enforcer over `/v1` with [`crate::authz::declare`] in its route table;
//! [`router_with_authz`] layers the enforcer itself. Either way the authentication layer runs
//! first and puts a [`Subject`] in the request's extensions; the org it names is the instance's
//! billing org.
//!
//! Bodies are parsed here, not by axum's `Json` extractor, so a bad body answers the same
//! `{error, message}` as every other error.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use loams_authz::{Authorizer, Subject};
use serde::de::DeserializeOwned;
use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::service::{CreateInstance, EngineService, UpdateInstance};
use crate::{CreateSession, EngineError, EngineKind, SqlRequest, SubmitJob};

type Svc = State<Arc<EngineService>>;

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, EngineError> {
    serde_json::from_slice(body)
        .map_err(|e| EngineError::Invalid(format!("the request body is not valid: {e}")))
}

/// `k=v` pairs of a query string, percent-decoded (`+` is a space).
fn query_pairs(raw: Option<&str>) -> Result<Vec<(String, String)>, EngineError> {
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(&s.replace('+', " "))
            .decode_utf8()
            .map(|c| c.into_owned())
            .map_err(|_| EngineError::Invalid("the query string is not UTF-8".into()))
    };
    raw.unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Ok((decode(k)?, decode(v)?))
        })
        .collect()
}

fn time_param(name: &str, v: &str) -> Result<OffsetDateTime, EngineError> {
    OffsetDateTime::parse(v, &Rfc3339)
        .map_err(|_| EngineError::Invalid(format!("{name} must be an RFC 3339 time, got {v:?}")))
}

fn accepted<T: serde::Serialize>(v: T) -> Response {
    (StatusCode::ACCEPTED, Json(v)).into_response()
}

fn created<T: serde::Serialize>(v: T) -> Response {
    (StatusCode::CREATED, Json(v)).into_response()
}

fn ok<T: serde::Serialize>(v: T) -> Response {
    Json(v).into_response()
}

fn respond(r: Result<Response, EngineError>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

async fn catalog(State(svc): Svc) -> Response {
    ok(svc.catalog())
}

async fn list(State(svc): Svc, Path(namespace): Path<String>, RawQuery(q): RawQuery) -> Response {
    respond(
        async {
            let mut kind = None;
            for (k, v) in query_pairs(q.as_deref())? {
                if k == "kind" {
                    kind = Some(v.parse::<EngineKind>()?);
                }
            }
            let instances = svc.list(&namespace, kind).await?;
            Ok::<_, EngineError>(ok(json!({ "instances": instances })))
        }
        .await,
    )
}

async fn create(
    State(svc): Svc,
    Path(namespace): Path<String>,
    subject: Option<Extension<Subject>>,
    body: Bytes,
) -> Response {
    respond(
        async {
            let req: CreateInstance = parse(&body)?;
            let org = subject.and_then(|Extension(s)| s.org);
            Ok::<_, EngineError>(accepted(svc.create(&namespace, org, req).await?))
        }
        .await,
    )
}

async fn get_one(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(svc.get(&namespace, &id).await.map(ok))
}

async fn update(
    State(svc): Svc,
    Path((namespace, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    respond(
        async {
            let req: UpdateInstance = parse(&body)?;
            Ok::<_, EngineError>(accepted(svc.update(&namespace, &id, req).await?))
        }
        .await,
    )
}

async fn remove(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(svc.delete(&namespace, &id).await.map(accepted))
}

async fn suspend(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(svc.suspend(&namespace, &id).await.map(accepted))
}

async fn resume(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(svc.resume(&namespace, &id).await.map(accepted))
}

async fn usage(
    State(svc): Svc,
    Path((namespace, id)): Path<(String, String)>,
    RawQuery(q): RawQuery,
) -> Response {
    respond(
        async {
            let (mut from, mut to) = (None, None);
            for (k, v) in query_pairs(q.as_deref())? {
                match k.as_str() {
                    "from" => from = Some(time_param("from", &v)?),
                    "to" => to = Some(time_param("to", &v)?),
                    _ => {}
                }
            }
            Ok::<_, EngineError>(ok(svc.usage(&namespace, &id, from, to).await?))
        }
        .await,
    )
}

async fn sql(
    State(svc): Svc,
    Path((namespace, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    respond(
        async {
            let req: SqlRequest = parse(&body)?;
            Ok::<_, EngineError>(ok(svc.sql(&namespace, &id, req).await?))
        }
        .await,
    )
}

async fn list_sessions(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(
        svc.list_sessions(&namespace, &id)
            .await
            .map(|sessions| ok(json!({ "sessions": sessions }))),
    )
}

async fn create_session(
    State(svc): Svc,
    Path((namespace, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    respond(
        async {
            let req: CreateSession = parse(&body)?;
            Ok::<_, EngineError>(created(svc.create_session(&namespace, &id, req).await?))
        }
        .await,
    )
}

async fn close_session(
    State(svc): Svc,
    Path((namespace, id, session)): Path<(String, String, String)>,
) -> Response {
    respond(
        svc.close_session(&namespace, &id, &session)
            .await
            .map(|()| StatusCode::NO_CONTENT.into_response()),
    )
}

async fn list_jobs(State(svc): Svc, Path((namespace, id)): Path<(String, String)>) -> Response {
    respond(
        svc.list_jobs(&namespace, &id)
            .await
            .map(|jobs| ok(json!({ "jobs": jobs }))),
    )
}

async fn submit_job(
    State(svc): Svc,
    Path((namespace, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    respond(
        async {
            let req: SubmitJob = parse(&body)?;
            Ok::<_, EngineError>(created(svc.submit_job(&namespace, &id, req).await?))
        }
        .await,
    )
}

async fn cancel_job(
    State(svc): Svc,
    Path((namespace, id, job)): Path<(String, String, String)>,
) -> Response {
    respond(svc.cancel_job(&namespace, &id, &job).await.map(ok))
}

/// The facade's routes, at their full `/v1/…` paths, with no authorization layer.
pub fn router(svc: Arc<EngineService>) -> Router {
    const I: &str = "/v1/namespaces/{namespace}/engines/{instance}";
    Router::new()
        .route("/v1/engines", get(catalog))
        .route("/v1/namespaces/{namespace}/engines", get(list).post(create))
        .route(I, get(get_one).patch(update).delete(remove))
        .route(&format!("{I}/suspend"), post(suspend))
        .route(&format!("{I}/resume"), post(resume))
        .route(&format!("{I}/usage"), get(usage))
        .route(&format!("{I}/sql"), post(sql))
        .route(
            &format!("{I}/sessions"),
            get(list_sessions).post(create_session),
        )
        .route(&format!("{I}/sessions/{{session}}"), delete(close_session))
        .route(&format!("{I}/jobs"), get(list_jobs).post(submit_job))
        .route(&format!("{I}/jobs/{{job}}/cancel"), post(cancel_job))
        .with_state(svc)
}

/// [`router`] behind IA2's deny-by-default enforcer with [`crate::authz::declare`].
pub fn router_with_authz(svc: Arc<EngineService>, authz: Arc<dyn Authorizer>) -> Router {
    let enforcer = loams_authz::http::Enforcer::new(authz, crate::authz::declare);
    router(svc).layer(axum::middleware::from_fn_with_state(
        enforcer,
        loams_authz::http::enforce,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings() {
        let q = query_pairs(Some("kind=flink&from=2026-10-27T10%3A00%3A00Z&x&a+b=c+d")).unwrap();
        assert_eq!(
            q,
            vec![
                ("kind".into(), "flink".into()),
                ("from".into(), "2026-10-27T10:00:00Z".into()),
                ("x".into(), String::new()),
                ("a b".into(), "c d".into()),
            ]
        );
        assert!(query_pairs(None).unwrap().is_empty());
        assert!(time_param("from", "yesterday").is_err());
    }
}
