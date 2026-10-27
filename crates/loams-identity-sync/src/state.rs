//! The sync's state: where it has read to, and what the events it applied say.

use std::collections::{BTreeMap, BTreeSet};

use loams_zitadel::events::Event;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Where the sync has read to in Zitadel's event list (D1622): the last creation date read,
/// and the events at exactly that date, so a re-read from that date skips them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub at: Option<OffsetDateTime>,
    /// `(aggregate type, aggregate id, sequence)` of the events read at `at`.
    #[serde(default)]
    pub seen: BTreeSet<(String, String, u64)>,
}

impl Position {
    /// Moves past `event`; `false` when it was read already (before `at`, or at `at` and
    /// seen). Events come oldest first.
    pub fn advance(&mut self, event: &Event) -> bool {
        let key = (
            event.aggregate_type.clone(),
            event.aggregate_id.clone(),
            event.sequence,
        );
        match self.at {
            Some(at) if event.created_at < at => false,
            Some(at) if event.created_at == at => self.seen.insert(key),
            _ => {
                self.at = Some(event.created_at);
                self.seen = BTreeSet::from([key]);
                true
            }
        }
    }
}

/// A user grant on the Loams project, as its events left it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRec {
    pub user: String,
    /// The organization that owns the grant (the event's resource owner).
    pub org: String,
    /// Its org role keys (`member`, `admin`, `owner`).
    pub roles: BTreeSet<String>,
    pub active: bool,
}

/// Everything the sync keeps between steps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub position: Position,
    /// Grants on the Loams project, by grant id.
    #[serde(default)]
    pub grants: BTreeMap<String, GrantRec>,
    /// Disabled users and why (`deactivated`, `locked`, `removed`): a user is enabled again
    /// only when no reason is left.
    #[serde(default)]
    pub disabled: BTreeMap<String, BTreeSet<String>>,
    /// The last sequence applied per aggregate id: an older event is skipped.
    #[serde(default)]
    pub applied: BTreeMap<String, u64>,
}

impl State {
    /// Whether `event` is newer than the last one applied to its aggregate; records it.
    pub fn newer(&mut self, event: &Event) -> bool {
        let key = format!("{}/{}", event.aggregate_type, event.aggregate_id);
        match self.applied.get(&key) {
            Some(&seq) if event.sequence <= seq => false,
            _ => {
                self.applied.insert(key, event.sequence);
                true
            }
        }
    }

    /// The org roles `user`'s active grants give in `org`; none while the user is disabled.
    pub fn roles(&self, user: &str, org: &str) -> BTreeSet<String> {
        if self.disabled.contains_key(user) {
            return BTreeSet::new();
        }
        self.grants
            .values()
            .filter(|g| g.active && g.user == user && g.org == org)
            .flat_map(|g| g.roles.iter().cloned())
            .collect()
    }

    /// The orgs in which `user` has a grant.
    pub fn orgs_of(&self, user: &str) -> BTreeSet<String> {
        self.grants
            .values()
            .filter(|g| g.user == user)
            .map(|g| g.org.clone())
            .collect()
    }

    /// Every org a grant names.
    pub fn orgs(&self) -> BTreeSet<String> {
        self.grants.values().map(|g| g.org.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::Value;
    use time::macros::datetime;

    use super::*;

    fn event(id: &str, seq: u64, at: OffsetDateTime) -> Event {
        Event {
            kind: "user.grant.added".into(),
            aggregate_type: "usergrant".into(),
            aggregate_id: id.into(),
            resource_owner: "o".into(),
            sequence: seq,
            created_at: at,
            payload: Value::Null,
        }
    }

    #[test]
    fn reread_dedupes_by_aggregate_sequence() {
        let t1 = datetime!(2026-10-26 09:10:50.518948 UTC);
        let t2 = datetime!(2026-10-26 09:11:00 UTC);
        let mut p = Position::default();
        assert!(p.advance(&event("a", 1, t1)));
        assert!(p.advance(&event("b", 1, t1)), "two events at one date");
        // Read again from just before t1: both are skipped.
        assert!(!p.advance(&event("a", 1, t1)));
        assert!(!p.advance(&event("b", 1, t1)));
        assert!(p.advance(&event("c", 1, t1)), "a third at that date is new");
        assert!(p.advance(&event("a", 2, t2)));
        assert_eq!(p.seen.len(), 1, "only the events at the new date are kept");
        assert!(!p.advance(&event("b", 1, t1)), "older than the position");
    }

    #[test]
    fn an_older_event_of_an_aggregate_is_skipped() {
        let t = datetime!(2026-10-26 09:00:00 UTC);
        let mut s = State::default();
        assert!(s.newer(&event("g", 2, t)));
        assert!(!s.newer(&event("g", 1, t)));
        assert!(!s.newer(&event("g", 2, t)));
        assert!(s.newer(&event("g", 3, t)));
    }

    #[test]
    fn a_disabled_user_has_no_roles() {
        let mut s = State::default();
        s.grants.insert(
            "g".into(),
            GrantRec {
                user: "u".into(),
                org: "o".into(),
                roles: BTreeSet::from(["admin".into()]),
                active: true,
            },
        );
        assert_eq!(s.roles("u", "o").len(), 1);
        s.disabled
            .insert("u".into(), BTreeSet::from(["locked".into()]));
        assert!(s.roles("u", "o").is_empty());
    }
}
