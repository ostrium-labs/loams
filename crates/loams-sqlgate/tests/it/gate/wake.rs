//! Wake on connect at the gate (plan SQ1 Task 5): `EnsureRunning`'s 1040,
//! sessions closed at a suspend with 1053, and the resume probe.
use std::time::{Duration, Instant};

use loams_sqlgate::codec::command::ErrPacket;
use loams_sqlgate::codec::handshake::TIDB_V8_5_8;
use loams_sqlgate::upstream::{Close, UpstreamError, UpstreamMember, probe};
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use super::{INTERNAL_PW, Options, harness};

#[tokio::test]
async fn resuming_branch_gets_1040_retry() {
    let h = harness(Options::default()).await;
    h.wake
        .resuming
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let Err(e) = h.tls("u_a", b"pa").await else {
        panic!("admitted while resuming");
    };
    assert_eq!(e.code, 1040);
    assert_eq!(e.message, "database is resuming, retry");
    assert_eq!(h.gate.open_connections("br_a"), 0, "no slot kept");
}

async fn expect_1053_then_eof(c: &mut super::client::Client) {
    let (seq, p) = c.wire.read().await.expect("an ERR before the close");
    let e = ErrPacket::decode(&p).expect("ERR");
    assert_eq!((seq, e.code), (0, 1053), "{e:?}");
    assert_eq!(&e.sql_state, b"08S01");
    assert_eq!(c.wire.read().await, None, "then closed");
}

async fn lease_released(h: &super::Harness) {
    for _ in 0..100 {
        if h.wake.live() == 0 && h.gate.open_connections("br_a") == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the session's lease and slot were not released");
}

#[tokio::test]
async fn idle_session_is_closed_with_1053_at_a_suspend() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    assert_eq!(c.query_info("SELECT 1").await, "tidb-a");
    assert_eq!(h.wake.live(), 1);
    let t0 = Instant::now();
    h.wake.close(Close::WhenIdle);
    expect_1053_then_eof(&mut c).await;
    assert!(t0.elapsed() < Duration::from_secs(2));
    lease_released(&h).await;
}

#[tokio::test]
async fn busy_session_finishes_its_command_then_closes() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    c.wire.write(0, b"\x03SLEEP").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.wake.close(Close::WhenIdle);
    let (_, answer) = c.wire.read().await.expect("the query's answer");
    let ok =
        loams_sqlgate::codec::command::OkPacket::decode(&answer, super::client::CAPS).expect("OK");
    assert_eq!(ok.info, b"slept");
    expect_1053_then_eof(&mut c).await;
    lease_released(&h).await;
}

#[tokio::test]
async fn kill_ends_a_busy_session_at_once() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    c.wire.write(0, b"\x03SLEEP").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t0 = Instant::now();
    h.wake.close(Close::Now);
    // Mid-command there is no packet boundary for an ERR: just closed.
    assert_eq!(c.wire.read().await, None);
    assert!(
        t0.elapsed() < Duration::from_millis(800),
        "before TiDB answered"
    );
    lease_released(&h).await;
}

#[tokio::test]
async fn cancelled_close_keeps_the_session() {
    let h = harness(Options::default()).await;
    let mut c = h.tls("u_a", b"pa").await.expect("login");
    c.wire.write(0, b"\x03SLEEP").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.wake.close(Close::WhenIdle);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A connection aborted the suspend.
    h.wake.close(Close::Open);
    let (_, answer) = c.wire.read().await.expect("the query's answer");
    assert_eq!(answer[0], 0x00);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(c.query_info("SELECT 2").await, "tidb-a", "still open");
    assert_eq!(h.wake.live(), 1);
}

#[tokio::test]
async fn probe_logs_in_over_tls_and_selects_1() {
    let h = harness(Options::default()).await;
    let member = UpstreamMember {
        addr: h.a.addr,
        server_name: ServerName::try_from("tidb.test").expect("name"),
    };
    let tls = TlsConnector::from(h.pki.client_config());
    let pw = loams_sqlgate::codec::auth::Password::new(INTERNAL_PW.to_vec());
    probe(&member, &tls, "ri_writer", &pw, TIDB_V8_5_8)
        .await
        .expect("probe");
    {
        let seen = h.a.seen.lock().expect("seen");
        assert_eq!(seen.logins.last().map(String::as_str), Some("ri_writer"));
        assert!(seen.commands.contains(&0x03), "SELECT 1 sent");
    }
    let bad = loams_sqlgate::codec::auth::Password::new(b"wrong".to_vec());
    assert_eq!(
        probe(&member, &tls, "ri_writer", &bad, TIDB_V8_5_8).await,
        Err(UpstreamError::Login(1045))
    );
    let closed = UpstreamMember {
        addr: "127.0.0.1:9".parse().expect("addr"),
        server_name: ServerName::try_from("tidb.test").expect("name"),
    };
    assert_eq!(
        probe(&closed, &tls, "ri_writer", &pw, TIDB_V8_5_8).await,
        Err(UpstreamError::Unavailable)
    );
}
