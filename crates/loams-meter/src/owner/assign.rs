//! Who owns which `_system/_meter` partition (design §63 §11.1, §11.5).
//!
//! Each `meter` instance holds a fenced lease per partition it owns, in the
//! metastore, under [`partition_key`]. The lease's epoch is the **owner
//! epoch**: it grows by one whenever another instance takes the partition,
//! every lease grant carries it, and a gate drops grants from an older one.
//!
//! Instances also keep a short membership lease ([`member_key`]), so each
//! knows how many are live and takes its fair share, `ceil(P / members)`:
//! adding an instance makes the others release their excess, which the new
//! one then takes. [`plan`] is the pure decision; [`Assigner`] runs it
//! against a [`PartitionLeases`] store.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_common::meta::{ApplyError, Consistency, MetaError, MetaStore};

/// The `_system/_meter` stream's partition count (§63 §7.1).
pub const METER_PARTITIONS: u32 = 32;

/// The metastore key prefix of the meter's leases.
pub const LEASE_PREFIX: &str = "meter/counters/";

/// How long a partition lease lasts without a renewal.
pub const PARTITION_TTL: Duration = Duration::from_secs(15);

/// How often held leases are renewed and the assignment re-planned.
pub const RENEW_EVERY: Duration = Duration::from_secs(5);

/// The lease key of partition `p`.
pub fn partition_key(p: u32) -> String {
    format!("{LEASE_PREFIX}partition/{p:03}")
}

/// The membership lease key of instance `owner`.
pub fn member_key(owner: &str) -> String {
    format!("{LEASE_PREFIX}member/{owner}")
}

/// The partition a lease key names, if it is a partition key.
pub fn partition_of_key(key: &str) -> Option<u32> {
    key.strip_prefix(LEASE_PREFIX)?
        .strip_prefix("partition/")?
        .parse()
        .ok()
}

/// The owner of the counters for `subject`: its `_system/_meter` partition,
/// Kafka's murmur2 of the key, as the stream's default partitioner (§63
/// §11.1), so a gate computes it without asking.
pub fn partition_of(subject: &str, partitions: u32) -> u32 {
    loams_cloudevents::partition::kafka_partition(subject.as_bytes(), partitions)
}

/// A lease as the store reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseView {
    /// `None` once released.
    pub holder: Option<String>,
    pub epoch: u64,
    pub deadline_ms: u64,
}

impl LeaseView {
    /// Held by someone at `now_ms`.
    pub fn is_held_at(&self, now_ms: u64) -> bool {
        self.holder.is_some() && now_ms < self.deadline_ms
    }
}

/// A lease this instance holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fenced {
    pub epoch: u64,
    pub deadline_ms: u64,
}

/// Why a lease operation failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// Another instance holds it.
    #[error("held by {0}")]
    Held(String),
    /// This instance no longer holds it at that epoch.
    #[error("lease lost")]
    Lost,
    #[error("lease store unavailable: {0}")]
    Unavailable(String),
}

/// The lease operations the assigner needs: the metastore's (§02 §7)
/// through [`MetaLeases`], or [`MemLeases`] in tests.
#[async_trait]
pub trait PartitionLeases: Send + Sync + fmt::Debug {
    fn now_ms(&self) -> u64;
    async fn acquire(&self, key: &str, owner: &str, ttl: Duration) -> Result<Fenced, LeaseError>;
    async fn renew(
        &self,
        key: &str,
        owner: &str,
        epoch: u64,
        ttl: Duration,
    ) -> Result<Fenced, LeaseError>;
    async fn release(&self, key: &str, owner: &str, epoch: u64) -> Result<(), LeaseError>;
    /// Every lease under `prefix`, released and expired ones included.
    async fn list(&self, prefix: &str) -> Result<Vec<(String, LeaseView)>, LeaseError>;
}

/// [`PartitionLeases`] on the metastore.
#[derive(Debug, Clone)]
pub struct MetaLeases(pub Arc<dyn MetaStore>);

fn lease_error(e: MetaError) -> LeaseError {
    match e {
        MetaError::Rejected(ApplyError::LeaseHeld { owner, .. }) => LeaseError::Held(owner),
        MetaError::Rejected(ApplyError::LeaseLost { .. } | ApplyError::Fenced { .. }) => {
            LeaseError::Lost
        }
        other => LeaseError::Unavailable(other.to_string()),
    }
}

#[async_trait]
impl PartitionLeases for MetaLeases {
    fn now_ms(&self) -> u64 {
        self.0.now_ms()
    }

    async fn acquire(&self, key: &str, owner: &str, ttl: Duration) -> Result<Fenced, LeaseError> {
        let g = self
            .0
            .acquire_lease(key, owner, ttl)
            .await
            .map_err(lease_error)?;
        Ok(Fenced {
            epoch: g.epoch,
            deadline_ms: g.deadline_ms,
        })
    }

    async fn renew(
        &self,
        key: &str,
        owner: &str,
        epoch: u64,
        ttl: Duration,
    ) -> Result<Fenced, LeaseError> {
        let g = self
            .0
            .renew_lease(key, owner, epoch, ttl)
            .await
            .map_err(lease_error)?;
        Ok(Fenced {
            epoch: g.epoch,
            deadline_ms: g.deadline_ms,
        })
    }

    async fn release(&self, key: &str, owner: &str, epoch: u64) -> Result<(), LeaseError> {
        self.0
            .release_lease(key, owner, epoch)
            .await
            .map_err(lease_error)
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, LeaseView)>, LeaseError> {
        let all = self
            .0
            .leases_with_prefix(Consistency::Linearizable, prefix)
            .await
            .map_err(lease_error)?;
        Ok(all
            .into_iter()
            .map(|(k, l)| {
                (
                    k,
                    LeaseView {
                        holder: l.owner,
                        epoch: l.epoch,
                        deadline_ms: l.deadline_ms,
                    },
                )
            })
            .collect())
    }
}

/// What the planner decided for one round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// Take a free or expired partition.
    Acquire(u32),
    /// Keep a partition this instance holds.
    Renew { partition: u32, epoch: u64 },
    /// Give up a partition (over the fair share, or lost).
    Release { partition: u32, epoch: u64 },
}

/// One round's decision, from the leases as listed.
///
/// - Every partition this instance still holds (holder = `me`, same epoch as
///   in `held`) is renewed, up to the fair share `ceil(partitions / members)`;
///   the highest-numbered ones beyond it are released, but only when another
///   member is live to take them.
/// - Below the share, free or expired partitions are acquired, lowest first.
pub fn plan(
    me: &str,
    partitions: u32,
    members: usize,
    now_ms: u64,
    leases: &BTreeMap<u32, LeaseView>,
    held: &BTreeMap<u32, u64>,
) -> Vec<Action> {
    let members = members.max(1) as u32;
    let share = partitions.div_ceil(members) as usize;
    let mine: Vec<(u32, u64)> = held
        .iter()
        .filter(|(p, epoch)| {
            leases
                .get(p)
                .is_some_and(|l| l.holder.as_deref() == Some(me) && l.epoch == **epoch)
        })
        .map(|(p, e)| (*p, *e))
        .collect();
    let mut actions = Vec::new();
    for (i, (p, epoch)) in mine.iter().enumerate() {
        if i < share || members == 1 {
            actions.push(Action::Renew {
                partition: *p,
                epoch: *epoch,
            });
        } else {
            actions.push(Action::Release {
                partition: *p,
                epoch: *epoch,
            });
        }
    }
    let mut want = share.saturating_sub(mine.len());
    for p in 0..partitions {
        if want == 0 {
            break;
        }
        if mine.iter().any(|(m, _)| *m == p) {
            continue;
        }
        let free = leases.get(&p).is_none_or(|l| !l.is_held_at(now_ms));
        if free {
            actions.push(Action::Acquire(p));
            want -= 1;
        }
    }
    actions
}

/// A change in what this instance owns, for the counter owners to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipChange {
    /// Now the owner of `partition` at `epoch`: rebuild its counters (§11.5).
    Gained { partition: u32, epoch: u64 },
    /// No longer the owner: drop its counters and stop granting.
    Lost { partition: u32, epoch: u64 },
}

/// Runs [`plan`] against a lease store.
#[derive(Debug)]
pub struct Assigner<L> {
    leases: L,
    me: String,
    partitions: u32,
    held: BTreeMap<u32, u64>,
}

impl<L: PartitionLeases> Assigner<L> {
    /// `me` must be unique per process incarnation (the metastore's rule).
    pub fn new(leases: L, me: impl Into<String>, partitions: u32) -> Self {
        Assigner {
            leases,
            me: me.into(),
            partitions,
            held: BTreeMap::new(),
        }
    }

    /// The partitions held now, with their epochs.
    pub fn held(&self) -> &BTreeMap<u32, u64> {
        &self.held
    }

    /// The epoch of `partition` if this instance holds it.
    pub fn epoch_of(&self, partition: u32) -> Option<u64> {
        self.held.get(&partition).copied()
    }

    /// One round: refresh membership, list the leases, apply the plan.
    /// Answers what changed. A store error loses nothing held: leases stay
    /// in `held` until a renewal is refused or their deadline passes.
    pub async fn round(&mut self) -> Result<Vec<OwnershipChange>, LeaseError> {
        // Membership: a lease of its own that expires with the instance.
        match self
            .leases
            .acquire(&member_key(&self.me), &self.me, PARTITION_TTL)
            .await
        {
            Ok(_) | Err(LeaseError::Held(_)) => {}
            Err(e) => return Err(e),
        }
        let now = self.leases.now_ms();
        let all = self.leases.list(LEASE_PREFIX).await?;
        let members = all
            .iter()
            .filter(|(k, l)| k.contains("/member/") && l.is_held_at(now))
            .count();
        let parts: BTreeMap<u32, LeaseView> = all
            .into_iter()
            .filter_map(|(k, l)| partition_of_key(&k).map(|p| (p, l)))
            .collect();
        let mut changes = Vec::new();
        // Leases taken over since the last round.
        let lost: Vec<(u32, u64)> = self
            .held
            .iter()
            .filter(|(p, e)| {
                !parts
                    .get(p)
                    .is_some_and(|l| l.holder.as_deref() == Some(&self.me) && l.epoch == **e)
            })
            .map(|(p, e)| (*p, *e))
            .collect();
        for (p, e) in lost {
            self.held.remove(&p);
            changes.push(OwnershipChange::Lost {
                partition: p,
                epoch: e,
            });
        }
        for action in plan(&self.me, self.partitions, members, now, &parts, &self.held) {
            match action {
                Action::Renew { partition, epoch } => {
                    let key = partition_key(partition);
                    match self
                        .leases
                        .renew(&key, &self.me, epoch, PARTITION_TTL)
                        .await
                    {
                        Ok(_) => {}
                        Err(LeaseError::Lost | LeaseError::Held(_)) => {
                            self.held.remove(&partition);
                            changes.push(OwnershipChange::Lost { partition, epoch });
                        }
                        Err(e) => tracing::warn!(partition, "renewing a meter partition: {e}"),
                    }
                }
                Action::Release { partition, epoch } => {
                    let key = partition_key(partition);
                    self.held.remove(&partition);
                    changes.push(OwnershipChange::Lost { partition, epoch });
                    if let Err(e) = self.leases.release(&key, &self.me, epoch).await {
                        tracing::warn!(partition, "releasing a meter partition: {e}");
                    }
                }
                Action::Acquire(partition) => {
                    let key = partition_key(partition);
                    match self.leases.acquire(&key, &self.me, PARTITION_TTL).await {
                        Ok(f) => {
                            self.held.insert(partition, f.epoch);
                            changes.push(OwnershipChange::Gained {
                                partition,
                                epoch: f.epoch,
                            });
                        }
                        Err(LeaseError::Held(_)) => {}
                        Err(e) => tracing::warn!(partition, "acquiring a meter partition: {e}"),
                    }
                }
            }
        }
        Ok(changes)
    }

    /// Releases everything (a clean shutdown), so others take over at once
    /// instead of after [`PARTITION_TTL`].
    pub async fn release_all(&mut self) -> Vec<OwnershipChange> {
        let mut changes = Vec::new();
        for (p, e) in std::mem::take(&mut self.held) {
            if let Err(err) = self.leases.release(&partition_key(p), &self.me, e).await {
                tracing::warn!(partition = p, "releasing a meter partition: {err}");
            }
            changes.push(OwnershipChange::Lost {
                partition: p,
                epoch: e,
            });
        }
        changes
    }
}

/// An in-memory [`PartitionLeases`] with a settable clock, with the
/// metastore's semantics (epoch + 1 on every change of holder).
#[derive(Debug, Default)]
pub struct MemLeases {
    state: std::sync::Mutex<MemState>,
}

#[derive(Debug, Default)]
struct MemState {
    now_ms: u64,
    leases: BTreeMap<String, LeaseView>,
}

impl MemLeases {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        let mut s = self.lock();
        s.now_ms += by.as_millis() as u64;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl PartitionLeases for MemLeases {
    fn now_ms(&self) -> u64 {
        self.lock().now_ms
    }

    async fn acquire(&self, key: &str, owner: &str, ttl: Duration) -> Result<Fenced, LeaseError> {
        let mut s = self.lock();
        let now = s.now_ms;
        let deadline_ms = now + ttl.as_millis() as u64;
        let entry = s.leases.entry(key.to_string()).or_insert(LeaseView {
            holder: None,
            epoch: 0,
            deadline_ms: 0,
        });
        if entry.is_held_at(now) {
            if entry.holder.as_deref() != Some(owner) {
                return Err(LeaseError::Held(entry.holder.clone().unwrap_or_default()));
            }
            entry.deadline_ms = deadline_ms;
        } else {
            entry.epoch += 1;
            entry.holder = Some(owner.to_string());
            entry.deadline_ms = deadline_ms;
        }
        Ok(Fenced {
            epoch: entry.epoch,
            deadline_ms,
        })
    }

    async fn renew(
        &self,
        key: &str,
        owner: &str,
        epoch: u64,
        ttl: Duration,
    ) -> Result<Fenced, LeaseError> {
        let mut s = self.lock();
        let now = s.now_ms;
        match s.leases.get_mut(key) {
            Some(l)
                if l.holder.as_deref() == Some(owner)
                    && l.epoch == epoch
                    && now < l.deadline_ms =>
            {
                l.deadline_ms = now + ttl.as_millis() as u64;
                Ok(Fenced {
                    epoch,
                    deadline_ms: l.deadline_ms,
                })
            }
            _ => Err(LeaseError::Lost),
        }
    }

    async fn release(&self, key: &str, owner: &str, epoch: u64) -> Result<(), LeaseError> {
        let mut s = self.lock();
        match s.leases.get_mut(key) {
            Some(l) if l.epoch == epoch && l.holder.as_deref() == Some(owner) => {
                l.holder = None;
                Ok(())
            }
            Some(l) if l.epoch == epoch && l.holder.is_none() => Ok(()),
            _ => Err(LeaseError::Lost),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, LeaseView)>, LeaseError> {
        Ok(self
            .lock()
            .leases
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(holder: &str, epoch: u64, deadline_ms: u64) -> LeaseView {
        LeaseView {
            holder: Some(holder.into()),
            epoch,
            deadline_ms,
        }
    }

    #[test]
    fn partition_keys_round_trip() {
        assert_eq!(partition_of_key(&partition_key(7)), Some(7));
        assert_eq!(partition_of_key(&member_key("a")), None);
    }

    #[test]
    fn subjects_partition_as_kafka_does() {
        // Kafka's murmur2 of "21" is -973932308; positive & mod 32.
        let expected = ((-973_932_308i32) as u32 & 0x7fff_ffff) % 32;
        assert_eq!(partition_of("21", 32), expected);
        assert!(partition_of("org_1", 32) < 32);
    }

    #[test]
    fn a_lone_instance_takes_every_partition() {
        let actions = plan("a", 4, 1, 0, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(
            actions,
            vec![
                Action::Acquire(0),
                Action::Acquire(1),
                Action::Acquire(2),
                Action::Acquire(3)
            ]
        );
    }

    #[test]
    fn a_second_member_makes_the_first_release_its_excess() {
        let mut leases = BTreeMap::new();
        let mut held = BTreeMap::new();
        for p in 0..4 {
            leases.insert(p, view("a", 1, 1_000));
            held.insert(p, 1);
        }
        let actions = plan("a", 4, 2, 0, &leases, &held);
        assert_eq!(
            actions,
            vec![
                Action::Renew {
                    partition: 0,
                    epoch: 1
                },
                Action::Renew {
                    partition: 1,
                    epoch: 1
                },
                Action::Release {
                    partition: 2,
                    epoch: 1
                },
                Action::Release {
                    partition: 3,
                    epoch: 1
                },
            ]
        );
        // The newcomer takes nothing held, and only up to its share.
        let b = plan("b", 4, 2, 0, &leases, &BTreeMap::new());
        assert!(b.is_empty());
    }

    #[test]
    fn expired_partitions_are_taken_over() {
        let mut leases = BTreeMap::new();
        leases.insert(0, view("dead", 3, 100));
        leases.insert(1, view("live", 1, 10_000));
        let actions = plan("b", 2, 2, 500, &leases, &BTreeMap::new());
        assert_eq!(actions, vec![Action::Acquire(0)]);
    }

    #[test]
    fn a_partition_taken_over_is_not_renewed() {
        let mut leases = BTreeMap::new();
        leases.insert(0, view("b", 2, 10_000));
        let mut held = BTreeMap::new();
        held.insert(0, 1);
        let actions = plan("a", 1, 2, 0, &leases, &held);
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[tokio::test]
    async fn failover_bumps_the_owner_epoch() {
        let leases = Arc::new(MemLeases::new());
        let mut a = Assigner::new(SharedLeases(leases.clone()), "a", 2);
        let gained = a.round().await.expect("round");
        assert_eq!(gained.len(), 2);
        assert_eq!(a.epoch_of(0), Some(1));
        // `a` dies: its leases expire, `b` takes over at epoch 2.
        leases.advance(PARTITION_TTL + Duration::from_secs(1));
        let mut b = Assigner::new(SharedLeases(leases.clone()), "b", 2);
        b.round().await.expect("round");
        assert_eq!(b.epoch_of(0), Some(2));
        assert_eq!(b.epoch_of(1), Some(2));
        // `a` comes back and learns it lost both.
        let changes = a.round().await.expect("round");
        assert!(changes.contains(&OwnershipChange::Lost {
            partition: 0,
            epoch: 1
        }));
        assert_eq!(a.epoch_of(0), None);
    }

    #[tokio::test]
    async fn two_instances_split_the_partitions() {
        let leases = Arc::new(MemLeases::new());
        let mut a = Assigner::new(SharedLeases(leases.clone()), "a", 4);
        a.round().await.expect("a");
        assert_eq!(a.held().len(), 4);
        let mut b = Assigner::new(SharedLeases(leases.clone()), "b", 4);
        b.round().await.expect("b joins");
        // `a` sees two members and releases two; `b` then takes them.
        a.round().await.expect("a rebalances");
        b.round().await.expect("b takes");
        assert_eq!(a.held().len(), 2);
        assert_eq!(b.held().len(), 2);
        assert!(a.held().keys().all(|p| !b.held().contains_key(p)));
    }

    /// `Arc<MemLeases>` as a store, for tests that share one between
    /// instances.
    #[derive(Debug, Clone)]
    struct SharedLeases(Arc<MemLeases>);

    #[async_trait]
    impl PartitionLeases for SharedLeases {
        fn now_ms(&self) -> u64 {
            self.0.now_ms()
        }
        async fn acquire(&self, k: &str, o: &str, t: Duration) -> Result<Fenced, LeaseError> {
            self.0.acquire(k, o, t).await
        }
        async fn renew(&self, k: &str, o: &str, e: u64, t: Duration) -> Result<Fenced, LeaseError> {
            self.0.renew(k, o, e, t).await
        }
        async fn release(&self, k: &str, o: &str, e: u64) -> Result<(), LeaseError> {
            self.0.release(k, o, e).await
        }
        async fn list(&self, p: &str) -> Result<Vec<(String, LeaseView)>, LeaseError> {
            self.0.list(p).await
        }
    }
}
