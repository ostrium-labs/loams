//! The limits of one function call (LV1 plan Task 3; design §45 §3.2,
//! D682): the CPU meter behind the interrupt handler, and the `console.*`
//! collector.
//!
//! **CPU.** QuickJS calls the runtime's interrupt handler every few
//! thousand bytecode operations. The meter counts the time the interpreter
//! runs for the current call (bundle evaluation or invocation) and excludes
//! the time a host call waits for the store, so a slow store never times a
//! function out. Past the limit the handler returns `true` and QuickJS
//! throws an uncatchable `InternalError: interrupted`. The same handler
//! stops a call that the host aborted (a storage error or a dropped
//! caller).
//!
//! **Console.** At most `lines` lines per call, each cut to `line_bytes`
//! bytes at a character boundary; later lines are dropped and counted.

use std::cell::Cell;
use std::time::{Duration, Instant};

use loams_live::{CallOutput, LogLevel, LogLine};

/// Counts a call's interpreter time against its limit.
#[derive(Debug)]
pub(crate) struct CpuMeter {
    limit: Cell<Duration>,
    used: Cell<Duration>,
    since: Cell<Option<Instant>>,
    timed_out: Cell<bool>,
}

impl CpuMeter {
    pub(crate) fn new() -> Self {
        CpuMeter {
            limit: Cell::new(Duration::MAX),
            used: Cell::new(Duration::ZERO),
            since: Cell::new(None),
            timed_out: Cell::new(false),
        }
    }

    /// Starts metering a new call with `limit`.
    pub(crate) fn start(&self, limit: Duration) {
        self.limit.set(limit);
        self.used.set(Duration::ZERO);
        self.timed_out.set(false);
        self.since.set(Some(Instant::now()));
    }

    /// Stops the clock (a host call waits, or the call ended).
    pub(crate) fn pause(&self) {
        if let Some(since) = self.since.take() {
            self.used
                .set(self.used.get().saturating_add(since.elapsed()));
        }
    }

    /// Restarts the clock after a pause.
    pub(crate) fn resume(&self) {
        if self.since.get().is_none() {
            self.since.set(Some(Instant::now()));
        }
    }

    /// The interpreter time of this call so far.
    pub(crate) fn used(&self) -> Duration {
        let running = self.since.get().map_or(Duration::ZERO, |s| s.elapsed());
        self.used.get().saturating_add(running)
    }

    /// Whether the call is past its limit; remembers it.
    pub(crate) fn check(&self) -> bool {
        if self.timed_out.get() {
            return true;
        }
        if self.used() > self.limit.get() {
            self.timed_out.set(true);
            return true;
        }
        false
    }

    /// Whether the call ran past its limit.
    pub(crate) fn timed_out(&self) -> bool {
        self.timed_out.get()
    }
}

/// Collects one call's `console.*` lines.
#[derive(Debug)]
pub(crate) struct Console {
    lines: usize,
    line_bytes: usize,
    out: CallOutput,
}

impl Console {
    pub(crate) fn new(lines: usize, line_bytes: usize) -> Self {
        Console {
            lines,
            line_bytes,
            out: CallOutput::default(),
        }
    }

    /// Adds a line, or counts it as dropped past the line limit.
    pub(crate) fn push(&mut self, level: LogLevel, line: &str) {
        if self.out.logs.len() >= self.lines {
            self.out.dropped = self.out.dropped.saturating_add(1);
            return;
        }
        let (line, truncated) = cut(line, self.line_bytes);
        self.out.logs.push(LogLine {
            level,
            line: line.to_string(),
            truncated,
        });
    }

    /// The lines so far, leaving the collector empty.
    pub(crate) fn take(&mut self) -> CallOutput {
        std::mem::take(&mut self.out)
    }
}

/// `s` cut to at most `max` bytes at a character boundary, and whether it
/// was cut.
pub(crate) fn cut(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_char_boundaries() {
        assert_eq!(cut("abc", 3), ("abc", false));
        assert_eq!(cut("abcd", 3), ("abc", true));
        // "é" is two bytes: a cut inside it backs off to before it.
        assert_eq!(cut("aé", 2), ("a", true));
    }

    #[test]
    fn console_drops_and_counts_past_the_line_limit() {
        let mut c = Console::new(2, 4);
        c.push(LogLevel::Log, "one");
        c.push(LogLevel::Warn, "twotwo");
        c.push(LogLevel::Log, "three");
        let out = c.take();
        assert_eq!(out.logs.len(), 2);
        assert_eq!(out.logs[1].line, "twot");
        assert!(out.logs[1].truncated);
        assert_eq!(out.dropped, 1);
    }

    #[test]
    fn meter_excludes_paused_time() {
        let m = CpuMeter::new();
        m.start(Duration::from_millis(50));
        m.pause();
        std::thread::sleep(Duration::from_millis(80));
        assert!(!m.check(), "a paused meter does not run");
        m.resume();
        std::thread::sleep(Duration::from_millis(60));
        assert!(m.check());
        assert!(m.timed_out());
    }
}
