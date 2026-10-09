//! An in-process mock of `loams.instance.v1`, for `LOAMS_MOCK=1`, tests and
//! CI. It serves the same generated service trait a real server will, over
//! real sockets, on loopback only (D111).
//!
//! The shared, scenario-driven mock of the whole app surface is
//! `loams-apps-mock` in the main repository; this one is the smallest thing
//! that lets the desktop run with no server at all.

// The generated trait returns `impl Encodable<_>`; these impls name the
// concrete message, which is the intended refinement.
#![allow(refining_impl_trait)]

use std::net::SocketAddr;
use std::sync::Arc;

use connectrpc::{RequestContext, Response, Router, Server, ServiceRequest, ServiceResult};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::connect::loams::instance::v1::{InstanceService, InstanceServiceExt as _};
use crate::proto::loams::instance::v1::{
    Edition, GetInstanceRequest, GetInstanceResponse, Org, Principal, PrincipalKind, SignInKind,
    SignInMethod, WhoAmIRequest, WhoAmIResponse,
};

/// The mock's instance id (a fixed ULID; nothing here is a secret).
pub const MOCK_INSTANCE_ID: &str = "01J9ZMOCKINSTANCE000000000";

struct MockInstance;

impl InstanceService for MockInstance {
    async fn get_instance(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetInstanceRequest>,
    ) -> ServiceResult<GetInstanceResponse> {
        Response::ok(GetInstanceResponse {
            instance_id: MOCK_INSTANCE_ID.into(),
            name: "Loams (mock)".into(),
            edition: Edition::EDITION_OSS.into(),
            server_version: "0.0.0-mock".into(),
            api_versions: vec!["loams.instance.v1".into()],
            // A loopback stack before the auth plan needs no sign-in (D111).
            sign_in_methods: vec![SignInMethod {
                kind: SignInKind::SIGN_IN_KIND_NONE.into(),
                display_name: "Local stack".into(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    async fn who_am_i(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, WhoAmIRequest>,
    ) -> ServiceResult<WhoAmIResponse> {
        Response::ok(WhoAmIResponse {
            principal: buffa::MessageField::some(Principal {
                id: "usr_mock_dana".into(),
                kind: PrincipalKind::PRINCIPAL_KIND_USER.into(),
                display_name: "Dana (mock)".into(),
                email: "dana@example.invalid".into(),
                ..Default::default()
            }),
            org: buffa::MessageField::some(Org {
                id: "org_mock".into(),
                name: "Mock Org".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

/// A running mock; stop it with [`MockServer::stop`] or by dropping the
/// handle (the task is aborted).
#[derive(Debug)]
pub struct MockServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl MockServer {
    /// Binds `listen` (loopback only; `127.0.0.1:0` picks a free port) and
    /// serves in the background.
    ///
    /// # Errors
    ///
    /// Refuses a non-loopback address, and fails if the address cannot be
    /// bound.
    pub async fn start(listen: SocketAddr) -> anyhow::Result<Self> {
        anyhow::ensure!(
            listen.ip().is_loopback(),
            "the mock binds loopback only (D111); refusing {listen}"
        );
        let bound = Server::bind(listen)
            .await
            .map_err(|e| anyhow::anyhow!("binding {listen}: {e}"))?;
        let addr = bound.local_addr()?;
        let router = Arc::new(MockInstance).register(Router::new());
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let stop = async move {
                let _ = rx.await;
            };
            if let Err(error) = bound.serve_with_graceful_shutdown(router, stop).await {
                tracing::error!(%error, "loams mock stopped");
            }
        });
        Ok(Self {
            addr,
            shutdown: Some(tx),
            task,
        })
    }

    /// The base URL to point a [`crate::client::LoamsClient`] at.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The bound address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops the server, waiting briefly for open connections.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let wait = std::time::Duration::from_millis(500);
        if tokio::time::timeout(wait, &mut self.task).await.is_err() {
            self.task.abort();
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::LoamsClient;

    #[tokio::test]
    async fn client_reads_the_mock_instance_over_connect() {
        let mock = MockServer::start("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let client = LoamsClient::connect(&mock.url()).unwrap();
        let instance = client.get_instance().await.unwrap();
        assert_eq!(instance.instance_id, MOCK_INSTANCE_ID);
        assert_eq!(instance.api_versions, vec!["loams.instance.v1".to_owned()]);
        let me = client.who_am_i(None).await.unwrap();
        assert_eq!(me.principal.display_name, "Dana (mock)");
        mock.stop().await;
    }

    #[tokio::test]
    async fn mock_refuses_non_loopback() {
        let err = MockServer::start("0.0.0.0:0".parse().unwrap())
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("loopback"));
    }

    #[test]
    fn client_refuses_https_until_tls_lands() {
        let err = LoamsClient::connect("https://loams.example").err().unwrap();
        assert!(err.to_string().contains("client-tls"));
        assert!(LoamsClient::connect("ftp://x").is_err());
    }
}
