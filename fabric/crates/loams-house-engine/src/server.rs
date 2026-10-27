//! `loams-house-engine-dev`'s HTTP server (plan HE1 Task 23): enough of
//! ClickHouse's HTTP interface for the oracle runner, on loopback only. It is
//! a test tool, never part of a release image; the House front serves
//! clients (Task 39).
//!
//! * `GET /ping`: `Ok.`
//! * `POST /?database=…&param_<name>=…&<setting>=…`: the body (after the
//!   `query` parameter, if any) is one statement; the answer is in its
//!   `FORMAT` (only `JSONCompactEachRowWithNamesAndTypes` until Task 36), and
//!   an error is ClickHouse's: a non-200 status, `X-ClickHouse-Exception-Code`
//!   and `Code: N. DB::Exception: … (NAME)`.
//! * `POST /loams/seed?database=…&table=…&engine=…&sorting_key=…`: a table's
//!   rows as `Parquet`, its columns as JSON in `X-Loams-Columns`
//!   (`loams-house-mine`'s seeded targets, D1804).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use datafusion::datasource::MemTable;
use loams_house_types::ChError;

use crate::catalog::Table;
use crate::engine::Engine;

/// The format the oracle reads (HE0 R6.2).
pub const ORACLE_FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

/// HTTP request parameters that are not settings.
const NOT_SETTINGS: &[&str] = &[
    "query",
    "database",
    "query_id",
    "default_format",
    "user",
    "password",
    "session_id",
    "session_timeout",
    "compress",
    "decompress",
];

struct Request {
    method: String,
    path: String,
    params: Vec<(String, String)>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request(stream: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if stream.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if stream.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    stream.read_exact(&mut body)?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect();
    Ok(Some(Request {
        method,
        path: path.to_string(),
        params,
        headers,
        body,
    }))
}

fn error(e: &ChError) -> Response {
    let status = match e.code {
        60 | 81 => 404,
        62 | 6 | 36 | 43 | 46 | 47 | 456 => 400,
        _ => 500,
    };
    Response {
        status,
        headers: vec![("X-ClickHouse-Exception-Code".into(), e.code.to_string())],
        body: format!(
            "Code: {}. DB::Exception: {}. ({}) (version {})\n",
            e.code,
            e.message,
            e.name,
            crate::registry::VERSION
        )
        .into_bytes(),
    }
}

fn handle(engine: &Engine, runtime: &tokio::runtime::Runtime, req: Request) -> Response {
    let param = |name: &str| {
        req.params
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/ping") | ("GET", "/") if req.params.is_empty() => Response {
            status: 200,
            headers: vec![],
            body: b"Ok.\n".to_vec(),
        },
        ("POST", "/loams/seed") => match seed(engine, &req, &param) {
            Ok(()) => Response {
                status: 200,
                headers: vec![],
                body: b"Ok.\n".to_vec(),
            },
            Err(e) => error(&e),
        },
        (_, "/") => {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let sql = match param("query") {
                Some(q) if !body.trim().is_empty() => format!("{q}\n{body}"),
                Some(q) => q,
                None => body,
            };
            let database = param("database").unwrap_or_else(|| "default".to_string());
            let params: Vec<(String, String)> = req
                .params
                .iter()
                .filter_map(|(k, v)| k.strip_prefix("param_").map(|n| (n.to_string(), v.clone())))
                .collect();
            let settings: Vec<(String, String)> = req
                .params
                .iter()
                .filter(|(k, _)| !k.starts_with("param_") && !NOT_SETTINGS.contains(&k.as_str()))
                .cloned()
                .collect();
            let default_format =
                param("default_format").unwrap_or_else(|| "TabSeparated".to_string());
            let result = runtime.block_on(engine.query(&sql, &params, &settings, &database));
            match result {
                Ok(answer) => {
                    let format = answer.format.clone().unwrap_or(default_format);
                    if format != ORACLE_FORMAT {
                        return error(&ChError::not_implemented(format!(
                            "The format {format} is not supported by the Loams House engine yet (plan HE1 Task 36)"
                        )));
                    }
                    let mut out = Vec::new();
                    let written = loams_house_formats::columns(&answer.schema).and_then(|cols| {
                        loams_house_formats::json_compact::write(
                            &cols,
                            &answer.batches,
                            &answer.settings.text(),
                            &mut out,
                        )
                    });
                    match written {
                        Ok(()) => Response {
                            status: 200,
                            headers: vec![(
                                "Content-Type".into(),
                                "application/json; charset=UTF-8".into(),
                            )],
                            body: out,
                        },
                        Err(e) => error(&e),
                    }
                }
                Err(e) => error(&e),
            }
        }
        _ => Response {
            status: 404,
            headers: vec![],
            body: b"Not found\n".to_vec(),
        },
    }
}

fn seed(
    engine: &Engine,
    req: &Request,
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ChError> {
    let bad = |m: &str| ChError::bad_arguments(format!("Seeding: {m}"));
    let database = param("database").ok_or_else(|| bad("no database"))?;
    let table = param("table").ok_or_else(|| bad("no table"))?;
    let columns = req
        .headers
        .get("x-loams-columns")
        .ok_or_else(|| bad("no X-Loams-Columns"))?;
    let columns: Vec<(String, String)> =
        serde_json::from_str(columns).map_err(|e| bad(&e.to_string()))?;
    let columns = columns
        .into_iter()
        .map(|(n, t)| Ok((n, loams_house_types::parse(&t)?)))
        .collect::<Result<Vec<_>, ChError>>()?;
    let (schema, batches) =
        crate::seed::read_parquet(&columns, bytes::Bytes::from(req.body.clone()))?;
    let data = MemTable::try_new(schema, vec![batches]).map_err(|e| bad(&e.to_string()))?;
    engine.catalog.insert(
        &database,
        &table,
        Table {
            data: Arc::new(data),
            engine: param("engine").unwrap_or_default(),
            sorting_key: param("sorting_key").unwrap_or_default(),
        },
    );
    Ok(())
}

fn connection(
    engine: &Engine,
    runtime: &tokio::runtime::Runtime,
    stream: TcpStream,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    while let Some(req) = read_request(&mut reader)? {
        let close = req
            .headers
            .get("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        let resp = handle(engine, runtime, req);
        let reason = if resp.status == 200 { "OK" } else { "Error" };
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\n",
            resp.status,
            resp.body.len()
        );
        for (k, v) in &resp.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        writer.write_all(head.as_bytes())?;
        writer.write_all(&resp.body)?;
        writer.flush()?;
        if close {
            break;
        }
    }
    Ok(())
}

/// Serves `listener` until the process ends, one thread per connection.
///
/// # Errors
///
/// A failure to accept connections.
pub fn serve(engine: Arc<Engine>, listener: TcpListener) -> io::Result<()> {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );
    for stream in listener.incoming() {
        let stream = stream?;
        let engine = engine.clone();
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            if let Err(e) = connection(&engine, &runtime, stream) {
                eprintln!("loams-house-engine-dev: connection: {e}");
            }
        });
    }
    Ok(())
}
