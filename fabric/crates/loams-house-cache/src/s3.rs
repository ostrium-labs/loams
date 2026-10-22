//! The S3 surface: path-style `GetObject` (with a range), `HeadObject` and
//! `ListObjectsV2`, the error documents S3 clients expect, and nothing else.
//!
//! The request's credentials (chDB signs with dummy keys) are ignored: who is
//! asking is known from the connection, and what it may read from its grant.

use std::convert::Infallible;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use loams_cache::CacheError;
use loams_store::{ObjectInfo, StoreError};

use crate::{Grant, HouseCache, SeenRequest, Upstream};

/// A response body.
pub type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// A body is read from the cache in pieces of at most this many bytes, so a
/// whole-object GET never holds the object.
pub const READ_PIECE: u64 = 8 * 1024 * 1024;

/// The most keys one `ListObjectsV2` page returns (S3's own cap).
pub const MAX_KEYS: usize = 1000;

static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// Answers one request of `worker`.
pub async fn handle(
    proxy: &HouseCache,
    worker: &str,
    request: Request<Incoming>,
) -> Response<Body> {
    let id = format!("{:016X}", REQUEST_IDS.fetch_add(1, Ordering::Relaxed));
    let method = request.method().clone();
    let target = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    proxy.record_with(|| SeenRequest {
        worker: worker.to_string(),
        method: method.to_string(),
        target: target.clone(),
        headers: request
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
    });

    let Some(grant) = proxy.grant_of(worker) else {
        proxy.count(None, |_| {});
        return error(
            &id,
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "this worker is not bound to a namespace",
            &target,
        );
    };
    let ns = Some(grant.namespace.as_str());
    let respond = |status: StatusCode, response: Response<Body>, bytes: u64| {
        proxy.count(ns, |m| {
            m.requests += 1;
            m.bytes_served += bytes;
            match status {
                StatusCode::FORBIDDEN => m.refused += 1,
                StatusCode::NOT_FOUND => m.not_found += 1,
                s if s.is_server_error() => m.upstream_errors += 1,
                _ => {}
            }
        });
        response
    };
    let refuse = |why: &str| {
        respond(
            StatusCode::FORBIDDEN,
            error(&id, StatusCode::FORBIDDEN, "AccessDenied", why, &target),
            0,
        )
    };

    let Some(upstream) = proxy.upstream(&grant.namespace) else {
        return refuse("this namespace has no store");
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let Some(path) = percent_decode(path) else {
        return respond(
            StatusCode::BAD_REQUEST,
            error(
                &id,
                StatusCode::BAD_REQUEST,
                "InvalidURI",
                "the path is not valid percent-encoding",
                &target,
            ),
            0,
        );
    };
    let Some(params) = parse_query(query) else {
        return respond(
            StatusCode::BAD_REQUEST,
            error(
                &id,
                StatusCode::BAD_REQUEST,
                "InvalidURI",
                "the query is not valid percent-encoding",
                &target,
            ),
            0,
        );
    };
    let path = path.strip_prefix('/').unwrap_or(&path);
    let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
    if bucket != upstream.bucket() {
        return refuse("the bucket is not this namespace's");
    }

    match (&method, key.is_empty()) {
        (&Method::GET, true) if param(&params, "list-type") == Some("2") => {
            let (status, response) = list(&id, &upstream, &grant, &params, &target).await;
            respond(status, response, 0)
        }
        (&Method::GET | &Method::HEAD, false) => {
            let Some(relative) = scoped_key(&upstream, &grant, key) else {
                return refuse("the key is outside this worker's namespace");
            };
            let head = method == Method::HEAD;
            let range = request.headers().get(header::RANGE).cloned();
            let started = std::time::SystemTime::now();
            let (status, response, bytes) =
                object(&id, &upstream, relative, head, range.as_ref(), &target).await;
            // The read's span, under the span of what the worker runs now
            // (HS1 Task 23).
            let tracer = proxy.tracer();
            if let Some(parent) = tracer.worker_span(worker) {
                tracer.record(
                    parent,
                    "house.cache.read",
                    started,
                    std::time::SystemTime::now(),
                    vec![
                        ("http.method", method.as_str().into()),
                        ("loams.key", relative.into()),
                        ("http.status_code", i64::from(status.as_u16()).into()),
                        ("loams.bytes", bytes.into()),
                    ],
                    status.is_client_error() || status.is_server_error(),
                );
            }
            respond(status, response, bytes)
        }
        (&Method::GET | &Method::HEAD, true) => respond(
            StatusCode::NOT_IMPLEMENTED,
            error(
                &id,
                StatusCode::NOT_IMPLEMENTED,
                "NotImplemented",
                "only ListObjectsV2 (list-type=2) lists a bucket",
                &target,
            ),
            0,
        ),
        _ => refuse("house-cache is read-only"),
    }
}

/// The store-relative key, when `key` is a plain key (no empty, `.` or `..`
/// segment) inside the upstream's prefix and the grant.
fn scoped_key<'k>(upstream: &Upstream, grant: &Grant, key: &'k str) -> Option<&'k str> {
    if key
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || key.contains(['\0', '\\'])
    {
        return None;
    }
    let relative = upstream.relative(key)?;
    grant.allows(relative).then_some(relative)
}

/// `GetObject` or `HeadObject`.
async fn object(
    id: &str,
    upstream: &Upstream,
    path: &str,
    head: bool,
    range: Option<&HeaderValue>,
    target: &str,
) -> (StatusCode, Response<Body>, u64) {
    let info = match upstream.head(path).await {
        Ok(info) => info,
        Err(err) => {
            let (status, response) = store_error(id, &err, target, head);
            return (status, response, 0);
        }
    };
    let size = info.size;
    let wanted = match range
        .and_then(|r| r.to_str().ok())
        .map(|r| parse_range(r, size))
    {
        None | Some(RangeAsk::Whole) => None,
        Some(RangeAsk::Part(part)) => Some(part),
        Some(RangeAsk::Unsatisfiable) => {
            let mut response = error(
                id,
                StatusCode::RANGE_NOT_SATISFIABLE,
                "InvalidRange",
                "the requested range is not satisfiable",
                target,
            );
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return (StatusCode::RANGE_NOT_SATISFIABLE, response, 0);
        }
    };
    let span = wanted.clone().unwrap_or(0..size);
    let status = if wanted.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, span.end - span.start)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::LAST_MODIFIED, http_date(&info))
        .header("x-amz-request-id", id)
        .header(header::SERVER, "loams-house-cache");
    if let Some(etag) = etag(&info) {
        builder = builder.header(header::ETAG, etag);
    }
    if let Some(part) = &wanted {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{size}", part.start, part.end - 1),
        );
    }
    if head {
        return (
            status,
            builder.body(empty()).unwrap_or_else(|_| internal(id)),
            0,
        );
    }

    // The first piece is read before the head goes out, so a store or cache
    // failure is a proper error response rather than a broken body.
    let first_end = span.end.min(span.start + READ_PIECE);
    let first = match upstream
        .cache()
        .read_with_size(path, size, span.start..first_end)
        .await
    {
        Ok(bytes) => bytes,
        Err(err) => {
            let (status, response) = cache_error(id, &err, target);
            return (status, response, 0);
        }
    };
    let served = span.end - span.start;
    if first_end == span.end {
        return (
            status,
            builder.body(full(first)).unwrap_or_else(|_| internal(id)),
            served,
        );
    }
    let cache = upstream.cache().clone();
    let path = path.to_string();
    let rest = futures::stream::unfold(first_end, move |at| {
        let cache = cache.clone();
        let path = path.clone();
        let end = span.end;
        async move {
            if at >= end {
                return None;
            }
            let to = end.min(at + READ_PIECE);
            let piece = cache
                .read_with_size(&path, size, at..to)
                .await
                .map(Frame::data)
                .map_err(std::io::Error::other);
            Some((piece, to))
        }
    });
    let stream = futures::stream::once(async move { Ok(Frame::data(first)) }).chain(rest);
    let body = StreamBody::new(stream).boxed_unsync();
    (
        status,
        builder.body(body).unwrap_or_else(|_| internal(id)),
        served,
    )
}

/// `ListObjectsV2`, within the grant's prefixes, when the grant allows listing.
async fn list(
    id: &str,
    upstream: &Upstream,
    grant: &Grant,
    params: &[(String, String)],
    target: &str,
) -> (StatusCode, Response<Body>) {
    let refuse = |why: &str| {
        (
            StatusCode::FORBIDDEN,
            error(id, StatusCode::FORBIDDEN, "AccessDenied", why, target),
        )
    };
    if !grant.list {
        return refuse("house-cache does not list for this worker (pinned reads never list)");
    }
    let prefix = param(params, "prefix").unwrap_or("");
    let Some(relative) = upstream.relative(prefix) else {
        return refuse("the prefix is outside this worker's namespace");
    };
    if !grant.allows(relative) {
        return refuse("the prefix is outside this worker's namespace");
    }
    let delimiter = param(params, "delimiter").filter(|d| !d.is_empty());
    let max_keys = param(params, "max-keys")
        .and_then(|m| m.parse::<usize>().ok())
        .unwrap_or(MAX_KEYS)
        .min(MAX_KEYS);
    let after = param(params, "continuation-token")
        .or_else(|| param(params, "start-after"))
        .and_then(|after| upstream.relative(after))
        .map(str::to_string);

    // The store lists by directory; S3 by string prefix. List the directory the
    // prefix is in and keep the keys that start with it.
    let dir = relative.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let mut keys: Vec<ObjectInfo> = Vec::new();
    let mut prefixes: Vec<String> = Vec::new();
    let mut cursor = after.clone();
    let mut truncated = false;
    'pages: loop {
        let page = match upstream
            .store()
            .list_page(dir, cursor.as_deref(), MAX_KEYS)
            .await
        {
            Ok(page) => page,
            Err(err) => return store_error(id, &err, target, false),
        };
        let full_page = page.len() == MAX_KEYS;
        for info in page {
            cursor = Some(info.path.clone());
            if !info.path.starts_with(relative) {
                continue;
            }
            if keys.len() + prefixes.len() == max_keys {
                truncated = true;
                break 'pages;
            }
            if let Some(delimiter) = delimiter
                && let Some(at) = info.path[relative.len()..].find(delimiter)
            {
                let common = info.path[..relative.len() + at + delimiter.len()].to_string();
                if prefixes.last() != Some(&common) {
                    prefixes.push(common);
                }
                continue;
            }
            keys.push(info);
        }
        if !full_page {
            break;
        }
    }

    let shown = |path: &str| {
        if upstream.prefix().is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", upstream.prefix())
        }
    };
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    xml.push_str(&format!("<Name>{}</Name>", escape(upstream.bucket())));
    xml.push_str(&format!("<Prefix>{}</Prefix>", escape(prefix)));
    xml.push_str(&format!(
        "<KeyCount>{}</KeyCount>",
        keys.len() + prefixes.len()
    ));
    xml.push_str(&format!("<MaxKeys>{max_keys}</MaxKeys>"));
    if let Some(delimiter) = delimiter {
        xml.push_str(&format!("<Delimiter>{}</Delimiter>", escape(delimiter)));
    }
    xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
    if let Some(token) = param(params, "continuation-token") {
        xml.push_str(&format!(
            "<ContinuationToken>{}</ContinuationToken>",
            escape(token)
        ));
    }
    if truncated {
        let last = keys
            .last()
            .map(|k| k.path.clone())
            .or_else(|| prefixes.last().cloned())
            .unwrap_or_default();
        xml.push_str(&format!(
            "<NextContinuationToken>{}</NextContinuationToken>",
            escape(&shown(&last))
        ));
    }
    for info in &keys {
        xml.push_str("<Contents>");
        xml.push_str(&format!("<Key>{}</Key>", escape(&shown(&info.path))));
        xml.push_str(&format!(
            "<LastModified>{}</LastModified>",
            iso_date(info.last_modified_ms)
        ));
        if let Some(etag) = etag(info) {
            xml.push_str(&format!("<ETag>{}</ETag>", escape(&etag)));
        }
        xml.push_str(&format!("<Size>{}</Size>", info.size));
        xml.push_str("<StorageClass>STANDARD</StorageClass></Contents>");
    }
    for common in &prefixes {
        xml.push_str(&format!(
            "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
            escape(&shown(common))
        ));
    }
    xml.push_str("</ListBucketResult>");
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/xml")
        .header("x-amz-request-id", id)
        .header(header::SERVER, "loams-house-cache")
        .body(full(Bytes::from(xml)))
        .unwrap_or_else(|_| internal(id));
    (StatusCode::OK, response)
}

/// What a `Range` header asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeAsk {
    /// No usable range: the whole object (S3 ignores a malformed or multiple
    /// range).
    Whole,
    /// These bytes.
    Part(Range<u64>),
    /// A range that starts past the end (`416`).
    Unsatisfiable,
}

/// Parses `bytes=a-b`, `bytes=a-` and `bytes=-n` against an object of `size`
/// bytes.
pub fn parse_range(value: &str, size: u64) -> RangeAsk {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeAsk::Whole;
    };
    if spec.contains(',') {
        return RangeAsk::Whole;
    }
    let Some((from, to)) = spec.trim().split_once('-') else {
        return RangeAsk::Whole;
    };
    let number = |text: &str| text.trim().parse::<u64>().ok();
    match (from.trim().is_empty(), to.trim().is_empty()) {
        (true, true) => RangeAsk::Whole,
        (true, false) => match number(to) {
            None => RangeAsk::Whole,
            Some(n) if n == 0 || size == 0 => RangeAsk::Unsatisfiable,
            Some(n) => RangeAsk::Part(size.saturating_sub(n)..size),
        },
        (false, open) => {
            let Some(start) = number(from) else {
                return RangeAsk::Whole;
            };
            let end = if open {
                size
            } else {
                match number(to) {
                    Some(last) if last >= start => last.saturating_add(1).min(size),
                    _ => return RangeAsk::Whole,
                }
            };
            if start >= size {
                RangeAsk::Unsatisfiable
            } else {
                RangeAsk::Part(start..end)
            }
        }
    }
}

/// Decodes `%XX` escapes; `None` for a malformed escape or bytes that are not
/// UTF-8. `+` stays a `+` (paths are not form-encoded).
pub fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes.get(at + 1..at + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn parse_query(query: &str) -> Option<Vec<(String, String)>> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((percent_decode(k)?, percent_decode(&v.replace('+', " "))?))
        })
        .collect()
}

fn param<'p>(params: &'p [(String, String)], name: &str) -> Option<&'p str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn etag(info: &ObjectInfo) -> Option<String> {
    let tag = info.version.e_tag.as_deref()?;
    Some(if tag.starts_with('"') {
        tag.to_string()
    } else {
        format!("\"{tag}\"")
    })
}

fn modified(info: &ObjectInfo) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(info.last_modified_ms)
}

fn http_date(info: &ObjectInfo) -> String {
    httpdate::fmt_http_date(modified(info))
}

/// `2026-10-11T08:30:00.000Z`, S3's listing date.
pub fn iso_date(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never: Infallible| match never {})
        .boxed_unsync()
}

fn empty() -> Body {
    Empty::new()
        .map_err(|never: Infallible| match never {})
        .boxed_unsync()
}

fn internal(id: &str) -> Response<Body> {
    let mut response = Response::new(full(Bytes::from_static(b"internal error")));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    if let Ok(value) = HeaderValue::from_str(id) {
        response.headers_mut().insert("x-amz-request-id", value);
    }
    response
}

/// An S3 error document.
fn error(
    id: &str,
    status: StatusCode,
    code: &str,
    message: &str,
    resource: &str,
) -> Response<Body> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{}</Message><Resource>{}</Resource><RequestId>{id}</RequestId></Error>",
        escape(message),
        escape(resource)
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/xml")
        .header("x-amz-request-id", id)
        .header(header::SERVER, "loams-house-cache")
        .body(full(Bytes::from(xml)))
        .unwrap_or_else(|_| internal(id))
}

fn store_error(
    id: &str,
    err: &StoreError,
    target: &str,
    head: bool,
) -> (StatusCode, Response<Body>) {
    let (status, code, message) = match err {
        StoreError::NotFound { .. } => {
            (StatusCode::NOT_FOUND, "NoSuchKey", "the key does not exist")
        }
        StoreError::InvalidPath(_) => (
            StatusCode::BAD_REQUEST,
            "InvalidArgument",
            "the key is not valid",
        ),
        // The store's own words may name its endpoint or account: they stay in
        // the front.
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailable",
            "the store could not be read",
        ),
    };
    let mut response = error(id, status, code, message, target);
    if head {
        *response.body_mut() = empty();
    }
    (status, response)
}

fn cache_error(id: &str, err: &CacheError, target: &str) -> (StatusCode, Response<Body>) {
    match err {
        CacheError::Store(err) => store_error(id, err, target, false),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            error(
                id,
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailable",
                "the object could not be read",
                target,
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_rfc_9110() {
        assert_eq!(parse_range("bytes=0-9", 100), RangeAsk::Part(0..10));
        assert_eq!(parse_range("bytes=90-200", 100), RangeAsk::Part(90..100));
        assert_eq!(parse_range("bytes=10-", 100), RangeAsk::Part(10..100));
        assert_eq!(parse_range("bytes=-10", 100), RangeAsk::Part(90..100));
        assert_eq!(parse_range("bytes=-500", 100), RangeAsk::Part(0..100));
        assert_eq!(parse_range("bytes=100-", 100), RangeAsk::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 100), RangeAsk::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), RangeAsk::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-2", 100), RangeAsk::Whole);
        assert_eq!(parse_range("bytes=0-1,4-5", 100), RangeAsk::Whole);
        assert_eq!(parse_range("items=0-1", 100), RangeAsk::Whole);
    }

    #[test]
    fn decoding_and_dates() {
        assert_eq!(
            percent_decode("/b/ns/1/a%20b").as_deref(),
            Some("/b/ns/1/a b")
        );
        assert_eq!(percent_decode("%2"), None);
        assert_eq!(percent_decode("%ff"), None);
        assert_eq!(iso_date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_date(1_791_966_600_123), "2026-10-14T08:30:00.123Z");
        assert_eq!(escape("a<&>\"'"), "a&lt;&amp;&gt;&quot;&apos;");
    }
}
