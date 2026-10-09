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
    assert_eq!(h.gate.stats().profile_mismatches, 1, "counted (M5)");
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

/// C1: an unknown-user flood never runs more Argon2id checks at once than
/// the verification semaphore allows, and legitimate logins still succeed
/// under it (a cached user at once; a full check after the flood).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_user_flood_is_bounded() {
    let h = harness(Options {
        verify_concurrency: 2,
        handshake_timeout: Duration::from_secs(60),
        ..Options::default()
    })
    .await;
    h.tls("u_a", b"pa").await.expect("warm the cache");
    let flood: Vec<_> = (0..12)
        .map(|i| {
            let (addr, config) = (h.addr, h.pki.client_config());
            tokio::spawn(async move {
                super::client::connect(
                    addr,
                    &format!("nobody{i}"),
                    b"guess",
                    Some((config, "localhost")),
                    None,
                )
                .await
                .err()
                .expect("refused")
                .code
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut cached = h
        .tls("u_a", b"pa")
        .await
        .expect("cached login under the flood");
    assert_eq!(cached.query_info("SELECT 1").await, "tidb-a");
    for f in flood {
        assert!([1045, 1040].contains(&f.await.unwrap()));
    }
    let s = h.gate.stats();
    assert!(
        s.verify_peak >= 1 && s.verify_peak <= 2,
        "peak {}",
        s.verify_peak
    );
    h.tls("u_b", b"pb")
        .await
        .expect("full login after the flood");
}

/// I1: 1040 never tells a known user from an unknown one. The database's
/// rate is charged only after a successful login, and the gate-wide
/// failure bucket is charged alike for wrong passwords and unknown users.
#[tokio::test]
async fn no_user_enumeration_through_1040() {
    let h = harness(Options {
        limits: LimitsConfig {
            max_connections_per_db: 100,
            connect_rate_per_sec: 0,
            connect_burst: 1,
        },
        auth_failure_burst: 2,
        auth_failure_rate_per_sec: 0,
        ..Options::default()
    })
    .await;
    h.tls("u_a", b"pa").await.expect("takes br_a's one token");
    // br_a is out of tokens, but a failed login still says 1045.
    assert_eq!(h.tls("u_a", b"nope").await.err().unwrap().code, 1045);
    assert_eq!(h.tls("u_nobody", b"x").await.err().unwrap().code, 1045);
    // A right password meets the database's rate only after login.
    assert_eq!(h.tls("u_a", b"pa").await.err().unwrap().code, 1040);
    // Two failures spent the gate-wide bucket: every full check now gets
    // the same 1040, known user, unknown user or right password.
    let known = h.tls("u_a", b"nope").await.err().unwrap();
    let unknown = h.tls("u_nobody", b"x").await.err().unwrap();
    let right = h.tls("u_b", b"pb").await.err().unwrap();
    assert_eq!((known.code, unknown.code, right.code), (1040, 1040, 1040));
    assert_eq!(known.message, unknown.message);
    assert_eq!(known.message, right.message);
}

/// Waits until br_a has no open connection (the slot is freed).
async fn slot_freed(h: &super::Harness) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.gate.open_connections("br_a") > 0 {
        assert!(Instant::now() < deadline, "slot still held");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// I2: when TiDB closes, the gate closes the client and frees the slot.
#[tokio::test]
async fn upstream_eof_closes_client_and_frees_slot() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    assert_eq!(h.gate.open_connections("br_a"), 1);
    assert_eq!(c.command(b"\x03BYE").await, None, "client sees EOF");
    slot_freed(&h).await;
}

/// I2: a session with no traffic for the idle timeout is closed.
#[tokio::test]
async fn idle_sessions_are_closed() {
    let h = harness(Options {
        idle_timeout: Duration::from_millis(300),
        ..Options::default()
    })
    .await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(c.command(b"\x03SELECT 2").await, None, "closed when idle");
    slot_freed(&h).await;
}

/// M1: a gate ERR never lands inside a TiDB packet. TiDB writes one packet
/// in two halves 200 ms apart; a refused command sent meanwhile is
/// answered only after that packet is whole.
#[tokio::test]
async fn gate_errors_land_on_packet_boundaries() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    c.wire.write(0, b"\x03SLOW").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    c.wire.write(0, b"\x11u_b\0").await;
    let (_, first) = c.wire.read().await.expect("slow reply");
    let ok = loams_sqlgate::codec::command::OkPacket::decode(&first, super::client::CAPS)
        .expect("a whole OK");
    assert!(ok.info.starts_with(b"slow-"));
    let (_, second) = c.wire.read().await.expect("refusal");
    assert_eq!(
        code(&loams_sqlgate::codec::command::ErrPacket::decode(&second).unwrap()),
        1235
    );
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
}

/// M2: a refused command longer than one frame (16 MiB - 1) is dropped
/// whole, continuation frames included, and answered once.
#[tokio::test]
async fn refused_command_over_max_frame_is_dropped_whole() {
    use loams_sqlgate::codec::packet::MAX_FRAME;
    use tokio::io::AsyncWriteExt;
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    let mut payload = vec![0x11];
    payload.resize(MAX_FRAME + 10, b'p');
    let mut frames = Vec::new();
    loams_sqlgate::codec::packet::encode(&payload, &mut 0, &mut frames);
    c.wire.io.write_all(&frames).await.unwrap();
    c.wire.io.flush().await.unwrap();
    let (seq, p) = c.wire.read().await.expect("refusal");
    let e = loams_sqlgate::codec::command::ErrPacket::decode(&p).unwrap();
    assert_eq!((code(&e), seq), (1235, 2), "after frames 0 and 1");
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    assert!(!h.a.seen.lock().unwrap().commands.contains(&0x11));
}

/// M2: a command that does not start at sequence id 0 closes the session.
#[tokio::test]
async fn nonzero_sequence_at_command_start_closes() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    c.wire.write(3, b"\x03SELECT 1").await;
    assert_eq!(c.wire.read().await, None);
    slot_freed(&h).await;
}

/// M2, M3: commands TiDB does not dispatch get 1047 and KILL statements
/// get 1235; neither reaches TiDB.
#[tokio::test]
async fn unlisted_commands_and_kill_are_refused() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    let refused = |p: Vec<u8>| code(&loams_sqlgate::codec::command::ErrPacket::decode(&p).unwrap());
    assert_eq!(refused(c.command(&[0x0c, 5, 0, 0, 0]).await.unwrap()), 1047);
    assert_eq!(refused(c.command(&[0x00]).await.unwrap()), 1047);
    assert_eq!(refused(c.command(b"\x03KILL QUERY 5").await.unwrap()), 1235);
    assert_eq!(
        refused(c.command(b"\x03/*!50000 kill 5 */").await.unwrap()),
        1235
    );
    assert_eq!(refused(c.command(b"\x16KILL 5").await.unwrap()), 1235);
    assert_eq!(c.query_info("SELECT 'KILL'").await, "tidb-a");
    let seen = h.a.seen.lock().unwrap().commands.clone();
    assert_eq!(seen, vec![0x03], "only the SELECT reached TiDB");
}

/// M7: discarded bytes are zeroed.
#[tokio::test]
async fn discarded_bytes_are_zeroed() {
    let mut src: &[u8] = b"secret-password";
    let mut buf = [0u8; 4];
    loams_sqlgate::relay::discard(&mut src, 15, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, [0; 4]);
    assert!(src.is_empty());
}

/// M5: a missing or forbidden database keeps its MySQL code, in the gate's
/// words (no internal user).
#[tokio::test]
async fn upstream_database_errors_keep_their_codes() {
    let h = harness(Options::default()).await;
    for (db, want) in [("missing", 1049), ("forbidden", 1044)] {
        let e = super::client::connect(
            h.addr,
            "u_a",
            b"pa",
            Some((h.pki.client_config(), "localhost")),
            Some(db),
        )
        .await
        .err()
        .expect("refused");
        assert_eq!(e.code, want, "{db}: {e:?}");
        assert!(
            e.message.contains(db) && !e.message.contains("ri_"),
            "{e:?}"
        );
    }
    assert!(slot_free_now(&h));
}

fn slot_free_now(h: &super::Harness) -> bool {
    h.gate.open_connections("br_a") == 0
}

/// M6: a gate takes no plaintext unless configured to (the desktop sets
/// loopback); the policy parses from its flag value.
#[test]
fn gateways_default_to_no_plaintext() {
    use loams_sqlgate::server::GateConfig;
    let pki = super::pki::Pki::new();
    let tls = loams_sqlgate::server::sni_server_config(vec![loams_sqlgate::server::SniCert {
        names: vec!["localhost".into()],
        chain: pki.gate_chain.clone(),
        key: pki.gate_key.clone_key(),
    }])
    .unwrap();
    assert_eq!(
        GateConfig::new(tls, pki.client_config()).plaintext,
        PlaintextPolicy::Never
    );
    assert_eq!("never".parse(), Ok(PlaintextPolicy::Never));
    assert_eq!("loopback".parse(), Ok(PlaintextPolicy::LoopbackOnly));
    assert!("always".parse::<PlaintextPolicy>().is_err());
}
