//! The worker protocol's framing and conversions (LV1 plan Ruling 5 and
//! Task 5): `loams.live.worker.v1` messages, each a 4-byte big-endian
//! length and that many bytes, over the worker's stdin (host to worker) and
//! stdout (worker to host).
//!
//! Both ends bound every frame ([`MAX_FRAME_BYTES`]) and decode with an
//! explicit recursion limit. The host treats the worker as hostile: a frame
//! that is too large, malformed or out of turn is a crash, never an error
//! the host trusts.

use std::io::{self, Read, Write};

use buffa::{DecodeOptions, Message};
use loams_live::{CallOutput, FnKind, LiveError, LiveValue, LogLevel, LogLine, Visibility};
use loams_live_proto::loams::live::worker::v1 as wire;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::runtime::{Failure, FunctionMeta};
use crate::validators;

pub(crate) use wire::__buffa::oneof::done::Outcome as DoneOutcome;
pub(crate) use wire::__buffa::oneof::failure::Kind as FailureKind;
pub(crate) use wire::__buffa::oneof::host_message::Message as ToWorker;
pub(crate) use wire::__buffa::oneof::host_reply::Answer as ReplyAnswer;
pub(crate) use wire::__buffa::oneof::worker_message::Message as ToHost;
pub(crate) use wire::{
    CallContext, Done, FunctionKind, FunctionVisibility, HostCall, HostError, HostMessage,
    HostReply, Invoke, Load, Loaded, Probe as ProbeFrame, ProbeDone, ProbeKind, WorkerMessage,
};

/// The largest frame either side accepts: a 16 MiB bundle, or a result
/// within `Limits::max_result_bytes` (8 MiB of conversion budget, which
/// encodes to at most about twice that), with room to spare.
pub(crate) const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// The deepest message nesting decoded. A value nests two messages per
/// level (`Value`, then `Array` or `Object`), values are at most 64 levels
/// deep (the runtime's conversion limit), and the envelope adds a few.
const RECURSION_LIMIT: u32 = 160;

/// Why a frame could not be read.
#[derive(Debug)]
pub(crate) enum FrameError {
    /// The other side closed the stream between frames.
    Closed,
    /// An I/O error, a frame over [`MAX_FRAME_BYTES`], a stream cut inside
    /// a frame or a message that does not decode.
    Broken(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed => f.write_str("closed its stream"),
            FrameError::Broken(why) => f.write_str(why),
        }
    }
}

fn decode<M: Message>(bytes: &[u8]) -> Result<M, FrameError> {
    DecodeOptions::new()
        .with_recursion_limit(RECURSION_LIMIT)
        .with_max_message_size(MAX_FRAME_BYTES)
        .decode_from_slice(bytes)
        .map_err(|e| FrameError::Broken(format!("sent a frame that does not decode: {e}")))
}

/// The length prefix of a frame of `len` bytes, or why it is refused.
fn prefix(len: usize) -> io::Result<[u8; 4]> {
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {len} bytes is larger than {MAX_FRAME_BYTES}"),
        ));
    }
    let len = u32::try_from(len).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(len.to_be_bytes())
}

/// The length a prefix announces, checked against [`MAX_FRAME_BYTES`].
fn announced(prefix: [u8; 4]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::Broken(format!(
            "announced a frame of {len} bytes, more than {MAX_FRAME_BYTES}"
        )));
    }
    Ok(len)
}

/// Reads one frame (blocking: the worker's side). `Closed` at a clean end
/// of stream.
pub(crate) fn read_frame<M: Message>(input: &mut impl Read) -> Result<M, FrameError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match input.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Err(FrameError::Closed),
            Ok(0) => return Err(FrameError::Broken("cut a frame's length short".into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Broken(format!("reading a frame: {e}"))),
        }
    }
    let mut body = vec![0u8; announced(head)?];
    input
        .read_exact(&mut body)
        .map_err(|e| FrameError::Broken(format!("reading a frame: {e}")))?;
    decode(&body)
}

/// Writes one frame (blocking: the worker's side) and flushes it.
pub(crate) fn write_frame(output: &mut impl Write, message: &impl Message) -> io::Result<()> {
    let body = message.encode_to_vec();
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&prefix(body.len())?);
    frame.extend_from_slice(&body);
    output.write_all(&frame)?;
    output.flush()
}

/// Reads one frame (the host's side). `Closed` at a clean end of stream.
pub(crate) async fn read_frame_async<M: Message>(
    input: &mut (impl AsyncRead + Unpin),
) -> Result<M, FrameError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match input.read(&mut head[got..]).await {
            Ok(0) if got == 0 => return Err(FrameError::Closed),
            Ok(0) => return Err(FrameError::Broken("cut a frame's length short".into())),
            Ok(n) => got += n,
            Err(e) => return Err(FrameError::Broken(format!("reading a frame: {e}"))),
        }
    }
    let len = announced(head)?;
    let mut body = Vec::new();
    // Read in steps, so a worker that announces a large frame and sends
    // little has not made the host allocate it all.
    let mut limited = input.take(len as u64);
    limited
        .read_to_end(&mut body)
        .await
        .map_err(|e| FrameError::Broken(format!("reading a frame: {e}")))?;
    if body.len() != len {
        return Err(FrameError::Broken("cut a frame short".into()));
    }
    decode(&body)
}

/// Writes one frame (the host's side) and flushes it.
pub(crate) async fn write_frame_async(
    output: &mut (impl AsyncWrite + Unpin),
    message: &impl Message,
) -> io::Result<()> {
    let body = message.encode_to_vec();
    output.write_all(&prefix(body.len())?).await?;
    output.write_all(&body).await?;
    output.flush().await
}

// ---- conversions ----

/// A worker message.
pub(crate) fn to_host(message: ToHost) -> WorkerMessage {
    WorkerMessage {
        message: Some(message),
        ..Default::default()
    }
}

/// A host message.
pub(crate) fn to_worker(message: ToWorker) -> HostMessage {
    HostMessage {
        message: Some(message),
        ..Default::default()
    }
}

/// The wire form of a function's metadata; its validator travels as the
/// descriptor `loams:server`'s `v` builds, which the host parses again.
pub(crate) fn meta_to_wire(meta: &FunctionMeta) -> wire::FunctionMeta {
    wire::FunctionMeta {
        path: meta.path.clone(),
        kind: match meta.kind {
            FnKind::Query => FunctionKind::FUNCTION_KIND_QUERY,
            FnKind::Mutation => FunctionKind::FUNCTION_KIND_MUTATION,
        }
        .into(),
        visibility: match meta.visibility {
            Visibility::Public => FunctionVisibility::FUNCTION_VISIBILITY_PUBLIC,
            Visibility::Internal => FunctionVisibility::FUNCTION_VISIBILITY_INTERNAL,
        }
        .into(),
        args: meta
            .args
            .as_ref()
            .map(|v| validators::descriptor(v).to_proto())
            .into(),
        ..Default::default()
    }
}

/// A function's metadata as the worker reported it, checked as the host
/// checks anything from a worker.
pub(crate) fn meta_from_wire(meta: wire::FunctionMeta) -> Result<FunctionMeta, String> {
    let kind = match meta.kind.as_known() {
        Some(FunctionKind::FUNCTION_KIND_QUERY) => FnKind::Query,
        Some(FunctionKind::FUNCTION_KIND_MUTATION) => FnKind::Mutation,
        _ => return Err(format!("reported no kind for {:?}", meta.path)),
    };
    let visibility = match meta.visibility.as_known() {
        Some(FunctionVisibility::FUNCTION_VISIBILITY_PUBLIC) => Visibility::Public,
        Some(FunctionVisibility::FUNCTION_VISIBILITY_INTERNAL) => Visibility::Internal,
        _ => return Err(format!("reported no visibility for {:?}", meta.path)),
    };
    let args = match meta.args.into_option() {
        None => None,
        Some(descriptor) => {
            let descriptor = LiveValue::from_proto(descriptor)
                .map_err(|e| format!("reported a bad validator for {:?}: {e}", meta.path))?;
            Some(
                validators::parse(&descriptor)
                    .map_err(|e| format!("reported a bad validator for {:?}: {e}", meta.path))?,
            )
        }
    };
    Ok(FunctionMeta {
        path: meta.path,
        kind,
        visibility,
        args,
    })
}

/// The wire form of a failure.
pub(crate) fn failure_to_wire(failure: &Failure) -> wire::Failure {
    let kind = match failure {
        Failure::Host(i) => FailureKind::HostError(u32::try_from(*i).unwrap_or(u32::MAX)),
        Failure::Live(e) => match e {
            LiveError::FunctionError(m) => FailureKind::FunctionError(m.clone()),
            LiveError::FunctionTimeout { function, .. } => FailureKind::Timeout(function.clone()),
            LiveError::FunctionOutOfMemory { function, .. } => {
                FailureKind::OutOfMemory(function.clone())
            }
            LiveError::LimitExceeded { limit, message } => {
                FailureKind::LimitExceeded(Box::new(wire::LimitExceeded {
                    limit: (*limit).to_string(),
                    message: message.clone(),
                    ..Default::default()
                }))
            }
            LiveError::InvalidArgument(m) => FailureKind::InvalidArgument(m.clone()),
            other => FailureKind::Internal(other.to_string()),
        },
    };
    wire::Failure {
        kind: Some(kind),
        ..Default::default()
    }
}

/// The limits a worker may report exceeded; anything else is refused.
const LIMITS: &[&str] = &[
    "max_result_bytes",
    "max_document_bytes",
    "max_bundle_bytes",
    "max_exports",
];

/// A failure as the worker reported it. The limits named in timeouts and
/// out-of-memory errors are the host's own configuration, not the
/// worker's word.
pub(crate) fn failure_from_wire(
    failure: wire::Failure,
    cpu_limit: std::time::Duration,
    memory_limit: usize,
) -> Result<Failure, String> {
    Ok(match failure.kind {
        None => return Err("reported a failure of no kind".into()),
        Some(FailureKind::HostError(i)) => Failure::Host(i as usize),
        Some(FailureKind::FunctionError(m)) => Failure::Live(LiveError::FunctionError(m)),
        Some(FailureKind::Timeout(function)) => Failure::Live(LiveError::FunctionTimeout {
            function,
            limit: cpu_limit,
        }),
        Some(FailureKind::OutOfMemory(function)) => Failure::Live(LiveError::FunctionOutOfMemory {
            function,
            limit: memory_limit,
        }),
        Some(FailureKind::LimitExceeded(l)) => {
            let Some(limit) = LIMITS.iter().find(|name| **name == l.limit) else {
                return Err(format!("reported an unknown limit {:?}", l.limit));
            };
            Failure::Live(LiveError::LimitExceeded {
                limit,
                message: l.message,
            })
        }
        Some(FailureKind::InvalidArgument(m)) => Failure::Live(LiveError::InvalidArgument(m)),
        Some(FailureKind::Internal(m)) => Failure::Live(LiveError::Internal(m)),
    })
}

/// The wire form of a call's console output.
pub(crate) fn logs_to_wire(output: &CallOutput) -> Vec<wire::LogLine> {
    output
        .logs
        .iter()
        .map(|l| wire::LogLine {
            level: match l.level {
                LogLevel::Log => wire::LogLevel::LOG_LEVEL_LOG,
                LogLevel::Debug => wire::LogLevel::LOG_LEVEL_DEBUG,
                LogLevel::Info => wire::LogLevel::LOG_LEVEL_INFO,
                LogLevel::Warn => wire::LogLevel::LOG_LEVEL_WARN,
                LogLevel::Error => wire::LogLevel::LOG_LEVEL_ERROR,
            }
            .into(),
            line: l.line.clone(),
            truncated: l.truncated,
            ..Default::default()
        })
        .collect()
}

/// A call's console output as the worker reported it, held to the
/// configured limits again.
pub(crate) fn logs_from_wire(
    logs: Vec<wire::LogLine>,
    dropped: u64,
    lines: usize,
    line_bytes: usize,
) -> CallOutput {
    let mut out = CallOutput {
        logs: Vec::new(),
        dropped,
    };
    for l in logs {
        if out.logs.len() >= lines {
            out.dropped = out.dropped.saturating_add(1);
            continue;
        }
        let level = match l.level.as_known() {
            Some(wire::LogLevel::LOG_LEVEL_DEBUG) => LogLevel::Debug,
            Some(wire::LogLevel::LOG_LEVEL_INFO) => LogLevel::Info,
            Some(wire::LogLevel::LOG_LEVEL_WARN) => LogLevel::Warn,
            Some(wire::LogLevel::LOG_LEVEL_ERROR) => LogLevel::Error,
            _ => LogLevel::Log,
        };
        let (line, cut) = crate::limits::cut(&l.line, line_bytes);
        out.logs.push(LogLine {
            level,
            line: line.to_string(),
            truncated: l.truncated || cut,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke() -> HostMessage {
        to_worker(ToWorker::Invoke(Box::new(Invoke {
            path: "m:f".into(),
            args: buffa::MessageField::some(LiveValue::Str("x".into()).to_proto()),
            ..Default::default()
        })))
    }

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &invoke()).expect("write");
        write_frame(&mut buf, &invoke()).expect("write");
        let mut input = &buf[..];
        for _ in 0..2 {
            let m: HostMessage = read_frame(&mut input).expect("a frame");
            assert!(matches!(m.message, Some(ToWorker::Invoke(_))));
        }
        assert!(matches!(
            read_frame::<HostMessage>(&mut input),
            Err(FrameError::Closed)
        ));
    }

    #[test]
    fn oversized_and_cut_frames_are_refused() {
        let huge = u32::try_from(MAX_FRAME_BYTES + 1)
            .expect("fits")
            .to_be_bytes();
        assert!(matches!(
            read_frame::<HostMessage>(&mut &huge[..]),
            Err(FrameError::Broken(_))
        ));
        let mut buf = Vec::new();
        write_frame(&mut buf, &invoke()).expect("write");
        buf.truncate(buf.len() - 1);
        assert!(matches!(
            read_frame::<HostMessage>(&mut &buf[..]),
            Err(FrameError::Broken(_))
        ));
        assert!(matches!(
            read_frame::<HostMessage>(&mut &buf[..2]),
            Err(FrameError::Broken(_))
        ));
    }

    #[tokio::test]
    async fn async_frames_match_blocking_ones() {
        let mut buf = Vec::new();
        write_frame_async(&mut buf, &invoke()).await.expect("write");
        let mut sync = Vec::new();
        write_frame(&mut sync, &invoke()).expect("write");
        assert_eq!(buf, sync);
        let m: HostMessage = read_frame_async(&mut &buf[..]).await.expect("a frame");
        assert!(matches!(m.message, Some(ToWorker::Invoke(_))));
        assert!(matches!(
            read_frame_async::<HostMessage>(&mut &buf[..0]).await,
            Err(FrameError::Closed)
        ));
    }

    #[test]
    fn failures_round_trip_and_unknown_limits_are_refused() {
        let cpu = std::time::Duration::from_secs(1);
        for f in [
            Failure::Host(3),
            Failure::Live(LiveError::FunctionError("boom".into())),
            Failure::Live(LiveError::FunctionTimeout {
                function: "m:f".into(),
                limit: cpu,
            }),
            Failure::Live(LiveError::FunctionOutOfMemory {
                function: "m:f".into(),
                limit: 7,
            }),
            Failure::Live(LiveError::LimitExceeded {
                limit: "max_result_bytes",
                message: "big".into(),
            }),
            Failure::Live(LiveError::InvalidArgument("bad".into())),
        ] {
            let back = failure_from_wire(failure_to_wire(&f), cpu, 7).expect("decodes");
            assert_eq!(format!("{back:?}"), format!("{f:?}"));
        }
        let forged = wire::Failure {
            kind: Some(FailureKind::LimitExceeded(Box::new(wire::LimitExceeded {
                limit: "anything".into(),
                ..Default::default()
            }))),
            ..Default::default()
        };
        assert!(failure_from_wire(forged, cpu, 7).is_err());
    }

    #[test]
    fn logs_are_held_to_the_limits_again() {
        let logs = vec![
            wire::LogLine {
                line: "a".repeat(10),
                ..Default::default()
            };
            3
        ];
        let out = logs_from_wire(logs, 1, 2, 4);
        assert_eq!(out.logs.len(), 2);
        assert_eq!(out.dropped, 2);
        assert_eq!(out.logs[0].line, "aaaa");
        assert!(out.logs[0].truncated);
    }
}
