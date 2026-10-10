//! The serverless lifecycle of one Loams SQL branch (design §47 §14, D733;
//! plan SQ1 Task 5; spec `spec/tla/router/Lifecycle.tla`).
//!
//! ```text
//! RUNNING -> SUSPENDING -> SUSPENDED -> RESUMING -> RUNNING
//! ```
//!
//! - **Suspend** (the idle detector's [`Input::Idle`]): the gate stops
//!   admitting (new connections are held), idle sessions are closed with
//!   1053 ([`Output::CloseIdle`]), the others get 30 s
//!   ([`LifecycleConfig::quiesce_timeout`]) and are then killed
//!   ([`Output::CloseAll`]), and the pool scales to 0.
//! - **Resume** (a connection to a suspended branch): scale the pool to
//!   [`LifecycleConfig::replicas`], probe it (port open, `SELECT 1`), then
//!   admit the held connections. A held connection is refused (1040,
//!   `database is resuming, retry`) after 30 s
//!   ([`LifecycleConfig::hold_timeout`]).
//! - **Concurrent suspend and connect: one wins.** A connection that
//!   arrives before the scale-down is issued aborts the suspend and is
//!   admitted at once; after that the suspend finishes and a resume
//!   follows. No session is ever admitted to a pool being scaled to zero.
//!
//! The machine holds the branch's durable [`Record`] (state, saga step and
//! epoch) and the gate's connections. Its driver (`loams-sqldb`'s
//! lifecycle host) persists every [`Output::Persist`] before executing the
//! other outputs of the same input, runs `Scale` and `Probe` and feeds
//! their results back. After a crash the driver rebuilds the machine with
//! [`Lifecycle::recover`] from the stored record and the gate's surviving
//! connections; [`Input::Start`] then repeats the step in progress. Every
//! step is idempotent, so a repeat is safe.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::machine::{Ctx, Machine, Millis};
use crate::trace::SpecEvent;

/// The TLA+ module this machine implements.
pub const SPEC: &str = "Lifecycle";

/// A branch's state (plan SQ1 "Shared contracts").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    /// Being created (keyspace, bootstrap; Task 9).
    Creating,
    /// Its pool runs; connections are admitted.
    Running,
    /// The suspend saga runs.
    Suspending,
    /// Scaled to zero.
    Suspended,
    /// The resume saga runs.
    Resuming,
    /// Being copied into a new branch (Task 13).
    Branching,
    /// Being restored (Task 16).
    Restoring,
    /// Failed; an operator acts.
    Failed,
    /// Being deleted.
    Deleting,
}

impl State {
    /// The name in the API and in spec events.
    pub fn name(self) -> &'static str {
        match self {
            State::Creating => "CREATING",
            State::Running => "RUNNING",
            State::Suspending => "SUSPENDING",
            State::Suspended => "SUSPENDED",
            State::Resuming => "RESUMING",
            State::Branching => "BRANCHING",
            State::Restoring => "RESTORING",
            State::Failed => "FAILED",
            State::Deleting => "DELETING",
        }
    }
}

/// The step of the saga in progress ([`Step::None`] between sagas).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// No saga runs.
    None,
    /// Suspend: close idle sessions.
    CloseIdle,
    /// Suspend: wait (up to 30 s) for the other sessions to close.
    Quiesce,
    /// Suspend: the remaining sessions are being killed.
    Kill,
    /// Suspend: scale the pool to 0.
    ScaleDown,
    /// Resume: scale the pool up.
    ScaleUp,
    /// Resume: wait for a member's port and `SELECT 1`.
    Probe,
}

impl Step {
    /// The name in spec events and step keys.
    pub fn name(self) -> &'static str {
        match self {
            Step::None => "none",
            Step::CloseIdle => "close_idle",
            Step::Quiesce => "quiesce",
            Step::Kill => "kill",
            Step::ScaleDown => "scale_down",
            Step::ScaleUp => "scale_up",
            Step::Probe => "probe",
        }
    }
}

/// A branch's durable lifecycle record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The state.
    pub state: State,
    /// The saga step in progress.
    pub step: Step,
    /// Bumped by every saga run, so step keys are unique per run.
    pub epoch: u64,
}

/// A record that could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("lifecycle record: {0}")]
pub struct RecordError(String);

impl Record {
    /// A record at rest in `state`, epoch 0.
    pub fn new(state: State) -> Self {
        Self {
            state,
            step: Step::None,
            epoch: 0,
        }
    }

    /// The deterministic id of the step in progress, `<saga>/<epoch>/<step>`
    /// (e.g. `suspend/3/scale_down`): the durable-execution step id and the
    /// idempotency key of the step's effect. `None` between sagas.
    pub fn step_key(&self) -> Option<String> {
        let saga = match self.step {
            Step::None => return None,
            Step::CloseIdle | Step::Quiesce | Step::Kill | Step::ScaleDown => "suspend",
            Step::ScaleUp | Step::Probe => "resume",
        };
        Some(format!("{saga}/{}/{}", self.epoch, self.step.name()))
    }

    /// The stored form (postcard, as the shard-map record).
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap_or_default()
    }

    /// Decodes [`Record::encode`]'s output.
    pub fn decode(bytes: &[u8]) -> Result<Self, RecordError> {
        postcard::from_bytes(bytes).map_err(|e| RecordError(e.to_string()))
    }
}

/// A client connection, as the gate numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u64);

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

/// Timings and the resume size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleConfig {
    /// Members a resume starts (at least 1).
    pub replicas: u32,
    /// How long a connection is held before 1040 (30 s, §47 §12).
    pub hold_timeout: u64,
    /// How long a suspend waits for busy sessions before killing them (30 s).
    pub quiesce_timeout: u64,
    /// The first retry delay of a failed scale or probe; doubled per
    /// failure up to `retry_max`.
    pub retry_min: u64,
    /// The longest retry delay.
    pub retry_max: u64,
}

impl LifecycleConfig {
    /// 30 s hold and quiesce, retries from 200 ms to 5 s.
    pub fn new(replicas: u32) -> Self {
        Self {
            replicas: replicas.max(1),
            hold_timeout: 30_000,
            quiesce_timeout: 30_000,
            retry_min: 200,
            retry_max: 5_000,
        }
    }
}

/// What the driver delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The first input, after [`Lifecycle::new`] or [`Lifecycle::recover`]:
    /// repeats the step in progress.
    Start,
    /// A client authenticated and wants a session (`EnsureRunning`). It
    /// waits for [`Output::Admit`] or [`Output::Refuse`].
    Connect(ConnId),
    /// A session ended, or a waiting client went away.
    Closed(ConnId),
    /// The idle detector: no command for `suspend_after`.
    Idle,
    /// Time passed (deliver at [`Lifecycle::next_deadline`]).
    Tick,
    /// [`Output::Scale`] succeeded.
    Scaled {
        /// The member count asked for.
        replicas: u32,
    },
    /// [`Output::Scale`] failed; it is retried.
    ScaleFailed,
    /// [`Output::Probe`]'s result.
    Probed {
        /// A member answered `SELECT 1`.
        ok: bool,
    },
}

/// What the driver executes, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Store the record before executing the outputs after it. At most one
    /// per input, always first.
    Persist(Record),
    /// Relay this connection to the pool.
    Admit(ConnId),
    /// Answer 1040 `database is resuming, retry` (or `unavailable` in
    /// states the lifecycle does not serve).
    Refuse(ConnId),
    /// Close idle sessions with 1053, and every other session once it is
    /// idle.
    CloseIdle,
    /// Close every session now (1053).
    CloseAll,
    /// A suspend was aborted: sessions stay open.
    CancelClose,
    /// Set the pool's member count; answer with [`Input::Scaled`] or
    /// [`Input::ScaleFailed`].
    Scale(u32),
    /// Wait for a member's port and `SELECT 1`; answer with
    /// [`Input::Probed`].
    Probe,
}

/// A command and its retry state.
#[derive(Clone, Copy, Debug)]
struct Pending {
    command: Output,
    failures: u32,
    /// `None` while the command runs; otherwise when to issue it again.
    retry_at: Option<Millis>,
}

/// The machine; see the module docs.
#[derive(Debug)]
pub struct Lifecycle {
    config: LifecycleConfig,
    record: Record,
    /// Held connections and when they are refused.
    held: BTreeMap<ConnId, Millis>,
    sessions: BTreeSet<ConnId>,
    quiesce_deadline: Option<Millis>,
    pending: Option<Pending>,
    recovered: bool,
    /// The record changed during the current input.
    dirty: bool,
}

impl Lifecycle {
    /// A machine for a branch whose record is `record`, with no
    /// connections.
    pub fn new(config: LifecycleConfig, record: Record) -> Self {
        Self {
            config,
            record,
            held: BTreeMap::new(),
            sessions: BTreeSet::new(),
            quiesce_deadline: None,
            pending: None,
            recovered: false,
            dirty: false,
        }
    }

    /// A machine rebuilt after a crash: the stored record, the connections
    /// the gate still holds (with their arrival times) and its sessions.
    pub fn recover(
        config: LifecycleConfig,
        record: Record,
        held: impl IntoIterator<Item = (ConnId, Millis)>,
        sessions: impl IntoIterator<Item = ConnId>,
    ) -> Self {
        let hold = config.hold_timeout;
        let mut m = Self::new(config, record);
        m.held = held
            .into_iter()
            .map(|(c, at)| (c, at.saturating_add(hold)))
            .collect();
        m.sessions = sessions.into_iter().collect();
        m.recovered = true;
        m
    }

    /// The durable record.
    pub fn record(&self) -> Record {
        self.record
    }

    /// Held connections.
    pub fn held(&self) -> impl Iterator<Item = ConnId> + '_ {
        self.held.keys().copied()
    }

    /// Admitted sessions.
    pub fn sessions(&self) -> impl Iterator<Item = ConnId> + '_ {
        self.sessions.iter().copied()
    }

    /// When the driver must deliver the next [`Input::Tick`].
    pub fn next_deadline(&self) -> Option<Millis> {
        let held = self.held.values().min().copied();
        let retry = self.pending.and_then(|p| p.retry_at);
        [held, retry, self.quiesce_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    fn emit(&self, ctx: &mut Ctx<'_>, action: &'static str, conn: Option<ConnId>) {
        ctx.trace.emit(SpecEvent {
            spec: SPEC,
            action,
            fields: vec![
                (
                    "conn",
                    conn.map(|c| c.to_string()).unwrap_or_default().into(),
                ),
                ("state", self.record.state.name().into()),
                ("step", self.record.step.name().into()),
            ],
        });
    }

    fn set(&mut self, state: State, step: Step) {
        self.record.state = state;
        self.record.step = step;
        self.dirty = true;
    }

    /// Issues `command` unless it is already running.
    fn issue(&mut self, command: Output, out: &mut Vec<Output>) {
        if self
            .pending
            .is_some_and(|p| p.command == command && p.retry_at.is_none())
        {
            return;
        }
        let failures = self
            .pending
            .filter(|p| p.command == command)
            .map_or(0, |p| p.failures);
        self.pending = Some(Pending {
            command,
            failures,
            retry_at: None,
        });
        out.push(command);
    }

    /// Schedules a retry of the failed `command`.
    fn failed(&mut self, ctx: &Ctx<'_>, command: Output) {
        let Some(p) = self.pending.as_mut() else {
            return;
        };
        if p.command != command || p.retry_at.is_some() {
            return;
        }
        let delay = self
            .config
            .retry_min
            .saturating_mul(1 << p.failures.min(16))
            .min(self.config.retry_max);
        p.failures += 1;
        p.retry_at = Some(ctx.now.saturating_add(delay));
    }

    /// Takes the in-flight `command` if it is the one running.
    fn finished(&mut self, command: Output) -> bool {
        let running = self
            .pending
            .is_some_and(|p| p.command == command && p.retry_at.is_none());
        if running {
            self.pending = None;
        }
        running
    }

    /// Runs the current step until it waits for a result, a session or a
    /// deadline.
    fn advance(&mut self, ctx: &mut Ctx<'_>, out: &mut Vec<Output>) {
        loop {
            match self.record.step {
                Step::CloseIdle => {
                    out.push(Output::CloseIdle);
                    self.set(State::Suspending, Step::Quiesce);
                    self.quiesce_deadline =
                        Some(ctx.now.saturating_add(self.config.quiesce_timeout));
                    self.emit(ctx, "CloseIdle", None);
                }
                Step::Quiesce | Step::Kill if self.sessions.is_empty() => {
                    self.quiesce_deadline = None;
                    self.set(State::Suspending, Step::ScaleDown);
                    self.emit(ctx, "Quiesced", None);
                }
                Step::ScaleDown => return self.issue(Output::Scale(0), out),
                Step::ScaleUp => return self.issue(Output::Scale(self.config.replicas), out),
                Step::Probe => return self.issue(Output::Probe, out),
                Step::None | Step::Quiesce | Step::Kill => return,
            }
        }
    }

    fn start(&mut self, ctx: &mut Ctx<'_>, out: &mut Vec<Output>) {
        if self.recovered {
            self.emit(ctx, "Restart", None);
        }
        // Repeat the step in progress (each is idempotent).
        match self.record.step {
            Step::Quiesce => {
                out.push(Output::CloseIdle);
                self.quiesce_deadline = Some(ctx.now.saturating_add(self.config.quiesce_timeout));
            }
            Step::Kill => out.push(Output::CloseAll),
            _ => {}
        }
        self.advance(ctx, out);
        // Connections the gate held across the crash.
        if self.record.state == State::Running {
            self.admit_held(out);
        } else if !self.held.is_empty() {
            self.wake(ctx, out);
        }
    }

    fn admit_held(&mut self, out: &mut Vec<Output>) {
        for c in std::mem::take(&mut self.held).into_keys() {
            self.sessions.insert(c);
            out.push(Output::Admit(c));
        }
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>, c: ConnId, out: &mut Vec<Output>) {
        if self.sessions.contains(&c) || self.held.contains_key(&c) {
            return;
        }
        match self.record.state {
            State::Running => {
                self.sessions.insert(c);
                self.emit(ctx, "Admit", Some(c));
                out.push(Output::Admit(c));
            }
            State::Suspending | State::Suspended | State::Resuming => {
                self.held
                    .insert(c, ctx.now.saturating_add(self.config.hold_timeout));
                self.emit(ctx, "Hold", Some(c));
                self.wake(ctx, out);
            }
            // Outside the lifecycle's scope (Tasks 9, 13, 16).
            State::Creating
            | State::Branching
            | State::Restoring
            | State::Failed
            | State::Deleting => out.push(Output::Refuse(c)),
        }
    }

    /// Held connections want the branch running.
    fn wake(&mut self, ctx: &mut Ctx<'_>, out: &mut Vec<Output>) {
        match (self.record.state, self.record.step) {
            // Before the scale-down the connection wins.
            (State::Suspending, Step::CloseIdle | Step::Quiesce | Step::Kill) => {
                self.quiesce_deadline = None;
                self.set(State::Running, Step::None);
                self.emit(ctx, "AbortSuspend", None);
                out.push(Output::CancelClose);
                self.admit_held(out);
            }
            (State::Suspended, Step::None) => {
                self.record.epoch += 1;
                self.set(State::Resuming, Step::ScaleUp);
                self.emit(ctx, "StartResume", None);
                self.advance(ctx, out);
            }
            _ => {}
        }
    }

    fn closed(&mut self, ctx: &mut Ctx<'_>, c: ConnId, out: &mut Vec<Output>) {
        if self.sessions.remove(&c) {
            self.emit(ctx, "Close", Some(c));
            self.advance(ctx, out);
        } else if self.held.remove(&c).is_some() {
            self.emit(ctx, "Timeout", Some(c));
        }
    }

    fn tick(&mut self, ctx: &mut Ctx<'_>, out: &mut Vec<Output>) {
        let now = ctx.now;
        let expired: Vec<ConnId> = self
            .held
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(c, _)| *c)
            .collect();
        for c in expired {
            self.held.remove(&c);
            self.emit(ctx, "Timeout", Some(c));
            out.push(Output::Refuse(c));
        }
        if self.record.step == Step::Quiesce && self.quiesce_deadline.is_some_and(|d| d <= now) {
            self.quiesce_deadline = None;
            self.set(State::Suspending, Step::Kill);
            self.emit(ctx, "Kill", None);
            out.push(Output::CloseAll);
            self.advance(ctx, out);
        }
        if let Some(p) = self.pending
            && p.retry_at.is_some_and(|at| at <= now)
        {
            self.issue(p.command, out);
        }
    }

    fn scaled(&mut self, ctx: &mut Ctx<'_>, replicas: u32, out: &mut Vec<Output>) {
        if !self.finished(Output::Scale(replicas)) {
            return;
        }
        match self.record.step {
            Step::ScaleDown => {
                self.emit(ctx, "ScaleDown", None);
                self.set(State::Suspended, Step::None);
                self.emit(ctx, "Suspended", None);
                if !self.held.is_empty() {
                    self.wake(ctx, out);
                }
            }
            Step::ScaleUp => {
                self.emit(ctx, "ScaleUp", None);
                self.set(State::Resuming, Step::Probe);
                self.emit(ctx, "ScaledUp", None);
                self.advance(ctx, out);
            }
            _ => {}
        }
    }

    fn probed(&mut self, ctx: &mut Ctx<'_>, ok: bool, out: &mut Vec<Output>) {
        if !ok {
            self.failed(ctx, Output::Probe);
            return;
        }
        if !self.finished(Output::Probe) || self.record.step != Step::Probe {
            return;
        }
        self.set(State::Running, Step::None);
        self.emit(ctx, "Resumed", None);
        self.admit_held(out);
    }
}

impl Machine for Lifecycle {
    type Input = Input;
    type Output = Output;

    fn on(&mut self, ctx: &mut Ctx<'_>, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        self.dirty = false;
        match input {
            Input::Start => self.start(ctx, &mut out),
            Input::Connect(c) => self.connect(ctx, c, &mut out),
            Input::Closed(c) => self.closed(ctx, c, &mut out),
            Input::Idle => {
                if self.record.state == State::Running {
                    self.record.epoch += 1;
                    self.set(State::Suspending, Step::CloseIdle);
                    self.emit(ctx, "StartSuspend", None);
                    self.advance(ctx, &mut out);
                }
            }
            Input::Tick => self.tick(ctx, &mut out),
            Input::Scaled { replicas } => self.scaled(ctx, replicas, &mut out),
            Input::ScaleFailed => {
                if let Some(p) = self.pending
                    && matches!(p.command, Output::Scale(_))
                {
                    self.failed(ctx, p.command);
                }
            }
            Input::Probed { ok } => self.probed(ctx, ok, &mut out),
        }
        if self.dirty {
            out.insert(0, Output::Persist(self.record));
        }
        out
    }
}
