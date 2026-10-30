//! Resumable transfers and job runs on the engine (design §85, DQ1 Task 5):
//! `loams-transfer`'s routes under `/v1/namespaces/{ns}/transfers`, `/runs`
//! and `/runner/claim`, on the engine's metastore (leases and fenced
//! pointers) and bucket, behind the engine's authentication layer.
//!
//! Authorization (D2443, ruling D2444): the request must carry the
//! principal the authentication layer put there, as every other REST route
//! does. Reading and writing need any authenticated principal; claiming and
//! feeding runs (`runner`) needs an API key, the service credential a
//! runner holds. Reserved namespaces are refused. Namespace roles come with
//! the REST API's own (design 76).
//!
//! HTTP/3: with TLS files on the main port, [`h3_endpoint`] binds UDP at the
//! main listener's port number and the same router serves HTTP/3 there
//! (D2437); the TCP listener is the HTTP/2 fallback.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use http::HeaderMap;
use loams_common::NamespaceId;
use loams_common::meta::{Consistency, MetaStore, is_reserved_namespace};
use loams_store::Store;
use loams_transfer::server::{Access, Api, Authorizer, Denied};
use loams_transfer::{MetaCheckpoints, TransfersConfig};

use crate::auth::{Principal, PrincipalKind};

/// The engine's [`Authorizer`]: the authenticated principal, and the
/// namespace by name from the metastore.
#[derive(Debug)]
pub struct EngineAuthorizer {
    meta: Arc<dyn MetaStore>,
}

/// Whether `principal` may use a route that needs `need`.
pub fn allowed(principal: &Principal, need: Access) -> bool {
    match need {
        Access::Read | Access::Write => true,
        Access::Runner => matches!(
            principal.kind,
            PrincipalKind::ApiKey | PrincipalKind::DevLoopback
        ),
    }
}

#[async_trait]
impl Authorizer for EngineAuthorizer {
    async fn authorize(
        &self,
        _headers: &HeaderMap,
        extensions: &http::Extensions,
        ns: &str,
        need: Access,
    ) -> Result<NamespaceId, Denied> {
        let principal = extensions
            .get::<Principal>()
            .ok_or(Denied::Unauthenticated)?;
        if !allowed(principal, need) {
            return Err(Denied::Forbidden);
        }
        if is_reserved_namespace(ns) {
            return Err(Denied::NoNamespace);
        }
        match self.meta.namespace_by_name(Consistency::Local, ns).await {
            Ok(Some(n)) => Ok(n.id),
            _ => Err(Denied::NoNamespace),
        }
    }
}

/// The transfer and run routes on this engine's metastore and bucket.
pub fn routes(meta: Arc<dyn MetaStore>, store: Store) -> axum::Router {
    let auth = Arc::new(EngineAuthorizer {
        meta: Arc::clone(&meta),
    });
    let api: Api = loams_transfer::standalone::api(
        store,
        Arc::new(MetaCheckpoints::new(meta)),
        TransfersConfig::default(),
        auth,
    );
    loams_transfer::server::router(api)
}

/// The environment switch for HTTP/3 on the main port: on by default when
/// the main port has TLS files; `LOAMS_TRANSFER_H3=off` turns it off.
pub const H3_ENV: &str = "LOAMS_TRANSFER_H3";

/// A QUIC endpoint at the main listener's port number, when the main port
/// has TLS files and HTTP/3 is not turned off. A failure to bind is logged
/// and leaves the engine on HTTP/2 only: clients fall back (D2317).
pub fn h3_endpoint(
    tls: &crate::tls::TlsConfig,
    addr: SocketAddr,
) -> Option<loams_transfer::net::h3::Endpoint> {
    if std::env::var(H3_ENV).is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
        return None;
    }
    let (Some(cert), Some(key)) = (&tls.cert_file, &tls.key_file) else {
        return None;
    };
    let id = match loams_transfer::net::ServerIdentity::from_pem_files(cert, key) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(%err, "HTTP/3 is off: the TLS files could not be read for QUIC");
            return None;
        }
    };
    match loams_transfer::net::h3::bind_server(addr, &id) {
        Ok(ep) => {
            tracing::info!(%addr, "HTTP/3 (QUIC) is served on UDP at the main port");
            Some(ep)
        }
        Err(err) => {
            tracing::warn!(%err, %addr, "HTTP/3 is off: UDP could not be bound");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(kind: PrincipalKind) -> Principal {
        Principal {
            subject: "s".into(),
            kind,
            issuer: None,
            display_name: "s".into(),
            email: None,
            grant: Default::default(),
        }
    }

    #[test]
    fn runner_routes_need_an_api_key() {
        assert!(allowed(&principal(PrincipalKind::User), Access::Write));
        assert!(!allowed(&principal(PrincipalKind::User), Access::Runner));
        assert!(!allowed(&principal(PrincipalKind::Session), Access::Runner));
        assert!(allowed(&principal(PrincipalKind::ApiKey), Access::Runner));
    }
}
