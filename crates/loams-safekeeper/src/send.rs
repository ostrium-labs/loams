//! The interpreted `START_REPLICATION` the pageserver speaks (§28 §6.7,
//! Q112; PG2 Task 31): committed WAL decoded into records, filtered to the
//! reader's shard, encoded and compressed, in process.
//!
//! The pageserver asks for it with a `protocol` startup option, Neon's
//! `PostgresClientProtocol` as JSON (`utils/src/postgres_client.rs`), and
//! names its shard with `shard_count`, `shard_number` and
//! `shard_stripe_size`. The sender follows the fork's
//! `safekeeper/src/send_interpreted_wal.rs`:
//!
//! - WAL is read up to `commit_lsn` in chunks of at most [`MAX_SEND_SIZE`],
//!   cut at a block boundary unless the chunk reaches the end, and never
//!   across a segment (the fork's `wal_reader_stream.rs`);
//! - each chunk is fed to the connection's decoder, which returns the
//!   encoded batch of records for its shard, or nothing when the chunk
//!   completes no record (the fork's `InterpretedWalReader`);
//! - each batch goes out as one CopyData message: tag `'0'`,
//!   `streaming_lsn` (the chunk's end), `commit_lsn` (the readable end), then
//!   the batch (pq_proto's `BeMessage::InterpretedWalRecords`);
//! - keepalives and the pageserver's `'z'` feedback are as on the vanilla
//!   stream.
//!
//! The decoding itself is Neon's `wal_decoder`, which needs the fork's
//! Postgres headers. It is reached through [`WalInterpreter`], so this crate
//! builds without it: the `loams-wal-decoder` crate implements the trait,
//! and a `loams-wal` without one refuses the protocol (PG2 rulings R31.2,
//! R31.3). One decoder serves one connection, for its own shard (R31.4).
//!
//! Neon's code is Apache-2.0 (see the repository's `NOTICE`); the framing and
//! the option shapes are ported from it.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::info;

use crate::Error;
use crate::pgwire;
use crate::proto::MAX_SEND_SIZE;
use crate::service::{WalService, send};
use crate::store::WalStore;
use crate::types::{Lsn, TimelineId};

/// The CopyData tag of an interpreted batch (pq_proto's
/// `INTERPRETED_WAL_RECORD_TAG`).
pub const INTERPRETED_TAG: u8 = b'0';

/// How the records of a batch are serialized (Neon's `InterpretedFormat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireFormat {
    Bincode,
    Protobuf,
}

/// How a batch is compressed (Neon's `Compression`, externally tagged:
/// `{"zstd":{"level":1}}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Compression {
    Zstd { level: i8 },
}

/// The interpreted protocol's arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct InterpretedProtocol {
    pub format: WireFormat,
    pub compression: Option<Compression>,
}

/// What a replication client asked for (Neon's `PostgresClientProtocol`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", content = "args", rename_all = "kebab-case")]
pub enum ClientProtocol {
    Vanilla,
    Interpreted(InterpretedProtocol),
}

impl ClientProtocol {
    /// The `protocol` startup option; vanilla when it is absent.
    pub fn from_option(value: Option<&str>) -> Result<Self, Error> {
        match value {
            None => Ok(Self::Vanilla),
            Some(v) => serde_json::from_str(v)
                .map_err(|e| Error::Protocol(format!("bad protocol option {v:?}: {e}"))),
        }
    }
}

/// The pageserver shard a reader serves (Neon's `ShardIdentity` without the
/// layout version). `count` 0 is an unsharded tenant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShardSpec {
    pub number: u8,
    pub count: u8,
    /// In pages; 0 when unsharded.
    pub stripe_size: u32,
}

impl ShardSpec {
    pub const UNSHARDED: Self = Self {
        number: 0,
        count: 0,
        stripe_size: 0,
    };

    /// The shard options of an interpreted request, as the fork's
    /// `handler.rs` takes them: `shard_count`, `shard_number` and
    /// `shard_stripe_size` are all required (the pageserver always sends
    /// them; an unsharded tenant has count and number 0).
    pub fn from_options(opts: &HashMap<String, String>) -> Result<Self, Error> {
        let (Some(count), Some(number), Some(stripe_size)) = (
            num::<u8>(opts, "shard_count")?,
            num::<u8>(opts, "shard_number")?,
            num::<u32>(opts, "shard_stripe_size")?,
        ) else {
            return Err(Error::Protocol("Shard params were not specified".into()));
        };
        if count == 0 && number != 0 {
            return Err(Error::Protocol(format!(
                "shard_number {number} of an unsharded tenant"
            )));
        }
        if count > 0 && number >= count {
            return Err(Error::Protocol(format!(
                "shard_number {number} out of shard_count {count}"
            )));
        }
        // The decoder maps keys to shards by stripe: a sharded tenant with
        // stripes of 0 pages would divide by zero.
        if count > 1 && stripe_size == 0 {
            return Err(Error::Protocol(format!(
                "shard_stripe_size 0 for {count} shards"
            )));
        }
        Ok(Self {
            number,
            count,
            stripe_size,
        })
    }

    /// A vanilla request must not name a shard (the fork's `handler.rs`).
    pub fn refuse_on_vanilla(opts: &HashMap<String, String>) -> Result<(), Error> {
        if ["shard_count", "shard_number", "shard_stripe_size"]
            .iter()
            .any(|k| opts.contains_key(*k))
        {
            return Err(Error::Protocol(
                "Shard params specified for vanilla protocol".into(),
            ));
        }
        Ok(())
    }
}

fn num<T: std::str::FromStr>(
    opts: &HashMap<String, String>,
    key: &str,
) -> Result<Option<T>, Error> {
    opts.get(key)
        .map(|v| {
            v.parse::<T>()
                .map_err(|_| Error::Protocol(format!("bad {key} {v:?}")))
        })
        .transpose()
}

/// Neon's `wal_decoder`, behind a trait (implemented by `loams-wal-decoder`).
pub trait WalInterpreter: Send + Sync + 'static {
    /// A decoder for one reader: WAL from `start` (a record boundary, the
    /// pageserver's `last_record_lsn`), of server version `pg_version` (for
    /// example 170005), encoded as `protocol` asks, for `shard` only.
    fn decoder(
        &self,
        start: Lsn,
        pg_version: u32,
        protocol: InterpretedProtocol,
        shard: ShardSpec,
    ) -> Result<Box<dyn ShardDecoder>, Error>;
}

/// One reader's decoder.
#[async_trait::async_trait]
pub trait ShardDecoder: Send {
    /// Feed `wal`, which starts at `start` and follows the previous call's
    /// bytes, and return the encoded batch of the records it completes for
    /// this shard. `None` when there is nothing to send for this chunk: it
    /// completed no record. A batch may hold no records; it still carries
    /// `next_record_lsn`.
    async fn decode(&mut self, start: Lsn, wal: Bytes) -> Result<Option<Bytes>, Error>;
}

/// The interpreter a [`WalService`] serves the protocol with.
#[derive(Clone)]
pub struct Interpreter(pub Arc<dyn WalInterpreter>);

impl std::fmt::Debug for Interpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interpreter")
    }
}

/// Append one interpreted batch as a CopyData message.
pub fn put_interpreted(buf: &mut BytesMut, streaming_lsn: Lsn, commit_lsn: Lsn, batch: &[u8]) {
    let mut msg = BytesMut::with_capacity(17 + batch.len());
    msg.put_u8(INTERPRETED_TAG);
    msg.put_u64(streaming_lsn.0);
    msg.put_u64(commit_lsn.0);
    msg.put_slice(batch);
    pgwire::put_copy_data(buf, &msg);
}

/// Serve an interpreted `START_REPLICATION` from `start_lsn` until the
/// reader hangs up. Errors before `CopyBothResponse` go back to the caller,
/// which reports them on the connection.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream<S, R, W>(
    svc: Arc<WalService<S>>,
    interpreter: &dyn WalInterpreter,
    tl: TimelineId,
    start_lsn: Lsn,
    protocol: InterpretedProtocol,
    shard: ShardSpec,
    rd: R,
    mut wr: W,
) -> Result<(), Error>
where
    S: WalStore,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let head = svc.store().load(&tl).await?.ok_or(Error::NotFound(tl))?;
    if start_lsn < head.trimmed_lsn {
        return Err(Error::Trimmed {
            from: start_lsn,
            trimmed: head.trimmed_lsn,
        });
    }
    let seg_size = match head.server.wal_seg_size {
        0 => DEFAULT_WAL_SEG_SIZE,
        n => u64::from(n),
    };
    let mut decoder = interpreter.decoder(start_lsn, head.server.pg_version, protocol, shard)?;
    let mut buf = BytesMut::new();
    pgwire::put_copy_both(&mut buf);
    send(&mut wr, &mut buf).await?;
    info!(%tl, start = %start_lsn, ?protocol, ?shard, "interpreted replication started");

    let (reader, mut fb) = svc.feedback_reader(tl, rd);
    let mut watch = svc.subscribe(tl);
    let mut keepalive = tokio::time::interval(svc.config().keepalive_interval);
    let mut at = start_lsn;

    let res: Result<(), Error> = async {
        loop {
            watch.borrow_and_update();
            let st = svc.current(tl).await?;
            let end = st.commit_lsn.min(st.flush_lsn);
            if at < end {
                let want = chunk_len(at, end, seg_size);
                let chunk = read_chunk(&svc, tl, at, want).await?;
                if chunk.is_empty() {
                    return Err(Error::Store(format!("no WAL at {at} below {end}")));
                }
                let next = Lsn(at.0 + chunk.len() as u64);
                if let Some(batch) = decoder.decode(at, chunk).await? {
                    put_interpreted(&mut buf, next, end, &batch);
                    send(&mut wr, &mut buf).await?;
                }
                at = next;
                // Feedback is taken during a catch-up too, so that a reader
                // writing feedback is never left blocked.
                if fb.fold() {
                    return Ok(()); // the reader hung up
                }
                fb.persist_if_due(&**svc.store(), tl).await?;
                continue;
            }
            tokio::select! {
                _ = watch.changed() => {}
                _ = tokio::time::sleep(svc.config().poll_interval) => {}
                _ = keepalive.tick() => {
                    let mut msg = BytesMut::with_capacity(18);
                    msg.put_u8(b'k');
                    msg.put_u64(end.0);
                    msg.put_i64(pgwire::pg_now_us());
                    // request_reply, as the fork's interpreted sender sets it.
                    msg.put_u8(1);
                    pgwire::put_copy_data(&mut buf, &msg);
                    send(&mut wr, &mut buf).await?;
                }
                closed = fb.changed() => if closed {
                    return Ok(()); // the reader hung up
                },
            }
            fb.persist_if_due(&**svc.store(), tl).await?;
        }
    }
    .await;
    reader.abort();
    fb.finish(&**svc.store(), tl).await;
    res
}

/// Postgres' WAL page size.
const XLOG_BLCKSZ: u64 = 8192;
/// The WAL segment size when the head does not record one.
const DEFAULT_WAL_SEG_SIZE: u64 = 16 << 20;

/// How much WAL the next chunk from `at` holds, readable up to `end`: at most
/// [`MAX_SEND_SIZE`], cut back to a page boundary unless it reaches `end`,
/// and never past the end of `at`'s segment.
fn chunk_len(at: Lsn, end: Lsn, seg_size: u64) -> usize {
    let mut stop = at.0 + MAX_SEND_SIZE as u64;
    if stop >= end.0 {
        stop = end.0;
    } else {
        stop -= stop % XLOG_BLCKSZ;
    }
    let seg_end = (at.0 / seg_size + 1) * seg_size;
    (stop.min(seg_end) - at.0) as usize
}

/// Up to `want` contiguous bytes of WAL from `at`, as one buffer.
async fn read_chunk<S: WalStore>(
    svc: &WalService<S>,
    tl: TimelineId,
    at: Lsn,
    want: usize,
) -> Result<Bytes, Error> {
    let mut out = BytesMut::with_capacity(want);
    let mut pos = at;
    while out.len() < want {
        let pieces = svc.read_wal(tl, pos, want - out.len()).await?;
        let before = out.len();
        for (lsn, bytes) in pieces {
            // Pieces start at or before `pos` (a store chunk may begin
            // earlier); anything after a gap waits for the next read.
            if lsn.0 > pos.0 {
                break;
            }
            let skip = (pos.0 - lsn.0) as usize;
            if skip >= bytes.len() {
                continue;
            }
            let take = (bytes.len() - skip).min(want - out.len());
            out.extend_from_slice(&bytes[skip..skip + take]);
            pos = Lsn(pos.0 + take as u64);
            if out.len() == want {
                break;
            }
        }
        if out.len() == before {
            break;
        }
    }
    Ok(out.freeze())
}

#[cfg(test)]
mod tests {
    use bytes::Buf;

    use super::*;

    #[test]
    fn protocol_option_parses_neons_json() {
        assert_eq!(
            ClientProtocol::from_option(None).unwrap(),
            ClientProtocol::Vanilla
        );
        assert_eq!(
            ClientProtocol::from_option(Some(r#"{"type":"vanilla"}"#)).unwrap(),
            ClientProtocol::Vanilla
        );
        // The pageserver's default `wal_receiver_protocol`.
        assert_eq!(
            ClientProtocol::from_option(Some(
                r#"{"type":"interpreted","args":{"format":"protobuf","compression":{"zstd":{"level":1}}}}"#
            ))
            .unwrap(),
            ClientProtocol::Interpreted(InterpretedProtocol {
                format: WireFormat::Protobuf,
                compression: Some(Compression::Zstd { level: 1 }),
            })
        );
        assert_eq!(
            ClientProtocol::from_option(Some(
                r#"{"type":"interpreted","args":{"format":"bincode","compression":null}}"#
            ))
            .unwrap(),
            ClientProtocol::Interpreted(InterpretedProtocol {
                format: WireFormat::Bincode,
                compression: None,
            })
        );
        for bad in [
            "interpreted",
            r#"{"type":"telepathy"}"#,
            r#"{"type":"interpreted","args":{"format":"json","compression":null}}"#,
        ] {
            assert!(ClientProtocol::from_option(Some(bad)).is_err(), "{bad}");
        }
    }

    fn opts(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn shard_options_follow_the_forks_handler() {
        // Interpreted: all three, as the pageserver sends them.
        assert_eq!(
            ShardSpec::from_options(&opts(&[
                ("shard_count", "0"),
                ("shard_number", "0"),
                ("shard_stripe_size", "2048"),
            ]))
            .unwrap(),
            ShardSpec {
                number: 0,
                count: 0,
                stripe_size: 2048
            }
        );
        assert_eq!(
            ShardSpec::from_options(&opts(&[
                ("shard_count", "4"),
                ("shard_number", "3"),
                ("shard_stripe_size", "2048"),
            ]))
            .unwrap(),
            ShardSpec {
                number: 3,
                count: 4,
                stripe_size: 2048
            }
        );
        for bad in [
            opts(&[]),
            opts(&[("shard_count", "2"), ("shard_number", "1")]),
            opts(&[("shard_count", "2"), ("shard_stripe_size", "8")]),
            opts(&[
                ("shard_count", "4"),
                ("shard_number", "4"),
                ("shard_stripe_size", "8"),
            ]),
            opts(&[
                ("shard_count", "0"),
                ("shard_number", "1"),
                ("shard_stripe_size", "8"),
            ]),
            opts(&[
                ("shard_count", "x"),
                ("shard_number", "0"),
                ("shard_stripe_size", "8"),
            ]),
            opts(&[
                ("shard_count", "2"),
                ("shard_number", "1"),
                ("shard_stripe_size", "0"),
            ]),
        ] {
            assert!(ShardSpec::from_options(&bad).is_err(), "{bad:?}");
        }
        // Vanilla: none at all.
        assert!(ShardSpec::refuse_on_vanilla(&opts(&[])).is_ok());
        assert!(ShardSpec::refuse_on_vanilla(&opts(&[("shard_count", "0")])).is_err());
        assert!(ShardSpec::refuse_on_vanilla(&opts(&[("shard_stripe_size", "8")])).is_err());
    }

    #[test]
    fn chunks_end_on_a_page_or_the_end_and_stay_in_a_segment() {
        let seg = 16 << 20;
        let max = MAX_SEND_SIZE as u64;
        // A full chunk from an unaligned start ends on a page boundary.
        assert_eq!(
            chunk_len(Lsn(0x1000_0028), Lsn(0x2000_0000), seg) as u64,
            max - 0x28
        );
        // The end of the readable WAL is not rounded.
        assert_eq!(chunk_len(Lsn(0x1000_0028), Lsn(0x1000_0100), seg), 0xd8);
        // A chunk stops at its segment's end.
        assert_eq!(chunk_len(Lsn(0x10ff_e000), Lsn(0x2000_0000), seg), 0x2000);
    }

    #[test]
    fn interpreted_batch_framing() {
        let mut buf = BytesMut::new();
        put_interpreted(&mut buf, Lsn(0x10), Lsn(0x20), b"abc");
        let mut b = buf.freeze();
        assert_eq!(b.get_u8(), b'd');
        assert_eq!(b.get_u32() as usize, 4 + 1 + 16 + 3);
        assert_eq!(b.get_u8(), b'0');
        assert_eq!(b.get_u64(), 0x10);
        assert_eq!(b.get_u64(), 0x20);
        assert_eq!(&b[..], b"abc");
    }
}
