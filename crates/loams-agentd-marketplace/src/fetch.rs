//! agentd's [`Fetcher`]: HTTPS only, with a size cap and a timeout (MK1 Task
//! 4b). The marketplace reads every byte it uses through the `Fetcher` trait,
//! so another location (MS1 Task 5's Walrus aggregators) is a second
//! implementation behind the same trait, never a change here.

use std::time::Duration;

use crate::market::Fetcher;

/// The largest body the fetcher reads: an npm tarball of the biggest agent
/// we expect, with room. Indexes and bundles are far smaller.
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

/// How long one request may take, connect to last byte.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// An HTTPS client for sources, tarballs and provenance bundles. Refuses any
/// URL that is not `https://`, a body over its cap, and any status but 2xx.
#[derive(Debug, Clone)]
pub struct HttpsFetcher {
    client: reqwest::Client,
    max_bytes: usize,
}

impl HttpsFetcher {
    /// The production fetcher: the web PKI roots, the system proxy.
    ///
    /// # Errors
    ///
    /// When the TLS client cannot be built.
    pub fn new() -> Result<Self, String> {
        Self::builder(reqwest::Client::builder())
    }

    /// Also trusts `der` (a test CA). Tests only: production trusts the web
    /// PKI alone.
    ///
    /// # Errors
    ///
    /// When the certificate does not parse or the client cannot be built.
    pub fn with_root_certificate(der: &[u8]) -> Result<Self, String> {
        let cert = reqwest::Certificate::from_der(der).map_err(|e| e.to_string())?;
        Self::builder(reqwest::Client::builder().add_root_certificate(cert))
    }

    fn builder(builder: reqwest::ClientBuilder) -> Result<Self, String> {
        let client = builder
            .https_only(true)
            .timeout(TIMEOUT)
            .user_agent(concat!(
                "loams-agentd-marketplace/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            max_bytes: MAX_BODY_BYTES,
        })
    }

    /// A lower body cap (tests).
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

#[async_trait::async_trait]
impl Fetcher for HttpsFetcher {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        if !url.starts_with("https://") {
            return Err(format!("refusing a non-https URL: {url}"));
        }
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP {} for {url}", status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > self.max_bytes as u64)
        {
            return Err(format!("{url} is larger than {} bytes", self.max_bytes));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?
        {
            if body.len() + chunk.len() > self.max_bytes {
                return Err(format!("{url} is larger than {} bytes", self.max_bytes));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}
