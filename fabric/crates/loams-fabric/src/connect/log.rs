//! The role's log: every line, the SDK's and the families' `tracing` events
//! included, goes through the process's [`Scrubber`] before it is written,
//! so no secret an instance resolved reaches stderr (Review Focus 1). A small
//! hand-written subscriber: one line per event, no spans.

use std::fmt::{self, Write as _};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use loams_connector_sdk::Scrubber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Writes scrubbed lines to `out`.
pub struct ScrubbingSubscriber {
    scrubber: Arc<Scrubber>,
    out: Mutex<Box<dyn Write + Send>>,
    max: Level,
    next_id: AtomicU64,
}

impl fmt::Debug for ScrubbingSubscriber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScrubbingSubscriber")
            .field("max", &self.max)
            .finish_non_exhaustive()
    }
}

impl ScrubbingSubscriber {
    pub fn new(scrubber: Arc<Scrubber>, out: Box<dyn Write + Send>, max: Level) -> Self {
        Self {
            scrubber,
            out: Mutex::new(out),
            max,
            next_id: AtomicU64::new(1),
        }
    }

    /// To stderr, at `RUST_LOG`'s level (`error`, `warn`, `info` (default),
    /// `debug`, `trace`; a filter list takes its first bare level).
    pub fn stderr(scrubber: Arc<Scrubber>) -> Self {
        let max = std::env::var("RUST_LOG")
            .ok()
            .and_then(|v| {
                v.split(',')
                    .find(|d| !d.contains('='))
                    .and_then(|d| d.trim().parse::<Level>().ok())
            })
            .unwrap_or(Level::INFO);
        Self::new(scrubber, Box::new(std::io::stderr()), max)
    }
}

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

impl Subscriber for ScrubbingSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.max
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::from_level(self.max))
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        let mut line = Line(format!("{} {}:", meta.level(), meta.target()));
        event.record(&mut line);
        let text = self.scrubber.scrub(&line.0);
        // A secret in an encoded form the plain scrub cannot replace: drop
        // the line's detail rather than write it.
        let text = if self.scrubber.contains(&text) {
            format!(
                "{} {}: <a log line that held a secret was dropped>",
                meta.level(),
                meta.target()
            )
        } else {
            text
        };
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        let _ = writeln!(out, "{text}");
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}
