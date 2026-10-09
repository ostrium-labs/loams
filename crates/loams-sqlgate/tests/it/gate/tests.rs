use std::net::IpAddr;
use std::time::{Duration, Instant};

use loams_sqlgate::limits::LimitsConfig;
use loams_sqlgate::server::PlaintextPolicy;
use tokio::io::AsyncReadExt;

use super::{Options, harness};

fn code(e: &loams_sqlgate::codec::command::ErrPacket) -> u16 {
    e.code
}

#[tokio::test]
async fn caching_sha2_full_auth_over_tls_succeeds() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("full auth over TLS");
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    let s = h.gate.stats();
    assert_eq!((s.full_auths, s.fast_hits), (1, 0));
    // The cache now holds u_a: the next login is a fast-auth hit, over TLS
    // or over plaintext loopback.
    let mut c2 = h.tls("u_a", b"pa").await.expect("fast auth");
    assert_eq!(c2.query_info("SELECT 2").await, "tidb-a");
    let mut c3 = h.plain("u_a", b"pa").await.expect("fast auth on loopback");
    assert_eq!(c3.query_info("SELECT 3").await, "tidb-a");
    let s = h.gate.stats();
    assert_eq!((s.full_auths, s.fast_hits), (1, 2));
    // The gate logged in upstream as the role's internal user.
    assert!(
        h.a.seen
            .lock()
            .unwrap()
            .logins
            .iter()
            .all(|u| u == "ri_writer")
    );
}

#[tokio::test]
async fn wrong_password_is_1045() {
    let h = harness(Options::default()).await;
    let e = h.tls("u_a", b"nope").await.err().expect("refused");
    assert_eq!((code(&e), &e.sql_state), (1045, b"28000"));
    let e = h.tls("u_nobody", b"x").await.err().expect("refused");
    assert_eq!(code(&e), 1045, "unknown users look the same");
    // A cached user with a wrong password: fast miss, full check, 1045.
    h.tls("u_a", b"pa").await.expect("ok");
    let e = h.tls("u_a", b"nope").await.err().expect("refused");
    assert_eq!(code(&e), 1045);
}

#[tokio::test]
async fn plaintext_refused_off_loopback() {
    for (ip, allowed) in [
        ("127.0.0.1", true),
        ("127.8.9.10", true),
        ("::1", true),
        ("::ffff:127.0.0.1", true),
        ("10.0.0.1", false),
        ("::ffff:10.0.0.1", false),
        ("fd00::1", false),
    ] {
        let ip: IpAddr = ip.parse().expect("ip");
        assert_eq!(PlaintextPolicy::LoopbackOnly.allows(ip), allowed, "{ip}");
        assert!(!PlaintextPolicy::Never.allows(ip));
    }
    // A peer the policy treats as off-loopback: plaintext gets 3159.
    let h = harness(Options {
        plaintext: PlaintextPolicy::Never,
        ..Options::default()
    })
    .await;
    let e = h.plain("u_a", b"pa").await.err().expect("refused");
    assert_eq!(code(&e), 3159);
    h.tls("u_a", b"pa").await.expect("TLS still works");
}

#[tokio::test]
async fn change_user_is_refused() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    let err = c.command(&[0x11, b'u', 0]).await.expect("reply");
    let e = loams_sqlgate::codec::command::ErrPacket::decode(&err).expect("err");
    assert_eq!(e.code, 1235);
    // The connection stays usable, and TiDB never saw the command.
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    assert!(!h.a.seen.lock().unwrap().commands.contains(&0x11));
}

#[tokio::test]
async fn binlog_dump_is_refused() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    for cmd in [0x12u8, 0x1e, 0x15, 0x08, 0x0d] {
        let err = c.command(&[cmd, 0, 0, 0, 0]).await.expect("reply");
        let e = loams_sqlgate::codec::command::ErrPacket::decode(&err).expect("err");
        assert_eq!(e.code, 1235, "{cmd:#x}");
    }
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    let seen = h.a.seen.lock().unwrap().commands.clone();
    assert_eq!(seen, vec![0x03], "only the query reached TiDB");
}

#[tokio::test]
async fn handshake_deadline_closes_slow_client() {
    let h = harness(Options {
        handshake_timeout: Duration::from_millis(300),
        ..Options::default()
    })
    .await;
    let mut tcp = tokio::net::TcpStream::connect(h.addr)
        .await
        .expect("connect");
    let start = Instant::now();
    let mut buf = vec![0u8; 4096];
    let mut total = 0;
    // The greeting arrives; then nothing is sent and the gate closes.
    loop {
        match tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => total += n,
            Ok(Err(_)) => break,
            Err(_) => panic!("the gate did not close a silent client"),
        }
    }
    assert!(total > 0, "greeting");
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn connection_cap_returns_1040() {
    let h = harness(Options {
        limits: LimitsConfig {
            max_connections_per_db: 1,
            ..LimitsConfig::default()
        },
        ..Options::default()
    })
    .await;
    let c1 = h.tls("u_a", b"pa").await.expect("first");
    let e = h.tls("u_a", b"pa").await.err().expect("capped");
    assert_eq!(code(&e), 1040);
    // Another database has its own cap.
    h.tls("u_b", b"pb").await.expect("other db");
    drop(c1);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match h.tls("u_a", b"pa").await {
            Ok(_) => break,
            Err(e) if e.code == 1040 && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("slot not released: {e:?}"),
        }
    }
}

#[tokio::test]
async fn connection_rate_returns_1040() {
    let h = harness(Options {
        // No refill: logins take long in debug builds (Argon2id), so a burst
        // of 2 with no refill makes the third refusal deterministic.
        limits: LimitsConfig {
            connect_rate_per_sec: 0,
            connect_burst: 2,
            ..LimitsConfig::default()
        },
        ..Options::default()
    })
    .await;
    let _c1 = h.tls("u_a", b"pa").await.expect("1");
    let _c2 = h.tls("u_a", b"pa").await.expect("2");
    let e = h.tls("u_a", b"pa").await.err().expect("rate");
    assert_eq!(code(&e), 1040);
}

#[tokio::test]
async fn ping_is_not_activity() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    for _ in 0..3 {
        assert_eq!(c.command(&[0x0e]).await.expect("pong")[0], 0x00);
    }
    c.query_info("SELECT 1").await;
    c.query_info("SELECT 2").await;
    assert_eq!(h.activity.count("br_a"), 2, "pings are not activity");
    assert_eq!(h.activity.count("br_b"), 0);
}

#[tokio::test]
async fn gate_errors_are_redacted() {
    let h = harness(Options::default()).await;
    let e = h.tls("u_a", b"nope").await.err().expect("refused");
    assert_eq!(e.message, "Access denied for user 'u_a'");
    // No pool: a generic, retryable 1040 naming nothing internal.
    let e = h.tls("u_nopool", b"px").await.err().expect("unavailable");
    assert_eq!(e.code, 1040);
    for leak in ["br_", "ri_", "127.0.0.1", "argon2", "tidb"] {
        assert!(!e.message.contains(leak), "{leak} in {:?}", e.message);
    }
    // Secrets never print.
    let user = loams_sqlgate::auth::ResolvedUser {
        branch: "br_a".into(),
        role: loams_sqlgate::auth::Role::Writer,
        password_hash: loams_sqlgate::auth::hash_password(b"pa"),
    };
    let shown = format!("{user:?}");
    assert!(
        shown.contains("[redacted]") && !shown.contains("argon2"),
        "{shown}"
    );
}

#[tokio::test]
async fn tidb_sees_client_address_via_proxy_protocol() {
    let h = harness(Options::default()).await;
    let c = h.tls("u_a", b"pa").await.expect("login");
    let seen = h.a.seen.lock().unwrap().proxy_sources.clone();
    assert_eq!(
        seen.last(),
        Some(&c.local),
        "TiDB gets the client's address, not the gate's"
    );
}

#[tokio::test]
async fn upstream_login_refused_without_tls() {
    let h = harness(Options {
        upstream_ssl: false,
        ..Options::default()
    })
    .await;
    let e = h.tls("u_a", b"pa").await.err().expect("no upstream TLS");
    assert_eq!(e.code, 1040);
    let seen = h.a.seen.lock().unwrap().clone();
    assert_eq!(
        seen.plaintext_credentials, 0,
        "no credentials sent in clear"
    );
    assert!(seen.logins.is_empty());
}

/// The upstream leg follows the static profile (R3.6, R3.7). A TiDB whose
/// greeting lacks a flag the gate agreed with the client (here
/// `CLIENT_DEPRECATE_EOF`, which changes result framing) is refused before
/// any login: relaying across mismatched framing would corrupt results.
#[tokio::test]
async fn upstream_profile_drift_is_refused() {
    use loams_sqlgate::codec::handshake::Capabilities as C;
    let h = harness(Options {
        upstream_drop: C::DEPRECATE_EOF,
        ..Options::default()
    })
    .await;
    let e = h.tls("u_a", b"pa").await.err().expect("profile drift");
    assert_eq!(e.code, 1040);
    let seen = h.a.seen.lock().unwrap().clone();
    assert!(seen.logins.is_empty(), "no login on a drifted TiDB");
    assert_eq!(seen.plaintext_credentials, 0);
}

#[tokio::test]
async fn users_are_routed_to_their_own_branch() {
    let h = harness(Options::default()).await;
    let mut a = h.tls("u_a", b"pa").await.expect("a");
    let mut b = h.tls("u_b", b"pb").await.expect("b");
    assert_eq!(a.query_info("SELECT 1").await, "tidb-a");
    assert_eq!(b.query_info("SELECT 1").await, "tidb-b");
    assert!(
        h.b.seen
            .lock()
            .unwrap()
            .logins
            .iter()
            .all(|u| u == "ri_writer")
    );
    assert_eq!(
        h.a.seen.lock().unwrap().logins.len(),
        1,
        "B's client never reached A"
    );
    // u_a's password does not open u_b.
    assert_eq!(h.tls("u_b", b"pa").await.err().expect("refused").code, 1045);
}

#[tokio::test]
async fn database_and_sni_pass_through() {
    let h = harness(Options::default()).await;
    let c = super::client::connect(
        h.addr,
        "u_a",
        b"pa",
        Some((h.pki.client_config(), "db-a.sql.test")),
        Some("app"),
    )
    .await
    .expect("SNI name and database");
    drop(c);
    let dbs = h.a.seen.lock().unwrap().databases.clone();
    assert_eq!(dbs.last(), Some(&Some("app".to_owned())));
}

#[tokio::test]
async fn pem_files_configure_tls() {
    use loams_sqlgate::server::{sni_cert_from_pem, upstream_tls_from_ca_pem};
    let pki = super::pki::Pki::new();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqlgate-pem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("ca.pem"), &pki.ca_pem).expect("ca");
    std::fs::write(dir.join("cert.pem"), &pki.upstream_pem.0).expect("cert");
    std::fs::write(dir.join("key.pem"), &pki.upstream_pem.1).expect("key");
    let cert = sni_cert_from_pem(
        vec!["x".into()],
        &dir.join("cert.pem"),
        &dir.join("key.pem"),
    )
    .expect("pem");
    assert_eq!(cert.chain.len(), 1);
    assert!(format!("{cert:?}").contains("[redacted]"));
    upstream_tls_from_ca_pem(&dir.join("ca.pem")).expect("ca");
    assert!(upstream_tls_from_ca_pem(&dir.join("missing.pem")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// R3.12: the gate zeroes its own socket buffers. The client handshake
/// buffer keeps only unconsumed bytes; consumed bytes (a cleartext
/// password among them) and grown-out storage are zeroed.
#[test]
fn handshake_buffer_zeroes_consumed_bytes() {
    use loams_sqlgate::wire::SecretBuf;
    let mut b = SecretBuf::with_capacity(8);
    b.extend_from_slice(b"secret!!");
    b.consume(6);
    assert_eq!(b.as_slice(), b"!!");
    assert_eq!(b.storage(), b"\0\0\0\0\0\0!!", "consumed bytes are zeroed");
    // Reading more compacts, then grows: no stale copy is left behind.
    let tail = b.read_space(10);
    tail.copy_from_slice(b"password\0x");
    b.advance(10);
    assert_eq!(b.as_slice(), b"!!password\0x");
    assert!(b.storage().len() >= 12);
    assert!(b.storage()[12..].iter().all(|&x| x == 0));
    b.consume(12);
    assert!(
        b.storage().iter().all(|&x| x == 0),
        "all consumed: all zero"
    );
    // Short reads: only `advance`d bytes count.
    let space = b.read_space(4);
    space[..2].copy_from_slice(b"ab");
    b.advance(2);
    assert_eq!(b.as_slice(), b"ab");
    let taken = b.take();
    assert_eq!(taken, b"ab");
    assert!(b.as_slice().is_empty());
    assert!(b.storage().iter().all(|&x| x == 0), "take zeroes");
}

/// C2: accept errors (here EMFILE, out of file descriptors) are transient:
/// the gate logs, backs off and keeps serving.
#[tokio::test]
async fn accept_errors_do_not_stop_the_gate() {
    let h = harness(Options {
        accept_failures: 5,
        ..Options::default()
    })
    .await;
    let mut c = h.tls("u_a", b"pa").await.expect("served after EMFILE");
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
}

/// C2: the gate-wide connection cap and the per-IP handshake cap answer
/// 1040 in place of the greeting, and free their places when a connection
/// ends.
#[tokio::test]
async fn connection_caps_hold() {
    use super::client::{connect_from, tcp_from};
    use loams_sqlgate::limits::PreAuthConfig;
    let h = harness(Options {
        max_connections: 2,
        pre_auth: PreAuthConfig {
            per_ip_concurrent: 1,
            ..PreAuthConfig::default()
        },
        ..Options::default()
    })
    .await;
    let tls = || Some((h.pki.client_config(), "localhost"));
    let from = |ip: &str| Some(ip.parse::<IpAddr>().unwrap());

    // 127.0.0.1 holds a connection in its handshake: a second one from the
    // same IP is refused, another IP is not.
    let mut stalled = tcp_from(from("127.0.0.1"), h.addr).await;
    let mut greeting = [0u8; 4];
    stalled.read_exact(&mut greeting).await.expect("greeting");
    let e = connect_from(from("127.0.0.1"), h.addr, "u_a", b"pa", tls(), None)
        .await
        .err()
        .expect("per-IP handshake cap");
    assert_eq!(e.code, 1040);
    let other = connect_from(from("127.0.0.2"), h.addr, "u_a", b"pa", tls(), None)
        .await
        .expect("another IP");

    // Two connections open (stalled and other): the gate-wide cap of 2.
    let e = connect_from(from("127.0.0.3"), h.addr, "u_a", b"pa", tls(), None)
        .await
        .err()
        .expect("gate-wide cap");
    assert_eq!(e.code, 1040);

    // Places come back when connections end.
    drop(stalled);
    drop(other);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match connect_from(from("127.0.0.1"), h.addr, "u_a", b"pa", tls(), None).await {
            Ok(_) => break,
            Err(e) if e.code == 1040 && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("caps not released: {e:?}"),
        }
    }
}
