//! `loams.ca.internal.v1.CertificateService` over [`Authority`] (design §83
//! §6.3, D2310).
//!
//! - `Issue` needs no client certificate: the join token in the request is
//!   the credential (D2311).
//! - `Renew` needs the caller's [`VerifiedPeer`] in the request's
//!   extensions, which the mTLS listener puts there after the handshake (CT1
//!   Task 6). A request without one is `unauthenticated`.
//! - `TrustBundle` is public.

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, as the meter's adapters do.
#![allow(refining_impl_trait)]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use loams_proto::google::protobuf::Timestamp;
use loams_proto::loams::ca::internal::v1 as pb;
use loams_proto::loams::ca::internal::v1::{CertificateService, CertificateServiceExt};
use loams_transport::VerifiedPeer;

use crate::authority::{Authority, CertificateIssued};
use crate::error::CaServiceError;

/// The service's method paths, as the route table names them.
pub const ISSUE_PATH: &str = "/loams.ca.internal.v1.CertificateService/Issue";
pub const RENEW_PATH: &str = "/loams.ca.internal.v1.CertificateService/Renew";
pub const TRUST_BUNDLE_PATH: &str = "/loams.ca.internal.v1.CertificateService/TrustBundle";

#[derive(Debug)]
struct Adapter(Arc<Authority>);

/// `router` with the CA service registered.
pub fn register(router: connectrpc::Router, authority: Arc<Authority>) -> connectrpc::Router {
    CertificateServiceExt::register(Arc::new(Adapter(authority)), router)
}

fn connect<T>(r: Result<T, CaServiceError>) -> Result<T, ConnectError> {
    r.map_err(|e| e.to_connect())
}

fn timestamp(t: SystemTime) -> Timestamp {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    Timestamp {
        seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(d.subsec_nanos()).unwrap_or(0),
        ..Default::default()
    }
}

fn certificate(c: CertificateIssued) -> pb::Certificate {
    pb::Certificate {
        chain: c.chain.iter().map(|d| d.as_ref().to_vec()).collect(),
        serial: c.serial,
        not_after: timestamp(c.not_after).into(),
        renew_at: timestamp(c.renew_at).into(),
        ..Default::default()
    }
}

impl CertificateService for Adapter {
    async fn issue(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::IssueRequest>,
    ) -> ServiceResult<pb::IssueResponse> {
        let r = request.to_owned_message();
        let issued = connect(self.0.issue(&r.join_token, &r.csr, SystemTime::now()).await)?;
        Response::ok(pb::IssueResponse {
            certificate: certificate(issued).into(),
            ..Default::default()
        })
    }

    async fn renew(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RenewRequest>,
    ) -> ServiceResult<pb::RenewResponse> {
        let peer = connect(
            ctx.extensions()
                .get::<VerifiedPeer>()
                .cloned()
                .ok_or(CaServiceError::Unauthenticated),
        )?;
        let r = request.to_owned_message();
        let issued = connect(self.0.renew(&peer, &r.csr, SystemTime::now()).await)?;
        Response::ok(pb::RenewResponse {
            certificate: certificate(issued).into(),
            ..Default::default()
        })
    }

    async fn trust_bundle(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::TrustBundleRequest>,
    ) -> ServiceResult<pb::TrustBundleResponse> {
        let bundle = connect(self.0.bundle(SystemTime::now()).await)?;
        Response::ok(pb::TrustBundleResponse {
            roots: bundle.roots.iter().map(|d| d.as_ref().to_vec()).collect(),
            intermediates: bundle
                .intermediates
                .iter()
                .map(|d| d.as_ref().to_vec())
                .collect(),
            ..Default::default()
        })
    }
}
