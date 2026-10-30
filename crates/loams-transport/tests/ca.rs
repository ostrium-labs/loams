//! The CA, join tokens, identities and the deny list together (design §83 §6,
//! CT1 Task 3 part 2).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use loams_transport::ca::{BACKDATE, LEAF_LIFETIME};
use loams_transport::{
    Alpn, Ca, CaError, DenyList, Grant, Identity, JoinToken, Kind, MemoryTokens, SpiffeId,
    TokenError, TokenStore, new_key_and_csr, peer_identity,
};
use rustls::{ClientConnection, ServerConnection};
use rustls_pki_types::ServerName;

fn id(s: &str) -> SpiffeId {
    s.parse().unwrap()
}

fn open_ca(now: SystemTime) -> Ca {
    let material = Ca::init("c1", now).unwrap();
    Ca::load("c1", &material).unwrap()
}

/// Joins: redeem a token, make a key and CSR in memory, get a leaf.
fn join(
    ca: &Ca,
    tokens: &MemoryTokens,
    token: &JoinToken,
    ids: &[SpiffeId],
    serial: u64,
    now: SystemTime,
) -> Result<Identity, String> {
    let grant = tokens.redeem(token, now).map_err(|e| e.to_string())?;
    let (key, csr) = new_key_and_csr(ids, &["localhost".to_string()]).unwrap();
    let issued = ca
        .issue(&csr, &grant, serial, now)
        .map_err(|e| e.to_string())?;
    Ok(Identity::new(issued.chain, key, ca.bundle()).unwrap())
}

fn handshake(
    server: Arc<rustls::ServerConfig>,
    client: Arc<rustls::ClientConfig>,
) -> Result<ServerConnection, rustls::Error> {
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
            let mut buf = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut buf).unwrap();
            }
            s.read_tls(&mut buf.as_slice()).unwrap();
            s.process_new_packets()?;
            return Ok(s);
        }
    }
    panic!("handshake did not finish");
}

fn token_for(tokens: &MemoryTokens, ids: &[&str], now: SystemTime) -> JoinToken {
    let token = JoinToken::generate().unwrap();
    tokens.put(token.record(
        Grant {
            ids: ids.iter().map(|s| id(s)).collect(),
        },
        now,
    ));
    token
}

#[test]
fn a_joined_node_and_service_talk_mutual_tls() {
    let now = SystemTime::now();
    let ca = open_ca(now);
    let tokens = MemoryTokens::default();
    let node_token = token_for(
        &tokens,
        &["spiffe://c1/node/1", "spiffe://c1/role/gateway"],
        now,
    );
    let agent_token = token_for(&tokens, &["spiffe://c1/svc/meter-agent"], now);
    let node = join(
        &ca,
        &tokens,
        &node_token,
        &[id("spiffe://c1/node/1"), id("spiffe://c1/role/gateway")],
        1,
        now,
    )
    .unwrap();
    let agent = join(
        &ca,
        &tokens,
        &agent_token,
        &[id("spiffe://c1/svc/meter-agent")],
        2,
        now,
    )
    .unwrap();
    for alpn in [Alpn::H3, Alpn::H2] {
        let s = handshake(
            node.server_config(alpn).unwrap(),
            agent.client_config(alpn).unwrap(),
        )
        .unwrap();
        let caller = peer_identity(s.peer_certificates().unwrap()).unwrap();
        assert!(caller.has("svc/meter-agent"));
    }
    assert!(node.ids().has("role/gateway"));
    assert_eq!(node.ids().node().unwrap().name(), "1");
    assert_eq!(node.serial(), [1u8]);
    assert!(node.is_valid_at(now));
    assert!(!node.is_valid_at(now + LEAF_LIFETIME + Duration::from_secs(1)));
}

#[test]
fn leaves_live_a_day_and_renew_at_two_thirds() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let ca = open_ca(now);
    let tokens = MemoryTokens::default();
    let t = token_for(&tokens, &["spiffe://c1/node/9"], now);
    let ident = join(&ca, &tokens, &t, &[id("spiffe://c1/node/9")], 9, now).unwrap();
    assert_eq!(ident.not_before(), now - BACKDATE);
    assert_eq!(ident.not_after(), now + LEAF_LIFETIME);
    let life = LEAF_LIFETIME + BACKDATE;
    assert_eq!(ident.renew_at(), now - BACKDATE + life * 2 / 3);
    assert!(!ident.needs_renewal(now));
    assert!(ident.needs_renewal(now + Duration::from_secs(17 * 3600)));
}

#[test]
fn a_token_works_once_and_not_after_an_hour() {
    let now = SystemTime::now();
    let tokens = MemoryTokens::default();
    let t = token_for(&tokens, &["spiffe://c1/node/1"], now);
    let exposed = JoinToken::parse(&t.expose()).unwrap();
    assert!(tokens.redeem(&exposed, now).is_ok());
    assert_eq!(tokens.redeem(&exposed, now), Err(TokenError::Spent));
    let late = token_for(&tokens, &["spiffe://c1/node/2"], now);
    assert_eq!(
        tokens.redeem(&late, now + Duration::from_secs(3601)),
        Err(TokenError::Expired)
    );
    let unknown = JoinToken::generate().unwrap();
    assert_eq!(tokens.redeem(&unknown, now), Err(TokenError::Unknown));
    // The right id with the wrong secret is unknown too.
    let guessed = JoinToken::parse(&format!(
        "loamsjt1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        late.id
    ))
    .unwrap();
    assert_eq!(tokens.redeem(&guessed, now), Err(TokenError::Unknown));
    for bad in [
        "",
        "loamsjt1",
        "loamsjt2.a.b",
        "loamsjt1.a.notbase64!",
        "loamsjt1.a.AAAA",
    ] {
        assert!(JoinToken::parse(bad).is_err(), "{bad:?}");
    }
    assert!(
        !format!("{t:?}").contains(&t.expose()),
        "Debug leaks the secret"
    );
}

#[test]
fn the_ca_issues_only_what_the_grant_allows() {
    let now = SystemTime::now();
    let ca = open_ca(now);
    let grant = Grant {
        ids: vec![id("spiffe://c1/svc/meter-agent")],
    };
    let (_, csr) = new_key_and_csr(&[id("spiffe://c1/role/gateway")], &[]).unwrap();
    assert!(matches!(
        ca.issue(&csr, &grant, 1, now),
        Err(CaError::NotGranted(_))
    ));
    let (_, csr) = new_key_and_csr(&[id("spiffe://other/svc/meter-agent")], &[]).unwrap();
    assert!(matches!(
        ca.issue(&csr, &grant, 1, now),
        Err(CaError::WrongCluster(_))
    ));
    let (_, csr) = new_key_and_csr(&[], &["localhost".into()]).unwrap();
    assert!(matches!(
        ca.issue(&csr, &grant, 1, now),
        Err(CaError::NoIdentity)
    ));
    assert!(matches!(
        ca.issue(b"not a csr", &grant, 1, now),
        Err(CaError::Csr(_))
    ));
}

#[test]
fn a_csr_asking_to_be_a_ca_gets_a_plain_leaf() {
    let now = SystemTime::now();
    let ca = open_ca(now);
    let key = rcgen::KeyPair::generate().unwrap();
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.subject_alt_names.push(rcgen::SanType::URI(
        "spiffe://c1/svc/meter-agent".try_into().unwrap(),
    ));
    let csr = p.serialize_request(&key).unwrap();
    let grant = Grant {
        ids: vec![SpiffeId::new("c1", Kind::Svc, "meter-agent").unwrap()],
    };
    let issued = ca.issue(csr.der(), &grant, 7, now).unwrap();
    let (_, leaf) = x509_parser::parse_x509_certificate(issued.chain[0].as_ref()).unwrap();
    let bc = leaf.basic_constraints().unwrap().map(|b| b.value.ca);
    assert_ne!(bc, Some(true), "the CSR's CA request was copied");
    assert_eq!(issued.chain.len(), 2, "leaf and intermediate");
}

#[test]
fn the_deny_list_refuses_by_serial_and_by_identity() {
    let now = SystemTime::now();
    let ca = open_ca(now);
    let tokens = MemoryTokens::default();
    let t1 = token_for(&tokens, &["spiffe://c1/node/1"], now);
    let t2 = token_for(&tokens, &["spiffe://c1/node/2"], now);
    let n1 = join(&ca, &tokens, &t1, &[id("spiffe://c1/node/1")], 11, now).unwrap();
    let n2 = join(&ca, &tokens, &t2, &[id("spiffe://c1/node/2")], 12, now).unwrap();
    let deny = DenyList::default();
    assert!(!deny.denies(n1.serial(), n1.ids()));
    deny.revoke_serial(n1.serial());
    assert!(deny.denies(n1.serial(), n1.ids()));
    assert!(!deny.denies(n2.serial(), n2.ids()));
    deny.revoke_identity(id("spiffe://c1/node/2"));
    assert!(deny.denies(n2.serial(), n2.ids()));
}

#[test]
fn material_and_identities_do_not_print_keys() {
    let now = SystemTime::now();
    let material = Ca::init("c1", now).unwrap();
    let shown = format!("{material:?}");
    assert!(!shown.contains("PRIVATE KEY"));
    let ca = Ca::load("c1", &material).unwrap();
    let tokens = MemoryTokens::default();
    let t = token_for(&tokens, &["spiffe://c1/node/1"], now);
    let ident = join(&ca, &tokens, &t, &[id("spiffe://c1/node/1")], 1, now).unwrap();
    assert!(format!("{ident:?}").contains("<redacted>"));
}

#[test]
fn identities_load_from_pem_files() {
    let now = SystemTime::now();
    let material = Ca::init("c1", now).unwrap();
    let ca = Ca::load("c1", &material).unwrap();
    let grant = Grant {
        ids: vec![id("spiffe://c1/svc/agentd")],
    };
    let key = rcgen::KeyPair::generate().unwrap();
    let mut p = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.subject_alt_names.push(rcgen::SanType::URI(
        "spiffe://c1/svc/agentd".try_into().unwrap(),
    ));
    let csr = p.serialize_request(&key).unwrap();
    let issued = ca.issue(csr.der(), &grant, 3, now).unwrap();
    // A file layout like cert-manager's Secret (tls.crt, tls.key, ca.crt).
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("loams-transport-pem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pem = |der: &[u8], label: &str| {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    };
    let chain: String = issued.chain.iter().map(|c| pem(c, "CERTIFICATE")).collect();
    std::fs::write(dir.join("tls.crt"), chain).unwrap();
    std::fs::write(dir.join("tls.key"), key.serialize_pem()).unwrap();
    std::fs::write(dir.join("ca.crt"), &material.root_cert_pem).unwrap();
    let ident = Identity::from_pem_files(
        &dir.join("tls.crt"),
        &dir.join("tls.key"),
        &dir.join("ca.crt"),
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(ident.ids().has("svc/agentd"));
    assert_eq!(ident.chain().len(), 2);
    assert_eq!(ident.bundle().len(), 1);
}
