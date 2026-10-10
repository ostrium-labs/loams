//! A throwaway CA with a gate certificate and an upstream (TiDB) certificate.
use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct Pki {
    pub ca: CertificateDer<'static>,
    pub gate_chain: Vec<CertificateDer<'static>>,
    pub gate_key: PrivateKeyDer<'static>,
    pub upstream_chain: Vec<CertificateDer<'static>>,
    pub upstream_key: PrivateKeyDer<'static>,
    pub ca_pem: String,
    pub upstream_pem: (String, String),
}

impl Pki {
    pub fn new() -> Self {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().expect("ca key"))
            .expect("ca");
        let leaf = |names: Vec<String>| {
            let key = KeyPair::generate().expect("key");
            let cert = CertificateParams::new(names)
                .expect("params")
                .signed_by(&key, &ca)
                .expect("leaf");
            (cert, key)
        };
        let (gate, gate_key) = leaf(vec![
            "localhost".into(),
            "127.0.0.1".into(),
            "db-a.sql.test".into(),
        ]);
        let (up, up_key) = leaf(vec!["127.0.0.1".into(), "tidb.test".into()]);
        Self {
            ca: ca.der().clone(),
            gate_chain: vec![gate.der().clone()],
            gate_key: PrivatePkcs8KeyDer::from(gate_key.serialize_der()).into(),
            upstream_chain: vec![up.der().clone()],
            upstream_key: PrivatePkcs8KeyDer::from(up_key.serialize_der()).into(),
            ca_pem: ca.pem(),
            upstream_pem: (up.pem(), up_key.serialize_pem()),
        }
    }

    pub fn roots(&self) -> rustls::RootCertStore {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.ca.clone()).expect("ca");
        roots
    }

    /// A client config trusting the CA (the gate's upstream side, and test clients).
    pub fn client_config(&self) -> Arc<rustls::ClientConfig> {
        Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(self.roots())
            .with_no_client_auth(),
        )
    }

    /// The fake TiDB's server config.
    pub fn upstream_server_config(&self) -> Arc<rustls::ServerConfig> {
        Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(self.upstream_chain.clone(), self.upstream_key.clone_key())
            .expect("cert"),
        )
    }
}
