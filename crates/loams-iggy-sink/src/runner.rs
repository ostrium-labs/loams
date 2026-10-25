//! The sink's tasks: one discovery loop, and one loop per Iggy partition that
//! polls, writes and commits, in that order.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::mapping::{EVENT_TYPE_UNDECODABLE, to_event};
use crate::metrics::SinkMetrics;
use crate::source::{RawMessage, Source, TopicInfo};
use crate::target::{Target, TargetError};

/// The most events in one write: the engine's CloudEvents ingest takes at
/// most this many per request (`MAX_IDEMPOTENCY_KEYS`).
pub const MAX_BATCH: u32 = 1_000;

/// What the sink carries and how.
#[derive(Clone, Debug)]
pub struct SinkConfig {
    /// The Iggy stream to read (the Kafka gateway's is `kafka`).
    pub iggy_stream: String,
    /// The Loams namespace the events go to; each Iggy topic becomes the
    /// Loams stream of the same name.
    pub namespace: String,
    /// Only these topics, when set.
    pub topics: Option<Vec<String>>,
    /// Messages per poll, at most [`MAX_BATCH`].
    pub batch: u32,
    /// Payload bytes per write (a single larger message is written alone).
    pub max_batch_bytes: usize,
    /// How long an idle partition waits before polling again.
    pub poll_interval: Duration,
    /// How often the topic list is read again.
    pub discover_interval: Duration,
    /// The first and the longest wait between retries.
    pub min_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            iggy_stream: "kafka".into(),
            namespace: "kafka".into(),
            topics: None,
            batch: 500,
            max_batch_bytes: 4 * 1024 * 1024,
            poll_interval: Duration::from_millis(200),
            discover_interval: Duration::from_secs(5),
            min_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
        }
    }
}

/// Whether `name` can be a Loams stream: 1..=255 ASCII letters, digits, `-`,
/// `_` and `.`, not `.` or `..`, and not starting with `_` (reserved for
/// implicit objects). Kafka's own topic names are the same set, so only
/// internal (`__…`) and Iggy-only names are refused.
pub fn loams_stream_name(name: &str) -> Result<(), String> {
    if name.starts_with('_') {
        return Err("names starting with '_' are reserved in Loams".into());
    }
    let ok = !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err("not a Loams stream name (letters, digits, '-', '_' and '.')".into())
    }
}

/// The sink, before it starts.
pub struct Sink<S, T> {
    config: SinkConfig,
    source: Arc<S>,
    target: Arc<T>,
    metrics: Arc<SinkMetrics>,
}

impl<S, T> std::fmt::Debug for Sink<S, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sink")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A running sink.
#[derive(Debug)]
pub struct SinkHandle {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    metrics: Arc<SinkMetrics>,
}

impl SinkHandle {
    pub fn metrics(&self) -> Arc<SinkMetrics> {
        self.metrics.clone()
    }

    /// Stops every task at its next step: a write in progress finishes, and
    /// what was written but not committed is replayed by the next start.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }

    /// Kills every task where it stands, as a crash would (tests): nothing
    /// after the point each task had reached runs, and no commit is made.
    pub async fn abort(self) {
        self.task.abort();
        // Dropping the discovery task drops its JoinSet, which aborts the
        // partition tasks.
        let _ = self.task.await;
    }
}

/// Waits `d`, or less if the sink stops; `true` when it stopped.
async fn pause(stop: &mut watch::Receiver<bool>, d: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        () = tokio::time::sleep(d) => *stop.borrow(),
        _ = stop.changed() => true,
    }
}

struct Backoff {
    now: Duration,
    min: Duration,
    max: Duration,
}

impl Backoff {
    fn new(config: &SinkConfig) -> Self {
        Self {
            now: config.min_backoff,
            min: config.min_backoff,
            max: config.max_backoff,
        }
    }
    fn next(&mut self) -> Duration {
        let d = self.now;
        self.now = (self.now * 2).min(self.max);
        d
    }
    fn reset(&mut self) {
        self.now = self.min;
    }
}

impl<S: Source, T: Target> Sink<S, T> {
    pub fn new(config: SinkConfig, source: Arc<S>, target: Arc<T>) -> Self {
        Self {
            config,
            source,
            target,
            metrics: Arc::new(SinkMetrics::default()),
        }
    }

    /// Shares `metrics` (to keep counting across a restart).
    pub fn with_metrics(mut self, metrics: Arc<SinkMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Starts the discovery loop on the current runtime.
    pub fn start(self) -> SinkHandle {
        let (stop, stopped) = watch::channel(false);
        let metrics = self.metrics.clone();
        let task = tokio::spawn(self.discover(stopped));
        SinkHandle {
            stop,
            task,
            metrics,
        }
    }

    async fn discover(self, mut stop: watch::Receiver<bool>) {
        let mut config = self.config.clone();
        config.batch = config.batch.clamp(1, MAX_BATCH);
        let config = Arc::new(config);
        // Partitions running per topic, and the partition count Loams has.
        let mut running: BTreeMap<String, u32> = BTreeMap::new();
        let mut tasks = JoinSet::new();
        tracing::info!(
            iggy_stream = %config.iggy_stream,
            namespace = %config.namespace,
            "loams_sink started"
        );
        loop {
            match self.source.topics(&config.iggy_stream).await {
                Ok(topics) => {
                    for topic in topics {
                        self.adopt(&config, &topic, &mut running, &mut tasks, &stop)
                            .await;
                    }
                }
                Err(err) => {
                    tracing::debug!(%err, stream = %config.iggy_stream, "loams_sink: no topics yet");
                    self.metrics.discovery_error(&err.to_string());
                }
            }
            // Reap finished partition loops (they end only when stopping).
            while tasks.try_join_next().is_some() {}
            if pause(&mut stop, config.discover_interval).await {
                break;
            }
        }
        while tasks.join_next().await.is_some() {}
        tracing::info!("loams_sink stopped");
    }

    async fn adopt(
        &self,
        config: &Arc<SinkConfig>,
        topic: &TopicInfo,
        running: &mut BTreeMap<String, u32>,
        tasks: &mut JoinSet<()>,
        stop: &watch::Receiver<bool>,
    ) {
        if let Some(only) = &config.topics
            && !only.contains(&topic.name)
        {
            return;
        }
        if let Err(why) = loams_stream_name(&topic.name) {
            self.metrics.skip(&topic.name, &why);
            return;
        }
        let have = running.get(&topic.name).copied().unwrap_or(0);
        if topic.partitions <= have {
            return;
        }
        if let Err(err) = self
            .target
            .ensure_stream(&config.namespace, &topic.name, topic.partitions)
            .await
        {
            tracing::warn!(%err, topic = %topic.name, "loams_sink: the Loams stream is not ready");
            self.metrics
                .discovery_error(&format!("stream {}: {err}", topic.name));
            return;
        }
        for partition in have..topic.partitions {
            let job = PartitionJob {
                config: config.clone(),
                source: self.source.clone(),
                target: self.target.clone(),
                metrics: self.metrics.clone(),
                topic: topic.name.clone(),
                partition,
            };
            tasks.spawn(job.run(stop.clone()));
        }
        running.insert(topic.name.clone(), topic.partitions);
    }
}

struct PartitionJob<S, T> {
    config: Arc<SinkConfig>,
    source: Arc<S>,
    target: Arc<T>,
    metrics: Arc<SinkMetrics>,
    topic: String,
    partition: u32,
}

impl<S: Source, T: Target> PartitionJob<S, T> {
    async fn run(self, mut stop: watch::Receiver<bool>) {
        let (topic, partition) = (self.topic.as_str(), self.partition);
        let mut backoff = Backoff::new(&self.config);
        // Where to start: just after the last committed offset.
        let mut next = loop {
            match self
                .source
                .committed(&self.config.iggy_stream, topic, partition)
                .await
            {
                Ok(committed) => {
                    let next = committed.map_or(0, |o| o + 1);
                    self.metrics.update(topic, partition, |m| {
                        m.committed = committed;
                        m.next = next;
                    });
                    tracing::info!(topic, partition, next, "loams_sink: partition resumes");
                    break next;
                }
                Err(err) => {
                    self.metrics.error(topic, partition, &err.to_string());
                    if pause(&mut stop, backoff.next()).await {
                        return;
                    }
                }
            }
        };
        backoff.reset();
        loop {
            if *stop.borrow() {
                return;
            }
            let polled = self
                .source
                .poll(
                    &self.config.iggy_stream,
                    topic,
                    partition,
                    next,
                    self.config.batch,
                )
                .await;
            let messages = match polled {
                Ok(messages) => messages,
                Err(err) => {
                    self.metrics.error(topic, partition, &err.to_string());
                    if pause(&mut stop, backoff.next()).await {
                        return;
                    }
                    continue;
                }
            };
            backoff.reset();
            let batch = self.cut(messages, next);
            if batch.is_empty() {
                if pause(&mut stop, self.config.poll_interval).await {
                    return;
                }
                continue;
            }
            let last = batch.last().map_or(next, |m| m.offset);
            self.metrics
                .update(topic, partition, |m| m.polled += batch.len() as u64);
            let events: Vec<_> = batch
                .iter()
                .map(|m| to_event(&self.config.iggy_stream, topic, partition, m))
                .collect();
            let undecodable = events
                .iter()
                .filter(|e| e.ty() == EVENT_TYPE_UNDECODABLE)
                .count();
            if undecodable > 0 {
                tracing::warn!(
                    topic,
                    partition,
                    undecodable,
                    "loams_sink: kept undecodable messages as events"
                );
                self.metrics
                    .update(topic, partition, |m| m.undecodable += undecodable as u64);
            }
            if !self.write(&events, &mut stop).await {
                // Stopped before the target acknowledged: nothing is
                // committed, the next start writes the batch again.
                return;
            }
            self.commit(last, &mut stop).await;
            next = last + 1;
            self.metrics.update(topic, partition, |m| m.next = next);
        }
    }

    /// The contiguous run from `next` on, within the byte budget.
    fn cut(&self, messages: Vec<RawMessage>, next: u64) -> Vec<RawMessage> {
        let mut out = Vec::with_capacity(messages.len());
        let mut bytes = 0usize;
        let mut expect = next;
        for m in messages {
            if m.offset < expect {
                continue;
            }
            if m.offset > expect && out.is_empty() {
                // A gap at the start (retention removed the messages): carry
                // on from the first one there is.
                tracing::warn!(
                    topic = %self.topic,
                    partition = self.partition,
                    from = expect,
                    to = m.offset,
                    "loams_sink: offsets missing in Iggy (retention?), skipping ahead"
                );
            } else if m.offset > expect {
                break;
            }
            bytes += m.payload.len();
            if !out.is_empty() && bytes > self.config.max_batch_bytes {
                break;
            }
            expect = m.offset + 1;
            out.push(m);
        }
        out
    }

    /// Writes until the target acknowledges every event; `false` if the sink
    /// stopped first.
    async fn write(
        &self,
        events: &[loams_cloudevents::CloudEvent],
        stop: &mut watch::Receiver<bool>,
    ) -> bool {
        let (topic, partition) = (self.topic.as_str(), self.partition);
        let mut backoff = Backoff::new(&self.config);
        loop {
            let wait = match self
                .target
                .write(&self.config.namespace, topic, partition, events)
                .await
            {
                Ok(outcome) => {
                    self.metrics.update(topic, partition, |m| {
                        m.appended += outcome.appended as u64;
                        m.duplicates += outcome.duplicates as u64;
                    });
                    match outcome.in_flight {
                        None => return true,
                        Some(wait) => {
                            self.metrics.update(topic, partition, |m| m.in_flight += 1);
                            wait
                        }
                    }
                }
                Err(TargetError::Backpressure {
                    retry_after,
                    message,
                }) => {
                    tracing::debug!(topic, partition, %message, "loams_sink: backpressure");
                    self.metrics
                        .update(topic, partition, |m| m.backpressure += 1);
                    retry_after.unwrap_or_else(|| backoff.next())
                }
                Err(err) => {
                    tracing::warn!(topic, partition, %err, "loams_sink: write failed, retrying");
                    self.metrics.error(topic, partition, &err.to_string());
                    backoff.next()
                }
            };
            if pause(stop, wait).await {
                return false;
            }
        }
    }

    /// Commits `offset`, retrying a few times; a commit that keeps failing is
    /// left to the next batch's (the events are in Loams, and a replay of
    /// them is answered as duplicates).
    async fn commit(&self, offset: u64, stop: &mut watch::Receiver<bool>) {
        let (topic, partition) = (self.topic.as_str(), self.partition);
        let mut backoff = Backoff::new(&self.config);
        for _ in 0..5 {
            match self
                .source
                .commit(&self.config.iggy_stream, topic, partition, offset)
                .await
            {
                Ok(()) => {
                    self.metrics
                        .update(topic, partition, |m| m.committed = Some(offset));
                    return;
                }
                Err(err) => {
                    self.metrics
                        .error(topic, partition, &format!("commit: {err}"));
                    if pause(stop, backoff.next()).await {
                        return;
                    }
                }
            }
        }
        tracing::warn!(
            topic,
            partition,
            offset,
            "loams_sink: commit kept failing; the next one covers it"
        );
    }
}
