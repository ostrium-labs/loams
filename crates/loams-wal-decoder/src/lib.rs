//! WAL to the pageserver's interpreted record batches with the Neon fork's
//! `wal_decoder`, behind `loams-safekeeper`'s [`WalInterpreter`] (PG2 Task
//! 31, Q112).
//!
//! [`Decoder`] does for one reader what the fork's `InterpretedWalReader`
//! (`safekeeper/src/send_interpreted_wal.rs`, `run_impl`) does for one shard
//! sender, per chunk of WAL:
//!
//! - feed `WalStreamDecoder` and take every complete record;
//! - keep the parts for the reader's shard (`InterpretedWalRecord::
//!   from_bytes_filtered`), dropping empty records except on shard zero;
//! - when the chunk completes no record, send nothing;
//! - otherwise send one `InterpretedWalRecords` batch: the shard's new
//!   records with `next_record_lsn` at the last completed record, or, when
//!   the shard has seen them all, an empty batch at its own position;
//!   `raw_wal_start_lsn` is where the batch's raw WAL began;
//! - encode it with `ToWireFormat::to_wire` (protobuf or bincode, optionally
//!   zstd).
//!
//! [`NeonInterpreter`] plugs that into `loams-wal`, and
//! `src/bin/loams-wal-interpreted.rs` is `loams-wal` with it.
//!
//! Neon is Apache-2.0, Copyright Neon Inc. (see the repository's `NOTICE`).

use bytes::Bytes;
use loams_safekeeper::send::{
    Compression as LoamsCompression, InterpretedProtocol, ShardDecoder, ShardSpec, WalInterpreter,
    WireFormat,
};
use pageserver_api::models::ShardParameters;
use pageserver_api::shard::{ShardCount, ShardIdentity, ShardNumber, ShardStripeSize};
use postgres_ffi::PgMajorVersion;
use postgres_ffi::waldecoder::WalStreamDecoder;
use utils::lsn::Lsn;
use utils::postgres_client::{Compression, InterpretedFormat};
use wal_decoder::models::{InterpretedWalRecord, InterpretedWalRecords};
use wal_decoder::wire_format::ToWireFormat;

/// Errors of the wrapper, as text.
#[derive(Debug)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

fn err(e: impl std::fmt::Display) -> DecodeError {
    DecodeError(e.to_string())
}

/// The Postgres major version of a server version number (`170005` -> 17).
pub fn major(pg_server_version: u32) -> Result<PgMajorVersion, DecodeError> {
    match pg_server_version / 10_000 {
        14 => Ok(PgMajorVersion::PG14),
        15 => Ok(PgMajorVersion::PG15),
        16 => Ok(PgMajorVersion::PG16),
        17 => Ok(PgMajorVersion::PG17),
        other => Err(DecodeError(format!(
            "unsupported Postgres major version {other}"
        ))),
    }
}

/// The fork's shard identity for the startup options, as its `handler.rs`
/// builds it (`ShardIdentity::from_params`).
pub fn identity(shard: ShardSpec) -> ShardIdentity {
    ShardIdentity::from_params(
        ShardNumber(shard.number),
        ShardParameters {
            count: ShardCount(shard.count),
            stripe_size: ShardStripeSize(shard.stripe_size),
        },
    )
}

/// One reader's decoder: one shard, one stream position.
pub struct Decoder {
    wal: WalStreamDecoder,
    pg_version: PgMajorVersion,
    shard: ShardIdentity,
    format: InterpretedFormat,
    compression: Option<Compression>,
    /// The shard's position: the next record it has not been sent.
    next_record_lsn: Lsn,
    /// Where the raw WAL of the next batch began.
    batch_wal_start: Option<Lsn>,
}

impl Decoder {
    /// A decoder for WAL from `start_lsn` (a record boundary) of server
    /// version `pg_server_version`, for `shard`, encoding as `protocol` asks.
    pub fn new(
        start_lsn: u64,
        pg_server_version: u32,
        protocol: InterpretedProtocol,
        shard: ShardSpec,
    ) -> Result<Self, DecodeError> {
        let pg_version = major(pg_server_version)?;
        Ok(Self {
            wal: WalStreamDecoder::new(Lsn(start_lsn), pg_version),
            pg_version,
            shard: identity(shard),
            format: match protocol.format {
                WireFormat::Bincode => InterpretedFormat::Bincode,
                WireFormat::Protobuf => InterpretedFormat::Protobuf,
            },
            compression: protocol.compression.map(|c| match c {
                LoamsCompression::Zstd { level } => Compression::Zstd { level },
            }),
            next_record_lsn: Lsn(start_lsn),
            batch_wal_start: None,
        })
    }

    /// Feed `wal` (starting at `start`, contiguous with the previous call)
    /// and return the encoded batch to send for it, if any.
    pub async fn decode(&mut self, start: u64, wal: &[u8]) -> Result<Option<Bytes>, DecodeError> {
        let wal_end_lsn = Lsn(start + wal.len() as u64);
        if self.batch_wal_start.is_none() {
            self.batch_wal_start = Some(Lsn(start));
        }
        self.wal.feed_bytes(wal);
        let mut records = Vec::new();
        let mut max_next_record_lsn = None;
        let mut max_end_record_lsn = None;
        while let Some((next_record_lsn, recdata)) = self.wal.poll_decode().map_err(err)? {
            max_next_record_lsn = Some(next_record_lsn);
            max_end_record_lsn = Some(self.wal.lsn());
            let by_shard = InterpretedWalRecord::from_bytes_filtered(
                recdata,
                std::slice::from_ref(&self.shard),
                next_record_lsn,
                self.pg_version,
            )
            .map_err(err)?;
            for (shard, record) in by_shard {
                if !shard.is_shard_zero() && record.is_empty() {
                    continue;
                }
                if record.next_record_lsn > self.next_record_lsn {
                    records.push(record);
                }
            }
        }
        let (Some(max_next_record_lsn), Some(max_end_record_lsn)) =
            (max_next_record_lsn, max_end_record_lsn)
        else {
            return Ok(None);
        };
        let raw_wal_start_lsn = self
            .batch_wal_start
            .replace(max_end_record_lsn)
            .unwrap_or(Lsn(start));
        let batch = if max_next_record_lsn > self.next_record_lsn {
            InterpretedWalRecords {
                records,
                next_record_lsn: max_next_record_lsn,
                raw_wal_start_lsn: Some(raw_wal_start_lsn),
            }
        } else if wal_end_lsn > self.next_record_lsn {
            InterpretedWalRecords {
                records: Vec::new(),
                next_record_lsn: self.next_record_lsn,
                raw_wal_start_lsn: Some(raw_wal_start_lsn),
            }
        } else {
            return Ok(None);
        };
        self.next_record_lsn = self.next_record_lsn.max(max_next_record_lsn);
        batch
            .to_wire(self.format, self.compression)
            .await
            .map(Some)
            .map_err(err)
    }
}

/// [`WalInterpreter`] over [`Decoder`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NeonInterpreter;

struct Adapter(Decoder);

impl WalInterpreter for NeonInterpreter {
    fn decoder(
        &self,
        start: loams_safekeeper::Lsn,
        pg_version: u32,
        protocol: InterpretedProtocol,
        shard: ShardSpec,
    ) -> Result<Box<dyn ShardDecoder>, loams_safekeeper::Error> {
        let d = Decoder::new(start.0, pg_version, protocol, shard)
            .map_err(|e| loams_safekeeper::Error::Protocol(e.0))?;
        Ok(Box::new(Adapter(d)))
    }
}

#[async_trait::async_trait]
impl ShardDecoder for Adapter {
    async fn decode(
        &mut self,
        start: loams_safekeeper::Lsn,
        wal: Bytes,
    ) -> Result<Option<Bytes>, loams_safekeeper::Error> {
        self.0
            .decode(start.0, &wal)
            .await
            .map_err(|e| loams_safekeeper::Error::Protocol(format!("interpreting WAL: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_versions_map_to_majors() {
        assert!(matches!(major(170_005), Ok(PgMajorVersion::PG17)));
        assert!(matches!(major(160_009), Ok(PgMajorVersion::PG16)));
        assert!(major(130_000).is_err());
        assert!(major(180_000).is_err());
    }

    #[test]
    fn shard_identities_follow_the_handler() {
        let unsharded = identity(ShardSpec::UNSHARDED);
        assert!(unsharded.is_shard_zero());
        assert_eq!(unsharded.count, ShardCount(0));
        let s = identity(ShardSpec {
            number: 1,
            count: 2,
            stripe_size: 2048,
        });
        assert!(!s.is_shard_zero());
        assert_eq!(s.stripe_size, ShardStripeSize(2048));
    }
}
