//! Choosing the transport (§83 §8, D2317; §85 §10, D2435): race QUIC
//! against TCP+TLS, prefer QUIC, and remember for 15 minutes per server
//! that UDP failed.

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::Uri;
use serde::{Deserialize, Serialize};

use super::h2::H2Client;
use super::h3::H3Client;
use super::{Conn, Trust};

/// The transport policy for one server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// HTTP/3, falling back to HTTP/2+TLS.
    #[default]
    Auto,
    /// HTTP/3 only.
    H3,
    /// HTTP/2+TLS only.
    H2,
}

/// How to reach one server.
#[derive(Clone, Debug)]
pub struct ConnectOpts {
    /// `https://host[:port]`, or `http://` for a loopback engine.
    pub base: String,
    pub trust: Trust,
    pub mode: Mode,
    /// Where the UDP-failure memory lives (`None`: not remembered).
    pub memory: Option<PathBuf>,
    /// When TCP starts if QUIC has not connected (D2317: 250 ms).
    pub h2_delay: Duration,
    /// How long QUIC gets before it counts as failed.
    pub h3_timeout: Duration,
    /// How long a UDP failure is remembered (D2317: 15 minutes).
    pub remember: Duration,
}

impl ConnectOpts {
    pub fn new(base: impl Into<String>, trust: Trust) -> Self {
        ConnectOpts {
            base: base.into(),
            trust,
            mode: Mode::Auto,
            memory: None,
            h2_delay: Duration::from_millis(250),
            h3_timeout: Duration::from_secs(3),
            remember: Duration::from_secs(15 * 60),
        }
    }
}

/// The parts of a base URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub tls: bool,
    pub host: String,
    pub port: u16,
}

impl Target {
    /// The `scheme://authority` prefix for request URIs.
    pub fn origin(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// Parses a base URL. Plain `http://` is refused unless the host is
/// loopback (D2437).
pub fn parse_base(base: &str) -> io::Result<Target> {
    let uri: Uri = base.parse().map_err(|e| bad(format!("server URL: {e}")))?;
    let tls = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(bad("server URL must be https:// (or http:// to loopback)")),
    };
    let host = uri
        .host()
        .ok_or_else(|| bad("server URL has no host"))?
        .to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    if !tls && !is_loopback(&host) {
        return Err(bad(
            "plain http:// is only allowed to a loopback engine; use https://",
        ));
    }
    Ok(Target { tls, host, port })
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

async fn resolve(t: &Target) -> io::Result<SocketAddr> {
    let h = t.host.trim_start_matches('[').trim_end_matches(']');
    // IPv4 first: a dev or self-hosted server commonly binds 127.0.0.1 or a
    // v4 tailnet address while `localhost` also resolves to ::1.
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((h, t.port)).await?.collect();
    addrs.sort_by_key(|a| a.is_ipv6());
    addrs
        .into_iter()
        .next()
        .ok_or_else(|| bad(format!("{} did not resolve", t.host)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The per-server memory of UDP failures: `host:port` → until (ms).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Memory {
    udp_failed_until: BTreeMap<String, u64>,
}

fn load(path: &Option<PathBuf>) -> Memory {
    path.as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store(path: &Option<PathBuf>, m: &Memory) {
    let Some(p) = path else { return };
    let Ok(data) = serde_json::to_vec_pretty(m) else {
        return;
    };
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}

/// Whether UDP to this server is remembered as failed.
pub fn udp_remembered_failed(opts: &ConnectOpts) -> bool {
    let Ok(t) = parse_base(&opts.base) else {
        return false;
    };
    let key = format!("{}:{}", t.host, t.port);
    load(&opts.memory)
        .udp_failed_until
        .get(&key)
        .is_some_and(|until| *until > now_ms())
}

fn remember_udp(opts: &ConnectOpts, t: &Target, failed: bool) {
    if opts.memory.is_none() {
        return;
    }
    let key = format!("{}:{}", t.host, t.port);
    let mut m = load(&opts.memory);
    let now = now_ms();
    m.udp_failed_until.retain(|_, until| *until > now);
    if failed {
        let ms = u64::try_from(opts.remember.as_millis()).unwrap_or(u64::MAX);
        m.udp_failed_until.insert(key, now.saturating_add(ms));
    } else {
        m.udp_failed_until.remove(&key);
    }
    store(&opts.memory, &m);
}

async fn h3_owned(
    addr: SocketAddr,
    host: String,
    trust: Trust,
    limit: Duration,
) -> io::Result<H3Client> {
    match tokio::time::timeout(limit, H3Client::connect(addr, &host, &trust)).await {
        Ok(r) => r,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "QUIC handshake timed out",
        )),
    }
}

/// Connects to the server by the policy in `opts`.
pub async fn connect(opts: &ConnectOpts) -> io::Result<Conn> {
    let t = parse_base(&opts.base)?;
    let addr = resolve(&t).await?;
    if !t.tls {
        return Ok(Conn::H2(H2Client::connect_plain(addr).await?));
    }
    let h3 = || {
        Box::pin(h3_owned(
            addr,
            t.host.clone(),
            opts.trust.clone(),
            opts.h3_timeout,
        ))
    };
    let h2 = || H2Client::connect_tls(addr, &t.host, &opts.trust);
    match opts.mode {
        Mode::H3 => return Ok(Conn::H3(h3().await?)),
        Mode::H2 => return Ok(Conn::H2(h2().await?)),
        Mode::Auto => {}
    }
    if udp_remembered_failed(opts) {
        return Ok(Conn::H2(h2().await?));
    }
    let mut quic = h3();
    // QUIC alone for the head start.
    let early = tokio::select! {
        r = &mut quic => Some(r),
        () = tokio::time::sleep(opts.h2_delay) => None,
    };
    match early {
        Some(Ok(c)) => {
            remember_udp(opts, &t, false);
            return Ok(Conn::H3(c));
        }
        Some(Err(e)) => {
            tracing::info!(error = %e, server = %t.origin(), "QUIC failed; using HTTP/2+TLS");
            remember_udp(opts, &t, true);
            return Ok(Conn::H2(h2().await?));
        }
        None => {}
    }
    // Then both; QUIC wins if it lands first or TCP fails.
    let tcp = h2();
    tokio::pin!(tcp);
    tokio::select! {
        r = &mut quic => match r {
            Ok(c) => {
                remember_udp(opts, &t, false);
                Ok(Conn::H3(c))
            }
            Err(e) => {
                tracing::info!(error = %e, server = %t.origin(), "QUIC failed; using HTTP/2+TLS");
                remember_udp(opts, &t, true);
                Ok(Conn::H2(tcp.await?))
            }
        },
        r = &mut tcp => match r {
            Ok(c) => {
                // TCP landed first. Keep it, and let QUIC finish in the
                // background only to learn whether UDP works.
                let (opts2, t2) = (opts.clone(), t.clone());
                tokio::spawn(async move {
                    let failed = quic.await.is_err();
                    remember_udp(&opts2, &t2, failed);
                });
                Ok(Conn::H2(c))
            }
            Err(e2) => match quic.await {
                Ok(c) => Ok(Conn::H3(c)),
                Err(e3) => Err(io::Error::other(format!("QUIC: {e3}; TCP+TLS: {e2}"))),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_only_to_loopback() {
        assert!(parse_base("http://127.0.0.1:7700").is_ok());
        assert!(parse_base("http://localhost:7700").is_ok());
        assert!(parse_base("http://[::1]:7700").is_ok());
        assert!(parse_base("http://10.0.0.5:7700").is_err());
        assert!(parse_base("http://example.com").is_err());
        assert!(parse_base("ftp://example.com").is_err());
        let t = parse_base("https://example.com").expect("https");
        assert_eq!((t.port, t.tls), (443, true));
    }
}
