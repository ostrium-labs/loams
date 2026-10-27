//! The scroll API (#950): `_search?scroll=<keep alive>`, `GET|POST
//! /_search/scroll[/{id}]` and `DELETE /_search/scroll[/{id}|/_all]`, as
//! elasticsearch-py's `helpers.scan` uses them.
//!
//! A scroll keeps no server state. Its `_scroll_id` encodes the request
//! (index, body, URL parameters, page size), the sort values of the last
//! hit returned over the engine's effective sort, which ends with the
//! primary key (the next page is the engine's `search_after` from them), the
//! consistency token of the first page (later pages read at least that
//! version) and when it expires. Without a `sort` the pages are in `_doc`
//! order. Clearing a scroll only checks its id, since nothing is held.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::Uri;
use axum::response::Response;
use base64::Engine;
use loams_collection::ConsistencyToken;
use loams_query::{ReadConsistency, SortValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::exec::{expr_of, now_ms, page_after, resolve_options, views};
use super::{SEARCH_PARAMS, SearchOutcome, execute, parse_params};
use crate::EsGateway;
use crate::dbq::time_value;
use crate::error::EsError;
use crate::http::{Params, RequestCtx, fail, json_body, respond};
use crate::names::IndexExpr;

/// ES's `search.max_keep_alive` default.
const MAX_KEEP_ALIVE: Duration = Duration::from_secs(24 * 3600);

/// The parameters of `GET|POST /_search/scroll`.
const SCROLL_PARAMS: &[&str] = &["scroll", "scroll_id", "rest_total_hits_as_int"];

/// The URL parameters a scroll's later pages do not repeat.
const PAGE_PARAMS: &[&str] = &["scroll", "size", "from"];

/// What a `_scroll_id` carries.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScrollState {
    /// The namespace it was opened in.
    ns: String,
    /// The index (one, after alias resolution).
    index: String,
    /// The search body without `size`, `from` and `search_after`.
    body: Value,
    /// The URL query string without `scroll`, `size` and `from`.
    query: String,
    size: usize,
    /// Whether the request gave its own `sort` (else `_doc`).
    sorted: bool,
    /// The `sort` values of the last hit returned; `None` before any.
    after: Option<Value>,
    /// The last page was short: nothing is left.
    done: bool,
    /// The first page's consistency token.
    token: Option<String>,
    /// The total of the first page, as rendered.
    total: Value,
    keep_alive_ms: u64,
    /// Expiry, in Unix milliseconds.
    expires_ms: i64,
}

fn encode(state: &ScrollState) -> String {
    let json = serde_json::to_vec(state).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn missing(id: &str) -> EsError {
    let shown: String = id.chars().take(40).collect();
    EsError::new(
        404,
        "search_context_missing_exception",
        format!("No search context found for id [{shown}]"),
    )
}

/// The state of `id` in `ns`, unexpired; anything else is a missing
/// context.
fn decode(id: &str, ns: &str) -> Result<ScrollState, EsError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(id.trim())
        .map_err(|_| missing(id))?;
    let state: ScrollState = serde_json::from_slice(&bytes).map_err(|_| missing(id))?;
    if state.ns != ns || state.expires_ms < now_ms() {
        return Err(missing(id));
    }
    Ok(state)
}

/// A keep-alive, at most a day.
fn keep_alive(text: &str) -> Result<Duration, EsError> {
    let d = time_value("scroll", text)?
        .ok_or_else(|| EsError::illegal_argument("scroll keep alive must be positive"))?;
    if d > MAX_KEEP_ALIVE {
        return Err(EsError::illegal_argument(format!(
            "Keep alive for request ({text}) is too large. It must be less than (1d). This limit \
             can be set by changing the [search.max_keep_alive] cluster level setting."
        )));
    }
    Ok(d)
}

/// The query string without the parameters a page sets itself.
fn page_query(uri: &Uri) -> String {
    uri.query()
        .unwrap_or("")
        .split('&')
        .filter(|pair| {
            let name = pair.split_once('=').map_or(*pair, |(n, _)| n);
            !pair.is_empty() && !PAGE_PARAMS.contains(&name)
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Runs one page of `state` and moves it on: `after` becomes the last hit's
/// sort values, `done` is set by a short page, and `sort` leaves the hits
/// when the request had none.
async fn page(
    gw: &EsGateway,
    ctx: &RequestCtx,
    state: &mut ScrollState,
    path: &str,
) -> Result<SearchOutcome, EsError> {
    let params = Params::parse(Some(&state.query), path, SEARCH_PARAMS)?;
    let mut search = parse_params(&params)?;
    search.from = Some(0);
    search.size = Some(state.size);
    let indices = views(
        gw,
        ctx,
        &IndexExpr::List(vec![state.index.clone()]),
        resolve_options(&params, &search),
    )
    .await?;
    let Some(view) = indices.first() else {
        return Err(missing(&state.index));
    };
    let after: Option<Vec<SortValue>> = match &state.after {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| missing(&state.index))?),
    };
    let (mut outcome, last, n) = page_after(gw, ctx, view, &state.body, &search, after).await?;
    if let Some(last) = last {
        state.after = Some(serde_json::to_value(last).unwrap_or(Value::Null));
    }
    state.done = n < state.size;
    if !state.sorted
        && let Some(hits) = outcome.body["hits"]["hits"].as_array_mut()
    {
        for hit in hits {
            if let Value::Object(map) = hit {
                map.remove("sort");
            }
        }
    }
    Ok(outcome)
}

/// `_search?scroll=…`: the first page and its `_scroll_id`.
pub(crate) async fn open(
    gw: &EsGateway,
    ctx: &RequestCtx,
    uri: &Uri,
    index: Option<&str>,
    params: &Params,
    body: Value,
) -> Result<SearchOutcome, EsError> {
    let keep = keep_alive(params.str("scroll").unwrap_or_default())?;
    let mut body = match body {
        Value::Null => json!({}),
        Value::Object(map) => Value::Object(map),
        _ => return Err(EsError::parsing("the search body must be an object")),
    };
    let search = parse_params(params)?;
    let from = match search.from {
        Some(n) => n,
        None => body.get("from").and_then(Value::as_u64).unwrap_or(0) as usize,
    };
    if from > 0 {
        return Err(EsError::new(
            400,
            "action_request_validation_exception",
            "Validation Failed: 1: using [from] is not allowed in a scroll context;",
        ));
    }
    let size = match search.size {
        Some(n) => n,
        None => body.get("size").and_then(Value::as_u64).unwrap_or(10) as usize,
    };
    if size == 0 {
        return Err(EsError::new(
            400,
            "action_request_validation_exception",
            "Validation Failed: 1: [size] cannot be [0] in a scroll context;",
        ));
    }
    let Value::Object(map) = &mut body else {
        unreachable!("an object");
    };
    if map.get("search_after").is_some_and(|v| !v.is_null()) {
        return Err(EsError::new(
            400,
            "action_request_validation_exception",
            "Validation Failed: 1: `search_after` cannot be used in a scroll context.;",
        ));
    }
    for key in ["size", "from", "search_after"] {
        map.remove(key);
    }
    let sorted = search.sort.is_some() || map.get("sort").is_some_and(|v| !v.is_null());
    if !sorted {
        map.insert("sort".to_string(), json!(["_doc"]));
    }
    let indices = views(gw, ctx, &expr_of(index), resolve_options(params, &search)).await?;
    if indices.len() > 1 {
        return Err(EsError::unsupported("scroll over several indices"));
    }
    let now = now_ms();
    let keep_alive_ms = keep.as_millis() as u64;
    let mut state = ScrollState {
        ns: ctx.namespace.clone(),
        index: indices.first().map(|v| v.name.clone()).unwrap_or_default(),
        body,
        query: page_query(uri),
        size,
        sorted,
        after: None,
        done: true,
        token: None,
        total: Value::Null,
        keep_alive_ms,
        expires_ms: now.saturating_add(keep_alive_ms as i64),
    };
    let mut outcome = if indices.is_empty() {
        // Nothing to scroll: ES answers the empty page with an id too.
        let mut search = search.clone();
        search.size = Some(size);
        execute(gw, ctx, &[], &state.body, &search).await?
    } else {
        page(gw, ctx, &mut state, uri.path()).await?
    };
    state.token = outcome.read_token.as_ref().map(ToString::to_string);
    state.total = outcome.body["hits"]["total"].clone();
    insert_id(&mut outcome.body, &encode(&state));
    Ok(outcome)
}

/// Puts `_scroll_id` first, as ES does.
fn insert_id(body: &mut Value, id: &str) {
    if let Value::Object(map) = body {
        let mut out = Map::new();
        out.insert("_scroll_id".to_string(), json!(id));
        out.extend(std::mem::take(map));
        *map = out;
    }
}

/// The scroll id and keep-alive of a `/_search/scroll` request: the path,
/// the URL or the body (`scroll_id`, `scroll`).
fn scroll_request(
    uri: &Uri,
    path_id: Option<String>,
    body: &[u8],
) -> Result<(Params, String, Option<String>), EsError> {
    let params = Params::parse(uri.query(), uri.path(), SCROLL_PARAMS)?;
    let body = json_body(body)?;
    let from_body = |key: &str| {
        body.as_ref()
            .and_then(|b| b.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let id = path_id
        .or_else(|| params.str("scroll_id").map(str::to_string))
        .or_else(|| from_body("scroll_id"))
        .ok_or_else(|| {
            EsError::new(
                400,
                "action_request_validation_exception",
                "Validation Failed: 1: scrollId is missing;",
            )
        })?;
    let keep = params
        .str("scroll")
        .map(str::to_string)
        .or_else(|| from_body("scroll"));
    Ok((params, id, keep))
}

async fn next(
    gw: &EsGateway,
    ctx: &RequestCtx,
    uri: &Uri,
    path_id: Option<String>,
    body: &[u8],
) -> Result<SearchOutcome, EsError> {
    let (params, id, keep) = scroll_request(uri, path_id, body)?;
    let mut state = decode(&id, &ctx.namespace)?;
    if let Some(keep) = keep {
        state.keep_alive_ms = keep_alive(&keep)?.as_millis() as u64;
    }
    state.expires_ms = now_ms().saturating_add(state.keep_alive_ms as i64);
    let mut ctx = ctx.clone();
    if let Some(token) = state
        .token
        .as_deref()
        .and_then(|t| t.parse::<ConsistencyToken>().ok())
    {
        ctx.consistency = ReadConsistency::AtLeast(token);
    }
    // `rest_total_hits_as_int` of this request wins.
    if params.str("rest_total_hits_as_int").is_some() {
        let rest = params.bool("rest_total_hits_as_int")?.unwrap_or(false);
        let mut pairs: Vec<String> = state
            .query
            .split('&')
            .filter(|p| !p.is_empty() && !p.starts_with("rest_total_hits_as_int"))
            .map(str::to_string)
            .collect();
        pairs.push(format!("rest_total_hits_as_int={rest}"));
        state.query = pairs.join("&");
    }
    let mut outcome = if state.done {
        SearchOutcome {
            body: json!({
                "took": 0,
                "timed_out": false,
                "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
                "hits": {"total": state.total, "max_score": null, "hits": []},
            }),
            read_token: None,
        }
    } else {
        page(gw, &ctx, &mut state, "/_search").await?
    };
    if let Value::Object(hits) = &mut outcome.body["hits"]
        && state.total.is_null()
    {
        hits.remove("total");
    }
    insert_id(&mut outcome.body, &encode(&state));
    Ok(outcome)
}

/// `GET|POST /_search/scroll`.
pub(crate) async fn scroll(
    State(gw): State<EsGateway>,
    ctx: RequestCtx,
    uri: Uri,
    body: Bytes,
) -> Response {
    match next(&gw, &ctx, &uri, None, &body).await {
        Ok(outcome) => respond(&ctx, 200, &outcome.body),
        Err(err) => fail(&ctx, &err),
    }
}

/// `GET|POST /_search/scroll/{id}`.
pub(crate) async fn scroll_id(
    State(gw): State<EsGateway>,
    ctx: RequestCtx,
    uri: Uri,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    match next(&gw, &ctx, &uri, Some(id), &body).await {
        Ok(outcome) => respond(&ctx, 200, &outcome.body),
        Err(err) => fail(&ctx, &err),
    }
}

/// Clears ids: each live one counts as freed. With none freed but some
/// named, ES answers 404 with the same body.
fn clear_ids(ctx: &RequestCtx, ids: Vec<String>) -> Response {
    if ids.iter().any(|id| id == "_all") {
        return respond(ctx, 200, &json!({"succeeded": true, "num_freed": 0}));
    }
    let freed = ids
        .iter()
        .filter(|id| decode(id, &ctx.namespace).is_ok())
        .count();
    let status = if freed == 0 && !ids.is_empty() {
        404
    } else {
        200
    };
    respond(ctx, status, &json!({"succeeded": true, "num_freed": freed}))
}

/// The ids of a clear request's body: `scroll_id` as a string or a list.
fn body_ids(body: &[u8]) -> Result<Vec<String>, EsError> {
    let Some(body) = json_body(body)? else {
        return Ok(Vec::new());
    };
    Ok(match body.get("scroll_id") {
        Some(Value::String(id)) => id.split(',').map(str::to_string).collect(),
        Some(Value::Array(ids)) => ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    })
}

/// `DELETE /_search/scroll`.
pub(crate) async fn clear(ctx: RequestCtx, uri: Uri, body: Bytes) -> Response {
    let result = Params::parse(uri.query(), uri.path(), &["scroll_id"]).and_then(|params| {
        let mut ids = body_ids(&body)?;
        if let Some(list) = params.list("scroll_id") {
            ids.extend(list);
        }
        if ids.is_empty() {
            return Err(EsError::new(
                400,
                "action_request_validation_exception",
                "Validation Failed: 1: no scroll ids specified;",
            ));
        }
        Ok(ids)
    });
    match result {
        Ok(ids) => clear_ids(&ctx, ids),
        Err(err) => fail(&ctx, &err),
    }
}

/// `DELETE /_search/scroll/{id}` (a comma list, or `_all`).
pub(crate) async fn clear_id(ctx: RequestCtx, uri: Uri, Path(id): Path<String>) -> Response {
    match Params::parse(uri.query(), uri.path(), &[]) {
        Ok(_) => clear_ids(&ctx, id.split(',').map(str::to_string).collect()),
        Err(err) => fail(&ctx, &err),
    }
}
