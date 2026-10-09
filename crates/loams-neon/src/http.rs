//! The HTTP plumbing every client shares: JSON in and out, a bearer token,
//! per-call timeouts, and `{"msg": ...}` or `{"error": ...}` errors.

use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::Op;
use crate::{Component, NeonError, Secret};

/// The connect timeout of every call.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The whole-call timeout of a call with no longer one of its own.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// One component's base URL and token.
#[derive(Clone, Debug)]
pub(crate) struct Http {
    client: Client,
    pub(crate) component: Component,
    base: Url,
    token: Option<Secret<String>>,
}

/// An error body: `{"msg": ...}` from the storage components and
/// `loams-wal`, `{"error": ...}` from `compute_ctl`.
#[derive(serde::Deserialize)]
struct ErrorBody {
    msg: Option<String>,
    error: Option<String>,
}

impl Http {
    /// `base` is `http(s)://host:port` with an optional path prefix (kept:
    /// `http://gw/ps` calls `http://gw/ps/v1/...`). Credentials in the URL
    /// are refused: the token goes in `token`, which is never logged.
    pub(crate) fn new(
        component: Component,
        base: Url,
        token: Option<Secret<String>>,
    ) -> Result<Self, NeonError> {
        let setup = |msg: String| NeonError {
            status: 0,
            msg,
            component,
            op: Op::Setup,
        };
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(setup(format!(
                "the {component} URL must be http(s)://host[:port][/prefix]"
            )));
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(setup(format!(
                "the {component} URL carries credentials; pass the token instead"
            )));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(setup(format!(
                "the {component} URL has a query or fragment"
            )));
        }
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| setup(format!("no HTTP client: {e}")))?;
        Ok(Self {
            client,
            component,
            base,
            token,
        })
    }

    /// The base URL with `path` (which starts with `/`) after its prefix.
    pub(crate) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        let prefix = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{prefix}{path}"));
        url
    }

    pub(crate) fn request(&self, method: Method, path: &str, timeout: Duration) -> RequestBuilder {
        let mut req = self.client.request(method, self.url(path)).timeout(timeout);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token.expose());
        }
        req
    }

    /// Send, and read the answer as `T` (a `null` or empty body as `()`).
    pub(crate) async fn send<T: DeserializeOwned>(
        &self,
        op: Op,
        req: RequestBuilder,
    ) -> Result<T, NeonError> {
        let transport = |e: reqwest::Error| {
            // A timeout says nothing about the URL; reqwest's text names it.
            let e = e.without_url();
            NeonError::transport(self.component, op, e)
        };
        let resp = req.send().await.map_err(transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(transport)?;
        if !status.is_success() {
            let msg = serde_json::from_slice::<ErrorBody>(&body)
                .ok()
                .and_then(|e| e.msg.or(e.error))
                .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
            return Err(NeonError {
                status: status.as_u16(),
                msg,
                component: self.component,
                op,
            });
        }
        let body: &[u8] = if body.is_empty() { b"null" } else { &body };
        serde_json::from_slice(body).map_err(|e| {
            NeonError::transport(self.component, op, format!("unreadable answer: {e}"))
        })
    }

    pub(crate) async fn json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        op: Op,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T, NeonError> {
        self.send(op, self.request(method, path, DEFAULT_TIMEOUT).json(body))
            .await
    }

    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        op: Op,
        path: &str,
    ) -> Result<T, NeonError> {
        self.send(op, self.request(Method::GET, path, DEFAULT_TIMEOUT))
            .await
    }
}
