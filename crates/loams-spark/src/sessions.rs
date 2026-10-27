//! Session limits and idle timeouts (SP-a D1995).
//!
//! The proxy counts Spark Connect sessions per Loams namespace from the
//! `session_id` of each call: a session id the namespace has not used before
//! opens a session, `ReleaseSession` (or the idle sweeper) closes it, and
//! `CloneSession` opens the clone. Per namespace there is a cap on open
//! sessions and on concurrent operations (calls in flight, a streaming
//! `ExecutePlan` counting until its stream ends); either answers
//! `RESOURCE_EXHAUSTED`.
//!
//! A session whose last call ended more than `idle_timeout` ago, with no
//! call in flight, is idle: [`SessionTable::reap`] removes it and the proxy
//! sends the backend a `ReleaseSession`. An instance with no session and no
//! call for `instance_idle_timeout` is reported once by
//! [`SessionTable::idle_instances`], for the lifecycle's suspend
//! (scale-to-zero).
//!
//! Times are [`tokio::time::Instant`], so tests drive them with a paused clock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;
use tonic::Status;

/// The limits, per Loams namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Open sessions per namespace.
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    /// Calls in flight per namespace.
    #[serde(default = "default_max_operations")]
    pub max_operations: usize,
    /// A session with no call for this long is released.
    #[serde(default = "default_idle_timeout", with = "secs")]
    pub idle_timeout: Duration,
    /// An instance with no session for this long is reported for suspend.
    #[serde(default = "default_instance_idle_timeout", with = "secs")]
    pub instance_idle_timeout: Duration,
}

fn default_max_sessions() -> usize {
    8
}
fn default_max_operations() -> usize {
    16
}
fn default_idle_timeout() -> Duration {
    Duration::from_secs(30 * 60)
}
fn default_instance_idle_timeout() -> Duration {
    Duration::from_secs(15 * 60)
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_sessions: default_max_sessions(),
            max_operations: default_max_operations(),
            idle_timeout: default_idle_timeout(),
            instance_idle_timeout: default_instance_idle_timeout(),
        }
    }
}

mod secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer};

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_secs)
    }
}

#[derive(Debug)]
struct Session {
    user_id: String,
    last_seen: Instant,
    active: usize,
    opened: Instant,
}

#[derive(Debug, Default)]
struct Namespace {
    /// `(instance_id, session_id)` → session.
    sessions: HashMap<(String, String), Session>,
    active: usize,
}

#[derive(Debug)]
struct InstanceActivity {
    namespace: String,
    last_activity: Instant,
    reported_idle: bool,
}

#[derive(Debug, Default)]
struct Inner {
    namespaces: HashMap<String, Namespace>,
    instances: HashMap<String, InstanceActivity>,
}

/// The proxy's view of open sessions.
#[derive(Debug)]
pub struct SessionTable {
    limits: Limits,
    inner: Mutex<Inner>,
}

/// A session the sweeper released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaped {
    /// The namespace.
    pub namespace: String,
    /// The backend instance.
    pub instance_id: String,
    /// The Spark session id.
    pub session_id: String,
    /// The `user_context.user_id` the session was opened with.
    pub user_id: String,
}

/// An open session, for `SparkEngine::list_sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The backend instance.
    pub instance_id: String,
    /// The Spark session id.
    pub session_id: String,
    /// Who opened it.
    pub user_id: String,
    /// Calls in flight.
    pub active: usize,
    /// Since the last call ended (zero while busy).
    pub idle_for: Duration,
    /// Since it was opened.
    pub age: Duration,
}

/// What a call is, for the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// Any call: opens the session if new.
    Use,
    /// `ReleaseSession`: never opens one (releasing an unknown session is
    /// passed through so the backend answers).
    Release,
}

impl SessionTable {
    /// An empty table.
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// The limits.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain counters; keep serving.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a call on a session, opening it if new.
    ///
    /// # Errors
    ///
    /// `RESOURCE_EXHAUSTED` over a limit; `PERMISSION_DENIED` when another
    /// subject opened this session.
    pub fn begin(
        self: &Arc<Self>,
        namespace: &str,
        instance_id: &str,
        session_id: &str,
        user_id: &str,
        kind: CallKind,
        now: Instant,
    ) -> Result<CallGuard, Status> {
        let mut inner = self.lock();
        let ns = inner.namespaces.entry(namespace.to_owned()).or_default();
        if ns.active >= self.limits.max_operations {
            return Err(Status::resource_exhausted(format!(
                "this namespace has {} Spark operations in flight (the limit)",
                self.limits.max_operations
            )));
        }
        let key = (instance_id.to_owned(), session_id.to_owned());
        let tracked = match ns.sessions.get_mut(&key) {
            Some(s) if s.user_id != user_id => {
                return Err(Status::permission_denied(
                    "this Spark session belongs to another principal",
                ));
            }
            Some(s) => {
                s.active += 1;
                s.last_seen = now;
                true
            }
            None if kind == CallKind::Release => false,
            None => {
                if ns.sessions.len() >= self.limits.max_sessions {
                    return Err(Status::resource_exhausted(format!(
                        "this namespace has {} open Spark sessions (the limit); stop one with spark.stop()",
                        self.limits.max_sessions
                    )));
                }
                ns.sessions.insert(
                    key.clone(),
                    Session {
                        user_id: user_id.to_owned(),
                        last_seen: now,
                        active: 1,
                        opened: now,
                    },
                );
                true
            }
        };
        ns.active += 1;
        let activity = inner
            .instances
            .entry(instance_id.to_owned())
            .or_insert_with(|| InstanceActivity {
                namespace: namespace.to_owned(),
                last_activity: now,
                reported_idle: false,
            });
        activity.last_activity = now;
        activity.reported_idle = false;
        Ok(CallGuard {
            table: Arc::clone(self),
            namespace: namespace.to_owned(),
            key,
            tracked,
        })
    }

    /// Opens a session created by `CloneSession` (counted against the limit).
    ///
    /// # Errors
    ///
    /// `RESOURCE_EXHAUSTED` when the namespace is at its session limit (the
    /// proxy then releases the clone on the backend).
    pub fn open_clone(
        &self,
        namespace: &str,
        instance_id: &str,
        session_id: &str,
        user_id: &str,
        now: Instant,
    ) -> Result<(), Status> {
        let mut inner = self.lock();
        let ns = inner.namespaces.entry(namespace.to_owned()).or_default();
        let key = (instance_id.to_owned(), session_id.to_owned());
        if ns.sessions.contains_key(&key) {
            return Ok(());
        }
        if ns.sessions.len() >= self.limits.max_sessions {
            return Err(Status::resource_exhausted(
                "the clone would exceed this namespace's Spark session limit",
            ));
        }
        ns.sessions.insert(
            key,
            Session {
                user_id: user_id.to_owned(),
                last_seen: now,
                active: 0,
                opened: now,
            },
        );
        Ok(())
    }

    /// Forgets a session (after a successful `ReleaseSession`).
    pub fn close(&self, namespace: &str, instance_id: &str, session_id: &str) {
        let mut inner = self.lock();
        if let Some(ns) = inner.namespaces.get_mut(namespace) {
            ns.sessions
                .remove(&(instance_id.to_owned(), session_id.to_owned()));
        }
    }

    fn end(&self, namespace: &str, key: &(String, String), tracked: bool) {
        let now = Instant::now();
        let mut inner = self.lock();
        if let Some(ns) = inner.namespaces.get_mut(namespace) {
            ns.active = ns.active.saturating_sub(1);
            if tracked && let Some(s) = ns.sessions.get_mut(key) {
                s.active = s.active.saturating_sub(1);
                s.last_seen = now;
            }
        }
        if let Some(a) = inner.instances.get_mut(&key.0) {
            a.last_activity = now;
        }
    }

    /// Removes and returns every idle session.
    pub fn reap(&self, now: Instant) -> Vec<Reaped> {
        let idle = self.limits.idle_timeout;
        let mut out = Vec::new();
        let mut inner = self.lock();
        for (namespace, ns) in &mut inner.namespaces {
            ns.sessions.retain(|(instance_id, session_id), s| {
                let keep = s.active > 0 || now.saturating_duration_since(s.last_seen) < idle;
                if !keep {
                    out.push(Reaped {
                        namespace: namespace.clone(),
                        instance_id: instance_id.clone(),
                        session_id: session_id.clone(),
                        user_id: s.user_id.clone(),
                    });
                }
                keep
            });
        }
        inner
            .namespaces
            .retain(|_, ns| !ns.sessions.is_empty() || ns.active > 0);
        out.sort_by(|a, b| (&a.instance_id, &a.session_id).cmp(&(&b.instance_id, &b.session_id)));
        out
    }

    /// Instances with no session and no call for `instance_idle_timeout`,
    /// each reported once until it is used again: `(namespace, instance_id)`.
    pub fn idle_instances(&self, now: Instant) -> Vec<(String, String)> {
        let timeout = self.limits.instance_idle_timeout;
        let mut inner = self.lock();
        let Inner {
            namespaces,
            instances,
        } = &mut *inner;
        let mut out = Vec::new();
        for (instance_id, a) in instances.iter_mut() {
            if a.reported_idle || now.saturating_duration_since(a.last_activity) < timeout {
                continue;
            }
            let busy = namespaces
                .get(&a.namespace)
                .is_some_and(|ns| ns.sessions.keys().any(|(i, _)| i == instance_id));
            if !busy {
                a.reported_idle = true;
                out.push((a.namespace.clone(), instance_id.clone()));
            }
        }
        out.sort();
        out
    }

    /// The open sessions of a namespace.
    pub fn sessions(&self, namespace: &str, now: Instant) -> Vec<SessionInfo> {
        let inner = self.lock();
        let mut out: Vec<SessionInfo> = inner
            .namespaces
            .get(namespace)
            .map(|ns| {
                ns.sessions
                    .iter()
                    .map(|((instance_id, session_id), s)| SessionInfo {
                        instance_id: instance_id.clone(),
                        session_id: session_id.clone(),
                        user_id: s.user_id.clone(),
                        active: s.active,
                        idle_for: if s.active > 0 {
                            Duration::ZERO
                        } else {
                            now.saturating_duration_since(s.last_seen)
                        },
                        age: now.saturating_duration_since(s.opened),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        out
    }

    /// Open sessions and calls in flight of a namespace.
    pub fn counts(&self, namespace: &str) -> (usize, usize) {
        let inner = self.lock();
        inner
            .namespaces
            .get(namespace)
            .map_or((0, 0), |ns| (ns.sessions.len(), ns.active))
    }
}

/// A call in flight; dropping it ends the call (when a unary call answers or
/// a stream ends or is cancelled).
#[derive(Debug)]
pub struct CallGuard {
    table: Arc<SessionTable>,
    namespace: String,
    key: (String, String),
    tracked: bool,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.table.end(&self.namespace, &self.key, self.tracked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            max_sessions: 2,
            max_operations: 3,
            idle_timeout: Duration::from_secs(60),
            instance_idle_timeout: Duration::from_secs(120),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn session_limit_per_namespace() {
        let t = SessionTable::new(limits());
        let now = Instant::now();
        let a = t.begin("ns", "i", "s1", "u", CallKind::Use, now).unwrap();
        let b = t.begin("ns", "i", "s2", "u", CallKind::Use, now).unwrap();
        drop((a, b));
        let err = t
            .begin("ns", "i", "s3", "u", CallKind::Use, now)
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::ResourceExhausted);
        // Existing sessions still work; another namespace is unaffected.
        drop(t.begin("ns", "i", "s1", "u", CallKind::Use, now).unwrap());
        drop(
            t.begin("other", "j", "s3", "u", CallKind::Use, now)
                .unwrap(),
        );
        // Releasing frees a slot.
        t.close("ns", "i", "s2");
        drop(t.begin("ns", "i", "s3", "u", CallKind::Use, now).unwrap());
        assert_eq!(t.counts("ns"), (2, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn operation_limit_counts_calls_in_flight() {
        let t = SessionTable::new(limits());
        let now = Instant::now();
        let g: Vec<_> = (0..3)
            .map(|_| t.begin("ns", "i", "s1", "u", CallKind::Use, now).unwrap())
            .collect();
        assert_eq!(
            t.begin("ns", "i", "s1", "u", CallKind::Use, now)
                .unwrap_err()
                .code(),
            tonic::Code::ResourceExhausted
        );
        drop(g);
        assert_eq!(t.counts("ns"), (1, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn sessions_belong_to_their_principal() {
        let t = SessionTable::new(limits());
        let now = Instant::now();
        drop(
            t.begin("ns", "i", "s1", "alice", CallKind::Use, now)
                .unwrap(),
        );
        let err = t
            .begin("ns", "i", "s1", "bob", CallKind::Use, now)
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[tokio::test(start_paused = true)]
    async fn release_of_unknown_session_does_not_open_one() {
        let t = SessionTable::new(limits());
        let g = t
            .begin("ns", "i", "s9", "u", CallKind::Release, Instant::now())
            .unwrap();
        assert_eq!(t.counts("ns"), (0, 1));
        drop(g);
        assert_eq!(t.counts("ns"), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_sessions_are_reaped_but_busy_ones_are_not() {
        let t = SessionTable::new(limits());
        drop(
            t.begin("ns", "i", "idle", "u", CallKind::Use, Instant::now())
                .unwrap(),
        );
        let busy = t
            .begin("ns", "i", "busy", "u", CallKind::Use, Instant::now())
            .unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        let reaped = t.reap(Instant::now());
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].session_id, "idle");
        assert_eq!(reaped[0].user_id, "u");
        drop(busy);
        assert!(
            t.reap(Instant::now()).is_empty(),
            "just ended: not idle yet"
        );
        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(t.reap(Instant::now())[0].session_id, "busy");
        assert_eq!(t.counts("ns"), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_instances_are_reported_once() {
        let t = SessionTable::new(limits());
        drop(
            t.begin("ns", "i", "s1", "u", CallKind::Use, Instant::now())
                .unwrap(),
        );
        tokio::time::advance(Duration::from_secs(121)).await;
        // Still has a session (not reaped yet): not idle.
        assert!(t.idle_instances(Instant::now()).is_empty());
        t.reap(Instant::now());
        assert_eq!(
            t.idle_instances(Instant::now()),
            vec![("ns".to_owned(), "i".to_owned())]
        );
        assert!(t.idle_instances(Instant::now()).is_empty());
        // Used again: can be reported again later.
        drop(
            t.begin("ns", "i", "s2", "u", CallKind::Use, Instant::now())
                .unwrap(),
        );
        t.close("ns", "i", "s2");
        tokio::time::advance(Duration::from_secs(121)).await;
        assert_eq!(t.idle_instances(Instant::now()).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn clones_count_against_the_limit() {
        let t = SessionTable::new(limits());
        let now = Instant::now();
        drop(t.begin("ns", "i", "s1", "u", CallKind::Use, now).unwrap());
        t.open_clone("ns", "i", "s2", "u", now).unwrap();
        assert_eq!(
            t.open_clone("ns", "i", "s3", "u", now).unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        let listed = t.sessions("ns", now);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1].session_id, "s2");
    }

    #[test]
    fn limits_parse_with_defaults() {
        let l: Limits = toml::from_str("max_sessions = 3\nidle_timeout = 90").unwrap();
        assert_eq!(l.max_sessions, 3);
        assert_eq!(l.idle_timeout, Duration::from_secs(90));
        assert_eq!(l.max_operations, 16);
    }
}
