//! Product-demo video recording on top of the existing screencast path.
//!
//! # What this is
//!
//! [`VideoRecorder`] turns the frames the docked panel already receives from
//! `Page.startScreencast` into a single playable video file, so a demo of a
//! Zulip thread or a Plane issue can be handed to someone who does not have
//! Loams installed. It is a **consumer** of that frame stream, not a second
//! source of one: it never launches a browser and never opens a CDP
//! connection of its own.
//!
//! # Encoder: an external `ffmpeg`, and why
//!
//! The alternative was a pure-Rust encoder. It was evaluated and rejected, and
//! the reason is worth writing down because this is the kind of decision that
//! gets re-litigated every few months:
//!
//! - **A pure-Rust path needs three new dependencies, not one.** The frames
//!   arrive as PNG, so something must decode PNG; the video bitstream must be
//!   produced by an encoder; and the result must be wrapped in a container by a
//!   muxer. The only encoder-quality pure-Rust video encoder is `rav1e` (AV1).
//!   That is a heavyweight dependency for what is a side feature, it is slow
//!   on exactly the content a demo shows (flat UI regions, where AV1's motion
//!   search earns nothing), and it still needs a muxer on top.
//! - **`ffmpeg` adds no Rust dependency at all.** Nothing is linked and nothing
//!   is vendored, so `cargo deny` has nothing new to check and the Loams
//!   artefacts contain no encoder code. The `libvpx` / `libx264` licence
//!   questions attach to a *distribution of ffmpeg*, not to this repository:
//!   we invoke a binary, exactly as [`crate::engine`] does with Obscura.
//! - **It is the only option that can be verified on the build machine.**
//!   `ffmpeg` and `ffprobe` are present here, so `tests/recording.rs` opens
//!   the file this crate produces with a real demuxer and asserts on the
//!   stream it reports. A pure-Rust path could only have been asserted against
//!   its own parser, which proves nothing about whether anything else can play
//!   the result.
//!
//! # The tradeoff, stated plainly
//!
//! **`ffmpeg` is a runtime system dependency for recording.** It is an
//! *optional* one, and the optionality is the point: the sidebar browser itself
//! does not need it, so a machine without `ffmpeg` still gets a working panel,
//! and only [`VideoRecorder::start`] is refused — with an error naming the
//! variable to set and saying explicitly that the rest of the feature is fine
//! without it. Resolution follows the pattern the engine already established:
//! [`FFMPEG_BIN_ENV`] wins, otherwise `ffmpeg` is looked up on `PATH`. The
//! encoder the container needs is probed *before* any frame is accepted, so an
//! `ffmpeg` built without `libvpx` fails at start rather than after a
//! two-minute recording.
//!
//! Two smaller costs, both bounded and both paid in the same currency — disk
//! and a pause at the end:
//!
//! - **One PNG per recorded frame on disk**, in a scratch directory beside the
//!   output, for the length of the recording.
//! - **Encoding happens at stop.** That is what buys the next section.
//!
//! # Why finalisation is a separate step, and why that is the load-bearing
//! # choice
//!
//! Recording never pipes frames into a running encoder. It writes them to a
//! scratch directory and builds an ffconcat list whose per-frame `duration`
//! directives come from a **monotonic** clock. Finalisation is one `ffmpeg` run
//! over that complete list, and it is the only thing that ever writes to the
//! output path.
//!
//! The alternative — streaming PNGs into a long-lived `ffmpeg -i pipe:0` — was
//! rejected on the correctness requirement rather than on taste. A streaming
//! encoder is a child process that must be *asked* to finalise. If it is
//! killed — a panic, an abort, a closed laptop lid — the container it was
//! writing has no trailer, and the file cannot be repaired because the index is
//! incomplete. Here the frames survive that, so the same finalisation runs
//! from [`Drop`] and produces the same playable file. One code path finalises,
//! on the graceful path and on the failure path.
//!
//! # Timing comes from `Instant`, never from wall-clock
//!
//! Every timestamp in this module is a delta from [`std::time::Instant`]
//! epochs. The wall clock is never read: this module contains no reference to
//! [`std::time::SystemTime`] outside a comment, and
//! `no_wall_clock_is_read_anywhere_in_this_module` asserts that by reading the
//! source rather than trusting a comment.
//!
//! It has to be that way for a reason that is not theoretical. Those deltas
//! become the `duration` directives in the concat list, and those become the
//! container's timeline. An NTP step or a DST change in the middle of a demo
//! would otherwise produce negative or doubled frame lengths and a file that
//! players disagree with each other about the length of.
//!
//! A consequence worth having: because each frame's duration is the *real*
//! elapsed time since the previous one, a pause in the demo is a pause in the
//! video, at true speed, **without any frame being duplicated to fill it**.
//! The engine is activity-driven and emits nothing while the page is idle, so
//! this is the only way the gap survives at all. The one concession is
//! [`RecorderConfig::max_gap`], which clamps a single frame's hold so an
//! accidental stall does not become a frozen minute.
//!
//! # Fidelity limits
//!
//! Stated here so they are not discovered in a demo, and repeated in the crate
//! README and the user-facing wiki page:
//!
//! - **Video only. No audio, ever.** The engine has no audio output and the
//!   screencast carries none. No microphone, no system audio, no narration.
//! - **No editing and no post-processing.** Nothing is trimmed, re-timed,
//!   speed-changed, colour-graded, annotated, watermarked or composited. What
//!   the page rendered is what the file contains.
//! - **The frames are the panel's frames**, so everything the README's
//!   "Fidelity" section says about the panel is true of the recording: no
//!   scroll momentum, no caret, no text selection, coarser-than-60 Hz updates,
//!   and not pixel-identical to Chromium.
//! - **The frame rate is a cap, and excess frames are dropped and counted**
//!   ([`RecordingSummary::dropped`]) rather than queued. A recording of a fast
//!   animation is a recording of sampled frames.
//! - **The encoder's quality settings are this crate's, not the caller's.**
//!   There is deliberately no bitrate knob in the public API; the constants
//!   are in [`Container::output_args`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::Mutex;

use crate::cdp::CdpClient;
use crate::error::{Result, SidebarBrowserError};
use crate::frame::DecodedFrame;
use crate::panel::start_screencast_params;

/// The environment variable that overrides the encoder binary's path.
pub const FFMPEG_BIN_ENV: &str = "LOAMS_FFMPEG_BIN";

/// The default cap on recorded frames per second.
///
/// Ten is chosen for the content rather than for the codec: a docked panel is
/// a few hundred CSS pixels of text, the engine's software rasteriser is the
/// bottleneck long before this is, and a demo reads as smooth at ten. See
/// [`RecorderConfig::with_frame_rate`] for what the number does.
pub const DEFAULT_FRAME_RATE: u32 = 10;

/// The default ceiling on how long one frame may be held.
///
/// Without it, a two-minute pause while the agent thinks becomes a two-minute
/// frozen frame, and a closed laptop lid becomes a two-hour recording. A demo
/// wants its pauses; it does not want its accidents.
pub const DEFAULT_MAX_GAP: Duration = Duration::from_secs(2);

/// How long finalisation may take before the encoder is killed and the attempt
/// is reported as failed.
pub const DEFAULT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(120);

/// The output container, which fixes the extension, the muxer and the encoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Container {
    /// Matroska/WebM with VP9.
    ///
    /// The default for two reasons that are both about *failing safely*. It
    /// plays in every browser without a codec download, and a WebM that an
    /// encoder was killed while writing is still a parseable Matroska — which
    /// matters because [`publish`] is the last line of defence against a
    /// truncated file, and a container that cannot be parsed cannot be
    /// checked.
    #[default]
    WebM,
    /// ISO-BMFF (`.mp4`) with H.264, written `+faststart` so the index is at
    /// the front and the file starts playing before it has finished
    /// downloading.
    Mp4,
}

impl Container {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Container::WebM => "webm",
            Container::Mp4 => "mp4",
        }
    }

    /// The `ffmpeg` muxer name.
    pub fn muxer(self) -> &'static str {
        match self {
            Container::WebM => "webm",
            Container::Mp4 => "mp4",
        }
    }

    /// The encoder name that must appear in `ffmpeg -encoders` output.
    ///
    /// Checked at [`VideoRecorder::start`] so a build without it fails
    /// immediately, listing what it does have, rather than after the caller
    /// has recorded for a minute.
    fn encoder_token(self) -> &'static str {
        match self {
            Container::WebM => "libvpx-vp9",
            Container::Mp4 => "libx264",
        }
    }

    /// The output-side arguments, after the input and filter arguments.
    ///
    /// Public so a reviewer can see the quality settings without reading a
    /// subprocess; read-only by intent, since the alternative is a bitrate
    /// knob that will be set to something producing an unwatchable file.
    pub fn output_args(self) -> Vec<String> {
        let flag = |name: &str, value: &str| vec![name.to_string(), value.to_string()];
        match self {
            Container::WebM => [
                flag("-c:v", "libvpx-vp9"),
                flag("-pix_fmt", "yuv420p"),
                flag("-b:v", "800k"),
                flag("-crf", "32"),
                // Recording is latency-tolerant and the source is screen
                // content, so trading quality-per-cycle for cycles is correct
                // here in a way it would not be for a call.
                flag("-deadline", "realtime"),
                flag("-cpu-used", "6"),
                flag("-row-mt", "1"),
            ]
            .concat(),
            Container::Mp4 => [
                flag("-c:v", "libx264"),
                flag("-pix_fmt", "yuv420p"),
                flag("-preset", "veryfast"),
                flag("-crf", "25"),
                flag("-movflags", "+faststart"),
            ]
            .concat(),
        }
    }
}

impl fmt::Display for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// How to record.
#[derive(Clone, Debug)]
pub struct RecorderConfig {
    output: PathBuf,
    container: Container,
    frame_rate: u32,
    max_gap: Duration,
    ffmpeg: PathBuf,
    finalize_timeout: Duration,
}

impl RecorderConfig {
    /// Record to `output`.
    ///
    /// The extension is **not** inferred from `output`; it comes from
    /// [`Container`], which defaults to WebM. Set it with
    /// [`with_container`](Self::with_container) so the file's name says what
    /// is in it — a caller that asked for `.mp4` and got VP9 in a WebM
    /// container would otherwise find out at upload time.
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self {
            output: output.into(),
            container: Container::default(),
            frame_rate: DEFAULT_FRAME_RATE,
            max_gap: DEFAULT_MAX_GAP,
            ffmpeg: resolve_ffmpeg(),
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
        }
    }

    /// Choose the container.
    pub fn with_container(mut self, container: Container) -> Self {
        self.container = container;
        self
    }

    /// Set the frame-rate cap, in frames per second. Must be non-zero.
    ///
    /// This is a **cap on the recording rate**, not a request to resample: a
    /// frame arriving less than one interval after the previous accepted one
    /// is dropped and counted, and the frames that survive keep their real,
    /// monotonic durations.
    pub fn with_frame_rate(mut self, frame_rate: u32) -> Self {
        self.frame_rate = frame_rate;
        self
    }

    /// Set the ceiling on one frame's hold, which bounds a pause.
    pub fn with_max_gap(mut self, max_gap: Duration) -> Self {
        self.max_gap = max_gap;
        self
    }

    /// Use a specific `ffmpeg` binary.
    pub fn with_ffmpeg(mut self, program: impl Into<PathBuf>) -> Self {
        self.ffmpeg = program.into();
        self
    }

    /// Set how long finalisation may take.
    pub fn with_finalize_timeout(mut self, timeout: Duration) -> Self {
        self.finalize_timeout = timeout;
        self
    }

    /// The output path.
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// The container.
    pub fn container(&self) -> Container {
        self.container
    }

    /// The frame-rate cap.
    pub fn frame_rate(&self) -> u32 {
        self.frame_rate
    }

    /// The encoder binary.
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// The minimum interval between two recorded frames.
    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs(1) / self.frame_rate.max(1)
    }

    /// The path the encoder writes before the file is published.
    pub fn partial_path(&self) -> PathBuf {
        let mut name = self
            .output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("recording.{}", self.container.extension()));
        name.push_str(".loams-partial.");
        name.push_str(self.container.extension());
        self.output.with_file_name(name)
    }

    /// Reject a configuration that could not produce a file.
    ///
    /// Called by [`VideoRecorder::start`] rather than by the builder, because
    /// `RecorderConfig` is a plain value and "is this usable" has no answer
    /// until the encoder has been probed.
    pub fn validate(&self) -> Result<()> {
        if self.output.as_os_str().is_empty() {
            return Err(SidebarBrowserError::Rejected(
                "the recording output path is empty".to_string(),
            ));
        }
        if self.output.file_name().is_none() {
            return Err(SidebarBrowserError::Rejected(format!(
                "the recording output path {} has no file name",
                self.output.display()
            )));
        }
        if self.frame_rate == 0 {
            return Err(SidebarBrowserError::Rejected(
                "the frame rate is zero, so no frame would ever be recorded".to_string(),
            ));
        }
        Ok(())
    }
}

/// Locate the encoder binary.
///
/// [`FFMPEG_BIN_ENV`] wins if it is set, so an operator can point at a known
/// build; otherwise `ffmpeg` is looked up on `PATH`. Same shape as
/// [`crate::engine::resolve_program`], and for the same reason: an optional
/// external program is a deployment fact, not something to guess at.
pub fn resolve_ffmpeg() -> PathBuf {
    match std::env::var_os(FFMPEG_BIN_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from("ffmpeg"),
    }
}

/// What [`VideoRecorder::start`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    /// Recording began. The output path is now reserved.
    Started,
    /// A recording was already in progress and was left completely alone.
    ///
    /// A no-op rather than an error, because "start recording" is a thing a UI
    /// button and a keyboard shortcut both ask for and exactly one of them
    /// must win without the other being reported as a failure. It carries the
    /// frames already captured so a caller can tell "already running" from
    /// "started and then got nothing".
    ///
    /// No second scratch directory was created, no second screencast was
    /// started, and the encoder was not invoked. `tests/recording.rs` asserts
    /// all three against a fake CDP endpoint.
    AlreadyRecording {
        /// How many frames the running recording has accepted.
        frames: u64,
    },
}

/// What [`VideoRecorder::write_frame`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDisposition {
    /// The frame was written and will appear in the output.
    Recorded {
        /// The frame's index within the output.
        index: u64,
    },
    /// The frame arrived inside the configured frame interval and was dropped.
    ///
    /// Dropping rather than queueing is the same call
    /// [`crate::panel::PanelState`] makes for the same reason: a recording
    /// that lags is worse than a recording that samples.
    DroppedTooSoon,
    /// Nothing is being recorded.
    ///
    /// Not an error. The panel's frame loop and the recording toggle are
    /// independent, and a frame arriving after the user pressed stop is a
    /// normal thing for there to be.
    NotRecording,
}

/// What one finished recording produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSummary {
    /// The finalised file.
    pub path: PathBuf,
    /// The container it was written in.
    pub container: Container,
    /// Frames written into it.
    pub frames: u64,
    /// Frames dropped by the frame-rate cap.
    pub dropped: u64,
    /// The recorded length, from the monotonic clock.
    pub duration: Duration,
    /// The encoded file's size in bytes.
    pub bytes: u64,
}

impl fmt::Display for RecordingSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} frames ({:.2}s, {} dropped) in {}",
            self.frames,
            self.duration.as_secs_f64(),
            self.dropped,
            self.path.display()
        )
    }
}

/// The recording state machine.
///
/// Two states, and the transition out of the second is the whole design. A
/// recording either **finalises** — which publishes a playable file — or it is
/// **abandoned**, which publishes nothing. There is no third state in which a
/// file exists at the output path but will not play, because the output path
/// is only ever written by [`publish`] and [`publish`] only ever renames a
/// complete, non-empty, fsynced file into it.
#[derive(Debug)]
enum State {
    /// Not recording. The only state in which `start` may transition.
    Idle,
    /// Recording into a scratch directory.
    Recording(Box<Recording>),
}

#[derive(Debug)]
struct Recording {
    frames: FrameStore,
    /// When the recording started. Kept for the summary's duration even when
    /// every frame was dropped, so a caller can report "10s, nothing captured"
    /// rather than "0s".
    epoch: Instant,
    written: u64,
    dropped: u64,
    /// The stream id of the screencast this recorder started, if it started
    /// one. `None` means the recorder is borrowing the panel's screencast and
    /// must not stop it — see [`VideoRecorder::release_screencast`].
    owned_stream: Option<i64>,
}

/// The frames captured so far, and the holds between them.
///
/// Holds are accumulated here rather than derived from file names or file
/// timestamps, because both of those are wall-clock. Each is the
/// [`Instant`] delta since the previous accepted frame, clamped to `max_gap`.
#[derive(Debug)]
struct FrameStore {
    dir: TempDir,
    interval: Duration,
    max_gap: Duration,
    last_at: Option<Instant>,
    total: Duration,
    names: Vec<String>,
    /// Each frame's own hold, parallel to `names`.
    ///
    /// Kept per frame rather than only as a running total, because the
    /// timeline is written back out one directive at a time and a total cannot
    /// be decomposed after the fact.
    holds: Vec<Duration>,
}

impl FrameStore {
    fn new(dir: TempDir, interval: Duration, max_gap: Duration) -> Self {
        Self {
            dir,
            interval,
            max_gap,
            last_at: None,
            total: Duration::ZERO,
            names: Vec::new(),
            holds: Vec::new(),
        }
    }

    /// The hold this frame contributes to the timeline: the time since the
    /// previous one, clamped.
    ///
    /// The clamp is to `[interval, max_gap]`, and the lower bound matters when
    /// a caller configures a frame interval *longer* than the gap ceiling —
    /// `Duration::clamp` panics if min exceeds max, so the two are ordered
    /// here rather than trusted to the caller.
    fn hold_for(&self, at: Instant) -> Duration {
        let ceiling = self.max_gap.max(self.interval);
        match self.last_at {
            // The first frame is shown for one interval. It has no predecessor
            // to measure against, and leaving it instantaneous would make a
            // one-frame recording invisible in a player.
            None => self.interval,
            Some(last) => at
                .saturating_duration_since(last)
                .clamp(self.interval, ceiling),
        }
    }

    /// Whether `at` is far enough after the last accepted frame.
    fn admits(&self, at: Instant) -> bool {
        match self.last_at {
            None => true,
            Some(last) => at.saturating_duration_since(last) >= self.interval,
        }
    }

    async fn write(&mut self, at: Instant, bytes: &[u8]) -> std::io::Result<u64> {
        let hold = self.hold_for(at);
        self.total += hold;
        self.holds.push(hold);
        let index = self.names.len() as u64;
        let name = format!("{index:08}.png");
        let path = self.dir.path().join(&name);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|error| std::io::Error::other(format!("{}: {error}", path.display())))?;
        self.names.push(name);
        self.last_at = Some(at);
        Ok(index)
    }

    fn frames(&self) -> u64 {
        self.names.len() as u64
    }
}

/// A recorder. Cheap to clone; a clone is another handle on the same
/// recording, which is what makes the idempotent start worth having.
#[derive(Clone, Debug)]
pub struct VideoRecorder {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: RecorderConfig,
    state: Mutex<State>,
}

impl VideoRecorder {
    /// Build a recorder. Nothing is spawned and no file is created; see
    /// [`VideoRecorder::start`].
    pub fn new(config: RecorderConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(State::Idle),
            }),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &RecorderConfig {
        &self.inner.config
    }

    /// Whether a recording is in progress.
    pub async fn is_recording(&self) -> bool {
        matches!(&*self.inner.state.lock().await, State::Recording(_))
    }

    /// How many frames the running recording has accepted.
    pub async fn frames(&self) -> u64 {
        match &*self.inner.state.lock().await {
            State::Recording(recording) => recording.written,
            State::Idle => 0,
        }
    }

    /// Begin recording.
    ///
    /// Idempotent by construction and by test: the state is transitioned under
    /// the same lock `stop` takes, so two callers racing here cannot both see
    /// `Idle`. The second gets [`StartOutcome::AlreadyRecording`] and no
    /// second scratch directory, no second screencast, and no encoder
    /// invocation.
    ///
    /// The encoder is probed here rather than at finalisation so that a
    /// missing binary, or one built without the codec this container needs, is
    /// refused before a single frame is written rather than after the caller
    /// has spent two minutes recording.
    pub async fn start(&self) -> Result<StartOutcome> {
        let mut state = self.inner.state.lock().await;
        if let State::Recording(recording) = &*state {
            return Ok(StartOutcome::AlreadyRecording {
                frames: recording.written,
            });
        }
        self.inner.config.validate()?;
        probe_encoder(&self.inner.config).await?;
        let parent = self
            .inner
            .config
            .output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // The frames go beside the output rather than into the system
        // temporary directory so that publishing is a rename within one
        // filesystem: atomic, with no copy that could be interrupted part way.
        let dir = tempfile::Builder::new()
            .prefix(".loams-recording-frames-")
            .tempdir_in(parent)
            .map_err(|error| {
                SidebarBrowserError::Rejected(format!(
                    "cannot create a recording scratch directory in {}: {error}. The output \
                     directory has to be writable before recording starts, not at the end.",
                    parent.display()
                ))
            })?;
        *state = State::Recording(Box::new(Recording {
            frames: FrameStore::new(
                dir,
                self.inner.config.frame_interval(),
                self.inner.config.max_gap,
            ),
            epoch: Instant::now(),
            written: 0,
            dropped: 0,
            owned_stream: None,
        }));
        Ok(StartOutcome::Started)
    }

    /// Start the screencast that feeds this recording, and record that this
    /// recorder owns it.
    ///
    /// The recorder does not *need* this — the panel already runs a screencast
    /// and [`VideoRecorder::write_frame`] will take its frames regardless —
    /// but when a caller uses it the recorder can honour the cleanup contract
    /// end to end, so [`VideoRecorder::stop_all`] stops a screencast it
    /// started and nothing else. That distinction is the reason it is a
    /// separate method: a recorder that stopped a screencast it did not start
    /// would freeze the live panel on the user's screen.
    ///
    /// Idempotent the way `start` is: called twice, the second call is a
    /// no-op and the first screencast is left alone.
    pub async fn start_screencast(
        &self,
        client: &mut CdpClient,
        session_id: &str,
        width: u32,
        height: u32,
    ) -> Result<i64> {
        let mut state = self.inner.state.lock().await;
        let State::Recording(recording) = &mut *state else {
            return Err(SidebarBrowserError::Rejected(
                "start_screencast needs a recording in progress; call start first".to_string(),
            ));
        };
        if let Some(stream_id) = recording.owned_stream {
            return Ok(stream_id);
        }
        client
            .call(
                "Page.startScreencast",
                start_screencast_params(width, height),
                Some(session_id),
            )
            .await?;
        // Obscura does not return the stream id from `startScreencast`; it
        // puts it on each frame's `sessionId` (`page.rs::queue_screencast_frame`),
        // so the id becomes known from the first frame. `Page.stopScreencast`
        // takes no id, so 0 stands in until then without any risk of being
        // sent to the engine as a real value.
        recording.owned_stream = Some(0);
        Ok(0)
    }

    /// Offer a decoded frame to the running recording.
    ///
    /// The frame's bytes are taken unchanged: the PNG the engine produced is
    /// what gets encoded, so nothing in this crate can lose a pixel between the
    /// raster and the file.
    ///
    /// `at` is the moment the frame arrived and is the only clock consulted.
    /// It is a parameter rather than an internal `Instant::now()` so that a
    /// caller which already stamped the frame does not stamp it twice, and so
    /// a test can drive the cadence deterministically without sleeping.
    pub async fn write_frame(&self, frame: &DecodedFrame, at: Instant) -> Result<FrameDisposition> {
        let mut state = self.inner.state.lock().await;
        let State::Recording(recording) = &mut *state else {
            return Ok(FrameDisposition::NotRecording);
        };
        // Learn the stream id from the first frame, which is where Obscura puts
        // it, replacing the placeholder `start_screencast` left.
        if recording.frames.frames() == 0
            && let Some(existing) = recording.owned_stream
            && existing == 0
        {
            recording.owned_stream = Some(frame.stream_id);
        }
        if !recording.frames.admits(at) {
            recording.dropped += 1;
            return Ok(FrameDisposition::DroppedTooSoon);
        }
        let index = recording
            .frames
            .write(at, &frame.bytes)
            .await
            .map_err(|error| {
                SidebarBrowserError::profile_io(recording.frames.dir.path().to_path_buf(), error)
            })?;
        recording.written += 1;
        Ok(FrameDisposition::Recorded { index })
    }

    /// Finalise the recording and stop.
    ///
    /// `Ok(None)` when nothing was recording: stopping an idle recorder is a
    /// clean no-op, not an error, because the caller cannot always tell
    /// whether the user pressed stop twice or a previous stop already
    /// succeeded.
    ///
    /// With frames, this publishes a playable file. Without frames it removes
    /// the scratch directory and returns [`SidebarBrowserError::Recording`]
    /// naming a file that has **no** output — a zero-frame video is not a
    /// short video, and returning a summary for one would be a claim the file
    /// cannot back up.
    pub async fn stop(&self) -> Result<Option<RecordingSummary>> {
        let recording = self.take_recording().await;
        let Some(recording) = recording else {
            return Ok(None);
        };
        finalize(self.inner.config.clone(), recording)
            .await
            .map(Some)
    }

    /// Stop any screencast this recorder started, then finalise.
    ///
    /// The screencast release cannot fail the stop: a dead engine, a closed
    /// socket, or an engine without `Page.stopScreencast` all leave the caller
    /// wanting the video anyway. The error is logged and finalisation proceeds.
    pub async fn stop_all(
        &self,
        client: &mut CdpClient,
        session_id: &str,
    ) -> Result<Option<RecordingSummary>> {
        self.release_screencast(client, session_id).await;
        self.stop().await
    }

    /// Stop the screencast — but only if this recorder started it.
    ///
    /// The condition is not defensive programming, it is the whole point: the
    /// panel owns a screencast the recorder merely borrows, and stopping it
    /// here would freeze the live panel.
    pub async fn release_screencast(&self, client: &mut CdpClient, session_id: &str) {
        let owned = {
            let mut state = self.inner.state.lock().await;
            match &mut *state {
                State::Recording(recording) => recording.owned_stream.take(),
                State::Idle => None,
            }
        };
        if owned.is_none() {
            return;
        }
        if let Err(error) = client
            .call("Page.stopScreencast", Value::Null, Some(session_id))
            .await
        {
            tracing::warn!(
                error = %error,
                "the recorder could not release the screencast it started; the panel's own \
                 screencast, if any, is unaffected"
            );
        }
    }

    /// Stop recording and discard the frames.
    ///
    /// For the "cancel" action, and for a caller that has decided the demo is
    /// not worth keeping. The scratch directory is still deleted.
    ///
    /// Returns whether there was anything to abandon.
    pub async fn abandon(&self) -> bool {
        self.take_recording().await.is_some()
    }

    async fn take_recording(&self) -> Option<Recording> {
        let mut state = self.inner.state.lock().await;
        match std::mem::replace(&mut *state, State::Idle) {
            State::Idle => None,
            State::Recording(recording) => Some(*recording),
        }
    }

    /// The state lock, for the tests that need to drive the state machine
    /// without an encoder on the machine.
    #[cfg(test)]
    async fn state_for_test(&self) -> tokio::sync::MutexGuard<'_, State> {
        self.inner.state.lock().await
    }
}

impl Drop for Inner {
    /// Finalise on the way out, including after a panic.
    ///
    /// A `Drop` cannot await and cannot report a failure to anyone, so this is
    /// best effort by construction and the documentation says so rather than
    /// implying a guarantee it cannot keep. What it *does* guarantee is that
    /// nothing is left behind: if the frames are unreachable they go with the
    /// scratch directory [`TempDir`] owns, and the output path was never
    /// written, so the worst outcome is a discarded recording rather than a
    /// broken file at a path some later code will try to upload.
    ///
    /// `try_lock`, not `lock`: the only way another task holds this lock while
    /// the last handle drops is that it is mid-write or mid-finalise, and in
    /// both cases that task owns the frames and will publish them itself.
    fn drop(&mut self) {
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        let State::Recording(recording) = std::mem::replace(&mut *state, State::Idle) else {
            return;
        };
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = finalize_blocking(&config, *recording) {
                tracing::warn!(
                    error = %error,
                    "a recording was still in progress when its recorder went away; the frames \
                     have been discarded and nothing was published"
                );
            }
        });
    }
}

/// Finalise: the whole file, or nothing.
async fn finalize(config: RecorderConfig, recording: Recording) -> Result<RecordingSummary> {
    // `spawn_blocking` because the wait is a `try_wait` poll and the writes
    // are ordinary blocking filesystem calls. The child is reaped before this
    // returns on every path, and `recording.frames.dir` drops with it, taking
    // the scratch directory and the frame list with it.
    tokio::task::spawn_blocking(move || finalize_blocking(&config, recording))
        .await
        .map_err(|error| {
            SidebarBrowserError::Recording(format!("the finalisation task did not run: {error}"))
        })?
}

fn finalize_blocking(config: &RecorderConfig, recording: Recording) -> Result<RecordingSummary> {
    let Recording {
        frames,
        epoch,
        written,
        dropped,
        owned_stream: _,
    } = recording;
    if written == 0 {
        // `frames.dir` drops here and takes the scratch directory with it.
        // An error rather than a summary is the honest answer: there is no
        // playable zero-length video, and the output path was never created,
        // so there is nothing half-written for a caller to find.
        tracing::debug!(
            elapsed_ms = epoch.elapsed().as_millis() as u64,
            "a recording captured no frames and published nothing"
        );
        return Err(SidebarBrowserError::Recording(format!(
            "the recording captured no frames in {:.2}s, so there is no video to write. {} was \
             not created.",
            epoch.elapsed().as_secs_f64(),
            config.output.display()
        )));
    }

    let list_path = frames.dir.path().join("frames.ffconcat");
    write_frame_list(&frames, &list_path)?;

    let partial = config.partial_path();
    // Anything already at the partial path is a previous failed attempt's and
    // must not be encoded into this one.
    let _ = std::fs::remove_file(&partial);

    run_encoder(config, &list_path, &partial)?;
    let bytes = publish(&partial, &config.output)?;

    Ok(RecordingSummary {
        path: config.output.clone(),
        container: config.container,
        frames: written,
        dropped,
        duration: frames.total,
        bytes,
    })
}

/// Write the ffconcat list describing the captured frames.
///
/// Each `file` directive carries **that frame's own hold**, taken from the
/// monotonic deltas [`FrameStore`] accumulated, rather than the nominal
/// frame interval. That is what makes a pause in the demo a pause in the
/// video: a nominal interval everywhere would produce a file whose length
/// is `frames / frame_rate` regardless of how long the recording actually
/// took, which is precisely the wall-clock-blindness this module exists to
/// avoid.
///
/// The last file is repeated with no duration directive. That is the
/// ffconcat idiom for holding a still frame: without it the final frame's
/// own hold is dropped, and with a directive on the repeat the demuxer
/// emits a frame at the sum of the holds — which is what we want, since
/// that repeat is the frame a viewer sees last.
///
/// Every `file` entry is a bare name, which the concat demuxer resolves
/// against the list's own directory. The escape is applied anyway: "the
/// scratch directory name is generated" is exactly the kind of assumption
/// that rots when someone changes [`tempfile::Builder::prefix`].
fn write_frame_list(frames: &FrameStore, list_path: &Path) -> Result<()> {
    let mut list = String::from("ffconcat version 1.0\n");
    for (name, hold) in frames.names.iter().zip(&frames.holds) {
        list.push_str(&format!("file '{}'\n", ffconcat_escape(name)));
        list.push_str(&format!("duration {:.6}\n", hold.as_secs_f64()));
    }
    // The demuxer takes the last file's own length rather than its `duration`
    // directive, so the final frame is repeated to give it the last hold. This
    // is the documented way to hold a still frame in ffconcat and it is why
    // `frames.total` and the container's length agree.
    if let Some(last) = frames.names.last() {
        list.push_str(&format!("file '{}'\n", ffconcat_escape(last)));
    }
    std::fs::write(list_path, list).map_err(|error| {
        SidebarBrowserError::Recording(format!(
            "cannot write the recording frame list {}: {error}",
            list_path.display()
        ))
    })
}

/// Escape a filename for an ffconcat `file` directive.
///
/// ffconcat single-quotes the value and defines no escape for a quote, so the
/// only correct encoding closes the quote, emits a backslash-escaped quote,
/// and reopens.
fn ffconcat_escape(name: &str) -> String {
    name.replace('\'', r"'\''")
}

fn run_encoder(config: &RecorderConfig, list_path: &Path, partial: &Path) -> Result<String> {
    let mut command = std::process::Command::new(config.ffmpeg());
    command
        // `-nostdin` stops ffmpeg treating stdin as a keyboard. The input is a
        // file here, so nothing reads the pipe, but a build that disagreed
        // about the flag would otherwise block forever on a read from the
        // null stdin it was given.
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(list_path)
        // yuv420p needs even dimensions and the panel's size need not be even.
        // Rounding down by at most one pixel is the only resampling in the
        // whole path, and it happens in the encoder rather than here, so the
        // PNGs on disk stay byte-identical to what the engine sent.
        .args(["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"])
        // No `-r`. Forcing a constant output rate here would resample the
        // timeline and undo the whole reason for recording holds from the
        // monotonic clock: measured on 2.45s of real holds, `-r 10` produced a
        // 2.80s file, duplicating frames to reach a nominal rate. The concat
        // list's per-frame durations are already the truth, and letting ffmpeg
        // emit them unchanged is what makes a pause in the demo a pause in the
        // video.
        .args(config.container.output_args())
        .args(["-f", config.container.muxer()])
        .arg(partial)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        SidebarBrowserError::Encoder(format!(
            "running {}: {error}. Install ffmpeg, or set {FFMPEG_BIN_ENV} to it. Recording is \
             the only feature that needs it; the sidebar browser does not.",
            config.ffmpeg().display()
        ))
    })?;

    let waited = wait_with_deadline(&mut child, config.finalize_timeout);
    let stderr = read_stderr(&mut child);
    let status = match waited {
        Ok(status) => status,
        Err(error) => {
            // A timed-out encoder is killed and reaped rather than left
            // running: a child that outlives its recording is a leaked
            // process, and the partial is removed either way so nothing
            // unplayable survives at any path.
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(partial);
            return Err(error);
        }
    };
    if !status.success() {
        let _ = std::fs::remove_file(partial);
        return Err(SidebarBrowserError::Encoder(format!(
            "{} exited with {status} while writing {}. Encoder said: {}",
            config.ffmpeg().display(),
            config.output.display(),
            if stderr.trim().is_empty() {
                "<nothing>"
            } else {
                stderr.trim()
            }
        )));
    }
    Ok(stderr)
}

/// Move a fully written file to its final name, durably.
///
/// The order is the point. `fsync` the file so its contents are durable;
/// rename; `fsync` the directory so the rename is durable. A crash at any point
/// leaves either the previous state or the complete file — never a file whose
/// name says "finished" and whose bytes are not.
fn publish(partial: &Path, output: &Path) -> Result<u64> {
    let bytes = std::fs::metadata(partial)
        .map_err(|error| {
            SidebarBrowserError::Recording(format!(
                "the encoder reported success but {} is not there: {error}",
                partial.display()
            ))
        })?
        .len();
    if bytes == 0 {
        let _ = std::fs::remove_file(partial);
        return Err(SidebarBrowserError::Recording(format!(
            "the encoder produced a zero-byte {}; there is nothing playable to publish",
            output.display()
        )));
    }
    sync_file(partial)?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty())
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| SidebarBrowserError::profile_io(parent.to_path_buf(), error))?;
    }
    std::fs::rename(partial, output).map_err(|error| {
        SidebarBrowserError::Recording(format!(
            "cannot move the finished recording {} to {}: {error}. The encoded file is still at \
             the temporary path.",
            partial.display(),
            output.display()
        ))
    })?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    Ok(bytes)
}

fn sync_file(path: &Path) -> Result<()> {
    let file =
        std::fs::File::open(path).map_err(|error| SidebarBrowserError::profile_io(path, error))?;
    file.sync_all()
        .map_err(|error| SidebarBrowserError::profile_io(path.to_path_buf(), error))
}

/// `fsync` a directory so a rename into it is durable.
///
/// Opening a directory for sync is not permitted on every platform, and a
/// recording that has already been written and renamed is not worth failing
/// over: the failure is logged and the publish still stands.
fn sync_dir(path: &Path) -> Result<()> {
    match std::fs::File::open(path) {
        Ok(dir) => {
            if let Err(error) = dir.sync_all() {
                tracing::debug!(
                    path = %path.display(),
                    error = %error,
                    "could not fsync the directory holding the finished recording; the file \
                     itself was fsynced before the rename"
                );
            }
            Ok(())
        }
        Err(error) => {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                "could not open the directory holding the finished recording for fsync"
            );
            Ok(())
        }
    }
}

fn wait_with_deadline(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                return Err(SidebarBrowserError::Encoder(format!(
                    "waiting for the encoder: {error}"
                )));
            }
        }
        if started.elapsed() >= timeout {
            return Err(SidebarBrowserError::Recording(format!(
                "the encoder did not finish within {}s and was killed. Nothing was published, so \
                 no unplayable file was left behind.",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_stderr(child: &mut std::process::Child) -> String {
    use std::io::Read as _;
    let Some(mut pipe) = child.stderr.take() else {
        return String::new();
    };
    let mut buffer = String::new();
    let _ = pipe.read_to_string(&mut buffer);
    buffer
}

/// Check that the configured encoder exists and can produce this container's
/// codec, before any frame is accepted.
async fn probe_encoder(config: &RecorderConfig) -> Result<()> {
    let program = config.ffmpeg().to_path_buf();
    let probed = tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(&program)
            .args(["-nostdin", "-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        (program, output)
    })
    .await
    .map_err(|error| {
        SidebarBrowserError::Encoder(format!("the encoder probe did not run: {error}"))
    })?;
    let (program, output) = probed;
    let output = output.map_err(|error| {
        SidebarBrowserError::Encoder(format!(
            "running {}: {error}. Install ffmpeg, or set {FFMPEG_BIN_ENV} to it. Recording is \
             the only feature that needs it; the sidebar browser itself does not, so the panel \
             works without it.",
            program.display()
        ))
    })?;
    let listing = String::from_utf8_lossy(&output.stdout).into_owned();
    let token = config.container.encoder_token();
    if listing
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(token))
    {
        return Ok(());
    }
    let available: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter(|name| name.starts_with("libvpx") || name.starts_with("libx264"))
        .collect();
    Err(SidebarBrowserError::Encoder(format!(
        "{} cannot encode a {}: it has no {token} encoder. Loams needs VP9 for .webm and H.264 \
         for .mp4. Install an ffmpeg built with them, or set {FFMPEG_BIN_ENV}. Encoders this \
         build has that Loams could use: {}.",
        program.display(),
        config.container.extension(),
        if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &Path) -> RecorderConfig {
        RecorderConfig::new(dir.join("demo.webm"))
    }

    /// A frame the fake engine could have produced, built through the crate's
    /// own decoder so the tests exercise the real byte shape.
    fn sample_frame() -> DecodedFrame {
        use base64::Engine as _;
        crate::frame::decode_frame(
            7,
            &base64::engine::general_purpose::STANDARD.encode(crate::frame::png_fixture(4, 4)),
            crate::frame::FrameFormat::Png,
            0,
        )
        .expect("a decodable frame")
    }

    /// How many scratch directories are in `dir`.
    ///
    /// A leaked scratch directory is the failure this module's design is most
    /// exposed to, so it is checked directly rather than inferred from the
    /// absence of an output file.
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

    /// Put a recording into `Idle`→`Recording` without probing for an
    /// encoder, so the state-machine tests run on a machine that has no ffmpeg.
    ///
    /// The guard is dropped before returning so the caller can go on to use the
    /// public API, which takes the same lock.
    async fn force_recording(recorder: &VideoRecorder, dir: &Path, interval: Duration) {
        let mut state = recorder.state_for_test().await;
        *state = State::Recording(Box::new(Recording {
            frames: FrameStore::new(
                tempfile::Builder::new()
                    .prefix(".loams-recording-frames-")
                    .tempdir_in(dir)
                    .expect("a scratch directory"),
                interval,
                Duration::from_secs(2),
            ),
            epoch: Instant::now(),
            written: 0,
            dropped: 0,
            owned_stream: None,
        }));
    }

    async fn accounting(recorder: &VideoRecorder) -> (u64, u64) {
        let state = recorder.state_for_test().await;
        match &*state {
            State::Recording(recording) => (recording.written, recording.dropped),
            State::Idle => (0, 0),
        }
    }

    #[test]
    fn the_container_sets_the_extension_rather_than_guessing_it() {
        let config = RecorderConfig::new("/tmp/demo.mp4");
        assert_eq!(config.container(), Container::WebM);
        // Deliberately not inferred: a caller who asked for `.mp4` and got VP9
        // in a WebM container would otherwise find out at upload time. The
        // partial name carries both, so it can be mistaken for neither.
        assert_eq!(
            config.partial_path().file_name().unwrap().to_string_lossy(),
            "demo.mp4.loams-partial.webm"
        );
        assert_eq!(
            RecorderConfig::new("/tmp/demo.mp4")
                .with_container(Container::Mp4)
                .partial_path()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "demo.mp4.loams-partial.mp4"
        );
        assert_eq!(Container::WebM.output_args()[1], "libvpx-vp9");
        assert_eq!(Container::Mp4.output_args()[1], "libx264");
    }

    #[test]
    fn an_unusable_configuration_is_refused() {
        assert!(RecorderConfig::new("").validate().is_err());
        assert!(
            RecorderConfig::new("/tmp/demo.webm")
                .with_frame_rate(0)
                .validate()
                .is_err(),
            "a zero frame rate would record nothing, ever"
        );
        assert!(
            RecorderConfig::new("/tmp/demo.webm")
                .with_frame_rate(1)
                .validate()
                .is_ok()
        );
        assert_eq!(
            RecorderConfig::new("/tmp/demo.webm")
                .with_frame_rate(20)
                .frame_interval(),
            Duration::from_millis(50)
        );
    }

    #[tokio::test]
    async fn a_missing_encoder_is_refused_at_start_and_names_the_variable() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder =
            VideoRecorder::new(config_in(dir.path()).with_ffmpeg("/nonexistent/loams-not-ffmpeg"));
        let error = recorder
            .start()
            .await
            .expect_err("no encoder means no recording");
        let text = error.to_string();
        assert!(text.contains(FFMPEG_BIN_ENV), "got: {text}");
        assert!(
            text.contains("the sidebar browser itself does not"),
            "the error must say the rest of the feature works without ffmpeg, got: {text}"
        );
        assert!(
            !recorder.is_recording().await,
            "a refused start must not leave a half-started recording behind"
        );
        assert_eq!(scratch_dirs(dir.path()), 0);
        // And the refusal did not consume the encoder probe twice over: a
        // second attempt still reports the same actionable error rather than
        // something about state.
        assert!(
            recorder
                .start()
                .await
                .expect_err("still no encoder")
                .to_string()
                .contains(FFMPEG_BIN_ENV)
        );
    }

    #[tokio::test]
    async fn a_second_start_changes_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        force_recording(&recorder, dir.path(), Duration::from_millis(100)).await;
        // Pretend some frames are in, and that the recorder owns a screencast.
        {
            let mut state = recorder.state_for_test().await;
            let State::Recording(recording) = &mut *state else {
                panic!("forced");
            };
            recording.written = 4;
            recording.dropped = 1;
            recording.owned_stream = Some(7);
        }

        let outcome = recorder.start().await.expect("an idempotent start");
        assert_eq!(
            outcome,
            StartOutcome::AlreadyRecording { frames: 4 },
            "a second start reports the running recording and replaces nothing"
        );
        assert!(recorder.is_recording().await);
        assert_eq!(
            accounting(&recorder).await,
            (4, 1),
            "the running recording's accounting is untouched"
        );
        {
            let state = recorder.state_for_test().await;
            let State::Recording(recording) = &*state else {
                panic!("still recording");
            };
            assert_eq!(
                recording.owned_stream,
                Some(7),
                "the owned screencast was neither released nor replaced"
            );
        }
        assert_eq!(
            scratch_dirs(dir.path()),
            1,
            "exactly one scratch directory: a second start created no second recorder"
        );
        assert!(
            !dir.path().join("demo.webm").exists(),
            "and it touched no output file"
        );
    }

    #[tokio::test]
    async fn two_clones_racing_to_start_produce_one_recording() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        force_recording(&recorder, dir.path(), Duration::from_millis(100)).await;
        let clone = recorder.clone();
        let third = recorder.clone();
        let (a, b, c) = tokio::join!(recorder.start(), clone.start(), third.start());
        for outcome in [
            a.expect("a start"),
            b.expect("a start"),
            c.expect("a start"),
        ] {
            assert!(
                matches!(outcome, StartOutcome::AlreadyRecording { .. }),
                "every racing start but the first is a no-op, got {outcome:?}"
            );
        }
        assert_eq!(scratch_dirs(dir.path()), 1);
    }

    #[tokio::test]
    async fn stopping_an_idle_recorder_is_a_clean_no_op() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        assert_eq!(recorder.stop().await.expect("no encoder needed"), None);
        assert_eq!(recorder.stop().await.expect("and again"), None);
        assert_eq!(recorder.stop().await.expect("and a third time"), None);
        assert!(!recorder.is_recording().await);
        assert!(!dir.path().join("demo.webm").exists());
        assert_eq!(scratch_dirs(dir.path()), 0);
    }

    #[tokio::test]
    async fn abandoning_publishes_nothing_and_leaves_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        force_recording(&recorder, dir.path(), Duration::from_millis(1)).await;
        // One real frame, so the scratch directory has contents worth deleting.
        recorder
            .write_frame(&sample_frame(), Instant::now())
            .await
            .expect("a frame");
        assert_eq!(scratch_dirs(dir.path()), 1);

        assert!(recorder.abandon().await, "there was something to abandon");
        assert!(!recorder.abandon().await, "and nothing the second time");
        assert!(!recorder.is_recording().await);
        assert!(!dir.path().join("demo.webm").exists());
        assert_eq!(
            scratch_dirs(dir.path()),
            0,
            "the scratch directory must be removed on the abandon path"
        );
    }

    #[tokio::test]
    async fn a_recording_with_no_frames_reports_rather_than_publishing_an_empty_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        force_recording(&recorder, dir.path(), Duration::from_millis(100)).await;

        let error = recorder.stop().await.expect_err("no frames, no video");
        let text = error.to_string();
        assert!(text.contains("no frames"), "got: {text}");
        assert!(
            text.contains("demo.webm"),
            "the error must name the file that does not exist, got: {text}"
        );
        assert!(
            !dir.path().join("demo.webm").exists(),
            "a zero-frame recording must not leave a file a caller would upload"
        );
        assert_eq!(scratch_dirs(dir.path()), 0);
        assert!(
            !recorder.is_recording().await,
            "and the recorder is idle again"
        );
        assert_eq!(
            recorder.stop().await.expect("a later stop is a no-op"),
            None
        );
    }

    /// The cadence is decided entirely from the `Instant`s handed in, so this
    /// asserts the accept rule rather than a race with the scheduler.
    #[tokio::test]
    async fn the_cadence_comes_from_monotonic_timestamps() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()).with_frame_rate(10));
        force_recording(&recorder, dir.path(), Duration::from_millis(100)).await;
        let base = Instant::now();
        for (offset_ms, expected_kept) in [
            (0u64, true),
            (10, false),
            (99, false),
            (100, true),
            (105, false),
            (400, true),
        ] {
            let at = base + Duration::from_millis(offset_ms);
            let disposition = recorder
                .write_frame(&sample_frame(), at)
                .await
                .expect("a frame");
            let kept = matches!(disposition, FrameDisposition::Recorded { .. });
            assert_eq!(
                kept,
                expected_kept,
                "a frame {offset_ms}ms in should {}",
                if expected_kept {
                    "be kept"
                } else {
                    "be dropped"
                }
            );
        }
        assert_eq!(
            accounting(&recorder).await,
            (3, 3),
            "the frames at 0ms, 100ms and 400ms are kept and the other three are dropped"
        );
    }

    /// A frame offered while nothing is recording is not an error: the panel's
    /// frame loop and the recording toggle are independent, and a frame that
    /// arrives after the user pressed stop is a normal thing for there to be.
    #[tokio::test]
    async fn a_frame_offered_while_idle_is_reported_not_refused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        assert_eq!(
            recorder
                .write_frame(&sample_frame(), Instant::now())
                .await
                .expect("not an error"),
            FrameDisposition::NotRecording
        );
    }

    /// Holds come from monotonic deltas, clamped, and the first frame is held
    /// for one interval so a one-frame recording is not invisible.
    #[tokio::test]
    async fn holds_come_from_monotonic_deltas_and_are_clamped() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut store = FrameStore::new(
            tempfile::Builder::new()
                .prefix(".loams-recording-frames-")
                .tempdir_in(dir.path())
                .expect("a scratch directory"),
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        let base = Instant::now();
        let png = crate::frame::png_fixture(2, 2);
        assert_eq!(store.total, Duration::ZERO);
        store.write(base, &png).await.expect("a write");
        assert_eq!(
            store.total,
            Duration::from_millis(100),
            "the first frame is held for one interval, because it has no predecessor to measure"
        );
        // Ten seconds of silence. A demo wants its pauses; it does not want its
        // accidents, so the hold is clamped rather than faithfully enormous.
        store
            .write(base + Duration::from_secs(10), &png)
            .await
            .expect("a write");
        assert_eq!(store.total, Duration::from_millis(600));
        // A 200ms gap is inside `max_gap`, so it is kept exactly: clamping is
        // a ceiling, not a floor that flattens every hold to one interval.
        store
            .write(base + Duration::from_millis(10_200), &png)
            .await
            .expect("a write");
        assert_eq!(store.total, Duration::from_millis(800));
        assert_eq!(store.frames(), 3);
        assert_eq!(
            store.names,
            ["00000000.png", "00000001.png", "00000002.png"]
        );
    }

    /// The list names every frame, gives each a hold, and repeats the last one
    /// — the ffconcat way of making the final frame last as long as the others.
    #[tokio::test]
    async fn the_frame_list_carries_each_frames_own_hold() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut store = FrameStore::new(
            tempfile::Builder::new()
                .prefix(".loams-recording-frames-")
                .tempdir_in(dir.path())
                .expect("a scratch directory"),
            Duration::from_millis(100),
            Duration::from_secs(2),
        );
        let base = Instant::now();
        let png = crate::frame::png_fixture(2, 2);
        // Three real gaps: the 100ms interval for the first frame, then two
        // 300ms holds.
        for step in 0..3u64 {
            store
                .write(base + Duration::from_millis(step * 300), &png)
                .await
                .expect("a write");
        }
        let list_path = dir.path().join("frames.ffconcat");
        write_frame_list(&store, &list_path).expect("a frame list");
        assert_eq!(
            std::fs::read_to_string(&list_path).expect("the list"),
            "ffconcat version 1.0\n\
             file '00000000.png'\nduration 0.100000\n\
             file '00000001.png'\nduration 0.300000\n\
             file '00000002.png'\nduration 0.300000\n\
             file '00000002.png'\n",
            "each directive is that frame's own real hold, not the nominal interval; and the \
             final frame is repeated with no directive, which is the ffconcat idiom for holding \
             a still frame"
        );
        assert_eq!(store.total, Duration::from_millis(700));
    }

    #[test]
    fn ffconcat_escaping_survives_a_quote() {
        assert_eq!(ffconcat_escape("00000001.png"), "00000001.png");
        assert_eq!(ffconcat_escape("it's.png"), r"it'\''s.png");
    }

    #[test]
    fn a_zero_byte_partial_is_never_published() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let partial = dir.path().join("demo.webm.loams-partial.webm");
        let output = dir.path().join("demo.webm");
        std::fs::write(&partial, b"").expect("an empty file");
        let error = publish(&partial, &output).expect_err("an empty file is not a video");
        assert!(error.to_string().contains("zero-byte"), "got: {error}");
        assert!(!partial.exists(), "the partial must be cleaned up");
        assert!(!output.exists(), "and nothing published in its place");
    }

    #[test]
    fn a_missing_partial_is_reported_rather_than_published() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let error = publish(
            &dir.path().join("demo.webm.loams-partial.webm"),
            &dir.path().join("demo.webm"),
        )
        .expect_err("there is nothing to publish");
        assert!(
            error.to_string().contains("reported success"),
            "the error must say the encoder claimed success and the file is absent, got: {error}"
        );
    }

    #[test]
    fn publishing_renames_durably_and_leaves_no_partial() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let partial = dir.path().join("demo.webm.loams-partial.webm");
        let output = dir.path().join("demo.webm");
        std::fs::write(&partial, b"pretend this is a container").expect("a file");
        let bytes = publish(&partial, &output).expect("a published file");
        assert_eq!(bytes, b"pretend this is a container".len() as u64);
        assert!(output.exists());
        assert!(!partial.exists(), "no partial may survive a publish");
    }

    /// The wall clock must never be read in this module.
    ///
    /// A source grep rather than a type error, because `SystemTime::now()`
    /// compiles exactly as happily as `Instant::now()` and the difference only
    /// shows up when a clock step corrupts a finished file.
    #[test]
    fn no_wall_clock_is_read_anywhere_in_this_module() {
        // The source is split at this test so the check cannot match its own
        // text, which is the failure mode a naive grep of the whole file has.
        let source = include_str!("record.rs");
        let checked = match source.find("    fn no_wall_clock_is_read_anywhere_in_this_module") {
            Some(at) => &source[..at],
            None => source,
        };
        let offending: Vec<&str> = checked
            .lines()
            // Doc and line comments may name it — several do, to explain the
            // rule. Only code counts.
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains("SystemTime"))
            .collect();
        assert!(
            offending.is_empty(),
            "record.rs must take every timestamp from Instant, but these lines mention \
             SystemTime: {offending:?}"
        );
    }

    /// The recorded PNGs are byte-identical to what the engine sent, because
    /// nothing between the raster and the encode re-encodes them.
    #[tokio::test]
    async fn a_recorded_frame_is_the_engine_s_bytes() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let recorder = VideoRecorder::new(config_in(dir.path()));
        force_recording(&recorder, dir.path(), Duration::from_millis(1)).await;
        let frame = sample_frame();
        recorder
            .write_frame(&frame, Instant::now())
            .await
            .expect("a frame");
        let scratch = std::fs::read_dir(dir.path())
            .expect("a readable directory")
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".loams-recording-frames-")
            })
            .expect("a scratch directory");
        let stored = std::fs::read(scratch.path().join("00000000.png")).expect("the stored frame");
        assert_eq!(stored, frame.bytes);
    }
}
