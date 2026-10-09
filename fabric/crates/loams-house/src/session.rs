//! House sessions (HS1 Task 4; FL2 Task 4, §49 §10.3).
//!
//! A session lives in the front, keyed by the user **and** its `session_id`
//! (R3.12), and holds what outlives a statement: its settings (from `SET`), its
//! current database (from `USE`), its last consistency token, and — once it creates
//! a temporary table — the worker it is pinned to. Because the settings live here,
//! a session without temporary tables runs each statement on any worker and
//! survives that worker's recycling; one with temporary tables runs on its pinned
//! worker, whose session connection holds them (`max_pinned_workers_per_namespace`,
//! else `202`, Q690).
//!
//! `session_check=1` on an unknown session is `372 SESSION_NOT_FOUND`; a session is
//! used by one statement at a time (`373 SESSION_IS_LOCKED`); a session idle past its
//! `session_timeout` (default 60 s, at most 3 600) ends, and its pinned worker is
//! released. At most [`MAX_LIVE_SESSIONS`] live at once (FL2 Ruling 10), else `202`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use loams_house_ipc::SessionRef;
use sha2::{Digest, Sha256};

use crate::errors::{ChError, HouseError};
use crate::settings::Settings;

/// Live sessions per front (FL2 Ruling 10).
pub const MAX_LIVE_SESSIONS: usize = 1024;
/// `session_timeout`'s default, in seconds.
pub const SESSION_TIMEOUT_DEFAULT: u64 = 60;
/// `session_timeout`'s largest value, in seconds.
pub const SESSION_TIMEOUT_MAX: u64 = 3600;

/// What a request says about its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionParams {
    /// `session_id`.
    pub id: String,
    /// `session_timeout`.
    pub timeout: Duration,
    /// `session_check=1`: the session must exist.
    pub check: bool,
    /// `close_session=1`: end it after this statement.
    pub close: bool,
}

impl SessionParams {
    /// The session parameters of a request, if it names a session.
    pub fn from_params(params: &[(String, String)]) -> Result<Option<Self>, HouseError> {
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let Some(id) = get("session_id").filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        let seconds = match get("session_timeout") {
            None => SESSION_TIMEOUT_DEFAULT,
            Some(raw) => raw
                .parse::<u64>()
                .ok()
                .filter(|t| *t <= SESSION_TIMEOUT_MAX)
                .ok_or_else(|| {
                    HouseError::from(ChError::bad_arguments(format!(
                        "Invalid session timeout: '{raw}', max is {SESSION_TIMEOUT_MAX} seconds"
                    )))
                })?,
        };
        Ok(Some(Self {
            id: id.to_string(),
            timeout: Duration::from_secs(seconds),
            check: get("session_check") == Some("1"),
            close: get("close_session") == Some("1"),
        }))
    }
}

/// A session's state.
#[derive(Debug, Clone)]
pub struct HouseSession {
    /// `session_id`.
    pub id: String,
    /// The user it belongs to.
    pub user: String,
    /// The user's namespace.
    pub namespace: u64,
    /// The current database (`USE`).
    pub database: String,
    /// Settings from `SET`.
    pub settings: Settings,
    /// The last consistency token (realtime tables, HS1 Task 14).
    pub last_token: Option<String>,
    /// The worker holding its temporary tables, if any.
    pub pinned: Option<String>,
    timeout: Duration,
    last_used: Instant,
}

impl HouseSession {
    /// The worker-side session for a pinned session (R3.12's key).
    pub fn worker_ref(&self, close: bool) -> SessionRef {
        SessionRef {
            key: format!("{}/{}/{}", self.user.len(), self.user, self.id),
            timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
            close,
        }
    }
}

/// The opaque key a load balancer routes a session by (`X-Loams-Session-Affinity`):
/// the same user and `session_id` always give the same key.
pub fn affinity_key(user: &str, id: &str) -> String {
    let digest = Sha256::digest(format!("{}/{user}/{id}", user.len()).as_bytes());
    digest[..12].iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug)]
struct Slot {
    busy: AtomicBool,
    state: Mutex<HouseSession>,
}

impl Slot {
    fn state(&self) -> MutexGuard<'_, HouseSession> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

type Key = (String, String);

/// Called with a worker id when a pinned session ends.
pub type Unpin = Arc<dyn Fn(&str) + Send + Sync>;

/// The front's sessions.
pub struct SessionTable {
    sessions: Mutex<HashMap<Key, Arc<Slot>>>,
    max_live: usize,
    unpin: Unpin,
}

impl std::fmt::Debug for SessionTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionTable")
            .field("live", &self.live())
            .field("max_live", &self.max_live)
            .finish_non_exhaustive()
    }
}

impl SessionTable {
    /// An empty table; `unpin` releases a pinned worker when its session ends.
    pub fn new(max_live: usize, unpin: Unpin) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            max_live,
            unpin,
        })
    }

    fn map(&self) -> MutexGuard<'_, HashMap<Key, Arc<Slot>>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sessions live now.
    pub fn live(&self) -> usize {
        self.map().len()
    }

    /// Ends sessions idle past their timeout (not those in use), releasing their
    /// pinned workers.
    pub fn sweep(&self) {
        let now = Instant::now();
        let mut ended = Vec::new();
        self.map().retain(|_, slot| {
            if slot.busy.load(Ordering::Acquire) {
                return true;
            }
            let state = slot.state();
            if now.duration_since(state.last_used) <= state.timeout {
                return true;
            }
            ended.extend(state.pinned.clone());
            false
        });
        for worker in ended {
            (self.unpin)(&worker);
        }
    }

    /// The session a request names, for this statement only: `372`, `373` or `202`
    /// when it cannot be had.
    pub fn checkout(
        self: &Arc<Self>,
        user: &str,
        namespace: u64,
        params: &SessionParams,
    ) -> Result<SessionGuard, HouseError> {
        self.sweep();
        let key = (user.to_string(), params.id.clone());
        let slot = {
            let mut map = self.map();
            match map.get(&key) {
                Some(slot) => Arc::clone(slot),
                None => {
                    if params.check {
                        return Err(HouseError::from(ChError::session_not_found(format!(
                            "Session {} not found",
                            params.id
                        ))));
                    }
                    if map.len() >= self.max_live {
                        return Err(HouseError::from(ChError::too_many_simultaneous_queries(
                            format!("Too many sessions: at most {} are live", self.max_live),
                        )));
                    }
                    let slot = Arc::new(Slot {
                        busy: AtomicBool::new(false),
                        state: Mutex::new(HouseSession {
                            id: params.id.clone(),
                            user: user.to_string(),
                            namespace,
                            database: "default".to_string(),
                            settings: Settings::new(),
                            last_token: None,
                            pinned: None,
                            timeout: params.timeout,
                            last_used: Instant::now(),
                        }),
                    });
                    map.insert(key.clone(), Arc::clone(&slot));
                    slot
                }
            }
        };
        if slot.busy.swap(true, Ordering::AcqRel) {
            return Err(HouseError::from(ChError::session_is_locked(format!(
                "Session {} is locked by a concurrent client",
                params.id
            ))));
        }
        slot.state().timeout = params.timeout;
        Ok(SessionGuard {
            table: Arc::clone(self),
            slot,
            key,
            close: params.close,
        })
    }
}

/// A session checked out for one statement. Dropping it marks it used and free;
/// with `close_session=1`, it ends the session (releasing its pinned worker).
pub struct SessionGuard {
    table: Arc<SessionTable>,
    slot: Arc<Slot>,
    key: Key,
    close: bool,
}

impl std::fmt::Debug for SessionGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGuard")
            .field("id", &self.key.1)
            .field("close", &self.close)
            .finish()
    }
}

impl SessionGuard {
    /// The session's state.
    pub fn state(&self) -> MutexGuard<'_, HouseSession> {
        self.slot.state()
    }

    /// Whether this statement ends the session.
    pub fn closing(&self) -> bool {
        self.close
    }

    /// `X-Loams-Session-Affinity`.
    pub fn affinity_key(&self) -> String {
        affinity_key(&self.key.0, &self.key.1)
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let pinned = {
            let mut state = self.slot.state();
            state.last_used = Instant::now();
            state.pinned.clone()
        };
        self.slot.busy.store(false, Ordering::Release);
        if self.close {
            self.table.map().remove(&self.key);
            if let Some(worker) = pinned {
                (self.table.unpin)(&worker);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(id: &str, extra: &[(&str, &str)]) -> SessionParams {
        let mut all = vec![("session_id".to_string(), id.to_string())];
        all.extend(extra.iter().map(|(a, b)| (a.to_string(), b.to_string())));
        SessionParams::from_params(&all).expect("ok").expect("some")
    }

    fn table(unpinned: Arc<Mutex<Vec<String>>>) -> Arc<SessionTable> {
        SessionTable::new(
            2,
            Arc::new(move |w: &str| unpinned.lock().expect("lock").push(w.to_string())),
        )
    }

    #[test]
    fn check_lock_cap_close_and_expiry() {
        let unpinned = Arc::new(Mutex::new(Vec::new()));
        let table = table(Arc::clone(&unpinned));
        assert_eq!(
            table
                .checkout("alice", 1, &params("s", &[("session_check", "1")]))
                .expect_err("unknown")
                .code(),
            372
        );
        let held = table.checkout("alice", 1, &params("s", &[])).expect("new");
        assert_eq!(
            table
                .checkout("alice", 1, &params("s", &[]))
                .expect_err("locked")
                .code(),
            373
        );
        // Another user's session of the same id is another session.
        let carol = table
            .checkout("carol", 1, &params("s", &[]))
            .expect("carol's own");
        assert_eq!(
            table
                .checkout("bob", 1, &params("x", &[]))
                .expect_err("cap")
                .code(),
            202
        );
        drop(carol);
        held.state().pinned = Some("w1".to_string());
        drop(held);
        let again = table
            .checkout(
                "alice",
                1,
                &params("s", &[("session_check", "1"), ("close_session", "1")]),
            )
            .expect("exists");
        drop(again);
        assert_eq!(
            unpinned.lock().expect("lock").as_slice(),
            ["w1".to_string()],
            "closing unpins"
        );
        assert_eq!(table.live(), 1, "carol's remains");

        let short = table
            .checkout("dave", 1, &params("t", &[("session_timeout", "0")]))
            .expect("new");
        short.state().pinned = Some("w2".to_string());
        drop(short);
        std::thread::sleep(Duration::from_millis(10));
        table.sweep();
        assert!(
            unpinned.lock().expect("lock").contains(&"w2".to_string()),
            "expiry unpins"
        );
    }

    #[test]
    fn affinity_is_per_user_and_stable() {
        assert_eq!(affinity_key("a", "s"), affinity_key("a", "s"));
        assert_ne!(affinity_key("a", "s"), affinity_key("b", "s"));
        assert_eq!(affinity_key("a", "s").len(), 24);
    }

    #[test]
    fn timeouts() {
        let too_long = vec![
            ("session_id".to_string(), "s".to_string()),
            ("session_timeout".to_string(), "3601".to_string()),
        ];
        assert_eq!(
            SessionParams::from_params(&too_long)
                .expect_err("too long")
                .code(),
            36
        );
        assert_eq!(params("s", &[]).timeout, Duration::from_secs(60));
    }
}
