//! A Connect-RPC client for a local `loams` server (design 37, section 8).
//!
//! The transport is connect-rust's plaintext HTTP client, which is all a
//! loopback stack needs (D111: local stacks listen on 127.0.0.1 without TLS).
//! Remote HTTPS instances arrive with the `client-tls` feature in plan AP1n
//! Task 4, together with the QR/SPKI pinning of design 37 section 7.2.3.

use connectrpc::ConnectError;
use connectrpc::client::{CallOptions, ClientConfig, HttpClient};

use crate::connect::loams::instance::v1::InstanceServiceClient;
use crate::proto::loams::instance::v1::{
    GetInstanceRequest, GetInstanceResponse, WhoAmIRequest, WhoAmIResponse,
};

/// What can go wrong talking to a loams server.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The configured base URL is not usable.
    #[error("invalid loams server URL {url:?}: {reason}")]
    InvalidUrl {
        /// The URL as configured.
        url: String,
        /// Why it was refused.
        reason: String,
    },
    /// The server answered with a Connect error.
    #[error("loams server error: {0}")]
    Rpc(#[from] ConnectError),
}

/// The instance service client.
#[derive(Clone)]
pub struct LoamsClient {
    inner: InstanceServiceClient<HttpClient>,
}

impl std::fmt::Debug for LoamsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoamsClient").finish_non_exhaustive()
    }
}

impl LoamsClient {
    /// A client for `server_url`, for example `http://127.0.0.1:8080`.
    ///
    /// # Errors
    ///
    /// Fails for an unparseable URL or any scheme but `http` (see the module
    /// documentation for why `https` is not yet supported).
    pub fn connect(server_url: &str) -> Result<Self, ClientError> {
        let invalid = |reason: &str| ClientError::InvalidUrl {
            url: server_url.to_owned(),
            reason: reason.to_owned(),
        };
        let uri: http::Uri = server_url.parse().map_err(|_| invalid("not a valid URI"))?;
        match uri.scheme_str() {
            Some("http") => {}
            Some("https") => {
                return Err(invalid(
                    "https needs the client-tls transport (not built yet)",
                ));
            }
            _ => return Err(invalid("expected an http:// URL")),
        }
        let config = ClientConfig::new(uri);
        Ok(Self {
            inner: InstanceServiceClient::new(HttpClient::plaintext(), config),
        })
    }

    /// `GetInstance`: what the instance is and how to sign in. Needs no
    /// credentials.
    ///
    /// # Errors
    ///
    /// Any transport or Connect error.
    pub async fn get_instance(&self) -> Result<GetInstanceResponse, ClientError> {
        let response = self
            .inner
            .get_instance(GetInstanceRequest::default())
            .await?;
        Ok(response.into_owned())
    }

    /// `WhoAmI`: the calling principal. With no token, a real server answers
    /// `unauthenticated`; a loopback stack before the auth plan answers
    /// anonymously.
    ///
    /// # Errors
    ///
    /// Any transport or Connect error.
    pub async fn who_am_i(
        &self,
        access_token: Option<&str>,
    ) -> Result<WhoAmIResponse, ClientError> {
        let mut options = CallOptions::default();
        if let Some(token) = access_token {
            options = options.with_header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .inner
            .who_am_i_with_options(WhoAmIRequest::default(), options)
            .await?;
        Ok(response.into_owned())
    }
}
