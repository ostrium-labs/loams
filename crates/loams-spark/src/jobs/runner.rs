//! Runners and the supervisor that drives a run through its attempts
//! (D2002, D2003).
//!
//! A [`Runner`] starts one attempt and reports on it; it knows nothing about
//! retries. The [`Supervisor`] owns the loop: start an attempt, poll it until
//! it ends (or its timeout passes, or the run is canceled), record the
//! [`RunStatus`] after every transition, and wait out the backoff before the
//! next attempt. In the managed service this loop is the body of a Loams
//! durable function (design 26 §9.2: "the run is a Loams durable workflow"),
//! so a restart resumes it; the local runner runs it on a task.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::sync::watch;

use super::run::{AttemptOutcome, LogLink, Next, RunStatus, RunStore};
use super::spec::SparkJobSpec;
use super::{JobsError, RunId};

/// One attempt to start.
#[derive(Debug, Clone)]
pub struct Attempt {
    /// The run.
    pub run_id: RunId,
    /// 1-based.
    pub attempt: u32,
    /// The job.
    pub spec: SparkJobSpec,
}

/// A started attempt, as its runner names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptHandle {
    /// The runner's id for it (a pod name, a process key).
    pub id: String,
    /// Where its logs are.
    pub log: LogLink,
}

/// Where an attempt is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptPhase {
    /// Created, not running yet (a pod being scheduled, an image pulled).
    Pending,
    /// Running.
    Running,
    /// Over.
    Done(AttemptOutcome),
}

/// Starts attempts on one backend.
#[async_trait]
pub trait Runner: Send + Sync + std::fmt::Debug {
    /// Starts an attempt.
    async fn start(&self, attempt: &Attempt) -> Result<AttemptHandle, JobsError>;
    /// Where it is.
    async fn poll(&self, handle: &AttemptHandle) -> Result<AttemptPhase, JobsError>;
    /// Stops it (idempotent).
    async fn cancel(&self, handle: &AttemptHandle) -> Result<(), JobsError>;
    /// Its logs so far (the tail, for a long log).
    async fn logs(&self, handle: &AttemptHandle) -> Result<String, JobsError>;
}

/// Drives runs through their attempts.
#[derive(Debug, Clone)]
pub struct Supervisor {
    runner: Arc<dyn Runner>,
    store: Arc<dyn RunStore>,
    poll_every: Duration,
}

impl Supervisor {
    /// A supervisor that polls every `poll_every`.
    pub fn new(runner: Arc<dyn Runner>, store: Arc<dyn RunStore>, poll_every: Duration) -> Self {
        Self {
            runner,
            store,
            poll_every,
        }
    }

    /// The store statuses are written to.
    pub fn store(&self) -> &Arc<dyn RunStore> {
        &self.store
    }

    /// The runner.
    pub fn runner(&self) -> &Arc<dyn Runner> {
        &self.runner
    }

    /// Records a new pending run of `spec` and returns its status. Call
    /// [`Supervisor::drive`] next (on a task) to run it.
    pub async fn accept(&self, spec: &SparkJobSpec) -> Result<RunStatus, JobsError> {
        spec.validate()?;
        let status = RunStatus::new(RunId::new(), spec, OffsetDateTime::now_utc());
        self.store.put(&status).await?;
        Ok(status)
    }

    /// Runs `status`'s job to a terminal state and returns the final status.
    /// `cancel` flips to `true` to cancel it.
    pub async fn drive(
        &self,
        mut status: RunStatus,
        spec: &SparkJobSpec,
        mut cancel: watch::Receiver<bool>,
    ) -> RunStatus {
        let mut attempt = status.attempt;
        loop {
            if *cancel.borrow() {
                status.canceled(OffsetDateTime::now_utc());
                self.save(&status).await;
                return status;
            }
            attempt += 1;
            let handle = match self
                .runner
                .start(&Attempt {
                    run_id: status.run_id.clone(),
                    attempt,
                    spec: spec.clone(),
                })
                .await
            {
                Ok(h) => h,
                Err(err) => {
                    tracing::warn!(run = %status.run_id, attempt, %err, "spark attempt did not start");
                    status.attempt = attempt;
                    status.errored(&err, OffsetDateTime::now_utc());
                    self.save(&status).await;
                    return status;
                }
            };
            status.submitted(attempt, handle.log.clone(), OffsetDateTime::now_utc());
            self.save(&status).await;

            let outcome = self
                .watch_attempt(&mut status, spec, &handle, &mut cancel)
                .await;
            let next = status.attempt_ended(outcome, &spec.retry, OffsetDateTime::now_utc());
            self.save(&status).await;
            match next {
                Next::Done => return status,
                Next::Retry(wait) => {
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        Ok(()) = cancel.changed() => {}
                    }
                }
            }
        }
    }

    async fn watch_attempt(
        &self,
        status: &mut RunStatus,
        spec: &SparkJobSpec,
        handle: &AttemptHandle,
        cancel: &mut watch::Receiver<bool>,
    ) -> AttemptOutcome {
        let started = Instant::now();
        let timeout = spec.timeout_secs.map(Duration::from_secs);
        // A few consecutive poll errors are tolerated (an API server blip);
        // more end the attempt as failed, and the retry policy decides.
        let mut poll_errors = 0u32;
        loop {
            if *cancel.borrow() {
                self.stop(handle).await;
                return AttemptOutcome::Canceled { timed_out: false };
            }
            if timeout.is_some_and(|t| started.elapsed() >= t) {
                self.stop(handle).await;
                return AttemptOutcome::Canceled { timed_out: true };
            }
            match self.runner.poll(handle).await {
                Ok(AttemptPhase::Done(outcome)) => return outcome,
                Ok(AttemptPhase::Running) => {
                    poll_errors = 0;
                    if status.state != super::RunState::Running {
                        status.running(OffsetDateTime::now_utc());
                        self.save(status).await;
                    }
                }
                Ok(AttemptPhase::Pending) => poll_errors = 0,
                Err(err) => {
                    poll_errors += 1;
                    tracing::warn!(run = %status.run_id, %err, poll_errors, "spark attempt poll failed");
                    if poll_errors >= 5 {
                        self.stop(handle).await;
                        return AttemptOutcome::Failed {
                            exit_code: None,
                            message: format!("lost track of the attempt: {err}"),
                        };
                    }
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.poll_every) => {}
                Ok(()) = cancel.changed() => {}
            }
        }
    }

    async fn stop(&self, handle: &AttemptHandle) {
        if let Err(err) = self.runner.cancel(handle).await {
            tracing::warn!(attempt = %handle.id, %err, "could not stop a spark attempt");
        }
    }

    async fn save(&self, status: &RunStatus) {
        if let Err(err) = self.store.put(status).await {
            tracing::warn!(run = %status.run_id, %err, "could not record a spark run status");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::jobs::run::{MemoryRunStore, RunState};
    use crate::jobs::spec::{Backoff, RetryPolicy, SparkApp};

    /// A runner that plays a script of outcomes, one per attempt.
    #[derive(Debug, Default)]
    struct Scripted {
        outcomes: Mutex<VecDeque<AttemptOutcome>>,
        started: Mutex<Vec<u32>>,
        canceled: Mutex<u32>,
        hang: bool,
    }

    #[async_trait]
    impl Runner for Scripted {
        async fn start(&self, attempt: &Attempt) -> Result<AttemptHandle, JobsError> {
            self.started.lock().expect("lock").push(attempt.attempt);
            Ok(AttemptHandle {
                id: format!("a{}", attempt.attempt),
                log: LogLink {
                    attempt: attempt.attempt,
                    url: format!("mem://{}", attempt.attempt),
                },
            })
        }

        async fn poll(&self, _: &AttemptHandle) -> Result<AttemptPhase, JobsError> {
            if self.hang {
                return Ok(AttemptPhase::Running);
            }
            Ok(match self.outcomes.lock().expect("lock").pop_front() {
                Some(o) => AttemptPhase::Done(o),
                None => AttemptPhase::Running,
            })
        }

        async fn cancel(&self, _: &AttemptHandle) -> Result<(), JobsError> {
            *self.canceled.lock().expect("lock") += 1;
            Ok(())
        }

        async fn logs(&self, h: &AttemptHandle) -> Result<String, JobsError> {
            Ok(h.id.clone())
        }
    }

    fn spec(attempts: u32) -> SparkJobSpec {
        let mut s = SparkJobSpec::new(
            "etl",
            SparkApp::Pyspark {
                file: "etl.py".into(),
                py_files: vec![],
            },
        );
        s.retry = RetryPolicy {
            max_attempts: attempts,
            backoff: Backoff::Fixed { secs: 0 },
        };
        s
    }

    fn sup(runner: Arc<Scripted>) -> (Supervisor, MemoryRunStore) {
        let store = MemoryRunStore::new();
        (
            Supervisor::new(runner, Arc::new(store.clone()), Duration::from_millis(1)),
            store,
        )
    }

    #[tokio::test]
    async fn retries_until_success() {
        let runner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::from([
                AttemptOutcome::Failed {
                    exit_code: Some(1),
                    message: "x".into(),
                },
                AttemptOutcome::Succeeded,
            ])),
            ..Scripted::default()
        });
        let (sup, store) = sup(runner.clone());
        let s = spec(3);
        let status = sup.accept(&s).await.expect("accept");
        let (_tx, rx) = watch::channel(false);
        let done = sup.drive(status, &s, rx).await;
        assert_eq!(done.state, RunState::Succeeded);
        assert_eq!(done.attempt, 2);
        assert_eq!(done.logs.len(), 2);
        assert_eq!(*runner.started.lock().expect("lock"), vec![1, 2]);
        let stored = store.get(&done.run_id).await.expect("get").expect("stored");
        assert_eq!(stored, done);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let fail = || AttemptOutcome::Failed {
            exit_code: Some(3),
            message: "bad".into(),
        };
        let runner = Arc::new(Scripted {
            outcomes: Mutex::new(VecDeque::from([fail(), fail(), fail()])),
            ..Scripted::default()
        });
        let (sup, _) = sup(runner.clone());
        let s = spec(2);
        let status = sup.accept(&s).await.expect("accept");
        let (_tx, rx) = watch::channel(false);
        let done = sup.drive(status, &s, rx).await;
        assert_eq!(done.state, RunState::Failed);
        assert_eq!(done.exit_code, Some(3));
        assert_eq!(runner.started.lock().expect("lock").len(), 2);
    }

    #[tokio::test]
    async fn cancel_stops_the_attempt() {
        let runner = Arc::new(Scripted {
            hang: true,
            ..Scripted::default()
        });
        let (sup, _) = sup(runner.clone());
        let s = spec(3);
        let status = sup.accept(&s).await.expect("accept");
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn({
            let sup = sup.clone();
            let s = s.clone();
            async move { sup.drive(status, &s, rx).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).expect("send");
        let done = task.await.expect("join");
        assert_eq!(done.state, RunState::Canceled);
        assert_eq!(*runner.canceled.lock().expect("lock"), 1);
        assert_eq!(runner.started.lock().expect("lock").len(), 1);
    }

    #[tokio::test]
    async fn timeout_is_a_failed_attempt() {
        let runner = Arc::new(Scripted {
            hang: true,
            ..Scripted::default()
        });
        let (sup, _) = sup(runner.clone());
        let mut s = spec(1);
        s.timeout_secs = Some(1);
        let status = sup.accept(&s).await.expect("accept");
        let (_tx, rx) = watch::channel(false);
        let done = sup.drive(status, &s, rx).await;
        assert_eq!(done.state, RunState::Failed);
        assert_eq!(done.message.as_deref(), Some("timed out"));
        assert_eq!(*runner.canceled.lock().expect("lock"), 1);
    }

    #[tokio::test]
    async fn an_invalid_spec_is_refused() {
        let (sup, _) = sup(Arc::new(Scripted::default()));
        let mut s = spec(1);
        s.name = "Nope".into();
        assert!(matches!(
            sup.accept(&s).await,
            Err(JobsError::InvalidSpec(_))
        ));
    }
}
