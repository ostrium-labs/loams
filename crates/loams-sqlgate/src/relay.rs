//! The gate's relay between a logged-in client and its TiDB connection
//! (plan SQ1 Task 4, fix round 1: I2, M1, M2, M3, M7).
//!
//! - Both directions run in one task under `select!`: when either ends
//!   (EOF, an error, a protocol violation, the idle timeout or the gate's
//!   shutdown), both connections close and the database slot is freed.
//! - Client → TiDB is read frame by frame. A command must start at
//!   sequence id 0; continuation frames of a message over `MAX_FRAME` are
//!   tracked and must follow in order. A command outside
//!   [`crate::codec::command::RELAYED`], a refused one, and a `KILL`
//!   statement are answered by the gate and never forwarded; the bytes of
//!   a discarded command are zeroed.
//! - TiDB → client is copied packet by packet, so a gate ERR always lands
//!   on a packet boundary.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use crate::codec::command::{Command, ErrPacket, KILL_SCAN, classify, starts_with_kill};
use crate::codec::packet::{HEADER_LEN, MAX_FRAME, encode};
use crate::limits::{ActivitySink, Slot};
use crate::upstream::Upstream;
use crate::wire::{ClientStream, zero};

/// `COM_QUERY` and `COM_STMT_PREPARE`: checked for `KILL`.
const QUERY: u8 = 0x03;
const PREPARE: u8 = 0x16;

/// What one relay needs besides its two connections.
pub(crate) struct Relay {
    pub branch: String,
    pub activity: Arc<dyn ActivitySink>,
    pub slot: Slot,
    pub idle_timeout: Duration,
    pub shutdown: watch::Receiver<bool>,
}

/// The last time a byte moved, in milliseconds since `base`.
struct Clock {
    base: Instant,
    last: AtomicU64,
}

impl Clock {
    fn touch(&self) {
        let ms = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last.store(ms, Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        self.base
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last.load(Ordering::Relaxed)))
    }
}

/// Relays until either side ends, then closes both.
pub(crate) async fn relay(client: ClientStream, upstream: Upstream, mut r: Relay) {
    let (cr, cw) = tokio::io::split(client);
    let (ur, uw) = tokio::io::split(upstream);
    let cw = Mutex::new(cw);
    let clock = Clock {
        base: Instant::now(),
        last: AtomicU64::new(0),
    };
    let idle = async {
        loop {
            let left = r.idle_timeout.saturating_sub(clock.idle_for());
            if left.is_zero() {
                return;
            }
            tokio::time::sleep(left).await;
        }
    };
    let stop = async {
        while !*r.shutdown.borrow_and_update() {
            if r.shutdown.changed().await.is_err() {
                // The gate is gone: nothing will ask us to stop.
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        _ = client_to_upstream(cr, uw, &cw, &r.branch, &*r.activity, &clock) => {}
        _ = upstream_to_client(ur, &cw, &clock) => {}
        () = idle => tracing::debug!("idle timeout"),
        () = stop => {}
    }
    // Close the client cleanly (TLS close_notify) without waiting long.
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        cw.lock().await.shutdown().await
    })
    .await;
    drop(r.slot);
}

fn frame_len(header: &[u8; HEADER_LEN]) -> usize {
    usize::from(header[0]) | usize::from(header[1]) << 8 | usize::from(header[2]) << 16
}

/// TiDB → client, one whole packet per lock of the client's writer.
async fn upstream_to_client(
    mut ur: ReadHalf<Upstream>,
    cw: &Mutex<WriteHalf<ClientStream>>,
    clock: &Clock,
) -> io::Result<()> {
    let mut header = [0u8; HEADER_LEN];
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        ur.read_exact(&mut header).await?;
        clock.touch();
        let mut remaining = frame_len(&header);
        let mut w = cw.lock().await;
        w.write_all(&header).await?;
        while remaining > 0 {
            let n = remaining.min(buf.len());
            ur.read_exact(&mut buf[..n]).await?;
            w.write_all(&buf[..n]).await?;
            remaining -= n;
        }
        w.flush().await?;
        drop(w);
        clock.touch();
    }
}

/// A protocol violation by the client: the connection is closed.
fn violation(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// Reads and drops `n` bytes, zeroing them (they may hold credentials,
/// e.g. a refused `COM_CHANGE_USER`).
#[doc(hidden)]
pub async fn discard<R: AsyncRead + Unpin>(
    cr: &mut R,
    mut n: usize,
    buf: &mut [u8],
) -> io::Result<()> {
    while n > 0 {
        let k = n.min(buf.len());
        let read = cr.read_exact(&mut buf[..k]).await;
        zero(&mut buf[..k]);
        read?;
        n -= k;
    }
    Ok(())
}

/// The state of the client's current message.
enum Message {
    /// Between commands: the next frame starts one (sequence id 0).
    Start,
    /// A message over `MAX_FRAME` continues with frame `next`; `refused`
    /// holds the gate's answer when the message is being dropped.
    Continues {
        next: u8,
        refused: Option<ErrPacket>,
    },
}

async fn client_to_upstream(
    mut cr: ReadHalf<ClientStream>,
    mut uw: WriteHalf<Upstream>,
    cw: &Mutex<WriteHalf<ClientStream>>,
    branch: &str,
    activity: &dyn ActivitySink,
    clock: &Clock,
) -> io::Result<()> {
    let mut header = [0u8; HEADER_LEN];
    let mut buf = vec![0u8; 64 * 1024];
    let mut state = Message::Start;
    loop {
        cr.read_exact(&mut header).await?;
        clock.touch();
        let len = frame_len(&header);
        let seq = header[3];
        let more = len == MAX_FRAME;
        let (refused, first, peeked) = match state {
            Message::Continues { next, refused } => {
                if seq != next {
                    return Err(violation("out-of-order continuation frame"));
                }
                (refused, None, 0)
            }
            Message::Start => {
                if seq != 0 {
                    return Err(violation("a command must start at sequence id 0"));
                }
                if len == 0 {
                    return Err(violation("empty command"));
                }
                // The first byte, and for statements the first KILL_SCAN
                // bytes, decide whether the command is relayed.
                cr.read_exact(&mut buf[..1]).await?;
                let peek = if matches!(buf[0], QUERY | PREPARE) {
                    let n = (len - 1).min(KILL_SCAN);
                    cr.read_exact(&mut buf[1..=n]).await?;
                    1 + n
                } else {
                    1
                };
                let command = classify(&buf[..1]);
                let mut refused = command.and_then(Command::refusal);
                if refused.is_none()
                    && matches!(buf[0], QUERY | PREPARE)
                    && starts_with_kill(&buf[1..peek], !more && peek == len)
                {
                    refused = Some(ErrPacket::new(
                        1235,
                        *b"42000",
                        "Loams SQL does not support KILL",
                    ));
                }
                if refused.is_none() && command.is_some_and(Command::is_activity) {
                    activity.command(branch);
                }
                (refused, command, peek)
            }
        };
        let remaining = len - peeked;
        match refused {
            Some(err) => {
                zero(&mut buf[..peeked]);
                discard(&mut cr, remaining, &mut buf).await?;
                if more {
                    state = Message::Continues {
                        next: seq.wrapping_add(1),
                        refused: Some(err),
                    };
                    continue;
                }
                let mut out = Vec::new();
                let mut s = seq.wrapping_add(1);
                encode(&err.encode(), &mut s, &mut out);
                let mut w = cw.lock().await;
                w.write_all(&out).await?;
                w.flush().await?;
                state = Message::Start;
            }
            None => {
                uw.write_all(&header).await?;
                uw.write_all(&buf[..peeked]).await?;
                let mut left = remaining;
                while left > 0 {
                    let n = left.min(buf.len());
                    cr.read_exact(&mut buf[..n]).await?;
                    uw.write_all(&buf[..n]).await?;
                    left -= n;
                }
                uw.flush().await?;
                state = if more {
                    Message::Continues {
                        next: seq.wrapping_add(1),
                        refused: None,
                    }
                } else {
                    Message::Start
                };
                if first == Some(Command::Quit) && !more {
                    return Ok(());
                }
            }
        }
    }
}
