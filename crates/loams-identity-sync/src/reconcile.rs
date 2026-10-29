//! The reconciliation (design 76 §8.1, D1608, D1623): the backstop for a lost or misapplied
//! event.
//!
//! For each org the sync knows of, it lists the user grants on the Loams project in Zitadel,
//! reads the `member`, `admin` and `owner` tuples on `org:<org>` from OpenFGA, and diffs them.
//! Zitadel's listing is a read model that can lag its events by a moment, so a difference is
//! repaired only when the listing and the sync's own event state agree on it; otherwise it is
//! counted as drift and left for the next step or run. Drift above 0 is an alert.

use std::collections::BTreeSet;

use loams_authz::Tuple;

use crate::{IdentitySync, ROLE_KEYS, Result, role_tuples};

/// Grants per `ListUserGrants` page.
const PAGE: u32 = 100;

/// What one reconciliation found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub orgs: usize,
    /// Tuples written because Zitadel and the event state grant them and OpenFGA lacked them.
    pub written: usize,
    /// Tuples deleted because neither Zitadel nor the event state grants them.
    pub deleted: usize,
    /// Differences left alone because Zitadel's listing and the event state disagree.
    pub drift: usize,
}

impl Report {
    pub fn log(&self) {
        let drift = self.drift + self.written + self.deleted;
        if drift > 0 {
            tracing::warn!(
                target: "loams_identity_sync",
                orgs = self.orgs, written = self.written, deleted = self.deleted,
                unresolved = self.drift, drift,
                "reconciliation found drift"
            );
        } else {
            tracing::info!(target: "loams_identity_sync", orgs = self.orgs, drift = 0, "reconciled");
        }
    }
}

type Membership = BTreeSet<(String, String)>;

/// Reconciles every org the sync's state names.
///
/// # Errors
///
/// Zitadel, OpenFGA or the state store failing.
pub async fn reconcile(sync: &IdentitySync) -> Result<Report> {
    let state = sync.store.load().await?.unwrap_or_default();
    let mut report = Report::default();
    for org in state.orgs() {
        report.orgs += 1;
        // What Zitadel lists now.
        let mut listed: Membership = BTreeSet::new();
        let mut offset = 0;
        loop {
            let page = sync
                .zitadel
                .list_user_grants(&org, &sync.config.project_id, offset, PAGE)
                .await?;
            let n = page.len();
            for g in page.into_iter().filter(|g| g.active()) {
                if state.disabled.contains_key(&g.user_id) {
                    continue;
                }
                for role in g
                    .role_keys
                    .iter()
                    .filter(|r| ROLE_KEYS.contains(&r.as_str()))
                {
                    listed.insert((g.user_id.clone(), role.clone()));
                }
            }
            if n < PAGE as usize {
                break;
            }
            offset += n as u64;
        }
        // What the events applied give.
        let users: BTreeSet<String> = state
            .grants
            .values()
            .filter(|g| g.org == org)
            .map(|g| g.user.clone())
            .collect();
        let applied: Membership = users
            .iter()
            .flat_map(|u| state.roles(u, &org).into_iter().map(|r| (u.clone(), r)))
            .collect();
        // What OpenFGA holds.
        let object = format!("org:{org}");
        let mut held: Membership = BTreeSet::new();
        for role in ROLE_KEYS {
            for t in sync.tuples.read(None, Some(role), &object).await? {
                if let Some(user) = t.user.strip_prefix("user:") {
                    held.insert((user.to_owned(), role.to_owned()));
                }
            }
        }
        let mut writes: Vec<Tuple> = Vec::new();
        let mut deletes: Vec<Tuple> = Vec::new();
        for (user, role) in listed.union(&applied) {
            if held.contains(&(user.clone(), role.clone())) {
                continue;
            }
            let tuple = role_tuples(user, &org, &BTreeSet::from([role.clone()]));
            if listed.contains(&(user.clone(), role.clone()))
                && applied.contains(&(user.clone(), role.clone()))
            {
                writes.extend(tuple);
            } else {
                report.drift += 1;
            }
        }
        for (user, role) in &held {
            let key = (user.clone(), role.clone());
            if listed.contains(&key) && applied.contains(&key) {
                continue;
            }
            if !listed.contains(&key) && !applied.contains(&key) {
                deletes.extend(role_tuples(user, &org, &BTreeSet::from([role.clone()])));
            } else {
                report.drift += 1;
            }
        }
        sync.tuples.write(&writes, &deletes).await?;
        report.written += writes.len();
        report.deleted += deletes.len();
    }
    Ok(report)
}
