//! A minimal proposer and pageserver-side reader, for tools and tests: push
//! a committed WAL range to any safekeeper-protocol server (Neon's or
//! `loams-wal`) as walproposer would, and read it back over the interpreted
//! protocol as the pageserver does (PG2 Task 31's golden fixtures and their
//! tests).

use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use tokio::net::TcpStream;

use crate::Error;
use crate::pgwire::client;
use crate::proto::{
    AcceptorMessage, AppendRequest, AppendRequestHeader, MAX_SEND_SIZE, ProposerElected,
    ProposerGreeting, ProposerMessage, VoteRequest,
};
use crate::types::{Configuration, Lsn, TermHistory, TermLsn, TimelineId};

/// The pageserver's `protocol` option at its default (protobuf, zstd level
/// 1), as Neon serializes `PostgresClientProtocol`.
pub const PAGESERVER_PROTOCOL: &str =
    r#"{"type":"interpreted","args":{"format":"protobuf","compression":{"zstd":{"level":1}}}}"#;

/// A recorded WAL range: `start_lsn` (u64), `pg_version` (u32) and
/// `system_id` (u64), big-endian, then the WAL.
#[derive(Clone, Debug)]
pub struct WalRange {
    pub start: Lsn,
    pub pg_version: u32,
    pub system_id: u64,
    pub wal: Bytes,
}

impl WalRange {
    pub fn parse(mut raw: Bytes) -> Result<Self, Error> {
        if raw.len() < 20 {
            return Err(Error::Protocol("WAL range file too short".into()));
        }
        let start = Lsn(raw.get_u64());
        let pg_version = raw.get_u32();
        let system_id = raw.get_u64();
        Ok(Self {
            start,
            pg_version,
            system_id,
            wal: raw,
        })
    }

    pub fn end(&self) -> Lsn {
        Lsn(self.start.0 + self.wal.len() as u64)
    }
}

/// Connect and send the startup packet for `tl`, with `extra` options.
pub async fn connect(addr: &str, tl: TimelineId, extra: &str) -> Result<TcpStream, Error> {
    let mut s = TcpStream::connect(addr)
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    let options = format!(
        "-c timeline_id={} tenant_id={} {extra}",
        tl.timeline, tl.tenant
    );
    client::startup(
        &mut s,
        &[
            ("user", "loams"),
            ("dbname", "replication"),
            ("options", &options),
        ],
    )
    .await?;
    Ok(s)
}

async fn send(s: &mut TcpStream, m: ProposerMessage) -> Result<(), Error> {
    let mut buf = BytesMut::new();
    m.serialize(&mut buf);
    client::send_copy_data(s, &buf).await
}

async fn recv(s: &mut TcpStream) -> Result<AcceptorMessage, Error> {
    let body = client::recv_copy_data(s)
        .await?
        .ok_or_else(|| Error::Io("stream ended".into()))?;
    AcceptorMessage::parse(body)
}

/// Push `range` to a fresh timeline as term 1, which starts at the range's
/// start, and commit all of it. Returns the proposer's connection: keep it
/// open while reading (a proposer that goes away is not an error, but some
/// servers then stop serving the term).
pub async fn push_committed(
    addr: &str,
    tl: TimelineId,
    range: &WalRange,
) -> Result<TcpStream, Error> {
    let start = range.start;
    let end = range.end();
    let mut p = connect(addr, tl, "").await?;
    client::query(
        &mut p,
        "START_WAL_PUSH (proto_version '3', allow_timeline_creation 'true')",
    )
    .await?;
    client::expect_copy_both(&mut p).await?;
    send(
        &mut p,
        ProposerMessage::Greeting(ProposerGreeting {
            tenant_id: tl.tenant,
            timeline_id: tl.timeline,
            mconf: Configuration::default(),
            pg_version: range.pg_version,
            system_id: range.system_id,
            wal_seg_size: 16 << 20,
        }),
    )
    .await?;
    let AcceptorMessage::Greeting(_) = recv(&mut p).await? else {
        return Err(Error::Protocol("expected a greeting".into()));
    };
    send(
        &mut p,
        ProposerMessage::VoteRequest(VoteRequest {
            generation: 0,
            term: 1,
        }),
    )
    .await?;
    let AcceptorMessage::VoteResponse(v) = recv(&mut p).await? else {
        return Err(Error::Protocol("expected a vote".into()));
    };
    if !v.vote_given {
        return Err(Error::Protocol(
            "vote refused: the timeline is not fresh".into(),
        ));
    }
    send(
        &mut p,
        ProposerMessage::Elected(ProposerElected {
            generation: 0,
            term: 1,
            start_streaming_at: start,
            term_history: TermHistory(vec![TermLsn {
                term: 1,
                lsn: start,
            }]),
        }),
    )
    .await?;
    let mut at = start;
    for chunk in range.wal.chunks(MAX_SEND_SIZE) {
        let next = Lsn(at.0 + chunk.len() as u64);
        let req = AppendRequest {
            h: AppendRequestHeader {
                generation: 0,
                term: 1,
                begin_lsn: at,
                end_lsn: next,
                commit_lsn: start,
                truncate_lsn: Lsn(0),
            },
            wal: Bytes::copy_from_slice(chunk),
        };
        send(&mut p, ProposerMessage::Append(req)).await?;
        at = next;
    }
    loop {
        if let AcceptorMessage::AppendResponse(r) = recv(&mut p).await?
            && r.flush_lsn == end
        {
            break;
        }
    }
    // The commit, in a heartbeat. Neon's safekeeper does not answer it; a
    // reader waits for the commit anyway.
    let commit = AppendRequest {
        h: AppendRequestHeader {
            generation: 0,
            term: 1,
            begin_lsn: end,
            end_lsn: end,
            commit_lsn: end,
            truncate_lsn: Lsn(0),
        },
        wal: Bytes::new(),
    };
    send(&mut p, ProposerMessage::Append(commit)).await?;
    Ok(p)
}

/// Read `tl` from `start` over the interpreted protocol with `shard_opts`
/// (`shard_count=.. shard_number=.. shard_stripe_size=..`), until a batch's
/// `streaming_lsn` reaches `end`. Returns every `'0'` CopyData body;
/// keepalives are dropped.
pub async fn read_interpreted(
    addr: &str,
    tl: TimelineId,
    shard_opts: &str,
    start: Lsn,
    end: Lsn,
) -> Result<Vec<Bytes>, Error> {
    let mut r = connect(
        addr,
        tl,
        &format!("protocol={PAGESERVER_PROTOCOL} {shard_opts}"),
    )
    .await?;
    client::query(&mut r, &format!("START_REPLICATION PHYSICAL {start}")).await?;
    client::expect_copy_both(&mut r).await?;
    let mut out = Vec::new();
    loop {
        let body = tokio::time::timeout(Duration::from_secs(10), client::recv_copy_data(&mut r))
            .await
            .map_err(|_| Error::Io("no batch within 10 s".into()))??
            .ok_or_else(|| Error::Io("stream ended".into()))?;
        match body.first() {
            Some(b'0') if body.len() >= 17 => {
                let streaming = u64::from_be_bytes(body[1..9].try_into().unwrap_or_default());
                out.push(body);
                if streaming >= end.0 {
                    return Ok(out);
                }
            }
            Some(b'k') => {}
            other => {
                return Err(Error::Protocol(format!(
                    "unexpected replication message {other:?}"
                )));
            }
        }
    }
}
