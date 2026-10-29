//! mTLS handshakes between `server_config` and `client_config`, in memory
//! (no sockets), with a throwaway CA (design §83 §6, §7; CT1 Task 3).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use loams_transport::{Alpn, peer_identity};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::{ClientConnection, ServerConnection};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};

struct Ca {
    cert: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
}

fn ca() -> Ca {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key).unwrap();
    Ca {
        cert: cert.der().clone(),
        issuer: Issuer::new(params, key),
    }
}

fn leaf(ca: &Ca, ids: &[&str]) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    for id in ids {
        p.subject_alt_names
            .push(SanType::URI((*id).try_into().unwrap()));
    }
    p.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let cert = p.signed_by(&key, &ca.issuer).unwrap();
    (
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

/// Drives both ends to completion in memory; the error is the first side's.
fn handshake(
    server: Arc<rustls::ServerConfig>,
    client: Arc<rustls::ClientConfig>,
) -> Result<(ServerConnection, ClientConnection), rustls::Error> {
    let mut s = ServerConnection::new(server).unwrap();
    let mut c = ClientConnection::new(client, ServerName::try_from("localhost").unwrap()).unwrap();
    for _ in 0..16 {
        let mut buf = Vec::new();
        while c.wants_write() {
            c.write_tls(&mut buf).unwrap();
        }
        s.read_tls(&mut buf.as_slice()).unwrap();
        s.process_new_packets()?;
        let mut buf = Vec::new();
        while s.wants_write() {
            s.write_tls(&mut buf).unwrap();
        }
        c.read_tls(&mut buf.as_slice()).unwrap();
        c.process_new_packets()?;
        if !s.is_handshaking() && !c.is_handshaking() {
            // The client's last flight (its certificate and Finished).
            let mut buf = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut buf).unwrap();
            }
            s.read_tls(&mut buf.as_slice()).unwrap();
            s.process_new_packets()?;
            return Ok((s, c));
        }
    }
    panic!("handshake did not finish");
}

#[test]
fn mutual_tls_names_the_peer_on_both_sides() {
    let ca = ca();
    let bundle = [ca.cert.clone()];
    let (schain, skey) = leaf(&ca, &["spiffe://c1/node/1", "spiffe://c1/role/gateway"]);
    let (cchain, ckey) = leaf(&ca, &["spiffe://c1/svc/meter-agent"]);
    for alpn in [Alpn::H3, Alpn::H2] {
        let server =
            loams_transport::server_config(schain.clone(), skey.clone_key(), &bundle, alpn)
                .unwrap();
        let client =
            loams_transport::client_config(cchain.clone(), ckey.clone_key(), &bundle, alpn)
                .unwrap();
        let (s, c) = handshake(server, client).unwrap();
        assert_eq!(s.alpn_protocol(), Some(alpn.id()));
        assert_eq!(s.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));
        let caller = peer_identity(s.peer_certificates().unwrap()).unwrap();
        assert!(caller.has("svc/meter-agent"));
        assert!(!caller.has("role/gateway"));
        assert_eq!(caller.cluster(), "c1");
        let server_id = peer_identity(c.peer_certificates().unwrap()).unwrap();
        assert_eq!(server_id.node().unwrap().name(), "1");
        assert!(server_id.has("role/gateway"));
    }
}

#[test]
fn a_client_from_another_ca_is_refused() {
    let ours = ca();
    let theirs = ca();
    let (schain, skey) = leaf(&ours, &["spiffe://c1/node/1"]);
    let (cchain, ckey) = leaf(&theirs, &["spiffe://c1/role/gateway"]);
    let server =
        loams_transport::server_config(schain, skey, std::slice::from_ref(&ours.cert), Alpn::H2)
            .unwrap();
    // The stranger trusts our CA, so only the server's check can refuse it.
    let client =
        loams_transport::client_config(cchain, ckey, std::slice::from_ref(&ours.cert), Alpn::H2)
            .unwrap();
    assert!(handshake(server, client).is_err());
}

#[test]
fn a_client_without_a_certificate_is_refused() {
    let ca = ca();
    let (schain, skey) = leaf(&ca, &["spiffe://c1/node/1"]);
    let server =
        loams_transport::server_config(schain, skey, std::slice::from_ref(&ca.cert), Alpn::H2)
            .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.cert.clone()).unwrap();
    let mut anon = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    anon.alpn_protocols = vec![b"h2".to_vec()];
    assert!(handshake(server, Arc::new(anon)).is_err());
}

#[test]
fn a_certificate_without_a_spiffe_id_has_no_identity() {
    let ca = ca();
    let (chain, _) = leaf(&ca, &[]);
    assert!(peer_identity(&chain).is_err());
    let (chain, _) = leaf(&ca, &["spiffe://a/node/1", "spiffe://b/role/gateway"]);
    assert!(
        peer_identity(&chain).is_err(),
        "two clusters in one certificate"
    );
}

#[test]
fn zero_rtt_is_off_on_both_sides() {
    let ca = ca();
    let (chain, key) = leaf(&ca, &["spiffe://c1/node/1"]);
    let s = loams_transport::server_config(
        chain.clone(),
        key.clone_key(),
        std::slice::from_ref(&ca.cert),
        Alpn::H3,
    )
    .unwrap();
    let c = loams_transport::client_config(chain, key, std::slice::from_ref(&ca.cert), Alpn::H3)
        .unwrap();
    assert_eq!(s.max_early_data_size, 0);
    assert!(!c.enable_early_data);
}

#[test]
fn an_empty_bundle_is_an_error() {
    let ca = ca();
    let (chain, key) = leaf(&ca, &["spiffe://c1/node/1"]);
    assert!(loams_transport::server_config(chain, key, &[], Alpn::H2).is_err());
}
