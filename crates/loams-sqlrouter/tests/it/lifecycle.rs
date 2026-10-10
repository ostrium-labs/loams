//! The `Lifecycle` machine (plan SQ1 Task 5): its invariants under random
//! runs with crashes, a connection at every point of a suspend, and its
//! traces validated against `spec/tla/router/Lifecycle.tla` with TLC.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use loams_sqlrouter::machine::{Ctx, Machine, Millis};
use loams_sqlrouter::machines::lifecycle::{
    ConnId, Input, Lifecycle, LifecycleConfig, Output, Record, SPEC, State, Step,
};
use loams_sqlrouter::trace::{SpecEvent, SpecValue, VecSink, tla_sequence};
use rand::{RngExt as _, SeedableRng as _};

const SECOND: u64 = 1_000;

fn config() -> LifecycleConfig {
    LifecycleConfig::new(1)
}

fn running() -> Record {
    Record::new(State::Running)
}

/// The machine with a model of the world around it: the runtime's pool,
/// the gate's held connections and sessions, and the durable store. Every
/// output is applied at once (as the host does, one input at a time), and
/// the invariants are checked after each input.
struct World {
    m: Lifecycle,
    config: LifecycleConfig,
    now: u64,
    rng: rand::rngs::ChaCha8Rng,
    sink: VecSink,
    /// The durable record (the last `Persist`).
    store: Record,
    /// Members the runtime runs.
    pool: u32,
    /// The runtime or probe call being executed.
    in_flight: Option<Output>,
    /// Waiting for `Admit` or `Refuse`, with their arrival time.
    waiting: BTreeMap<ConnId, u64>,
    /// Relayed to a member.
    sessions: BTreeSet<ConnId>,
    /// Sessions the gate is asked to close (idle ones, or all).
    closing: Option<Output>,
    refused: BTreeSet<ConnId>,
    admitted_ever: BTreeSet<ConnId>,
    next_conn: u64,
    /// Whether the next probe fails.
    probe_fails: bool,
}

impl World {
    fn new(record: Record, seed: u64) -> Self {
        let config = config();
        let pool = if record.state == State::Running {
            config.replicas
        } else {
            0
        };
        let mut w = World {
            m: Lifecycle::new(config.clone(), record),
            config,
            now: 0,
            rng: rand::rngs::ChaCha8Rng::seed_from_u64(seed),
            sink: VecSink::default(),
            store: record,
            pool,
            in_flight: None,
            waiting: BTreeMap::new(),
            sessions: BTreeSet::new(),
            closing: None,
            refused: BTreeSet::new(),
            admitted_ever: BTreeSet::new(),
            next_conn: 1,
            probe_fails: false,
        };
        w.feed(Input::Start);
        w
    }

    fn feed(&mut self, input: Input) {
        let mut rng = rand::rngs::ChaCha8Rng::seed_from_u64(self.now);
        let mut ctx = Ctx {
            now: Millis(self.now),
            rng: &mut rng,
            trace: &mut self.sink,
        };
        let outputs = self.m.on(&mut ctx, input);
        for o in outputs {
            self.apply(o);
        }
        self.check();
    }

    fn apply(&mut self, o: Output) {
        match o {
            Output::Persist(r) => self.store = r,
            Output::Admit(c) => {
                assert!(self.waiting.remove(&c).is_some(), "admitted {c:?} twice");
                assert!(self.pool > 0, "{c:?} admitted to a stopped pool");
                self.sessions.insert(c);
                self.admitted_ever.insert(c);
            }
            Output::Refuse(c) => {
                assert!(self.waiting.remove(&c).is_some(), "refused {c:?} twice");
                self.refused.insert(c);
            }
            Output::CloseIdle | Output::CloseAll => self.closing = Some(o),
            Output::CancelClose => self.closing = None,
            Output::Scale(_) | Output::Probe => {
                assert!(self.in_flight.is_none(), "two commands in flight");
                self.in_flight = Some(o);
            }
        }
    }

    /// The model's invariants (Lifecycle.tla's, on the world).
    fn check(&self) {
        let r = self.m.record();
        assert_eq!(r, self.store, "the machine's record is persisted");
        if !self.sessions.is_empty() {
            assert!(self.pool > 0, "sessions on a stopped pool");
        }
        if r.state == State::Suspended {
            assert_eq!(self.pool, 0, "suspended with members");
            assert!(self.sessions.is_empty(), "suspended with sessions");
        }
        if r.state == State::Running {
            assert!(self.m.held().next().is_none(), "running and holding");
        }
        assert_eq!(
            self.m.sessions().collect::<BTreeSet<_>>(),
            self.sessions,
            "the machine's sessions are the gate's"
        );
        assert_eq!(
            self.m.held().collect::<BTreeSet<_>>(),
            self.waiting.keys().copied().collect::<BTreeSet<_>>(),
            "the machine holds exactly the waiting connections"
        );
    }

    fn connect(&mut self) -> ConnId {
        let c = ConnId(self.next_conn);
        self.next_conn += 1;
        self.waiting.insert(c, self.now);
        self.feed(Input::Connect(c));
        c
    }

    fn close(&mut self, c: ConnId) {
        if self.sessions.remove(&c) || self.waiting.remove(&c).is_some() {
            self.feed(Input::Closed(c));
        }
    }

    fn advance(&mut self, ms: u64) {
        self.now += ms;
        self.feed(Input::Tick);
    }

    /// Executes the command in flight: its effect, then its result.
    fn complete(&mut self) {
        match self.in_flight.take() {
            Some(Output::Scale(n)) => {
                if n == 0 {
                    assert!(self.sessions.is_empty(), "scaled to zero under sessions");
                }
                self.pool = n;
                self.feed(Input::Scaled { replicas: n });
            }
            Some(Output::Probe) => {
                let ok = self.pool > 0 && !self.probe_fails;
                self.feed(Input::Probed { ok });
            }
            _ => {}
        }
    }

    /// The command fails before its effect.
    fn fail(&mut self) {
        match self.in_flight.take() {
            Some(Output::Scale(_)) => self.feed(Input::ScaleFailed),
            Some(Output::Probe) => self.feed(Input::Probed { ok: false }),
            _ => {}
        }
    }

    /// The saga process restarts from the store: the command in flight
    /// may have taken effect, but its result is lost. The gate's held
    /// connections and sessions survive.
    fn crash(&mut self, effect: bool) {
        if let Some(Output::Scale(n)) = self.in_flight.take()
            && effect
        {
            if n == 0 {
                assert!(self.sessions.is_empty(), "scaled to zero under sessions");
            }
            self.pool = n;
        }
        self.closing = None;
        let held: Vec<(ConnId, Millis)> = self
            .waiting
            .iter()
            .map(|(c, at)| (*c, Millis(*at)))
            .collect();
        self.m = Lifecycle::recover(
            self.config.clone(),
            self.store,
            held,
            self.sessions.iter().copied(),
        );
        self.feed(Input::Start);
    }

    /// The gate closes one session it was asked to close.
    fn gate_closes_one(&mut self) {
        if self.closing.is_some()
            && let Some(c) = self.sessions.iter().next().copied()
        {
            self.close(c);
        }
    }

    fn actions(&self) -> BTreeSet<&'static str> {
        self.sink.0.iter().map(|e| e.action).collect()
    }

    /// Runs commands to completion until nothing is in flight.
    fn settle(&mut self) {
        for _ in 0..50 {
            if self.in_flight.is_none() {
                self.advance(SECOND);
                if self.in_flight.is_none() {
                    return;
                }
            }
            self.complete();
        }
        panic!("the machine does not settle");
    }
}

fn random_step(w: &mut World) {
    let roll = w.rng.random_range(0..100);
    match roll {
        0..=14 => {
            w.connect();
        }
        15..=24 => {
            let all: Vec<ConnId> = w.sessions.iter().chain(w.waiting.keys()).copied().collect();
            if !all.is_empty() {
                let c = all[w.rng.random_range(0..all.len())];
                w.close(c);
            }
        }
        25..=34 => w.feed(Input::Idle),
        35..=49 => {
            let ms = w.rng.random_range(0..40 * SECOND);
            w.advance(ms);
        }
        50..=69 => w.complete(),
        70..=74 => w.fail(),
        75..=79 => {
            let effect = w.rng.random_bool(0.5);
            w.crash(effect);
        }
        80..=89 => w.gate_closes_one(),
        _ => w.probe_fails = w.rng.random_bool(0.3),
    }
}

/// Random runs, crashes included, never put a session on a stopped pool,
/// never strand a held connection, and keep the store in step.
#[test]
fn random_runs_keep_the_invariants() {
    for seed in 0..400 {
        let start = if seed % 2 == 0 {
            running()
        } else {
            Record::new(State::Suspended)
        };
        let mut w = World::new(start, seed);
        for _ in 0..200 {
            random_step(&mut w);
        }
        // Liveness: with no more faults, every held connection is answered
        // within the hold deadline.
        w.probe_fails = false;
        for _ in 0..40 {
            if w.in_flight.is_some() {
                w.complete();
            } else {
                w.advance(SECOND);
            }
        }
        assert!(
            w.waiting.is_empty(),
            "seed {seed}: stranded {:?}",
            w.waiting
        );
    }
}

/// A connection that arrives at any point of a suspend is admitted to a
/// running pool: before the scale-down it aborts the suspend (it wins),
/// after it the suspend finishes and a resume follows (the suspend wins).
#[test]
fn connect_at_every_point_of_a_suspend() {
    // The suspend's points: after Idle (closing), after the kill deadline,
    // after the last session closed (scale-down in flight), after the
    // scale-down's effect, after its result.
    for point in 0..5 {
        let mut w = World::new(running(), point);
        let busy = w.connect();
        w.feed(Input::Idle);
        assert_eq!(w.m.record().state, State::Suspending);
        if point >= 1 {
            w.advance(31 * SECOND);
            assert_eq!(w.m.record().step, Step::Kill);
            assert_eq!(w.closing, Some(Output::CloseAll));
        }
        if point >= 2 {
            w.close(busy);
            assert_eq!(w.in_flight, Some(Output::Scale(0)));
        }
        if point >= 3 {
            // The effect lands; its result is still on its way.
            w.in_flight = None;
            w.pool = 0;
        }
        if point >= 4 {
            w.in_flight = Some(Output::Scale(0));
            w.complete();
            assert_eq!(w.m.record().state, State::Suspended);
        }
        let c = w.connect();
        if point <= 1 {
            assert_eq!(w.m.record().state, State::Running, "point {point}");
            assert!(w.sessions.contains(&c), "the connection wins at {point}");
            assert_eq!(w.closing, None, "closing is cancelled");
            assert!(w.in_flight.is_none(), "no scale-down at {point}");
            continue;
        }
        assert!(w.waiting.contains_key(&c), "held at {point}");
        if point == 3 {
            // The result arrives after the connection.
            w.in_flight = Some(Output::Scale(0));
        }
        w.settle();
        assert_eq!(w.m.record().state, State::Running, "point {point}");
        assert!(
            w.sessions.contains(&c),
            "admitted after the resume at {point}"
        );
        assert_eq!(w.pool, 1);
    }
}

#[test]
fn held_connection_is_refused_after_the_deadline() {
    let mut w = World::new(Record::new(State::Suspended), 1);
    let c = w.connect();
    assert_eq!(
        w.in_flight,
        Some(Output::Scale(1)),
        "a connect wakes the pool"
    );
    w.advance(29 * SECOND);
    assert!(w.waiting.contains_key(&c));
    w.advance(SECOND);
    assert!(w.refused.contains(&c), "1040 after 30 s");
    // The resume goes on without it.
    w.settle();
    assert_eq!(w.m.record().state, State::Running);
}

#[test]
fn failed_commands_are_retried_with_backoff() {
    let mut w = World::new(Record::new(State::Suspended), 2);
    let c = w.connect();
    w.fail();
    assert!(w.in_flight.is_none());
    let retry = w.m.next_deadline().expect("a retry is scheduled");
    assert!(retry.0 > w.now && retry.0 <= w.now + 5 * SECOND);
    w.advance(retry.0 - w.now);
    assert_eq!(w.in_flight, Some(Output::Scale(1)), "retried");
    w.complete();
    w.probe_fails = true;
    w.complete();
    assert_eq!(w.m.record().step, Step::Probe);
    w.probe_fails = false;
    w.settle();
    assert!(w.sessions.contains(&c));
}

#[test]
fn idle_is_ignored_unless_running() {
    let mut w = World::new(Record::new(State::Suspended), 3);
    w.feed(Input::Idle);
    assert_eq!(w.m.record().state, State::Suspended);
    let mut w = World::new(running(), 4);
    w.feed(Input::Idle);
    // No sessions: straight to the scale-down.
    assert_eq!(w.m.record().step, Step::ScaleDown);
    w.feed(Input::Idle);
    assert_eq!(w.m.record().step, Step::ScaleDown);
}

#[test]
fn other_states_refuse_connections() {
    for s in [
        State::Creating,
        State::Branching,
        State::Restoring,
        State::Failed,
        State::Deleting,
    ] {
        let mut w = World::new(Record::new(s), 5);
        let c = w.connect();
        assert!(w.refused.contains(&c), "{s:?}");
    }
}

#[test]
fn records_carry_deterministic_step_keys() {
    let mut w = World::new(running(), 6);
    let epoch = w.m.record().epoch;
    let _busy = w.connect();
    w.feed(Input::Idle);
    let r = w.m.record();
    assert_eq!(r.epoch, epoch + 1);
    assert_eq!(
        r.step_key().as_deref(),
        Some(format!("suspend/{}/quiesce", epoch + 1).as_str())
    );
    assert_eq!(running().step_key(), None);
    // Records round-trip through the store's encoding.
    assert_eq!(Record::decode(&r.encode()).expect("decode"), r);
}

// --- Traces and TLC -------------------------------------------------------

fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/tla/router")
}

/// The sample run: every Lifecycle action, a crash included.
fn sample_trace() -> Vec<SpecEvent> {
    let mut w = World::new(running(), 0);
    w.sink.0.insert(0, init_event(running()));
    let c1 = w.connect();
    let c2 = w.connect();
    w.close(c2);
    // A suspend the next connection aborts.
    w.feed(Input::Idle);
    let c3 = w.connect();
    w.close(c1);
    w.close(c3);
    // A suspend that scales down; a connection held meanwhile; a crash
    // after the scale-down's effect.
    w.feed(Input::Idle);
    let c4 = w.connect();
    w.crash(true);
    w.complete();
    // The resume, with one failed probe.
    w.complete();
    w.probe_fails = true;
    w.complete();
    w.probe_fails = false;
    w.settle();
    assert!(w.sessions.contains(&c4));
    // A suspend that has to kill a busy session.
    w.feed(Input::Idle);
    w.advance(30 * SECOND);
    w.close(c4);
    w.complete();
    assert_eq!(w.m.record().state, State::Suspended);
    // A held connection that times out; the resume finishes anyway.
    let c6 = w.connect();
    w.advance(30 * SECOND);
    assert!(w.refused.contains(&c6));
    w.settle();
    w.sink.0
}

/// The sample with one event no Lifecycle behaviour explains: a session
/// admitted while the pool scales down.
fn mutated(trace: &[SpecEvent]) -> Vec<SpecEvent> {
    let at = trace
        .iter()
        .position(|e| e.action == "ScaleDown")
        .expect("a scale-down");
    let mut bad = trace[at].clone();
    bad.action = "Admit";
    bad.fields[0].1 = SpecValue::Str("c9".into());
    let mut out = trace.to_vec();
    out.insert(at + 1, bad);
    out
}

fn conns(traces: &[&[SpecEvent]]) -> String {
    let mut set = BTreeSet::new();
    for t in traces {
        for e in t.iter() {
            if let Some((_, SpecValue::Str(c))) = e.fields.iter().find(|(k, _)| *k == "conn")
                && !c.is_empty()
            {
                set.insert(format!("{c:?}"));
            }
        }
    }
    format!("{{{}}}", set.into_iter().collect::<Vec<_>>().join(", "))
}

/// The record a run starts from, as the trace's first ("Init") record.
fn init_event(record: Record) -> SpecEvent {
    SpecEvent {
        spec: SPEC,
        action: "Init",
        fields: vec![
            ("conn", "".into()),
            ("state", record.state.name().into()),
            ("step", record.step.name().into()),
        ],
    }
}

fn trace_module(name: &str, header: &str, traces: &[(&str, &[SpecEvent])]) -> String {
    let all: Vec<&[SpecEvent]> = traces.iter().map(|(_, t)| *t).collect();
    let mut out = format!(
        "{:-^77}\n{header}\nEXTENDS LifecycleTrace\n\nTraceConns == {}\n",
        format!(" MODULE {name} "),
        conns(&all)
    );
    for (def, t) in traces {
        out.push_str(&format!("\n{def} ==\n{}\n", tla_sequence(t)));
    }
    out.push_str(&format!("{}\n", "=".repeat(77)));
    out
}

const SAMPLE_HEADER: &str = "(* Generated by crates/loams-sqlrouter/tests/it/lifecycle.rs from the     *)\n\
(* machine's sample run; do not edit. Regenerate with                     *)\n\
(* LOAMS_BLESS=1 cargo test -p loams-sqlrouter --test it lifecycle_trace  *)";

fn tla2tools() -> Option<PathBuf> {
    #[allow(clippy::disallowed_methods, reason = "a test locates the spec tools")]
    let cache = std::env::var_os("LOAMS_SPEC_TOOLS")
        .map(PathBuf::from)
        .or_else(|| {
            #[allow(clippy::disallowed_methods, reason = "a test locates the spec tools")]
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/loam/spec-tools"))
        })?;
    let jar = cache.join("tla2tools-1.7.4.jar");
    let java = Command::new("java").arg("-version").output().is_ok();
    (java && jar.exists()).then_some(jar)
}

/// Runs TLC on `model` with `cfg` in `dir`; returns its output.
fn tlc(jar: &Path, dir: &Path, model: &str, cfg: &str) -> String {
    let meta = dir.join(format!("meta-{cfg}"));
    let out = Command::new("java")
        .arg(format!("-Djava.io.tmpdir={}", dir.display()))
        .args(["-cp"])
        .arg(jar)
        .args(["tlc2.TLC", "-workers", "1", "-deadlock", "-metadir"])
        .arg(&meta)
        .args(["-config", cfg, model])
        .current_dir(dir)
        .output()
        .expect("java");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const TRACE_CFG: &str = "CONSTANTS\n    Trace <- TraceData\n    Conns <- TraceConns\n    Replicas = 1\n    LateAbort = FALSE\nSPECIFICATION TraceSpec\nINVARIANT TypeOK\nPROPERTY TraceMatched\n";

/// The machine's traces are behaviours of Lifecycle.tla (D311): the
/// committed sample (which the `tla` CI job checks too) matches the
/// machine, and TLC explains it and random runs with crashes, and rejects
/// a mutated trace.
#[test]
fn lifecycle_trace_validates() {
    let sample = sample_trace();
    let bad = mutated(&sample);
    let module = trace_module(
        "MCLifecycleTrace",
        SAMPLE_HEADER,
        &[("SampleTrace", &sample), ("MutatedTrace", &bad)],
    );
    let committed = spec_dir().join("MCLifecycleTrace.tla");
    #[allow(clippy::disallowed_methods, reason = "a test's bless switch")]
    let bless = std::env::var("LOAMS_BLESS").is_ok_and(|v| v == "1");
    if bless {
        std::fs::write(&committed, &module).expect("bless");
    }
    assert_eq!(
        std::fs::read_to_string(&committed).unwrap_or_default(),
        module,
        "spec/tla/router/MCLifecycleTrace.tla is stale: rerun with LOAMS_BLESS=1"
    );

    let Some(jar) = tla2tools() else {
        println!("skipped TLC: needs java and tla2tools 1.7.4 (scripts/spec/check.sh fetches it)");
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lifecycle-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("work dir");
    for f in ["Lifecycle.tla", "LifecycleTrace.tla"] {
        std::fs::copy(spec_dir().join(f), dir.join(f)).expect("copy spec");
    }
    let mut runs: Vec<(String, Vec<SpecEvent>, bool)> = vec![
        ("sample".into(), sample.clone(), true),
        ("mutated".into(), bad, false),
    ];
    for seed in [11, 12, 13, 14] {
        let start = if seed % 2 == 0 {
            Record::new(State::Suspended)
        } else {
            running()
        };
        let mut w = World::new(start, seed);
        for _ in 0..150 {
            random_step(&mut w);
        }
        w.sink.0.insert(0, init_event(start));
        runs.push((format!("random{seed}"), w.sink.0, true));
    }
    for (name, events, valid) in runs {
        let model = format!("MCLifecycleTraceRun_{name}");
        std::fs::write(
            dir.join(format!("{model}.tla")),
            trace_module(&model, "", &[("TraceData", &events)]),
        )
        .expect("trace module");
        let cfg = format!("{model}.cfg");
        std::fs::write(dir.join(&cfg), TRACE_CFG).expect("cfg");
        let out = tlc(&jar, &dir, &format!("{model}.tla"), &cfg);
        let ok = out.contains("Model checking completed. No error has been found.");
        if valid {
            assert!(ok, "{name} ({} events) not validated:\n{out}", events.len());
        } else {
            assert!(
                out.contains("Temporal properties were violated"),
                "{name} should fail TraceMatched:\n{out}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every Lifecycle action has an emitter in the machine, and the machine
/// emits no action the spec lacks (§31 §11.3).
#[test]
fn every_spec_action_has_an_emitter() {
    let spec = std::fs::read_to_string(spec_dir().join("Lifecycle.tla")).expect("spec");
    let next = spec
        .split("\nNext ==")
        .nth(1)
        .and_then(|s| s.split("\n\n").next())
        .expect("Next");
    let mut actions = BTreeSet::new();
    for line in next.lines() {
        let body = line.trim().trim_start_matches("\\/").trim();
        let body = body.rsplit(':').next().unwrap_or(body);
        for part in body.split("\\/") {
            let name: String = part
                .trim()
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if !name.is_empty() {
                actions.insert(name);
            }
        }
    }
    let mut w = World::new(running(), 0);
    w.sink.0 = sample_trace();
    w.sink.0.retain(|e| e.action != "Init");
    let emitted: BTreeSet<String> = w.actions().into_iter().map(String::from).collect();
    assert_eq!(emitted, actions, "spec actions vs the machine's events");
    assert!(w.sink.0.iter().all(|e| e.spec == SPEC));
}

/// TLC checks Lifecycle at the PR bounds; the unsafe LateAbort variant
/// must break NoSessionOnStoppedPool (the `tla` CI job runs the same).
#[test]
fn lifecycle_spec_holds_small_bounds() {
    if tla2tools().is_none() {
        println!("skipped: needs java and tla2tools 1.7.4 (scripts/spec/check.sh fetches it)");
        return;
    }
    let check = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/spec/check.sh");
    for cfg in ["MCLifecycle_Small.cfg", "MCLifecycle_LateAbort.cfg"] {
        let out = Command::new(&check)
            .args(["Lifecycle", cfg])
            .output()
            .expect("check.sh");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && text.contains("PASS Lifecycle"),
            "{cfg}: {text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
