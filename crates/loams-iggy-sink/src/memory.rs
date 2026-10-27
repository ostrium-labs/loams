//! In-memory [`Source`] and [`Target`] with fault injection, for the sink's
//! tests and for anyone testing a target against the runner.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use loams_cloudevents::CloudEvent;

use crate::source::{RawMessage, Source, SourceError, TopicInfo};
use crate::target::{Target, TargetError, WriteOutcome};

type Key = (String, String, u32);

#[derive(Debug, Default)]
struct SourceState {
    topics: BTreeMap<(String, String), u32>,
    messages: HashMap<Key, Vec<RawMessage>>,
    committed: HashMap<Key, u64>,
    /// Calls that fail before succeeding again.
    fail_polls: u32,
    fail_commits: u32,
    commits: u64,
}

/// An Iggy stand-in: topics, messages, committed offsets.
#[derive(Debug, Default)]
pub struct MemorySource {
    state: Mutex<SourceState>,
}

impl MemorySource {
    fn lock(&self) -> std::sync::MutexGuard<'_, SourceState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create_topic(&self, stream: &str, topic: &str, partitions: u32) {
        self.lock()
            .topics
            .insert((stream.into(), topic.into()), partitions);
    }

    /// Appends a message with `payload` and returns its offset.
    pub fn send(&self, stream: &str, topic: &str, partition: u32, payload: &[u8]) -> u64 {
        let mut state = self.lock();
        let log = state
            .messages
            .entry((stream.into(), topic.into(), partition))
            .or_default();
        let offset = log.len() as u64;
        log.push(RawMessage {
            offset,
            id: 0,
            timestamp_us: 1_700_000_000_000_000 + offset,
            origin_timestamp_us: 0,
            payload: Bytes::copy_from_slice(payload),
            headers: Ok(Vec::new()),
        });
        offset
    }

    pub fn committed_offset(&self, stream: &str, topic: &str, partition: u32) -> Option<u64> {
        self.lock()
            .committed
            .get(&(stream.into(), topic.into(), partition))
            .copied()
    }

    /// The next `n` polls fail.
    pub fn fail_next_polls(&self, n: u32) {
        self.lock().fail_polls = n;
    }

    /// The next `n` commits fail.
    pub fn fail_next_commits(&self, n: u32) {
        self.lock().fail_commits = n;
    }

    /// Successful commits so far.
    pub fn commits(&self) -> u64 {
        self.lock().commits
    }
}

#[async_trait]
impl Source for MemorySource {
    async fn topics(&self, stream: &str) -> Result<Vec<TopicInfo>, SourceError> {
        let state = self.lock();
        let topics: Vec<_> = state
            .topics
            .iter()
            .filter(|((s, _), _)| s == stream)
            .map(|((_, t), p)| TopicInfo {
                name: t.clone(),
                partitions: *p,
            })
            .collect();
        if topics.is_empty() {
            return Err(SourceError::NotFound(format!("stream {stream}")));
        }
        Ok(topics)
    }

    async fn committed(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
    ) -> Result<Option<u64>, SourceError> {
        Ok(self.committed_offset(stream, topic, partition))
    }

    async fn poll(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
        count: u32,
    ) -> Result<Vec<RawMessage>, SourceError> {
        let mut state = self.lock();
        if state.fail_polls > 0 {
            state.fail_polls -= 1;
            return Err(SourceError::Iggy("injected poll failure".into()));
        }
        let log = state
            .messages
            .get(&(stream.into(), topic.into(), partition))
            .cloned()
            .unwrap_or_default();
        Ok(log
            .into_iter()
            .filter(|m| m.offset >= offset)
            .take(count as usize)
            .collect())
    }

    async fn commit(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
    ) -> Result<(), SourceError> {
        let mut state = self.lock();
        if state.fail_commits > 0 {
            state.fail_commits -= 1;
            return Err(SourceError::Iggy("injected commit failure".into()));
        }
        state.commits += 1;
        state
            .committed
            .insert((stream.into(), topic.into(), partition), offset);
        Ok(())
    }
}

/// One fault the target answers a write with.
#[derive(Clone, Debug)]
pub enum Fault {
    Backpressure(Option<Duration>),
    Unavailable,
    /// Appends the first `n` events, then answers in flight (as the engine
    /// does when another writer holds a claim).
    PartialInFlight(usize),
}

#[derive(Debug, Default)]
struct TargetState {
    streams: BTreeMap<(String, String), u32>,
    /// Appended events per `(namespace, stream)`, with their partition.
    events: BTreeMap<(String, String), Vec<(u32, CloudEvent)>>,
    seen: HashMap<(String, String), std::collections::HashSet<[u8; 32]>>,
    faults: VecDeque<Fault>,
    writes: u64,
    /// When set, `ensure_stream` refuses with this.
    refuse_streams: Option<String>,
}

/// A Loams stand-in that deduplicates by `source` + `id`, as the engine's
/// ledger does.
#[derive(Debug, Default)]
pub struct MemoryTarget {
    state: Mutex<TargetState>,
}

impl MemoryTarget {
    fn lock(&self) -> std::sync::MutexGuard<'_, TargetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues faults for the next writes, one per write.
    pub fn inject(&self, faults: impl IntoIterator<Item = Fault>) {
        self.lock().faults.extend(faults);
    }

    pub fn refuse_streams(&self, why: Option<&str>) {
        self.lock().refuse_streams = why.map(str::to_string);
    }

    /// The appended events of `namespace/stream`, in append order.
    pub fn events(&self, namespace: &str, stream: &str) -> Vec<(u32, CloudEvent)> {
        self.lock()
            .events
            .get(&(namespace.into(), stream.into()))
            .cloned()
            .unwrap_or_default()
    }

    pub fn stream_partitions(&self, namespace: &str, stream: &str) -> Option<u32> {
        self.lock()
            .streams
            .get(&(namespace.into(), stream.into()))
            .copied()
    }

    pub fn writes(&self) -> u64 {
        self.lock().writes
    }
}

#[async_trait]
impl Target for MemoryTarget {
    async fn ensure_stream(
        &self,
        namespace: &str,
        stream: &str,
        partitions: u32,
    ) -> Result<(), TargetError> {
        let mut state = self.lock();
        if let Some(why) = &state.refuse_streams {
            return Err(TargetError::Unavailable(why.clone()));
        }
        let have = state
            .streams
            .entry((namespace.into(), stream.into()))
            .or_insert(partitions);
        if *have < partitions {
            return Err(TargetError::Rejected(format!(
                "{namespace}/{stream} has {have} partitions, Iggy has {partitions}"
            )));
        }
        Ok(())
    }

    async fn write(
        &self,
        namespace: &str,
        stream: &str,
        partition: u32,
        events: &[CloudEvent],
    ) -> Result<WriteOutcome, TargetError> {
        let mut state = self.lock();
        state.writes += 1;
        let key = (namespace.to_string(), stream.to_string());
        if !state.streams.contains_key(&key) {
            return Err(TargetError::Rejected(format!(
                "stream {namespace}/{stream} not found"
            )));
        }
        let fault = state.faults.pop_front();
        let limit = match fault {
            Some(Fault::Backpressure(retry_after)) => {
                return Err(TargetError::Backpressure {
                    retry_after,
                    message: "injected".into(),
                });
            }
            Some(Fault::Unavailable) => return Err(TargetError::Unavailable("injected".into())),
            Some(Fault::PartialInFlight(n)) => Some(n),
            None => None,
        };
        let mut outcome = WriteOutcome::default();
        for (i, event) in events.iter().enumerate() {
            if limit.is_some_and(|n| i >= n) {
                outcome.in_flight = Some(Duration::from_millis(10));
                break;
            }
            let fresh = state
                .seen
                .entry(key.clone())
                .or_default()
                .insert(event.dedup_key());
            if fresh {
                state
                    .events
                    .entry(key.clone())
                    .or_default()
                    .push((partition, event.clone()));
                outcome.appended += 1;
            } else {
                outcome.duplicates += 1;
            }
        }
        Ok(outcome)
    }
}
