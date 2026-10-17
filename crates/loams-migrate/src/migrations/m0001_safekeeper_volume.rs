//! `0001-safekeeper-volume` (design §54 §9.3, D945; UP1 Task 10): 0.1.0
//! gives `safekeeper1` a volume. preview.2's safekeeper keeps its WAL in the
//! container's filesystem, so recreating it loses whatever the pageserver
//! has not uploaded yet. This migration drains first (Task 9): with the
//! computes stopped, it waits until every timeline's uploaded LSN reaches the
//! safekeeper's flushed LSN, then records that the CLI may recreate
//! `safekeeper1`. A timeline that does not drain fails the migration
//! (`postgres_not_drained`), and `loams upgrade` stops there: it recreates
//! `safekeeper1` only after `migrate` succeeds (Task 13). The included
//! Postgres compose file cannot depend on `migrate`, so a hand-run
//! `compose up` across this step is not a supported upgrade (the upgrade
//! guide says so).
//!
//! `Reversible`: preview.2's compose file ignores the volume, and its
//! safekeeper starts empty on drained timelines. A stack without Postgres
//! has nothing to drain.

use std::time::Duration;

use async_trait::async_trait;

use crate::cx::HostChange;
use crate::formats::{FormatId, Store};
use crate::migration::{Class, Cursor, Cx, Estimate, Migration, MigrationError, Step};
use crate::snapshot::postgres::{DRAIN_TIMEOUT, PostgresStorage};

/// See the module documentation.
#[derive(Debug)]
pub struct SafekeeperVolume {
    /// How long the drain may take.
    pub timeout: Duration,
    /// How often the drain polls.
    pub poll: Duration,
}

/// The compiled instance.
pub static SAFEKEEPER_VOLUME: SafekeeperVolume = SafekeeperVolume {
    timeout: DRAIN_TIMEOUT,
    poll: Duration::from_secs(2),
};

fn failed(e: impl std::fmt::Display) -> MigrationError {
    let message = e.to_string();
    let code = if message.contains("postgres_not_drained") {
        "postgres_not_drained"
    } else {
        "postgres"
    };
    MigrationError::new(code, message)
}

#[async_trait]
impl Migration for SafekeeperVolume {
    fn id(&self) -> &'static str {
        "0001-safekeeper-volume"
    }

    fn store(&self) -> Store {
        Store::Host
    }

    fn formats(&self) -> &'static [(FormatId, u16, u16)] {
        &[]
    }

    fn class(&self) -> Class {
        Class::Reversible
    }

    fn estimate(&self, _cx: &Cx) -> Result<Estimate, MigrationError> {
        Ok(Estimate {
            rows: 0,
            bytes: 0,
            seconds: 30,
        })
    }

    async fn apply(&self, cx: &mut Cx, _cursor: Option<Cursor>) -> Result<Step, MigrationError> {
        let Some((pageserver, safekeeper)) = cx.postgres.clone() else {
            return Ok(Step::Done);
        };
        let storage = PostgresStorage::new(&pageserver, &safekeeper).map_err(failed)?;
        if !storage.present().await.map_err(failed)? {
            return Ok(Step::Done);
        }
        storage
            .drain(self.timeout, self.poll)
            .await
            .map_err(failed)?;
        cx.before_switch()?;
        cx.host_cx().request(HostChange::Recreate {
            service: "safekeeper1".to_string(),
            reason: "give it the safekeeper-data volume; its WAL is drained".to_string(),
        });
        Ok(Step::Done)
    }

    async fn verify(&self, _cx: &Cx) -> Result<(), MigrationError> {
        Ok(())
    }

    async fn abandon(&self, _cx: &mut Cx) -> Result<(), MigrationError> {
        Ok(())
    }
}
