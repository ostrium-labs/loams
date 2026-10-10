//! Recording, end to end, against a fake engine and a real encoder.
//!
//! # What is and is not proven here
//!
//! Everything in this file runs against a **fake CDP endpoint** and a
//! **synthetic frame source**, because Obscura 0.2.3 is a 70 MB release binary
//! that is not installed on the build machine. No test here has talked to a
//! real engine, and the tests that need `ffmpeg` say so when it is absent rather
//! than skipping quietly into a green run that proves nothing.
//!
//! What that leaves, stated plainly so nobody has to infer it:
//!
//! - **Proven:** the state machine (double start, stop-when-idle, abandon,
//!   partial startup), the frame-acknowledgement wire contract including
//!   Obscura's integer `sessionId`, the monotonic cadence and the clamping of
//!   long holds, that no scratch directory and no partial file survives any
//!   terminal path, and — where `ffmpeg` and `ffprobe` are installed — that
//!   `ffprobe` opens the file and reports a decodable video stream with the
//!   expected frame count.
//! - **Unverified:** that Obscura 0.2.3 actually emits frames a human would
//!   accept as a demo. Every PNG in this file is a fixture the crate generated
//!   itself. The first run against a real engine is the first time this feature
//!   has been shown to work, and it has to be done by a person.
//!
//! # Why a fake engine at all
//!
//! `tests/wire_contract.rs` already establishes the fake engine's shapes against
//! the pinned source: a flat top-level `sessionId`, an **integer**
//! `sessionId` on `Page.screencastFrame`, and `{id, result}` / `{id, error}`
//! envelopes. This file reuses those shapes rather than inventing a second
//! dialect, so a test that passes here is testing the same wire a real engine
//! speaks.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use loams_sidebar_browser::cdp::CdpClient;
use loams_sidebar_browser::record::{
    FFMPEG_BIN_ENV, FrameDisposition, RecorderConfig, StartOutcome, resolve_ffmpeg,
};
use loams_sidebar_browser::{
    CdpEvent, Container, FrameFormat, FramePump, SidebarBrowserError, VideoRecorder,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const SESSION: &str = "session-1";

// ------------------------------------------------------------- the fake engine

/// A stand-in for the engine's CDP endpoint, which answers commands and
/// records exactly what it was sent.
struct FakeEngine {
    port: u16,
    recorded: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}

type Recorded = std::sync::Arc<std::sync::Mutex<Vec<Value>>>;

impl FakeEngine {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        let recorded: Recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = recorded.clone();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(request): Result<Value, _> = serde_json::from_str(&text) else {
                    continue;
                };
                sink.lock()
                    .expect("the fake engine's record")
                    .push(request.clone());
                // Obscura's envelope, with `sessionId` echoed only where one
                // was sent (`obscura-cdp/src/server.rs`).
                let mut reply = serde_json::Map::new();
                reply.insert("id".to_string(), request["id"].clone());
                reply.insert("result".to_string(), json!({}));
                if let Some(session) = request.get("sessionId") {
                    reply.insert("sessionId".to_string(), session.clone());
                }
                if socket
                    .send(Message::Text(Value::Object(reply).to_string()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { port, recorded }
    }

    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/browser", self.port)
    }

    async fn connect(&self) -> CdpClient {
        CdpClient::connect(&self.url()).await.expect("a connection")
    }

    fn methods(&self) -> Vec<String> {
        self.recorded
            .lock()
            .expect("the record")
            .iter()
            .map(|message| message["method"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn messages(&self) -> Vec<Value> {
        self.recorded.lock().expect("the record").clone()
    }

    fn count_of(&self, method: &str) -> usize {
        self.methods().iter().filter(|seen| *seen == method).count()
    }
}

/// Wait until the fake engine has recorded `count` messages.
///
/// The server task ends when the client drops the socket, so a test cannot await
/// it and read a list: it waits for the count instead.
async fn wait_for(engine: &FakeEngine, count: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let seen = engine.messages();
        if seen.len() >= count || tokio::time::Instant::now() >= deadline {
            assert!(
                seen.len() >= count,
                "the fake engine recorded {} messages, expected {count}: {seen:?}",
                seen.len()
            );
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// --------------------------------------------------------------- the encoder

/// Whether a usable `ffmpeg` and `ffprobe` are installed.
///
/// Resolved once per test binary because the answer cannot change mid-run, and
/// because a `RecorderConfig` takes its binary from the environment.
fn encoder_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let ffmpeg = resolve_ffmpeg();
        probe(&ffmpeg).is_ok() && probe(&ffprobe_binary()).is_ok()
    })
}

fn ffprobe_binary() -> PathBuf {
    // Sits next to the ffmpeg that was resolved, so a test that points
    // `LOAMS_FFMPEG_BIN` at a specific build also finds that build's ffprobe.
    let ffmpeg = resolve_ffmpeg();
    match ffmpeg.parent() {
        Some(dir) => {
            let candidate = dir.join(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            });
            if candidate.exists() {
                candidate
            } else {
                PathBuf::from("ffprobe")
            }
        }
        None => PathBuf::from("ffprobe"),
    }
}

fn probe(program: &Path) -> Result<(), String> {
    let status = std::process::Command::new(program)
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("{program:?}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program:?} exited with {status}"))
    }
}

/// Skip a test with a message that says what is missing, so a green run on a
/// machine without an encoder is never mistaken for proof of playback.
macro_rules! require_encoder {
    () => {
        if !encoder_available() {
            eprintln!(
                "SKIPPED: no usable ffmpeg/ffprobe. Set {FFMPEG_BIN_ENV} to enable this test. \
                 Without it, end-to-end playback of a recorded demo is UNVERIFIED here."
            );
            return;
        }
    };
}

// ------------------------------------------------------------------ fixtures

fn png_bytes(width: u32, height: u32, colour: (u8, u8, u8)) -> Vec<u8> {
    // A real, colour-typed PNG rather than the crate's zero-pixel fixture, so
    // the encoder is compressing something a decoder has to actually inflate.
    // Built with the crate's own chunk writer through a minimal encoder so no
    // image dependency is added.
    crate_png(width, height, colour)
}

fn crate_png(width: u32, height: u32, colour: (u8, u8, u8)) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour RGB
    push_chunk(&mut out, b"IHDR", &ihdr);
    // Raw (filter byte 0 per row) RGB, zlib-wrapped with stored deflate
    // blocks so this needs no compressor.
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for _ in 0..height {
        // The per-row filter byte: 0 means "no filter", so the row is the raw
        // RGB triples below.
        raw.extend_from_slice(&[0]);
        for _ in 0..width {
            raw.extend_from_slice(&[colour.0, colour.1, colour.2]);
        }
    }
    push_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    push_chunk(&mut out, b"IEND", &[]);
    out
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(kind, data).to_be_bytes());
}

/// A zlib stream of stored (uncompressed) deflate blocks.
///
/// Written out rather than pulled from a compression crate: this is a test
/// fixture, and the crate deliberately carries no image or compression
/// dependency for one.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        return out;
    }
    let mut chunks = data.chunks(0xffff).peekable();
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 1 } else { 0 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    // zlib's checksum is the adler-32 as one big-endian `u32`, with the `b`
    // accumulator in the high half.
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut value: u32 = 0xffff_ffff;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            let index = ((value ^ u32::from(*byte)) & 0xff) as usize;
            value = CRC_TABLE[index] ^ (value >> 8);
        }
    };
    feed(kind);
    feed(data);
    value ^ 0xffff_ffff
}

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

/// A `Page.screencastFrame` event in Obscura's shape.
fn screencast_frame(stream_id: i64, png: &[u8]) -> CdpEvent {
    CdpEvent {
        method: "Page.screencastFrame".into(),
        params: json!({
            "data": base64::engine::general_purpose::STANDARD.encode(png),
            "metadata": {
                "offsetTop": 0.0,
                "pageScaleFactor": 1.0,
                "deviceWidth": 320,
                "deviceHeight": 240,
                "scrollOffsetX": 0.0,
                "scrollOffsetY": 0.0,
                "timestamp": 1.0,
            },
            // An **integer**, which is what Obscura puts here and what
            // `Page.screencastFrameAck` takes back. Chromium's CDP uses a
            // string; copying a Chromium example stalls the stream forever.
            "sessionId": stream_id,
        }),
        session_id: Some(SESSION.into()),
    }
}

fn config_in(dir: &Path, container: Container) -> RecorderConfig {
    // The file name follows the container, so the output path and what is
    // inside it agree — which is the whole point of the extension rule the
    // unit tests cover.
    RecorderConfig::new(dir.join(format!("demo.{}", container.extension())))
        .with_container(container)
        // A generous cap so a synthetic frame stream is never throttled by
        // accident: these tests are about wiring and finalisation, and the
        // cadence itself is driven by explicit `Instant`s in the test that
        // cares about it.
        .with_frame_rate(50)
        .with_ffmpeg(resolve_ffmpeg())
}

/// How many scratch directories the recorder has left in `dir`.
fn scratch_dirs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("a readable directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".loams-recording-frames-")
        })
        .count()
}

/// Everything in `dir` that looks like a leftover, for the cleanup assertions.
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("a readable directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with(".loams-recording-frames-") || name.contains("loams-partial")
        })
        .collect()
}

// ------------------------------------------------------- the wiring under test

/// Pump `frames` through a real `FramePump` into a real `VideoRecorder`,
/// acknowledging each one over the wire.
///
/// This is the whole path a demo takes minus the engine: an event arrives, the
/// pump decodes it, the recorder stores it, and the frame is acknowledged with
/// the integer session id. Returns the frames handed to the recorder, so a test
/// can assert on what it fed.
async fn record_through_the_pump(
    engine: &FakeEngine,
    recorder: &VideoRecorder,
    frames: &[(i64, Vec<u8>)],
    spacing: Duration,
) -> Result<(), SidebarBrowserError> {
    let mut client = engine.connect().await;
    let mut pump = FramePump::new(FrameFormat::Png);
    let base = Instant::now();
    for (step, (stream_id, png)) in frames.iter().enumerate() {
        let at = base + spacing * step as u32;
        // Acknowledge before offering, which is the order the panel uses: the
        // ack releases the engine's next frame, and the recorder is a consumer
        // that must not sit between the two.
        let decoded = pump
            .handle(&mut client, &screencast_frame(*stream_id, png))
            .await
            .expect("a decodable frame")
            .expect("a frame");
        recorder.write_frame(&decoded, at).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------- tests

/// A double start is a no-op, and it starts no second screencast.
#[tokio::test]
async fn a_double_start_opens_no_second_screencast_and_no_second_recorder() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let mut client = engine.connect().await;
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM));

    assert_eq!(
        recorder.start().await.expect("a start"),
        StartOutcome::Started
    );
    assert_eq!(
        recorder.start().await.expect("a no-op start"),
        StartOutcome::AlreadyRecording { frames: 0 }
    );
    assert_eq!(
        recorder.start().await.expect("a third start"),
        StartOutcome::AlreadyRecording { frames: 0 }
    );

    recorder
        .start_screencast(&mut client, SESSION, 320, 240)
        .await
        .expect("a screencast");
    recorder
        .start_screencast(&mut client, SESSION, 320, 240)
        .await
        .expect("a second call is a no-op");

    // Exactly one call reaches the wire: the second `start_screencast` was a
    // no-op, so there is nothing else to wait for.
    let seen = wait_for(&engine, 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        engine.count_of("Page.startScreencast"),
        1,
        "a second start must not open a second screencast; the engine saw {:?}",
        engine.methods()
    );
    assert_eq!(
        seen[0]["params"]["format"],
        json!("png"),
        "the recording's screencast is PNG, because that is what the recorder encodes"
    );
    assert_eq!(
        scratch_dirs(dir.path()),
        1,
        "and must not create a second scratch directory"
    );

    // Stopping releases exactly the one screencast it started.
    recorder
        .stop_all(&mut client, SESSION)
        .await
        .expect_err("no frames, so no video")
        .to_string();
    wait_for(&engine, 2).await;
    assert_eq!(
        engine.count_of("Page.stopScreencast"),
        1,
        "exactly the screencast the recorder started is released"
    );
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

/// Stopping when nothing is recording is a clean no-op, and needs no encoder.
#[tokio::test]
async fn stopping_when_idle_is_a_clean_no_op() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let recorder = VideoRecorder::new(
        RecorderConfig::new(dir.path().join("demo.webm"))
            // Deliberately a binary that does not exist: a stop that needs no
            // encoder must not need one.
            .with_ffmpeg("/nonexistent/loams-not-ffmpeg"),
    );
    assert_eq!(recorder.stop().await.expect("a clean no-op"), None);
    assert_eq!(recorder.stop().await.expect("still a no-op"), None);
    assert!(!dir.path().join("demo.webm").exists());
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

/// The failure path: a missing encoder is refused at start, and stopping after
/// the refusal is still a clean no-op with nothing left on disk.
#[tokio::test]
async fn a_failed_start_leaves_nothing_behind_and_still_stops_cleanly() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let recorder = VideoRecorder::new(
        RecorderConfig::new(dir.path().join("demo.webm"))
            .with_ffmpeg("/nonexistent/loams-not-ffmpeg"),
    );
    let error = recorder
        .start()
        .await
        .expect_err("no encoder, no recording");
    assert!(
        matches!(error, SidebarBrowserError::Encoder(_)),
        "got {error:?}"
    );
    assert!(error.to_string().contains(FFMPEG_BIN_ENV), "got: {error}");

    assert!(!recorder.is_recording().await);
    assert_eq!(recorder.stop().await.expect("a clean no-op"), None);
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    assert!(!dir.path().join("demo.webm").exists());

    // And the recorder is reusable: it did not get stuck half-started.
    assert!(
        recorder
            .start()
            .await
            .expect_err("still no encoder")
            .to_string()
            .contains(FFMPEG_BIN_ENV)
    );
}

/// The whole path, and the file a real demuxer reports.
///
/// This is the closest this repository gets to proving playback without the
/// engine binary: real PNG bytes, the crate's own decoder, the real encoder, and
/// then `ffprobe` opening the result and reporting a video stream with the
/// frames that went in.
#[tokio::test]
async fn a_recording_is_a_playable_file_that_a_decoder_can_open() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let output = dir.path().join("demo.webm");
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM));
    recorder.start().await.expect("a start");

    // Four distinct colours, so a decoder that silently dropped frames would
    // report a shorter stream than this.
    let frames: Vec<(i64, Vec<u8>)> = [(220, 40, 40), (40, 200, 60), (50, 70, 220), (230, 200, 40)]
        .into_iter()
        .map(|colour| (11, png_bytes(320, 240, colour)))
        .collect();

    record_through_the_pump(&engine, &recorder, &frames, Duration::from_millis(400))
        .await
        .expect("frames reach the recorder");

    let summary = recorder
        .stop()
        .await
        .expect("a stop")
        .expect("a recording to have happened");
    assert_eq!(summary.frames, 4, "{summary}");
    assert_eq!(summary.dropped, 0, "{summary}");
    assert_eq!(summary.path, output);
    assert_eq!(summary.container, Container::WebM);
    assert!(summary.bytes > 0, "{summary}");
    assert_eq!(
        leftovers(dir.path()),
        Vec::<String>::new(),
        "no scratch directory and no partial may survive a successful stop"
    );

    // What a player would see.
    let probe = ffprobe(&output);
    assert_eq!(probe.codec, "vp9", "ffprobe said: {probe:?}");
    assert_eq!(probe.width, 320, "ffprobe said: {probe:?}");
    assert_eq!(probe.height, 240, "ffprobe said: {probe:?}");
    assert!(
        probe.packets >= 4,
        "the encoder wrote {} packets for 4 input frames; a dropped or \
         duplicated frame shows up here. ffprobe said: {probe:?}",
        probe.packets
    );
    assert!(
        probe.duration_secs > 0.5,
        "the holds came from the monotonic clock, so the file must be longer than the \
         100ms of content it holds; ffprobe said: {probe:?}"
    );
    // The recorded length is the monotonic total: three 400ms gaps plus one
    // 20ms hold for the final frame.
    assert!(
        (probe.duration_secs - 1.22).abs() < 0.2,
        "expected about 1.22s of real holds, ffprobe reported {}",
        probe.duration_secs
    );
}

/// The ack really is an integer, on the recording path as well as the panel's.
///
/// Obscura parses `Page.screencastFrameAck`'s `sessionId` with `screencast_int32`
/// (`obscura-cdp/src/domains/page.rs`) where Chromium's CDP uses a string, and
/// a string here does not error — it silently fails to release the next frame,
/// which looks like a hung panel rather than a bug. So the assertion is on the
/// bytes, not on the absence of an error.
#[tokio::test]
async fn frames_are_acknowledged_with_an_integer_session_id_on_the_recording_path() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM));
    recorder.start().await.expect("a start");

    record_through_the_pump(
        &engine,
        &recorder,
        &[
            (11, png_bytes(64, 64, (10, 20, 30))),
            (12, png_bytes(64, 64, (40, 50, 60))),
        ],
        Duration::from_millis(1500),
    )
    .await
    .expect("frames reach the recorder");

    let seen = wait_for(&engine, 2).await;
    let acks: Vec<&Value> = seen
        .iter()
        .filter(|message| message["method"] == "Page.screencastFrameAck")
        .collect();
    assert_eq!(acks.len(), 2, "every frame is acknowledged: {seen:?}");
    for ack in &acks {
        assert!(
            ack["params"]["sessionId"].is_i64(),
            "Obscura parses this as an i32, so a string would never release the next frame; \
             got {:?}",
            ack["params"]["sessionId"]
        );
    }
    assert_eq!(acks[0]["params"]["sessionId"], json!(11));
    assert_eq!(acks[1]["params"]["sessionId"], json!(12));
    let _ = recorder.stop().await;
}

/// The same file in MP4, so both containers are known to finalise rather than
/// only the default being exercised.
#[tokio::test]
async fn an_mp4_recording_is_also_a_playable_file() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let output = dir.path().join("demo.mp4");
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::Mp4));
    recorder.start().await.expect("a start");
    record_through_the_pump(
        &engine,
        &recorder,
        &[
            (21, png_bytes(160, 120, (200, 30, 30))),
            (21, png_bytes(160, 120, (30, 200, 30))),
        ],
        Duration::from_millis(600),
    )
    .await
    .expect("frames reach the recorder");

    let summary = recorder.stop().await.expect("a stop").expect("a recording");
    assert_eq!(summary.container, Container::Mp4);
    assert_eq!(summary.path, output);
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());

    let probe = ffprobe(&output);
    assert_eq!(probe.codec, "h264", "ffprobe said: {probe:?}");
    assert!(probe.packets >= 2, "ffprobe said: {probe:?}");
    assert!(probe.duration_secs > 0.4, "ffprobe said: {probe:?}");
}

/// A cadence far above the frame rate drops and counts rather than queueing,
/// and the file that comes out is still playable.
#[tokio::test]
async fn a_burst_of_frames_is_sampled_not_queued() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let recorder = VideoRecorder::new(
        config_in(dir.path(), Container::WebM)
            .with_frame_rate(2)
            // A ceiling long enough not to interfere with the drop test.
            .with_max_gap(Duration::from_secs(5)),
    );
    recorder.start().await.expect("a start");

    // Thirty frames 10ms apart, at 2fps: one is kept and twenty-nine dropped.
    let frames: Vec<(i64, Vec<u8>)> = (0..30)
        .map(|index| (31, png_bytes(64, 64, ((index * 8) as u8, 100, 100))))
        .collect();
    record_through_the_pump(&engine, &recorder, &frames, Duration::from_millis(10))
        .await
        .expect("frames reach the recorder");

    let summary = recorder.stop().await.expect("a stop").expect("a recording");
    assert_eq!(summary.frames, 1, "{summary}");
    assert_eq!(summary.dropped, 29, "{summary}");
    let probe = ffprobe(&summary.path);
    assert!(probe.packets >= 1, "ffprobe said: {probe:?}");
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

/// A long silence is clamped, so a stalled agent does not become a frozen
/// minute in the file, and the clamping is visible in the duration.
#[tokio::test]
async fn a_long_silence_is_clamped_in_the_published_file() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let recorder = VideoRecorder::new(
        config_in(dir.path(), Container::WebM)
            // A 10fps interval and a 200ms ceiling, so the ten-second gap is
            // unmistakably clamped rather than merely shortened.
            .with_frame_rate(10)
            .with_max_gap(Duration::from_millis(200)),
    );
    recorder.start().await.expect("a start");
    record_through_the_pump(
        &engine,
        &recorder,
        &[
            (41, png_bytes(64, 64, (10, 10, 200))),
            (41, png_bytes(64, 64, (200, 10, 10))),
        ],
        Duration::from_secs(10),
    )
    .await
    .expect("frames reach the recorder");

    let summary = recorder.stop().await.expect("a stop").expect("a recording");
    assert_eq!(summary.frames, 2, "{summary}");
    assert_eq!(
        summary.duration,
        Duration::from_millis(300),
        "a 100ms interval for the first frame, then the ten-second gap clamped to the 200ms \
         ceiling; both from Instant, not the wall clock"
    );
    let probe = ffprobe(&summary.path);
    assert!(
        probe.duration_secs < 2.0,
        "a ten-second silence must not become a ten-second frozen frame; ffprobe said: {probe:?}"
    );
}

/// Dropping the last handle finalises rather than leaking: the frames are still
/// on disk when the recorder goes away, so the same finalisation runs.
///
/// This is the panic-adjacent path that a streaming encoder cannot serve: a
/// killed encoder leaves an unfinalized container, while these frames are
/// untouched on disk and get encoded normally.
#[tokio::test]
async fn dropping_the_recorder_finalises_and_deletes_nothing() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let output = dir.path().join("demo.webm");
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM));
    recorder.start().await.expect("a start");
    record_through_the_pump(
        &engine,
        &recorder,
        &[
            (51, png_bytes(64, 64, (1, 2, 3))),
            (51, png_bytes(64, 64, (3, 2, 1))),
        ],
        Duration::from_millis(300),
    )
    .await
    .expect("frames reach the recorder");

    drop(recorder);
    // The finalisation is a spawned blocking task, so wait for the file rather
    // than assuming it has landed by the time `drop` returns.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !output.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(output.exists(), "the drop path must still publish a file");

    let probe = ffprobe(&output);
    assert_eq!(probe.codec, "vp9", "ffprobe said: {probe:?}");
    assert!(probe.packets >= 2, "ffprobe said: {probe:?}");
    // The scratch directory goes when the frames do.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !leftovers(dir.path()).is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

/// Abandoning deletes the frames and publishes nothing, even after real frames
/// were written.
#[tokio::test]
async fn abandoning_a_live_recording_publishes_nothing_and_leaves_nothing() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let engine = FakeEngine::start().await;
    let output = dir.path().join("demo.webm");
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM));
    recorder.start().await.expect("a start");
    record_through_the_pump(
        &engine,
        &recorder,
        &[
            (61, png_bytes(64, 64, (9, 9, 9))),
            (61, png_bytes(64, 64, (8, 8, 8))),
            (61, png_bytes(64, 64, (7, 7, 7))),
        ],
        Duration::from_millis(200),
    )
    .await
    .expect("frames reach the recorder");
    assert_eq!(scratch_dirs(dir.path()), 1, "frames were written");

    assert!(recorder.abandon().await);
    assert!(!output.exists(), "an abandoned recording publishes nothing");
    assert_eq!(
        leftovers(dir.path()),
        Vec::<String>::new(),
        "and its frames are deleted rather than left for the next run to trip over"
    );
    assert_eq!(recorder.stop().await.expect("a no-op afterwards"), None);
}

/// The cadence rule, end to end through the public API with explicit
/// `Instant`s, so no assertion here depends on how fast the machine is.
#[tokio::test]
async fn the_cadence_is_monotonic_and_counts_what_it_dropped() {
    require_encoder!();
    let dir = tempfile::tempdir().expect("a temp dir");
    let recorder = VideoRecorder::new(config_in(dir.path(), Container::WebM).with_frame_rate(10));
    recorder.start().await.expect("a start");

    // One frame per accepted slot, with explicit `Instant`s so the cadence
    // assertion below depends on nothing but the timestamps handed in.
    let base = Instant::now();
    let mut kept = Vec::new();
    for (offset_ms, expect_kept) in [
        (0u64, true),
        (5, false),
        (99, false),
        (100, true),
        (250, true),
    ] {
        let at = base + Duration::from_millis(offset_ms);
        let disposition = recorder
            .write_frame(&png_frame(64, 64, (5, 5, 5)), at)
            .await
            .expect("a frame");
        if expect_kept {
            assert!(
                matches!(disposition, FrameDisposition::Recorded { .. }),
                "a frame {offset_ms}ms in should be kept, got {disposition:?}"
            );
            kept.push(offset_ms);
        } else {
            assert_eq!(disposition, FrameDisposition::DroppedTooSoon);
        }
    }
    let summary = recorder.stop().await.expect("a stop").expect("a recording");
    assert_eq!(summary.frames, kept.len() as u64, "{summary}");
    assert_eq!(summary.dropped, 2, "{summary}");
    // Holds are the real gaps between the kept frames, clamped: 100ms, then
    // 150ms, then one interval for the last frame.
    assert_eq!(summary.duration, Duration::from_millis(350), "{summary}");
    let probe = ffprobe(&summary.path);
    assert!(probe.duration_secs > 0.2, "ffprobe said: {probe:?}");
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

// ----------------------------------------------------------------- ffprobe

/// What `ffprobe` reports about a file.
#[derive(Debug)]
struct Probe {
    codec: String,
    width: u32,
    height: u32,
    packets: u64,
    duration_secs: f64,
}

fn ffprobe(path: &Path) -> Probe {
    let output = std::process::Command::new(ffprobe_binary())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_packets",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_packets",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running ffprobe");
    assert!(
        output.status.success(),
        "ffprobe refused to open {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "ffprobe's output was not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let streams = parsed["streams"]
        .as_array()
        .filter(|streams| !streams.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "ffprobe reported no video stream in {}: {parsed}",
                path.display()
            )
        });
    let stream = streams[0].clone();
    Probe {
        codec: stream["codec_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        width: stream["width"].as_u64().unwrap_or_default() as u32,
        height: stream["height"].as_u64().unwrap_or_default() as u32,
        packets: stream["nb_read_packets"]
            .as_str()
            .and_then(|value| value.parse().ok())
            .or_else(|| stream["nb_read_packets"].as_u64())
            .unwrap_or_default(),
        duration_secs: parsed["format"]["duration"]
            .as_str()
            .and_then(|value| value.parse().ok())
            .unwrap_or_default(),
    }
}

/// A decoded frame from a PNG, so `write_frame` gets the crate's own type.
fn png_frame(width: u32, height: u32, colour: (u8, u8, u8)) -> loams_sidebar_browser::DecodedFrame {
    loams_sidebar_browser::decode_frame(
        1,
        &base64::engine::general_purpose::STANDARD.encode(png_bytes(width, height, colour)),
        FrameFormat::Png,
        0,
    )
    .expect("a decodable frame")
}
