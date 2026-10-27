//! Loopback-only development HTTP endpoint. Never a production listener.
use crate::{catalog::MemoryCatalog, fixtures::Snapshot};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use loams_house_types::ChError;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::net::TcpListener;

/// Bind only loopback, including when called directly rather than through the CLI.
pub async fn bind(address: SocketAddr) -> anyhow::Result<TcpListener> {
    anyhow::ensure!(
        address.ip().is_loopback(),
        "the oracle dev server must bind a loopback address"
    );
    Ok(TcpListener::bind(address).await?)
}
/// Serve a listener returned by `bind`. Snapshots reload between corpus runs.
pub async fn serve(listener: TcpListener, fixtures: Option<PathBuf>) -> anyhow::Result<()> {
    anyhow::ensure!(listener.local_addr()?.ip().is_loopback(), "loopback only");
    let fixtures = Arc::new(fixtures);
    loop {
        let (stream, _) = listener.accept().await?;
        let fixtures = fixtures.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let fixtures = fixtures.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(reply(request, fixtures.as_deref()).await)
                }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("oracle HTTP connection: {error}");
            }
        });
    }
}
async fn reply(
    request: Request<Incoming>,
    fixtures: Option<&std::path::Path>,
) -> Response<Full<Bytes>> {
    match query(request, fixtures).await {
        Ok(bytes) => Response::new(Full::new(Bytes::from(bytes))),
        Err(error) => {
            let mut response = Response::new(Full::new(Bytes::from(format!("{error}\n"))));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response.headers_mut().insert(
                "X-ClickHouse-Exception-Code",
                error.code.to_string().parse().expect("numeric code"),
            );
            response
        }
    }
}
async fn query(
    request: Request<Incoming>,
    fixtures: Option<&std::path::Path>,
) -> Result<Vec<u8>, ChError> {
    if request.method() != hyper::Method::POST {
        return Err(ChError::bad_arguments("Use POST"));
    }
    let mut sql = String::new();
    let mut database = "default".to_string();
    let mut params = Vec::new();
    for (key, value) in url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
    {
        match key.as_ref() {
            "query" => sql = value.into_owned(),
            "database" => database = value.into_owned(),
            "default_format" if value == "JSONCompactEachRowWithNamesAndTypes" => {}
            key if key.starts_with("param_") => {
                params.push((key[6..].to_string(), value.into_owned()))
            }
            key => {
                return Err(ChError::unknown_setting(format!(
                    "Setting {key} is not supported yet"
                )));
            }
        }
    }
    let body = Limited::new(request.into_body(), 1024 * 1024)
        .collect()
        .await
        .map_err(|e| ChError::bad_arguments(e.to_string()))?
        .to_bytes();
    if !sql.is_empty() && !body.is_empty() {
        sql.push('\n');
    }
    sql.push_str(
        std::str::from_utf8(&body).map_err(|e| ChError::cannot_parse_text(e.to_string()))?,
    );
    let catalog = match fixtures {
        Some(root) if root.join("snapshot.json").exists() => {
            Snapshot::load(root)
                .map_err(|e| ChError::cannot_parse_text(format!("loading snapshot: {e:#}")))?
                .1
        }
        _ => MemoryCatalog::default(),
    };
    crate::execute::execute(&sql, &params, &database, &catalog).await
}
