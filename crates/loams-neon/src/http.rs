//! The HTTP plumbing every client shares: JSON in and out, a bearer token,
//! and `{"msg": ...}` errors.

use reqwest::{Client, Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::{Component, NeonError, Secret};

/// One component's base URL and token.
#[derive(Clone, Debug)]
pub(crate) struct Http {
    client: Client,
    pub(crate) component: Component,
    pub(crate) base: Url,
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
    pub(crate) fn new(component: Component, base: Url, token: Option<Secret<String>>) -> Self {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            client,
            component,
            base,
            token,
        }
    }

    pub(crate) fn url(&self, path: &str) -> Result<Url, NeonError> {
        self.base
            .join(path)
            .map_err(|e| NeonError::transport(self.component, e))
    }

    pub(crate) fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, NeonError> {
        let mut req = self.client.request(method, self.url(path)?);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token.expose());
        }
        Ok(req)
    }

    /// Send, and read the answer as `T` (a `null` or empty body as `()`).
    pub(crate) async fn send<T: DeserializeOwned>(
        &self,
        req: RequestBuilder,
    ) -> Result<T, NeonError> {
        let transport = |e| NeonError::transport(self.component, e);
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
            });
        }
        let body: &[u8] = if body.is_empty() { b"null" } else { &body };
        serde_json::from_slice(body)
            .map_err(|e| NeonError::transport(self.component, format!("unreadable answer: {e}")))
    }

    pub(crate) async fn json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T, NeonError> {
        self.send(self.request(method, path)?.json(body)).await
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, NeonError> {
        self.send(self.request(Method::GET, path)?).await
    }
}
