//! In-memory stores, for tests and for a sync whose state need not survive a restart.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;
use loams_authz::Tuple;

use crate::{Kind, Principals, Result, State, StateStore, Tuples};

fn key(t: &Tuple) -> (String, String, String) {
    (t.user.clone(), t.relation.clone(), t.object.clone())
}

/// Tuples in memory, with OpenFGA's `Read` filter semantics.
#[derive(Debug, Default)]
pub struct MemoryTuples(Mutex<BTreeSet<(String, String, String)>>);

impl MemoryTuples {
    /// Every tuple, as `user relation object`, sorted.
    pub fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(u, r, o)| format!("{u} {r} {o}"))
            .collect()
    }
}

#[async_trait]
impl Tuples for MemoryTuples {
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()> {
        let mut set = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for t in deletes {
            set.remove(&key(t));
        }
        for t in writes {
            set.insert(key(t));
        }
        Ok(())
    }

    async fn read(
        &self,
        user: Option<&str>,
        relation: Option<&str>,
        object: &str,
    ) -> loams_authz::Result<Vec<Tuple>> {
        let set = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let type_only = object.ends_with(':');
        Ok(set
            .iter()
            .filter(|(u, r, o)| {
                user.is_none_or(|x| x == u)
                    && relation.is_none_or(|x| x == r)
                    && if type_only {
                        o.starts_with(object)
                    } else {
                        o == object
                    }
            })
            .map(|(u, r, o)| Tuple::new(u.clone(), r.clone(), o.clone()))
            .collect())
    }
}

/// Principal links in memory: `(issuer, subject) → (kind, disabled)`.
#[derive(Debug, Default)]
pub struct MemoryPrincipals(Mutex<BTreeMap<(String, String), (Kind, bool)>>);

impl MemoryPrincipals {
    pub fn get(&self, issuer: &str, subject: &str) -> Option<(Kind, bool)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(issuer.to_owned(), subject.to_owned()))
            .copied()
    }

    /// Links `subject` as `kind`, replacing any link (as `Agents::create` does).
    pub fn put(&self, issuer: &str, subject: &str, kind: Kind) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((issuer.to_owned(), subject.to_owned()), (kind, false));
    }
}

#[async_trait]
impl Principals for MemoryPrincipals {
    async fn link_if_absent(&self, issuer: &str, subject: &str, kind: Kind) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((issuer.to_owned(), subject.to_owned()))
            .or_insert((kind, false));
        Ok(())
    }

    async fn set_disabled(
        &self,
        issuer: &str,
        subject: &str,
        kind_if_absent: Kind,
        disabled: bool,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((issuer.to_owned(), subject.to_owned()))
            .or_insert((kind_if_absent, false))
            .1 = disabled;
        Ok(())
    }
}

/// The sync's state in memory.
#[derive(Debug, Default)]
pub struct MemoryState(Mutex<Option<State>>);

#[async_trait]
impl StateStore for MemoryState {
    async fn load(&self) -> Result<Option<State>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    async fn save(&self, state: &State) -> Result<()> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state.clone());
        Ok(())
    }
}
