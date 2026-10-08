//! Record what a safekeeper sends the pageserver for a WAL range (PG2 Task
//! 31's golden fixtures): push the range as walproposer would, then read it
//! back over the interpreted protocol and write one line per `'0'` CopyData
//! body: `streaming_lsn` and `commit_lsn` (hex), the body's length and its
//! sha256. Keepalives are left out. See
//! `crates/loams-wal-decoder/tests/fixtures/README.md`.
//!
//! ```text
//! cargo run -p loams-safekeeper --features server --example record_interpreted -- \
//!     127.0.0.1:55454 wal-17.bin out.sha256 [shard_count shard_number shard_stripe_size]
//! ```
//!
//! The range may be zstd-compressed (`.zst`):
//!
//! ```text
//! zstd -d wal-17.bin.zst -o wal-17.bin
//! ```

use bytes::Bytes;
use loams_safekeeper::propose::{WalRange, push_committed, read_interpreted};
use loams_safekeeper::types::TimelineId;
use sha2::{Digest, Sha256};

const TENANT: &str = "3131313131313131313131313131313a";
const TIMELINE: &str = "3131313131313131313131313131313b";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 && args.len() != 7 {
        return Err(
            "usage: record_interpreted <safekeeper> <range> <out> [count number stripe]".into(),
        );
    }
    let (addr, range, out) = (&args[1], &args[2], &args[3]);
    // An unsharded tenant's pageserver sends count and number 0.
    let (count, number, stripe) = if args.len() == 7 {
        (args[4].as_str(), args[5].as_str(), args[6].as_str())
    } else {
        ("0", "0", "2048")
    };
    let shard = format!("shard_count={count} shard_number={number} shard_stripe_size={stripe}");
    let range = WalRange::parse(Bytes::from(std::fs::read(range)?))?;
    let tl = TimelineId::new(TENANT.parse()?, TIMELINE.parse()?);
    let _proposer = push_committed(addr, tl, &range).await?;
    eprintln!("pushed {}..{}", range.start, range.end());
    let bodies = read_interpreted(addr, tl, &shard, range.start, range.end()).await?;
    let mut text = String::from(
        "# The fork's sender for wal-17.bin: one line per interpreted\n\
         # CopyData body: streaming_lsn commit_lsn (hex), body length, sha256 of the body.\n",
    );
    for b in &bodies {
        let streaming = u64::from_be_bytes(b[1..9].try_into()?);
        let commit = u64::from_be_bytes(b[9..17].try_into()?);
        let sha = hex::encode(Sha256::digest(b));
        text.push_str(&format!("{streaming:X} {commit:X} {} {sha}\n", b.len()));
    }
    std::fs::write(out, text)?;
    eprintln!("recorded {} batches to {out}", bodies.len());
    Ok(())
}
