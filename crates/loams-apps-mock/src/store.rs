//! The mock's in-memory state and the approval change log that watch
//! streams resume from (AP0 Ruling 3).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tokio::sync::broadcast;

use crate::proto::loams::approvals::v1::{Approval, DecideApprovalResponse};
use crate::proto::loams::devices::v1::Device;
use crate::proto::loams::notifications::v1::Notification;
use crate::proto::loams::operations::v1::{Operation, OperationState, Progress};
use crate::seed::{Seed, ts};
use buffa::MessageField;

/// How many approval changes a cursor can resume across; older cursors get
/// a fresh snapshot with `snapshot_reset`.
pub(crate) const LOG_CAPACITY: usize = 256;

/// One change to the approvals, numbered by `seq`.
#[derive(Debug, Clone)]
pub(crate) struct ApprovalChange {
    pub(crate) seq: u64,
    pub(crate) approval: Approval,
}

/// A pairing code, redeemable once and only for [`PAIRING`] (§37 §7.2.1).
#[derive(Debug, Clone)]
pub(crate) struct Pairing {
    pub(crate) user_id: String,
    pub(crate) expires_at: SystemTime,
    pub(crate) used: bool,
    /// Wrong user codes typed against this pairing, which burn it at 5.
    pub(crate) failures: u32,
}

/// How long a pairing code is valid.
pub(crate) const PAIRING: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) approvals: BTreeMap<String, Approval>,
    pub(crate) operations: BTreeMap<String, Operation>,
    pub(crate) notifications: BTreeMap<String, Notification>,
    /// Device id → (owner principal id, device).
    pub(crate) devices: BTreeMap<String, (String, Device)>,
    /// Idempotency key → the response it produced (AP0 Ruling 5).
    pub(crate) decided: HashMap<String, DecideApprovalResponse>,
    /// The approval change log, oldest first, at most `LOG_CAPACITY` long.
    pub(crate) log: VecDeque<ApprovalChange>,
    pub(crate) seq: u64,
    /// Live pairing codes by code, and the typed user code → code index.
    pub(crate) pairings: HashMap<String, Pairing>,
    pub(crate) user_codes: HashMap<String, String>,
    /// Issued refresh tokens → the device they renew.
    pub(crate) refresh_tokens: HashMap<String, String>,
    /// Issued access tokens → the device they were issued for, so `WhoAmI`
    /// names the phone that presented it and not any device of its owner.
    pub(crate) issued: HashMap<String, String>,
    /// Fake Authentik authorization codes → the PKCE challenge they bind.
    pub(crate) authz_codes: HashMap<String, String>,
    /// Push target id → the APNs/FCM token or endpoint URL it was given.
    pub(crate) push_tokens: HashMap<String, String>,
    /// What a push gateway would have received, oldest first.
    pub(crate) push_log: Vec<crate::oauth::PushRecord>,
    /// Numbers the push log, kept apart from `seq` so a push does not move the
    /// approval change log's cursor.
    pub(crate) push_seq: u64,
}

#[derive(Debug)]
pub(crate) struct Store {
    pub(crate) seed: Seed,
    pub(crate) heartbeat: Duration,
    pub(crate) state: Mutex<State>,
    pub(crate) changes: broadcast::Sender<ApprovalChange>,
    /// Incremented by `POST /mock/drop-streams`; a watch stream ends when it
    /// changes, so the client reconnects and resumes.
    drops: tokio::sync::watch::Sender<u64>,
    /// What the mock's issuer is when a device cannot reach loopback, for
    /// example `http://10.0.2.2:8084` on the Android emulator.
    pub(crate) public_url: Option<String>,
}

impl Store {
    pub(crate) fn new(seed: Seed, heartbeat: Duration, public_url: Option<String>) -> Self {
        let state = State {
            approvals: seed
                .approvals
                .iter()
                .map(|a| (a.id.clone(), a.clone()))
                .collect(),
            operations: seed
                .operations
                .iter()
                .map(|o| (o.id.clone(), o.clone()))
                .collect(),
            notifications: seed
                .notifications
                .iter()
                .map(|n| (n.id.clone(), n.clone()))
                .collect(),
            devices: seed
                .devices
                .iter()
                .map(|(owner, d)| (d.id.clone(), (owner.clone(), d.clone())))
                .collect(),
            ..State::default()
        };
        let (changes, _) = broadcast::channel(LOG_CAPACITY);
        let (drops, _) = tokio::sync::watch::channel(0);
        Self {
            seed,
            heartbeat,
            state: Mutex::new(state),
            changes,
            drops,
            public_url,
        }
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The origin other devices should use to reach this mock, from
    /// `--public-url`, normalised without a trailing slash. `None` when the
    /// mock was told nothing, in which case a client falls back to the address
    /// it already dialled.
    pub(crate) fn base_url(&self) -> Option<String> {
        self.public_url
            .as_deref()
            .map(|url| url.trim_end_matches('/').to_owned())
    }

    /// Ends every open watch stream, so clients reconnect and resume the way
    /// they do across a network blip. What `POST /mock/drop-streams` is for
    /// (§37 §12): the streams are not corrupted, they are closed, and the
    /// client's resume cursor decides whether it gets a replay.
    ///
    /// Returns how many streams were open.
    pub(crate) fn drop_streams(&self) -> usize {
        self.drops.send_modify(|generation| *generation += 1);
        self.changes.receiver_count()
    }

    /// Watches for [`Store::drop_streams`]. A change ends the stream.
    pub(crate) fn dropped(&self) -> tokio::sync::watch::Receiver<u64> {
        self.drops.subscribe()
    }

    /// Records a push a gateway would have received, for `GET /mock/push-log`.
    pub(crate) fn record_push(&self, record: crate::oauth::PushRecord) {
        let mut state = self.lock();
        state.push_seq += 1;
        let seq = state.push_seq;
        state
            .push_log
            .push(crate::oauth::PushRecord { seq, ..record });
    }

    /// Advances every running operation by one step, which is what
    /// `POST /mock/tick` is for (§37 §12). An operation with progress reaches
    /// its total and succeeds.
    pub(crate) fn advance_operations(&self) {
        let mut state = self.lock();
        for operation in state.operations.values_mut() {
            if operation.state.as_known() != Some(OperationState::OPERATION_STATE_RUNNING) {
                continue;
            }
            let Some(progress) = operation.progress.as_option().cloned() else {
                continue;
            };
            let step = (progress.total / 10).max(1);
            let done = (progress.done + step).min(progress.total);
            let finished = done >= progress.total;
            operation.state = if finished {
                OperationState::OPERATION_STATE_SUCCEEDED.into()
            } else {
                OperationState::OPERATION_STATE_RUNNING.into()
            };
            operation.updated_at = ts(SystemTime::now());
            operation.progress = MessageField::some(Progress {
                done,
                phase: if finished {
                    "done".into()
                } else {
                    progress.phase.clone()
                },
                ..progress
            });
        }
    }
}

impl State {
    /// Stores a new revision of an approval and logs the change; the caller
    /// broadcasts the returned change after releasing the lock.
    pub(crate) fn record(&mut self, approval: Approval) -> ApprovalChange {
        self.seq += 1;
        let change = ApprovalChange {
            seq: self.seq,
            approval: approval.clone(),
        };
        self.approvals.insert(approval.id.clone(), approval);
        self.log.push_back(change.clone());
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
        change
    }

    /// The changes after `cursor`, or `None` when the cursor is older than
    /// the log (the stream must reset).
    pub(crate) fn since(&self, cursor: u64) -> Option<Vec<ApprovalChange>> {
        if cursor > self.seq {
            return None;
        }
        let oldest = self.log.front().map_or(self.seq + 1, |c| c.seq);
        if cursor + 1 < oldest {
            return None;
        }
        Some(
            self.log
                .iter()
                .filter(|c| c.seq > cursor)
                .cloned()
                .collect(),
        )
    }
}

/// Cursors are opaque to clients; the mock writes `c<seq>`.
pub(crate) fn cursor(seq: u64) -> String {
    format!("c{seq}")
}

pub(crate) fn parse_cursor(cursor: &str) -> Option<u64> {
    cursor.strip_prefix('c')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_replays_or_asks_for_a_reset() {
        let store = Store::new(Seed::demo(), Duration::from_secs(15), None);
        let mut state = store.lock();
        let approval = state.approvals.values().next().cloned().unwrap();
        for _ in 0..3 {
            state.record(approval.clone());
        }
        assert_eq!(state.since(1).unwrap().len(), 2);
        assert_eq!(state.since(3).unwrap().len(), 0);
        assert!(state.since(4).is_none(), "a cursor from the future resets");
        for _ in 0..LOG_CAPACITY {
            state.record(approval.clone());
        }
        assert!(state.since(1).is_none(), "an evicted cursor resets");
        assert_eq!(parse_cursor(&cursor(42)), Some(42));
        assert_eq!(parse_cursor("garbage"), None);
    }
}
