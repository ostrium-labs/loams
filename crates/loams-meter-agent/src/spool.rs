//! The agent's on-disk spool: its write-ahead log (design 01 §6.2).
//!
//! Every event is appended here, and fsynced, before the agent advances its
//! baselines. A forwarder then reads unacknowledged events in order, sends
//! them to a [`Sink`](crate::sink::Sink) and acknowledges them.
//!
//! Layout: segment files `seg-<first offset>.jsonl` of JSON lines
//! `{"offset": n, "event": {…}}`, and an `ack` file holding the highest
//! acknowledged offset, replaced by rename. Fully acknowledged segments are
//! deleted. A torn final line (a crash mid-append) is truncated on open.
//!
//! The spool is **bounded**: an append that would take it past
//! [`SpoolConfig::max_bytes`] fails with [`MeterError::SpoolFull`], and the
//! agent keeps the usage in its counters until the forwarder drains space.
//! Its file operations are blocking and small; the agent calls them from
//! its tick.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::MeterError;
use crate::event::MeterEvent;
use crate::sink::Sink;

/// Sizes of the spool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpoolConfig {
    /// The most bytes the segments may hold. Default 256 MiB.
    pub max_bytes: u64,
    /// A segment is closed once it holds this many bytes. Default 8 MiB.
    pub segment_bytes: u64,
}

impl Default for SpoolConfig {
    fn default() -> Self {
        Self {
            max_bytes: 256 << 20,
            segment_bytes: 8 << 20,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Line {
    offset: u64,
    event: MeterEvent,
}

#[derive(Debug)]
struct Segment {
    path: PathBuf,
    /// The last offset it holds (segments are never empty).
    last: u64,
    bytes: u64,
}

/// A bounded, durable queue of meter events.
#[derive(Debug)]
pub struct Spool {
    dir: PathBuf,
    config: SpoolConfig,
    segments: Vec<Segment>,
    next_offset: u64,
    acked: Option<u64>,
}

const ACK_FILE: &str = "ack";

impl Spool {
    /// Opens (or creates) the spool in `dir`, recovering its segments.
    pub fn open(dir: impl Into<PathBuf>, config: SpoolConfig) -> Result<Self, MeterError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| MeterError::io(&dir, e))?;
        let acked = read_ack(&dir.join(ACK_FILE))?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| MeterError::io(&dir, e))? {
            let entry = entry.map_err(|e| MeterError::io(&dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(first) = name
                .strip_prefix("seg-")
                .and_then(|s| s.strip_suffix(".jsonl"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                names.push((first, entry.path()));
            }
        }
        names.sort();
        let count = names.len();
        let mut segments = Vec::new();
        for (i, (_, path)) in names.into_iter().enumerate() {
            let lines = read_segment(&path, i + 1 == count)?;
            let Some(last) = lines.last().map(|line| line.offset) else {
                fs::remove_file(&path).map_err(|e| MeterError::io(&path, e))?;
                continue;
            };
            let bytes = fs::metadata(&path)
                .map_err(|e| MeterError::io(&path, e))?
                .len();
            segments.push(Segment { path, last, bytes });
        }
        let next_offset = segments
            .last()
            .map(|s| s.last + 1)
            .max(acked.map(|a| a + 1))
            .unwrap_or(0);
        let mut spool = Self {
            dir,
            config,
            segments,
            next_offset,
            acked,
        };
        spool.delete_acked()?;
        Ok(spool)
    }

    /// Appends `events` durably, in order. Returns the first offset used.
    pub fn append(&mut self, events: &[MeterEvent]) -> Result<u64, MeterError> {
        let first = self.next_offset;
        if events.is_empty() {
            return Ok(first);
        }
        let mut buf = Vec::new();
        for (i, event) in events.iter().enumerate() {
            let line = Line {
                offset: first + i as u64,
                event: event.clone(),
            };
            serde_json::to_writer(&mut buf, &line)
                .map_err(|e| MeterError::parse(&self.dir, e.to_string()))?;
            buf.push(b'\n');
        }
        let held = self.bytes();
        if held + buf.len() as u64 > self.config.max_bytes {
            return Err(MeterError::SpoolFull {
                bytes: held,
                limit: self.config.max_bytes,
            });
        }
        let reuse = self
            .segments
            .last()
            .is_some_and(|s| s.bytes < self.config.segment_bytes);
        let (path, created) = if reuse {
            let segment = self.segments.last().map(|s| s.path.clone());
            (segment.unwrap_or_default(), false)
        } else {
            (self.dir.join(format!("seg-{first:020}.jsonl")), true)
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| MeterError::io(&path, e))?;
        file.write_all(&buf).map_err(|e| MeterError::io(&path, e))?;
        file.sync_data().map_err(|e| MeterError::io(&path, e))?;
        if created {
            sync_dir(&self.dir)?;
        }
        let last = first + events.len() as u64 - 1;
        match self.segments.last_mut() {
            Some(segment) if !created => {
                segment.last = last;
                segment.bytes += buf.len() as u64;
            }
            _ => self.segments.push(Segment {
                path,
                last,
                bytes: buf.len() as u64,
            }),
        }
        self.next_offset = last + 1;
        Ok(first)
    }

    /// Up to `max` unacknowledged events, oldest first, with their offsets.
    pub fn pending(&self, max: usize) -> Result<Vec<(u64, MeterEvent)>, MeterError> {
        let mut out = Vec::new();
        for segment in &self.segments {
            if self.acked.is_some_and(|a| segment.last <= a) {
                continue;
            }
            for line in read_segment(&segment.path, false)? {
                if self.acked.is_some_and(|a| line.offset <= a) {
                    continue;
                }
                out.push((line.offset, line.event));
                if out.len() == max {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// Every event still on disk, acknowledged or not (for crash recovery).
    pub fn events(&self) -> Result<Vec<MeterEvent>, MeterError> {
        let mut out = Vec::new();
        for segment in &self.segments {
            out.extend(
                read_segment(&segment.path, false)?
                    .into_iter()
                    .map(|l| l.event),
            );
        }
        Ok(out)
    }

    /// Acknowledges every event up to and including `offset`, and deletes
    /// the segments that no longer hold unacknowledged events.
    pub fn ack(&mut self, offset: u64) -> Result<(), MeterError> {
        if self.next_offset == 0 || offset >= self.next_offset {
            return Err(MeterError::Config(format!(
                "ack of offset {offset}, but the spool ends before {}",
                self.next_offset
            )));
        }
        if self.acked.is_some_and(|a| offset <= a) {
            return Ok(());
        }
        let path = self.dir.join(ACK_FILE);
        let tmp = self.dir.join("ack.tmp");
        write_synced(&tmp, offset.to_string().as_bytes())?;
        fs::rename(&tmp, &path).map_err(|e| MeterError::io(&path, e))?;
        sync_dir(&self.dir)?;
        self.acked = Some(offset);
        self.delete_acked()
    }

    /// The highest acknowledged offset.
    pub fn acked(&self) -> Option<u64> {
        self.acked
    }

    /// The offset the next append gets.
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }

    /// Bytes held by the segments on disk.
    pub fn bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.bytes).sum()
    }

    fn delete_acked(&mut self) -> Result<(), MeterError> {
        let Some(acked) = self.acked else {
            return Ok(());
        };
        let mut kept = Vec::with_capacity(self.segments.len());
        for segment in self.segments.drain(..) {
            if segment.last <= acked {
                fs::remove_file(&segment.path).map_err(|e| MeterError::io(&segment.path, e))?;
            } else {
                kept.push(segment);
            }
        }
        self.segments = kept;
        Ok(())
    }
}

#[async_trait]
impl Sink for Spool {
    async fn send(&mut self, events: &[MeterEvent]) -> Result<(), MeterError> {
        self.append(events).map(|_| ())
    }
}

/// Reads a segment's lines. In the `last` segment, a torn final line (no
/// newline, or not JSON) is truncated away; anywhere else it is an error.
fn read_segment(path: &Path, last: bool) -> Result<Vec<Line>, MeterError> {
    let data = fs::read(path).map_err(|e| MeterError::io(path, e))?;
    let mut lines = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = data[start..].iter().position(|&b| b == b'\n');
        let parsed =
            end.and_then(|end| serde_json::from_slice::<Line>(&data[start..start + end]).ok());
        match (end, parsed) {
            (Some(end), Some(line)) => {
                lines.push(line);
                start += end + 1;
            }
            _ if last && is_tail(&data[start..]) => {
                let file = OpenOptions::new()
                    .write(true)
                    .open(path)
                    .map_err(|e| MeterError::io(path, e))?;
                file.set_len(start as u64)
                    .map_err(|e| MeterError::io(path, e))?;
                file.sync_all().map_err(|e| MeterError::io(path, e))?;
                break;
            }
            _ => {
                return Err(MeterError::parse(
                    path,
                    format!("corrupt line at byte {start}"),
                ));
            }
        }
    }
    Ok(lines)
}

/// Whether `rest` is a single torn line: at most one newline, at its end.
fn is_tail(rest: &[u8]) -> bool {
    match rest.iter().position(|&b| b == b'\n') {
        None => true,
        Some(i) => i + 1 == rest.len(),
    }
}

fn read_ack(path: &Path) -> Result<Option<u64>, MeterError> {
    match fs::read_to_string(path) {
        Ok(s) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| MeterError::parse(path, format!("not an offset: {s:?}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MeterError::io(path, e)),
    }
}

/// Writes `data` to `path` and fsyncs it.
pub(crate) fn write_synced(path: &Path, data: &[u8]) -> Result<(), MeterError> {
    let mut file = File::create(path).map_err(|e| MeterError::io(path, e))?;
    file.write_all(data).map_err(|e| MeterError::io(path, e))?;
    file.sync_all().map_err(|e| MeterError::io(path, e))
}

pub(crate) fn sync_dir(dir: &Path) -> Result<(), MeterError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| MeterError::io(dir, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Attribution, Measurement, Meter, SourceKind, time_from_unix_ms};

    fn event(seq: u64) -> MeterEvent {
        Measurement {
            attribution: Attribution::new("org", "ns"),
            source: SourceKind::Cgroup,
            meter: Meter::CpuUsec,
            value: seq + 1,
            window_start: time_from_unix_ms(0),
            window_end: time_from_unix_ms(1_000),
            resource: "r#ino=1".into(),
            counter: Some(seq + 1),
            peak_bytes: None,
            cpu_usec: None,
            estimated: false,
        }
        .into_event("n", seq)
    }

    fn offsets(p: &[(u64, MeterEvent)]) -> Vec<u64> {
        p.iter().map(|(o, _)| *o).collect()
    }

    #[test]
    fn append_pending_ack_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        assert_eq!(spool.append(&[event(0), event(1)]).unwrap(), 0);
        assert_eq!(spool.append(&[event(2)]).unwrap(), 2);
        assert_eq!(offsets(&spool.pending(10).unwrap()), vec![0, 1, 2]);
        assert_eq!(offsets(&spool.pending(2).unwrap()), vec![0, 1]);
        spool.ack(1).unwrap();
        assert_eq!(offsets(&spool.pending(10).unwrap()), vec![2]);
        drop(spool);

        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        assert_eq!(spool.acked(), Some(1));
        let pending = spool.pending(10).unwrap();
        assert_eq!(offsets(&pending), vec![2]);
        assert_eq!(pending[0].1, event(2));
        assert_eq!(spool.append(&[event(3)]).unwrap(), 3);
    }

    #[test]
    fn acked_segments_are_deleted_and_offsets_survive() {
        let dir = tempfile::tempdir().unwrap();
        let config = SpoolConfig {
            max_bytes: 1 << 20,
            segment_bytes: 1,
        };
        let mut spool = Spool::open(dir.path(), config).unwrap();
        for i in 0..3 {
            spool.append(&[event(i)]).unwrap();
        }
        let segs = |d: &Path| {
            fs::read_dir(d)
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("seg-")
                })
                .count()
        };
        assert_eq!(segs(dir.path()), 3);
        spool.ack(2).unwrap();
        assert_eq!(segs(dir.path()), 0);
        assert_eq!(spool.bytes(), 0);
        drop(spool);
        // Every segment is gone; the ack file still carries the offset.
        let spool = Spool::open(dir.path(), config).unwrap();
        assert_eq!(spool.next_offset(), 3);
    }

    #[test]
    fn a_full_spool_refuses_appends_until_drained() {
        let dir = tempfile::tempdir().unwrap();
        let one = serde_json::to_vec(&Line {
            offset: 0,
            event: event(0),
        })
        .unwrap()
        .len() as u64
            + 1;
        let config = SpoolConfig {
            max_bytes: one * 2 + one / 2,
            segment_bytes: 1,
        };
        let mut spool = Spool::open(dir.path(), config).unwrap();
        spool.append(&[event(0)]).unwrap();
        spool.append(&[event(1)]).unwrap();
        let err = spool.append(&[event(2)]).unwrap_err();
        assert!(matches!(err, MeterError::SpoolFull { .. }));
        assert_eq!(spool.next_offset(), 2);
        spool.ack(0).unwrap();
        assert_eq!(spool.append(&[event(2)]).unwrap(), 2);
    }

    #[test]
    fn a_torn_tail_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        spool.append(&[event(0)]).unwrap();
        let path = spool.segments[0].path.clone();
        drop(spool);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"offset\":1,\"ev").unwrap();
        drop(file);

        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        assert_eq!(offsets(&spool.pending(10).unwrap()), vec![0]);
        assert_eq!(spool.append(&[event(1)]).unwrap(), 1);
        assert_eq!(offsets(&spool.pending(10).unwrap()), vec![0, 1]);
    }

    #[test]
    fn corruption_before_the_tail_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        spool.append(&[event(0), event(1)]).unwrap();
        let path = spool.segments[0].path.clone();
        drop(spool);
        let data = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("garbage\n{data}")).unwrap();
        assert!(matches!(
            Spool::open(dir.path(), SpoolConfig::default()),
            Err(MeterError::Parse { .. })
        ));
    }

    #[test]
    fn ack_past_the_end_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), SpoolConfig::default()).unwrap();
        assert!(spool.ack(0).is_err());
        spool.append(&[event(0)]).unwrap();
        assert!(spool.ack(1).is_err());
        spool.ack(0).unwrap();
        spool.ack(0).unwrap();
    }
}
