//! Deterministic promise IDs (§74 §5.4, D1503, as amended by X1.R13).
//!
//! ```text
//! origin = <ns>.<wf>.<run>                               no ':'; one per workflow run
//! wft    = <origin>:wft.<scheduled_event_id>
//! act    = <origin>:act.<scheduled_event_id>.<attempt>
//! tmr    = <origin>:tmr.<started_event_id>
//! to     = <origin>:to.<event_id>.<kind>.<attempt>      kind ∈ s2s | s2c | sc2c | hb | wft
//! start  = <ns>.<schedule_id>.<nominal_time_unix_ms>    no ':'
//! sch    = <ns>.<schedule_id>.<generation>:sch.<wake_unix_ms>
//! ```
//!
//! Resonate calls the text before an ID's first `:` its **origin**: on the
//! blob and TiKV stores one origin is one document, rewritten by
//! compare-and-set on every change, and `task.heartbeat` takes tasks of one
//! origin only. So the origin is the workflow run: everything of a run
//! shares one document and nothing else does, and one heartbeat covers a
//! run. Every run-scoped ID has exactly one `:` (the Postgres store refuses
//! more), and `start` has none: it is the ID Resonate's schedule template
//! `<ns>.<schedule_id>.{{.timestamp}}` makes (FW7 profile §8.0).
//!
//! A `sch` ID is one wake-up of a Temporal schedule (D2070): a Resonate timer
//! the gateway arms for the schedule's next action time. Its origin is the
//! schedule's generation (`<ns>.<schedule_id>.<generation>`, a new
//! generation per create), so every timer of one schedule shares one
//! document and a deleted and re-created schedule never sees the old one's
//! timers. The schedule ID is encoded as a segment but never hashed (a
//! schedule ID past the limit is refused by [`PromiseId::encode_checked`]).
//!
//! - `<ns>` is `<loams-ns-id>.<temporal-ns-id>` (X1.R9), first, so a token
//!   prefix can scope a tenant. `<wf>`, `<run>` and `<schedule_id>` are
//!   percent-encoded: every byte except ASCII letters, digits, `-`, `_` and
//!   `~` is written `%XX` (upper-case hex), so `.`, `:` and `%` never appear
//!   raw and every ID is ASCII. `<ns>` is encoded the same way but keeps its
//!   `.`; the origin is split at its last two `.`, so parsing is exact. For
//!   the ULIDs and UUIDs the gateway uses the encoding is the identity.
//! - Event IDs, attempts and the nominal time are canonical decimal. A `to`
//!   ID carries the attempt, so attempt 2's start-to-close timeout is a
//!   different promise from attempt 1's.
//! - **Length.** When the encoded ID is longer than [`MAX_PROMISE_ID_LEN`],
//!   `<wf>` becomes `h` + 32 lower-case hex characters (the first 16 bytes of
//!   SHA-256 of the raw workflow ID) and the raw workflow ID goes in the tag
//!   `loams:wf`. A raw workflow ID whose encoding would itself read as
//!   `h` + 32 hex has its leading `h` written `%68`, so the two forms never
//!   collide and [`PromiseId::parse`] is exact: a hashed ID parses to
//!   [`WorkflowRef::Hash`], which cannot give the raw ID back (read it from
//!   the promise's `loams:wf` tag).
//! - **Tags.** Every promise carries `loams:ns`, `loams:kind` and, except
//!   `start`, `loams:run`, each with the raw (decoded) value.
//!
//! [`PromiseId::parse`] accepts only the canonical form, so
//! `parse(encode(x)) == x` and `encode(parse(s)) == s` (the `ids_roundtrip`
//! property test).

use std::collections::BTreeMap;
use std::fmt;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};

/// The longest promise ID the gateway creates, in bytes: the FW7 profile's
/// limit (MySQL and TiDB hold IDs in `VARCHAR(255)`; the other stores allow
/// more).
pub const MAX_PROMISE_ID_LEN: usize = 255;

/// Tag: the Loams namespace joined with the Temporal namespace (raw).
pub const TAG_NS: &str = "loams:ns";
/// Tag: the run ID (raw).
pub const TAG_RUN: &str = "loams:run";
/// Tag: `wft`, `act`, `tmr`, `to`, `start` or `sch`.
pub const TAG_KIND: &str = "loams:kind";
/// Tag: the raw workflow ID, only when the ID carries its hash.
pub const TAG_WF: &str = "loams:wf";

/// A segment: everything but letters, digits, `-`, `_` and `~` is escaped.
const ENC_SEG: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'~');
/// The namespace: as a segment, but its `.` stays raw.
const ENC_NS: &AsciiSet = &ENC_SEG.remove(b'.');

/// The tags a promise carries.
pub type Tags = BTreeMap<String, String>;

/// Which Temporal timeout a `to` promise stands for.
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
    /// One timeout of one attempt of the event `event_id` (an activity's or
    /// workflow task's scheduled event).
    To {
        ns: String,
        wf: WorkflowRef,
        run: String,
        event_id: i64,
        kind: TimeoutKind,
        attempt: i32,
    },
    /// One firing of a Temporal schedule (DX4).
    Start {
        ns: String,
        schedule_id: String,
        nominal_time_unix_ms: i64,
    },
    /// One wake-up of a Temporal schedule (D2070): a timer at `wake_unix_ms`
    /// for the schedule's `generation`.
    Sch {
        ns: String,
        schedule_id: String,
        generation: String,
        wake_unix_ms: i64,
    },
}

/// Why a string is not a gateway promise ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("{0:?}: more than one ':'")]
    Colons(String),
    #[error("{0:?}: the origin is not <ns>.<wf>.<run> (or <ns>.<schedule>.<time>)")]
    Origin(String),
    #[error("{id:?}: unknown kind {kind:?}")]
    UnknownKind { id: String, kind: String },
    #[error(
        "{id:?}: kind {kind} takes {expected} '.'-separated parts after the ':', found {found}"
    )]
    Parts {
        id: String,
        kind: String,
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
    utf8_percent_encode(raw, ENC_SEG).to_string()
}

fn enc_ns(raw: &str) -> String {
    utf8_percent_encode(raw, ENC_NS).to_string()
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

/// Decode `seg`, accepting it only if `encode` gives it back.
fn dec_with(id: &str, seg: &str, encode: fn(&str) -> String) -> Result<String, IdError> {
    let non_canonical = || IdError::NonCanonical {
        id: id.to_string(),
        segment: seg.to_string(),
    };
    let raw = percent_decode_str(seg)
        .decode_utf8()
        .map_err(|_| non_canonical())?
        .into_owned();
    if encode(&raw) != seg {
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
    dec_with(id, seg, enc_wf).map(WorkflowRef::Id)
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

/// `a.b.c` split at its last two `.`: `(a, b, c)`, where `a` may hold `.`.
fn split3(id: &str, origin: &str) -> Result<(String, String, String), IdError> {
    let mut parts = origin.rsplitn(3, '.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(c), Some(b), Some(a)) => Ok((a.to_string(), b.to_string(), c.to_string())),
        _ => Err(IdError::Origin(id.to_string())),
    }
}

impl PromiseId {
    /// `wft`, `act`, `tmr`, `to` or `start`: the `loams:kind` tag (and,
    /// except `start`, the first word after the `:`).
    pub fn kind(&self) -> &'static str {
        match self {
            PromiseId::Wft { .. } => "wft",
            PromiseId::Act { .. } => "act",
            PromiseId::Tmr { .. } => "tmr",
            PromiseId::To { .. } => "to",
            PromiseId::Start { .. } => "start",
            PromiseId::Sch { .. } => "sch",
        }
    }

    /// The `<loams-ns-id>.<temporal-ns-id>` namespace.
    pub fn ns(&self) -> &str {
        match self {
            PromiseId::Wft { ns, .. }
            | PromiseId::Act { ns, .. }
            | PromiseId::Tmr { ns, .. }
            | PromiseId::To { ns, .. }
            | PromiseId::Start { ns, .. }
            | PromiseId::Sch { ns, .. } => ns,
        }
    }

    /// The run ID, for every kind but `start` and `sch`.
    pub fn run(&self) -> Option<&str> {
        match self {
            PromiseId::Wft { run, .. }
            | PromiseId::Act { run, .. }
            | PromiseId::Tmr { run, .. }
            | PromiseId::To { run, .. } => Some(run),
            PromiseId::Start { .. } | PromiseId::Sch { .. } => None,
        }
    }

    /// The workflow segment, for every kind but `start` and `sch`.
    pub fn wf(&self) -> Option<&WorkflowRef> {
        match self {
            PromiseId::Wft { wf, .. }
            | PromiseId::Act { wf, .. }
            | PromiseId::Tmr { wf, .. }
            | PromiseId::To { wf, .. } => Some(wf),
            PromiseId::Start { .. } | PromiseId::Sch { .. } => None,
        }
    }

    /// The schedule ID, for `start` and `sch`.
    pub fn schedule_id(&self) -> Option<&str> {
        match self {
            PromiseId::Start { schedule_id, .. } | PromiseId::Sch { schedule_id, .. } => {
                Some(schedule_id)
            }
            _ => None,
        }
    }

    /// The ID with `wf` written as `wf_seg`.
    fn render(&self, wf_seg: &str) -> String {
        let origin = |ns: &str, run: &str| format!("{}.{wf_seg}.{}", enc_ns(ns), enc(run));
        match self {
            PromiseId::Wft {
                ns,
                run,
                scheduled_event_id,
                ..
            } => format!("{}:wft.{scheduled_event_id}", origin(ns, run)),
            PromiseId::Act {
                ns,
                run,
                scheduled_event_id,
                attempt,
                ..
            } => format!("{}:act.{scheduled_event_id}.{attempt}", origin(ns, run)),
            PromiseId::Tmr {
                ns,
                run,
                started_event_id,
                ..
            } => format!("{}:tmr.{started_event_id}", origin(ns, run)),
            PromiseId::To {
                ns,
                run,
                event_id,
                kind,
                attempt,
                ..
            } => format!("{}:to.{event_id}.{kind}.{attempt}", origin(ns, run)),
            PromiseId::Start {
                ns,
                schedule_id,
                nominal_time_unix_ms,
            } => format!("{}.{}.{nominal_time_unix_ms}", enc_ns(ns), enc(schedule_id)),
            PromiseId::Sch {
                ns,
                schedule_id,
                generation,
                wake_unix_ms,
            } => format!(
                "{}.{}.{}:sch.{wake_unix_ms}",
                enc_ns(ns),
                enc(schedule_id),
                enc(generation)
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

    /// The origin Resonate sees (the text before the `:`): the run's
    /// `<ns>.<wf>.<run>`, or the whole ID for `start`.
    pub fn origin(&self) -> String {
        let (id, _) = self.encode();
        match id.split_once(':') {
            Some((o, _)) => o.to_string(),
            None => id,
        }
    }

    /// Parse a gateway promise ID in canonical form (module docs).
    pub fn parse(id: &str) -> Result<PromiseId, IdError> {
        let Some((origin, lineage)) = id.split_once(':') else {
            let (ns, schedule, time) = split3(id, id)?;
            return Ok(PromiseId::Start {
                ns: dec_with(id, &ns, enc_ns)?,
                schedule_id: dec_with(id, &schedule, enc)?,
                nominal_time_unix_ms: num(id, &time)?,
            });
        };
        if lineage.contains(':') {
            return Err(IdError::Colons(id.to_string()));
        }
        if let Some(rest) = lineage.strip_prefix("sch.") {
            // The origin is the schedule's generation; its middle segment is
            // a schedule ID, which is never hashed.
            let (ns, schedule, generation) = split3(id, origin)?;
            if rest.contains('.') {
                return Err(IdError::Parts {
                    id: id.to_string(),
                    kind: "sch".into(),
                    expected: 1,
                    found: rest.split('.').count(),
                });
            }
            return Ok(PromiseId::Sch {
                ns: dec_with(id, &ns, enc_ns)?,
                schedule_id: dec_with(id, &schedule, enc)?,
                generation: dec_with(id, &generation, enc)?,
                wake_unix_ms: num(id, rest)?,
            });
        }
        let (ns, wf, run) = split3(id, origin)?;
        let ns = dec_with(id, &ns, enc_ns)?;
        let wf = dec_wf(id, &wf)?;
        let run = dec_with(id, &run, enc)?;
        let parts: Vec<&str> = lineage.split('.').collect();
        let want = |n: usize| {
            if parts.len() == n {
                Ok(())
            } else {
                Err(IdError::Parts {
                    id: id.to_string(),
                    kind: parts[0].to_string(),
                    expected: n - 1,
                    found: parts.len() - 1,
                })
            }
        };
        match parts[0] {
            "wft" => {
                want(2)?;
                Ok(PromiseId::Wft {
                    ns,
                    wf,
                    run,
                    scheduled_event_id: num(id, parts[1])?,
                })
            }
            "act" => {
                want(3)?;
                Ok(PromiseId::Act {
                    ns,
                    wf,
                    run,
                    scheduled_event_id: num(id, parts[1])?,
                    attempt: num(id, parts[2])?,
                })
            }
            "tmr" => {
                want(2)?;
                Ok(PromiseId::Tmr {
                    ns,
                    wf,
                    run,
                    started_event_id: num(id, parts[1])?,
                })
            }
            "to" => {
                want(4)?;
                let kind = TimeoutKind::parse(parts[2]).ok_or_else(|| IdError::TimeoutKind {
                    id: id.to_string(),
                    kind: parts[2].to_string(),
                })?;
                Ok(PromiseId::To {
                    ns,
                    wf,
                    run,
                    event_id: num(id, parts[1])?,
                    kind,
                    attempt: num(id, parts[3])?,
                })
            }
            other => Err(IdError::UnknownKind {
                id: id.to_string(),
                kind: other.to_string(),
            }),
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
    fn sch_ids_roundtrip_and_never_hash() {
        let raw = format!("h{}", "0".repeat(32));
        let id = PromiseId::Sch {
            ns: "l.n".into(),
            schedule_id: raw.clone(),
            generation: "g1".into(),
            wake_unix_ms: 1_792_800_000_000,
        };
        let (s, tags) = id.encode();
        assert_eq!(s, format!("l.n.{raw}.g1:sch.1792800000000"));
        assert_eq!(PromiseId::parse(&s).unwrap(), id);
        assert_eq!(tags.get(TAG_KIND).map(String::as_str), Some("sch"));
        assert!(!tags.contains_key(TAG_RUN));
        assert_eq!(id.origin(), format!("l.n.{raw}.g1"));
        assert_eq!(id.schedule_id(), Some(raw.as_str()));
        assert!(PromiseId::parse("l.n.s.g:sch.1.2").is_err());
        assert!(PromiseId::parse("l.n.s.g:sch.01").is_err());
        assert!(PromiseId::parse("s.g:sch.1").is_err());
    }

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
        assert_eq!(s, format!("n.%68{}.r:tmr.1", "0".repeat(32)));
        assert_eq!(PromiseId::parse(&s).unwrap(), id);
    }
}
