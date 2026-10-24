//! Deterministic promise IDs (§74 §5.4, D1503).
//!
//! ```text
//! wft:{ns}:{wf}:{run}:{scheduled_event_id}
//! act:{ns}:{wf}:{run}:{scheduled_event_id}:{attempt}
//! tmr:{ns}:{wf}:{run}:{started_event_id}
//! to:{ns}:{wf}:{run}:{event_id}:{kind}        kind ∈ s2s | s2c | sc2c | hb | wft
//! start:{ns}:{schedule_id}:{nominal_time_unix_ms}
//! ```
//!
//! - `{ns}` is `<loams-ns-id>.<temporal-ns-id>` (X1.R9). It, `{wf}`, `{run}`
//!   and `{schedule_id}` are percent-encoded: every byte except ASCII letters,
//!   digits, `-`, `.`, `_` and `~` is written `%XX` (upper-case hex), so `:`
//!   and `%` never appear raw and every ID is ASCII. For the ULIDs and UUIDs
//!   the gateway puts in `{ns}` and `{run}` the encoding is the identity.
//! - Event IDs, attempts and the nominal time are canonical decimal.
//! - **Length.** When the encoded ID is longer than [`MAX_PROMISE_ID_LEN`],
//!   `{wf}` becomes `h` + 32 lower-case hex characters (the first 16 bytes of
//!   SHA-256 of the raw workflow ID) and the raw workflow ID goes in the tag
//!   `loams:wf`. A raw workflow ID whose encoding would itself read as
//!   `h` + 32 hex has its leading `h` written `%68`, so the two forms never
//!   collide and [`PromiseId::parse`] is exact: a hashed ID parses to
//!   [`WorkflowRef::Hash`], which cannot give the raw ID back (read it from
//!   the promise's `loams:wf` tag).
//! - **Tags.** Every promise carries `loams:ns`, `loams:kind` and, except
//!   `start:`, `loams:run`, each with the raw (decoded) value.
//!
//! [`PromiseId::parse`] accepts only the canonical form, so
//! `parse(encode(x)) == x` and `encode(parse(s)) == s` (the `ids_roundtrip`
//! property test).

use std::collections::BTreeMap;
use std::fmt;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};

/// The longest promise ID the gateway creates, in bytes. Resonate's MySQL and
/// TiDB stores hold IDs in `VARCHAR(255)` (31ffb201,
/// `resonate-server-mysql/migrations/0001_initial.sql`); SQLite and Postgres
/// have no limit and the protocol validates none. Taken as 255 until FW7
/// states the profile's value (D1591).
pub const MAX_PROMISE_ID_LEN: usize = 255;

/// Tag: the Loams namespace joined with the Temporal namespace (raw).
pub const TAG_NS: &str = "loams:ns";
/// Tag: the run ID (raw).
pub const TAG_RUN: &str = "loams:run";
/// Tag: `wft`, `act`, `tmr`, `to` or `start`.
pub const TAG_KIND: &str = "loams:kind";
/// Tag: the raw workflow ID, only when the ID carries its hash.
pub const TAG_WF: &str = "loams:wf";

/// Everything but the URI unreserved characters is escaped.
const ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The tags a promise carries.
pub type Tags = BTreeMap<String, String>;

/// Which Temporal timeout a `to:` promise stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TimeoutKind {
    /// Activity schedule-to-start.
    ScheduleToStart,
    /// Activity start-to-close.
    StartToClose,
    /// Activity schedule-to-close.
    ScheduleToClose,
    /// Activity heartbeat.
    Heartbeat,
    /// Workflow task start-to-close.
    WorkflowTask,
}

impl TimeoutKind {
    /// Every kind, in grammar order.
    pub const ALL: [TimeoutKind; 5] = [
        TimeoutKind::ScheduleToStart,
        TimeoutKind::StartToClose,
        TimeoutKind::ScheduleToClose,
        TimeoutKind::Heartbeat,
        TimeoutKind::WorkflowTask,
    ];

    /// The grammar's spelling: `s2s`, `s2c`, `sc2c`, `hb` or `wft`.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeoutKind::ScheduleToStart => "s2s",
            TimeoutKind::StartToClose => "s2c",
            TimeoutKind::ScheduleToClose => "sc2c",
            TimeoutKind::Heartbeat => "hb",
            TimeoutKind::WorkflowTask => "wft",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl fmt::Display for TimeoutKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The workflow-ID segment: the raw ID, or (parsed from a hashed promise ID)
/// the hash that stands for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkflowRef {
    /// The raw Temporal workflow ID.
    Id(String),
    /// The first 16 bytes of SHA-256 of the raw workflow ID. Only
    /// [`PromiseId::parse`] makes this; the raw ID is in the tag `loams:wf`.
    Hash([u8; 16]),
}

impl WorkflowRef {
    /// The hash a raw workflow ID is replaced by past the length limit.
    pub fn hash_of(raw: &str) -> [u8; 16] {
        let digest = Sha256::digest(raw.as_bytes());
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        out
    }

    /// The raw workflow ID, if this is one.
    pub fn as_id(&self) -> Option<&str> {
        match self {
            WorkflowRef::Id(s) => Some(s),
            WorkflowRef::Hash(_) => None,
        }
    }

    /// Whether this names the raw workflow ID `raw` (directly or by hash).
    pub fn matches(&self, raw: &str) -> bool {
        match self {
            WorkflowRef::Id(s) => s == raw,
            WorkflowRef::Hash(h) => *h == Self::hash_of(raw),
        }
    }
}

impl From<&str> for WorkflowRef {
    fn from(s: &str) -> Self {
        WorkflowRef::Id(s.to_string())
    }
}

impl From<String> for WorkflowRef {
    fn from(s: String) -> Self {
        WorkflowRef::Id(s)
    }
}

/// A gateway promise's identity: which event of which run it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PromiseId {
    /// A workflow task, by the event ID of its `WorkflowTaskScheduled`.
    Wft {
        ns: String,
        wf: WorkflowRef,
        run: String,
        scheduled_event_id: i64,
    },
    /// One attempt of an activity task.
    Act {
        ns: String,
        wf: WorkflowRef,
        run: String,
        scheduled_event_id: i64,
        attempt: i32,
    },
    /// A user timer, by the event ID of its `TimerStarted`.
    Tmr {
        ns: String,
        wf: WorkflowRef,
        run: String,
        started_event_id: i64,
    },
    /// One timeout of the event `event_id`.
    To {
        ns: String,
        wf: WorkflowRef,
        run: String,
        event_id: i64,
        kind: TimeoutKind,
    },
    /// One firing of a Temporal schedule (DX4).
    Start {
        ns: String,
        schedule_id: String,
        nominal_time_unix_ms: i64,
    },
}

/// Why a string is not a gateway promise ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("unknown promise-ID prefix in {0:?}")]
    UnknownPrefix(String),
    #[error("{id:?}: expected {expected} ':'-separated segments, found {found}")]
    Segments {
        id: String,
        expected: usize,
        found: usize,
    },
    #[error("{id:?}: segment {segment:?} is not in canonical form")]
    NonCanonical { id: String, segment: String },
    #[error("{id:?}: {segment:?} is not a canonical decimal")]
    Number { id: String, segment: String },
    #[error("{id:?}: unknown timeout kind {kind:?}")]
    TimeoutKind { id: String, kind: String },
    #[error(
        "promise ID is {len} bytes, past the limit of {limit} even with the workflow ID hashed"
    )]
    TooLong { len: usize, limit: usize },
}

fn enc(raw: &str) -> String {
    utf8_percent_encode(raw, ENCODE).to_string()
}

fn is_hash_form(s: &str) -> bool {
    s.len() == 33
        && s.starts_with('h')
        && s[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The raw workflow ID's segment: percent-encoded, with a leading `h`
/// escaped when the result would read as a hash.
fn enc_wf(raw: &str) -> String {
    let e = enc(raw);
    if is_hash_form(&e) {
        format!("%68{}", &e[1..])
    } else {
        e
    }
}

fn hash_segment(h: &[u8; 16]) -> String {
    format!("h{}", hex::encode(h))
}

fn dec(id: &str, seg: &str) -> Result<String, IdError> {
    let non_canonical = || IdError::NonCanonical {
        id: id.to_string(),
        segment: seg.to_string(),
    };
    let raw = percent_decode_str(seg)
        .decode_utf8()
        .map_err(|_| non_canonical())?
        .into_owned();
    if enc(&raw) != seg {
        return Err(non_canonical());
    }
    Ok(raw)
}

fn dec_wf(id: &str, seg: &str) -> Result<WorkflowRef, IdError> {
    if is_hash_form(seg) {
        let mut out = [0u8; 16];
        hex::decode_to_slice(&seg[1..], &mut out).map_err(|_| IdError::NonCanonical {
            id: id.to_string(),
            segment: seg.to_string(),
        })?;
        return Ok(WorkflowRef::Hash(out));
    }
    let non_canonical = || IdError::NonCanonical {
        id: id.to_string(),
        segment: seg.to_string(),
    };
    let raw = percent_decode_str(seg)
        .decode_utf8()
        .map_err(|_| non_canonical())?
        .into_owned();
    if enc_wf(&raw) != seg {
        return Err(non_canonical());
    }
    Ok(WorkflowRef::Id(raw))
}

fn num<T: std::str::FromStr + ToString>(id: &str, seg: &str) -> Result<T, IdError> {
    let err = || IdError::Number {
        id: id.to_string(),
        segment: seg.to_string(),
    };
    let v: T = seg.parse().map_err(|_| err())?;
    if v.to_string() != seg {
        return Err(err());
    }
    Ok(v)
}

impl PromiseId {
    /// `wft`, `act`, `tmr`, `to` or `start`: the ID's prefix and its
    /// `loams:kind` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            PromiseId::Wft { .. } => "wft",
            PromiseId::Act { .. } => "act",
            PromiseId::Tmr { .. } => "tmr",
            PromiseId::To { .. } => "to",
            PromiseId::Start { .. } => "start",
        }
    }

    /// The `<loams-ns-id>.<temporal-ns-id>` namespace.
    pub fn ns(&self) -> &str {
        match self {
            PromiseId::Wft { ns, .. }
            | PromiseId::Act { ns, .. }
            | PromiseId::Tmr { ns, .. }
            | PromiseId::To { ns, .. }
            | PromiseId::Start { ns, .. } => ns,
        }
    }

    /// The run ID, for every kind but `start`.
    pub fn run(&self) -> Option<&str> {
        match self {
            PromiseId::Wft { run, .. }
            | PromiseId::Act { run, .. }
            | PromiseId::Tmr { run, .. }
            | PromiseId::To { run, .. } => Some(run),
            PromiseId::Start { .. } => None,
        }
    }

    /// The workflow segment, for every kind but `start`.
    pub fn wf(&self) -> Option<&WorkflowRef> {
        match self {
            PromiseId::Wft { wf, .. }
            | PromiseId::Act { wf, .. }
            | PromiseId::Tmr { wf, .. }
            | PromiseId::To { wf, .. } => Some(wf),
            PromiseId::Start { .. } => None,
        }
    }

    /// The ID with `wf` written as `wf_seg`.
    fn render(&self, wf_seg: &str) -> String {
        match self {
            PromiseId::Wft {
                ns,
                run,
                scheduled_event_id,
                ..
            } => format!("wft:{}:{wf_seg}:{}:{scheduled_event_id}", enc(ns), enc(run)),
            PromiseId::Act {
                ns,
                run,
                scheduled_event_id,
                attempt,
                ..
            } => format!(
                "act:{}:{wf_seg}:{}:{scheduled_event_id}:{attempt}",
                enc(ns),
                enc(run)
            ),
            PromiseId::Tmr {
                ns,
                run,
                started_event_id,
                ..
            } => format!("tmr:{}:{wf_seg}:{}:{started_event_id}", enc(ns), enc(run)),
            PromiseId::To {
                ns,
                run,
                event_id,
                kind,
                ..
            } => format!("to:{}:{wf_seg}:{}:{event_id}:{kind}", enc(ns), enc(run)),
            PromiseId::Start {
                ns,
                schedule_id,
                nominal_time_unix_ms,
            } => format!(
                "start:{}:{}:{nominal_time_unix_ms}",
                enc(ns),
                enc(schedule_id)
            ),
        }
    }

    /// The promise ID and the tags it carries (module docs). Falls back to
    /// the hashed workflow segment past [`MAX_PROMISE_ID_LEN`]; an ID still
    /// too long after that (a namespace or run ID of hundreds of bytes) is
    /// returned as is, and [`PromiseId::encode_checked`] refuses it.
    pub fn encode(&self) -> (String, Tags) {
        let mut tags = Tags::new();
        tags.insert(TAG_NS.into(), self.ns().to_string());
        tags.insert(TAG_KIND.into(), self.kind().to_string());
        if let Some(run) = self.run() {
            tags.insert(TAG_RUN.into(), run.to_string());
        }
        let id = match self.wf() {
            None => self.render(""),
            Some(WorkflowRef::Hash(h)) => self.render(&hash_segment(h)),
            Some(WorkflowRef::Id(raw)) => {
                let plain = self.render(&enc_wf(raw));
                if plain.len() <= MAX_PROMISE_ID_LEN {
                    plain
                } else {
                    tags.insert(TAG_WF.into(), raw.clone());
                    self.render(&hash_segment(&WorkflowRef::hash_of(raw)))
                }
            }
        };
        (id, tags)
    }

    /// [`PromiseId::encode`], refusing an ID past the limit even when hashed.
    pub fn encode_checked(&self) -> Result<(String, Tags), IdError> {
        let (id, tags) = self.encode();
        if id.len() > MAX_PROMISE_ID_LEN {
            return Err(IdError::TooLong {
                len: id.len(),
                limit: MAX_PROMISE_ID_LEN,
            });
        }
        Ok((id, tags))
    }

    /// Parse a gateway promise ID in canonical form (module docs).
    pub fn parse(id: &str) -> Result<PromiseId, IdError> {
        let segs: Vec<&str> = id.split(':').collect();
        let want = |n: usize| {
            if segs.len() == n {
                Ok(())
            } else {
                Err(IdError::Segments {
                    id: id.to_string(),
                    expected: n,
                    found: segs.len(),
                })
            }
        };
        match segs[0] {
            "wft" => {
                want(5)?;
                Ok(PromiseId::Wft {
                    ns: dec(id, segs[1])?,
                    wf: dec_wf(id, segs[2])?,
                    run: dec(id, segs[3])?,
                    scheduled_event_id: num(id, segs[4])?,
                })
            }
            "act" => {
                want(6)?;
                Ok(PromiseId::Act {
                    ns: dec(id, segs[1])?,
                    wf: dec_wf(id, segs[2])?,
                    run: dec(id, segs[3])?,
                    scheduled_event_id: num(id, segs[4])?,
                    attempt: num(id, segs[5])?,
                })
            }
            "tmr" => {
                want(5)?;
                Ok(PromiseId::Tmr {
                    ns: dec(id, segs[1])?,
                    wf: dec_wf(id, segs[2])?,
                    run: dec(id, segs[3])?,
                    started_event_id: num(id, segs[4])?,
                })
            }
            "to" => {
                want(6)?;
                let kind = TimeoutKind::parse(segs[5]).ok_or_else(|| IdError::TimeoutKind {
                    id: id.to_string(),
                    kind: segs[5].to_string(),
                })?;
                Ok(PromiseId::To {
                    ns: dec(id, segs[1])?,
                    wf: dec_wf(id, segs[2])?,
                    run: dec(id, segs[3])?,
                    event_id: num(id, segs[4])?,
                    kind,
                })
            }
            "start" => {
                want(4)?;
                Ok(PromiseId::Start {
                    ns: dec(id, segs[1])?,
                    schedule_id: dec(id, segs[2])?,
                    nominal_time_unix_ms: num(id, segs[3])?,
                })
            }
            _ => Err(IdError::UnknownPrefix(id.to_string())),
        }
    }
}

impl fmt::Display for PromiseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode().0)
    }
}

/// The namespace segment: `<loams-ns-id>.<temporal-ns-id>` (X1.R9).
pub fn namespace(loams_ns_id: &str, temporal_ns_id: &str) -> String {
    format!("{loams_ns_id}.{temporal_ns_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_form_escape_keeps_raw_and_hash_apart() {
        let raw = format!("h{}", "0".repeat(32));
        let id = PromiseId::Tmr {
            ns: "n".into(),
            wf: raw.as_str().into(),
            run: "r".into(),
            started_event_id: 1,
        };
        let (s, _) = id.encode();
        assert_eq!(s, format!("tmr:n:%68{}:r:1", "0".repeat(32)));
        assert_eq!(PromiseId::parse(&s).unwrap(), id);
    }
}
