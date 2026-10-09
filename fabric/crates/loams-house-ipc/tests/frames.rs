//! The `hsw1` codec: every frame survives a round trip, and every way a frame can be
//! wrong is refused rather than guessed at (HS1 Task 2).

use bytes::Bytes;
use loams_house_ipc::{
    Analysis, Analyze, Bind, Chunk, Classification, CodecError, EngineError, Execute, Frame,
    FrameCodec, InputSpec, Limits, MAX_FRAME_BYTES, PROTOCOL_VERSION, Progress, QueryClass, Ready,
    SessionRef,
};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;

fn text() -> impl Strategy<Value = String> {
    // Any Unicode, including NUL and the empty string: a frame carries user SQL.
    ".{0,40}"
}

fn pairs() -> impl Strategy<Value = Vec<(String, String)>> {
    vec((text(), text()), 0..4)
}

fn blob() -> impl Strategy<Value = Bytes> {
    vec(any::<u8>(), 0..512).prop_map(Bytes::from)
}

fn progress() -> impl Strategy<Value = Progress> {
    (
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        (any::<u64>(), any::<u64>()),
    )
        .prop_map(
            |(
                rows_read,
                bytes_read,
                result_rows,
                result_bytes,
                elapsed_ns,
                rss_bytes,
                peak,
                (written_rows, written_bytes),
            )| {
                Progress {
                    rows_read,
                    bytes_read,
                    result_rows,
                    result_bytes,
                    written_rows,
                    written_bytes,
                    elapsed_ns,
                    rss_bytes,
                    peak_rss_bytes: peak,
                    sessions: (written_rows % 1000) as u32,
                }
            },
        )
}

fn limits() -> impl Strategy<Value = Limits> {
    (
        option::of(any::<u64>()),
        option::of(any::<u64>()),
        option::of(any::<u32>()),
        option::of(any::<u64>()),
        option::of(any::<u64>()),
        option::of(any::<u64>()),
    )
        .prop_map(
            |(
                max_execution_time_ms,
                max_memory_usage,
                max_threads,
                max_bytes_to_read,
                max_result_bytes,
                max_result_rows,
            )| Limits {
                max_execution_time_ms,
                max_memory_usage,
                max_threads,
                max_bytes_to_read,
                max_result_bytes,
                max_result_rows,
            },
        )
}

fn execute() -> impl Strategy<Value = Execute> {
    (
        text(),
        option::of(
            (text(), any::<u64>(), any::<bool>()).prop_map(|(key, timeout_ms, close)| SessionRef {
                key,
                timeout_ms,
                close,
            }),
        ),
        pairs(),
        vec(text(), 0..3),
        text(),
        text(),
        pairs(),
        limits(),
        option::of((text(), text()).prop_map(|(insert, format)| InputSpec { insert, format })),
    )
        .prop_map(
            |(query_id, session, settings, views, sql, format, params, limits, input)| Execute {
                query_id,
                session,
                settings,
                views,
                sql,
                format,
                params,
                limits,
                input,
            },
        )
}

fn frame() -> impl Strategy<Value = Frame> {
    let ready = (
        any::<u8>(),
        any::<u32>(),
        text(),
        text(),
        any::<u32>(),
        vec(text(), 0..4),
    )
        .prop_map(
            |(protocol, pid, chdb_version, clickhouse_version, boot_ms, settings)| {
                Frame::Ready(Ready {
                    protocol,
                    pid,
                    chdb_version,
                    clickhouse_version,
                    boot_ms,
                    settings,
                })
            },
        );
    let bind = (text(), text(), pairs(), option::of(text()), any::<u64>()).prop_map(
        |(namespace, isolation_class, settings, proxy_endpoint, temp_dir_quota_bytes)| {
            Frame::Bind(Bind {
                namespace,
                isolation_class,
                settings,
                proxy_endpoint,
                temp_dir_quota_bytes,
            })
        },
    );
    let error = (any::<i32>(), text(), text(), any::<bool>()).prop_map(
        |(code, name, message, poisoned)| Frame::Error {
            error: EngineError {
                code,
                name,
                message,
            },
            poisoned,
        },
    );
    prop_oneof![
        ready,
        bind,
        execute().prop_map(Frame::Execute),
        blob().prop_map(Frame::Input),
        Just(Frame::InputEnd),
        (blob(), any::<bool>())
            .prop_map(|(bytes, continued)| Frame::Chunk(Chunk { bytes, continued })),
        progress().prop_map(Frame::Progress),
        progress().prop_map(Frame::Stats),
        error,
        Just(Frame::Done),
        text().prop_map(Frame::Classify),
        (
            prop_oneof![
                Just(QueryClass::ReadOnly),
                Just(QueryClass::Mutating),
                Just(QueryClass::MutatingGlobal),
                Just(QueryClass::Control),
                Just(QueryClass::Unknown),
            ],
            any::<u32>()
        )
            .prop_map(|(class, statements)| Frame::Classified(Classification {
                class,
                statements
            })),
        (text(), vec((text(), text()), 0..3))
            .prop_map(|(sql, params)| Frame::Analyze(Analyze { sql, params })),
        (
            prop_oneof![Just(QueryClass::ReadOnly), Just(QueryClass::Unknown)],
            any::<u32>(),
            text(),
            option::of(text())
        )
            .prop_map(|(class, statements, ast, query_tree)| {
                Frame::Analyzed(Analysis {
                    classification: Classification { class, statements },
                    ast,
                    query_tree,
                })
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// Every frame, encoded and decoded, is the frame it was — through the
    /// blocking reader the worker uses and the async one the front uses.
    #[test]
    fn frames_roundtrip_property(frames in vec(frame(), 1..6)) {
        let mut wire = Vec::new();
        for frame in &frames {
            FrameCodec::write(&mut wire, frame).expect("encodes");
        }

        let mut reader = wire.as_slice();
        for frame in &frames {
            let decoded = FrameCodec::read(&mut reader).expect("decodes");
            prop_assert_eq!(decoded.as_ref(), Some(frame));
        }
        prop_assert_eq!(FrameCodec::read(&mut reader).expect("clean end"), None);

        let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        let decoded_async = runtime.block_on(async {
            let mut reader = wire.as_slice();
            let mut out = Vec::new();
            while let Some(frame) = FrameCodec::read_async(&mut reader).await.expect("decodes") {
                out.push(frame);
            }
            out
        });
        prop_assert_eq!(decoded_async, frames);
    }
}

#[test]
fn the_wire_starts_with_a_length_then_the_version() {
    let wire = FrameCodec::encode(&Frame::Done).expect("encodes");
    let length = u32::from_be_bytes([wire[0], wire[1], wire[2], wire[3]]) as usize;
    assert_eq!(
        length,
        wire.len() - 4,
        "the prefix counts the body, not itself"
    );
    assert_eq!(wire[4], PROTOCOL_VERSION, "the version byte leads the body");
    assert_eq!(PROTOCOL_VERSION, 1, "hsw1");
}

#[test]
fn unknown_version_is_refused_by_name() {
    let mut wire = FrameCodec::encode(&Frame::Done).expect("encodes");
    wire[4] = PROTOCOL_VERSION + 1;
    match FrameCodec::read(&mut wire.as_slice()) {
        Err(CodecError::UnknownVersion(version)) => assert_eq!(version, PROTOCOL_VERSION + 1),
        other => panic!("expected UnknownVersion, got {other:?}"),
    }
}

#[test]
fn oversized_length_is_refused_before_allocating() {
    let mut wire = Vec::new();
    wire.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_be_bytes());
    wire.push(PROTOCOL_VERSION);
    match FrameCodec::read(&mut wire.as_slice()) {
        Err(CodecError::TooLarge(length)) => assert_eq!(length, MAX_FRAME_BYTES + 1),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn empty_body_is_refused() {
    let wire = 0u32.to_be_bytes();
    assert!(matches!(
        FrameCodec::read(&mut wire.as_slice()),
        Err(CodecError::Empty)
    ));
}

#[test]
fn truncated_frame_is_an_error_not_an_end() {
    let wire = FrameCodec::encode(&Frame::Input(Bytes::from_static(b"abcdef"))).expect("encodes");
    for cut in 1..wire.len() {
        let result = FrameCodec::read(&mut &wire[..cut]);
        assert!(
            matches!(result, Err(CodecError::Truncated)),
            "a frame cut at {cut} of {} bytes must be Truncated, got {result:?}",
            wire.len()
        );
    }
}

#[test]
fn garbage_body_is_malformed() {
    let mut wire = Vec::new();
    wire.extend_from_slice(&3u32.to_be_bytes());
    wire.extend_from_slice(&[PROTOCOL_VERSION, 0xff, 0xff]);
    assert!(matches!(
        FrameCodec::read(&mut wire.as_slice()),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn trailing_bytes_inside_a_frame_are_malformed() {
    let mut wire = FrameCodec::encode(&Frame::Done).expect("encodes");
    wire.push(0);
    let length = (wire.len() - 4) as u32;
    wire[..4].copy_from_slice(&length.to_be_bytes());
    assert!(matches!(
        FrameCodec::read(&mut wire.as_slice()),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn chunks_split_at_the_limit_and_say_so() {
    let block = Bytes::from(vec![7u8; 10]);
    let pieces = Chunk::split(block.clone(), 4);
    assert_eq!(
        pieces.iter().map(|c| c.bytes.len()).collect::<Vec<_>>(),
        vec![4, 4, 2]
    );
    assert_eq!(
        pieces.iter().map(|c| c.continued).collect::<Vec<_>>(),
        vec![true, true, false],
        "every piece but the last says the block continues"
    );
    let joined: Vec<u8> = pieces.iter().flat_map(|c| c.bytes.to_vec()).collect();
    assert_eq!(joined, block.to_vec());
    assert_eq!(
        Chunk::split(Bytes::new(), 4),
        Vec::<Chunk>::new(),
        "an empty block is no chunk"
    );
}

#[test]
fn engine_error_renders_like_clickhouse() {
    let err = EngineError {
        code: 60,
        name: "UNKNOWN_TABLE".into(),
        message: "Unknown table expression identifier 'nope'".into(),
    };
    assert_eq!(
        err.to_clickhouse_text(),
        "Code: 60. DB::Exception: Unknown table expression identifier 'nope'. (UNKNOWN_TABLE)"
    );
}

#[test]
fn fatal_needs_both_the_code_and_the_text() {
    let fatal = EngineError {
        code: 236,
        name: "ABORTED".into(),
        message: "The server is shutting down due to a fatal error".into(),
    };
    assert!(fatal.is_fatal());
    let other_abort = EngineError {
        message: "Query was aborted".into(),
        ..fatal.clone()
    };
    assert!(!other_abort.is_fatal(), "236 alone is not a crash");
    let faked = EngineError {
        code: 395,
        name: "FUNCTION_THROW_IF_VALUE_IS_NON_ZERO".into(),
        ..fatal
    };
    assert!(!faked.is_fatal(), "the text alone is not a crash");
}
