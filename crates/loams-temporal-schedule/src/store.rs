//! Where schedules live (D2072): one row per schedule, changed by
//! compare-and-set on its version. [`MemScheduleStore`] for tests and the
//! dev mode; [`PgScheduleStore`] on the gateway's Loams Postgres database,
//! in its own table (`migrations/0001_schedules.sql`), beside but separate
//! from `loams-temporal-store`'s schema.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use loams_temporal_proto::temporal::api::common::v1::{Memo, SearchAttributes};
use loams_temporal_proto::temporal::api::schedule::v1::Schedule;
use prost::Message as _;
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};

use crate::state::Runtime;

/// `pg_advisory_xact_lock` key of this crate's migration ("ltsched\0").
const MIGRATION_LOCK: i64 = 0x6c74_7363_6865_6400;

const MIGRATION: &str = include_str!("../migrations/0001_schedules.sql");

/// A stored schedule.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRecord {
    pub namespace_id: String,
    pub schedule_id: String,
    pub generation: String,
    /// The create request's ID (a retried create is answered, not refused).
    pub request_id: String,
    /// The schedule, with its spec in canonical form.
    pub schedule: Schedule,
    pub memo: Option<Memo>,
    pub search_attributes: Option<SearchAttributes>,
    pub runtime: Runtime,
}

impl ScheduleRecord {
    /// Whether the schedule is paused.
    pub fn paused(&self) -> bool {
        self.schedule.state.as_ref().is_some_and(|s| s.paused)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ScheduleStoreError {
    #[error("schedule already exists")]
    Exists,
    #[error("schedule not found")]
    NotFound,
    #[error("version conflict: expected {expected}, stored {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("schedule store unavailable: {0}")]
    Unavailable(String),
    #[error("stored schedule corrupt: {0}")]
    Corrupt(String),
    #[error("database error: {0}")]
    Database(String),
}

/// The schedule store.
#[async_trait]
pub trait ScheduleStore: Send + Sync + 'static {
    /// Creates a schedule at version 1. [`ScheduleStoreError::Exists`] if the
    /// ID is taken.
    async fn create(&self, rec: &ScheduleRecord) -> Result<u64, ScheduleStoreError>;
    /// The schedule and its version.
    async fn get(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<Option<(ScheduleRecord, u64)>, ScheduleStoreError>;
    /// Replaces the schedule if its version is `expected`; answers the new
    /// version.
    async fn update(&self, rec: &ScheduleRecord, expected: u64) -> Result<u64, ScheduleStoreError>;
    /// Deletes it; `false` if there was none.
    async fn delete(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<bool, ScheduleStoreError>;
    /// Up to `limit` schedules of a namespace with IDs after `after`, by ID.
    async fn list(
        &self,
        namespace_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ScheduleRecord>, ScheduleStoreError>;
    /// Up to `limit` schedules (namespace ID, schedule ID) whose timer is due
    /// at or before `before_ms`, oldest first.
    async fn due(
        &self,
        before_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, ScheduleStoreError>;
}

type Rows = BTreeMap<(String, String), (ScheduleRecord, u64)>;

/// The store in memory.
#[derive(Debug, Default)]
pub struct MemScheduleStore {
    rows: Mutex<Rows>,
}

impl MemScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn rows(&self) -> std::sync::MutexGuard<'_, Rows> {
        self.rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn key(ns: &str, id: &str) -> (String, String) {
    (ns.to_string(), id.to_string())
}

#[async_trait]
impl ScheduleStore for MemScheduleStore {
    async fn create(&self, rec: &ScheduleRecord) -> Result<u64, ScheduleStoreError> {
        let mut rows = self.rows();
        let k = key(&rec.namespace_id, &rec.schedule_id);
        if rows.contains_key(&k) {
            return Err(ScheduleStoreError::Exists);
        }
        rows.insert(k, (rec.clone(), 1));
        Ok(1)
    }

    async fn get(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<Option<(ScheduleRecord, u64)>, ScheduleStoreError> {
        Ok(self.rows().get(&key(namespace_id, schedule_id)).cloned())
    }

    async fn update(&self, rec: &ScheduleRecord, expected: u64) -> Result<u64, ScheduleStoreError> {
        let mut rows = self.rows();
        let Some((stored, version)) = rows.get_mut(&key(&rec.namespace_id, &rec.schedule_id))
        else {
            return Err(ScheduleStoreError::NotFound);
        };
        if *version != expected {
            return Err(ScheduleStoreError::Conflict {
                expected,
                actual: *version,
            });
        }
        *stored = rec.clone();
        *version += 1;
        Ok(*version)
    }

    async fn delete(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<bool, ScheduleStoreError> {
        Ok(self
            .rows()
            .remove(&key(namespace_id, schedule_id))
            .is_some())
    }

    async fn list(
        &self,
        namespace_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ScheduleRecord>, ScheduleStoreError> {
        Ok(self
            .rows()
            .iter()
            .filter(|((ns, id), _)| ns == namespace_id && after.is_none_or(|a| id.as_str() > a))
            .take(limit)
            .map(|(_, (r, _))| r.clone())
            .collect())
    }

    async fn due(
        &self,
        before_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, ScheduleStoreError> {
        let mut due: Vec<(i64, String, String)> = self
            .rows()
            .values()
            .filter_map(|(r, _)| {
                r.runtime
                    .next_wake_ms
                    .filter(|w| *w <= before_ms)
                    .map(|w| (w, r.namespace_id.clone(), r.schedule_id.clone()))
            })
            .collect();
        due.sort();
        Ok(due
            .into_iter()
            .take(limit)
            .map(|(_, n, s)| (n, s))
            .collect())
    }
}

/// The store on Postgres (a session or direct endpoint, like
/// `loams-temporal-store`). Cheap to clone.
#[derive(Debug, Clone)]
pub struct PgScheduleStore {
    pool: PgPool,
}

fn map_err(e: sqlx::Error) -> ScheduleStoreError {
    use sqlx::Error as E;
    match &e {
        E::Io(_)
        | E::Tls(_)
        | E::PoolTimedOut
        | E::PoolClosed
        | E::WorkerCrashed
        | E::Protocol(_) => ScheduleStoreError::Unavailable(e.to_string()),
        E::ColumnDecode { .. } | E::Decode(_) | E::ColumnNotFound(_) | E::TypeNotFound { .. } => {
            ScheduleStoreError::Corrupt(e.to_string())
        }
        E::Database(d) => match d.code().as_deref() {
            Some("23505") => ScheduleStoreError::Exists,
            Some("22021" | "22P05" | "22P02") => ScheduleStoreError::InvalidArgument(e.to_string()),
            Some(c) if c.starts_with("08") || c.starts_with("57P") || c == "53300" => {
                ScheduleStoreError::Unavailable(e.to_string())
            }
            _ => ScheduleStoreError::Database(e.to_string()),
        },
        _ => ScheduleStoreError::Database(e.to_string()),
    }
}

fn opt_bytes<M: prost::Message>(m: &Option<M>) -> Option<Vec<u8>> {
    m.as_ref().map(|m| m.encode_to_vec())
}

fn decode_opt<M: prost::Message + Default>(
    b: Option<Vec<u8>>,
    what: &str,
) -> Result<Option<M>, ScheduleStoreError> {
    b.map(|b| {
        M::decode(b.as_slice()).map_err(|e| ScheduleStoreError::Corrupt(format!("{what}: {e}")))
    })
    .transpose()
}

fn record(r: &PgRow) -> Result<(ScheduleRecord, u64), ScheduleStoreError> {
    let schedule: Vec<u8> = r.try_get("schedule").map_err(map_err)?;
    let runtime: serde_json::Value = r.try_get("runtime").map_err(map_err)?;
    let version: i64 = r.try_get("version").map_err(map_err)?;
    let rec = ScheduleRecord {
        namespace_id: r.try_get("namespace_id").map_err(map_err)?,
        schedule_id: r.try_get("schedule_id").map_err(map_err)?,
        generation: r.try_get("generation").map_err(map_err)?,
        request_id: r.try_get("request_id").map_err(map_err)?,
        schedule: Schedule::decode(schedule.as_slice())
            .map_err(|e| ScheduleStoreError::Corrupt(format!("schedule: {e}")))?,
        memo: decode_opt(r.try_get("memo").map_err(map_err)?, "memo")?,
        search_attributes: decode_opt(
            r.try_get("search_attributes").map_err(map_err)?,
            "search attributes",
        )?,
        runtime: serde_json::from_value(runtime)
            .map_err(|e| ScheduleStoreError::Corrupt(format!("runtime: {e}")))?,
    };
    let version = u64::try_from(version)
        .map_err(|_| ScheduleStoreError::Corrupt("negative version".into()))?;
    Ok((rec, version))
}

fn runtime_json(rec: &ScheduleRecord) -> Result<serde_json::Value, ScheduleStoreError> {
    serde_json::to_value(&rec.runtime)
        .map_err(|e| ScheduleStoreError::InvalidArgument(format!("runtime: {e}")))
}

const COLUMNS: &str = "namespace_id, schedule_id, generation, version, request_id, schedule, \
                       memo, search_attributes, runtime";

impl PgScheduleStore {
    /// Connects with a small pool of its own.
    pub async fn connect(url: &str) -> Result<Self, ScheduleStoreError> {
        let opts = PgConnectOptions::from_str(url)
            .map_err(|e| ScheduleStoreError::InvalidArgument(format!("database URL: {e}")))?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .map_err(map_err)?;
        Ok(PgScheduleStore { pool })
    }

    /// Over an existing pool.
    pub fn with_pool(pool: PgPool) -> Self {
        PgScheduleStore { pool }
    }

    /// Creates the table and index if missing (idempotent, serialised by an
    /// advisory lock).
    pub async fn migrate(&self) -> Result<(), ScheduleStoreError> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(MIGRATION))
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        tx.commit().await.map_err(map_err)
    }
}

#[async_trait]
impl ScheduleStore for PgScheduleStore {
    async fn create(&self, rec: &ScheduleRecord) -> Result<u64, ScheduleStoreError> {
        sqlx::query(
            "INSERT INTO temporal_schedules (namespace_id, schedule_id, generation, version, \
             request_id, schedule, memo, search_attributes, runtime, next_wake_ms, paused) \
             VALUES ($1, $2, $3, 1, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&rec.namespace_id)
        .bind(&rec.schedule_id)
        .bind(&rec.generation)
        .bind(&rec.request_id)
        .bind(rec.schedule.encode_to_vec())
        .bind(opt_bytes(&rec.memo))
        .bind(opt_bytes(&rec.search_attributes))
        .bind(runtime_json(rec)?)
        .bind(rec.runtime.next_wake_ms)
        .bind(rec.paused())
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(1)
    }

    async fn get(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<Option<(ScheduleRecord, u64)>, ScheduleStoreError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM temporal_schedules WHERE namespace_id = $1 AND schedule_id = $2"
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(namespace_id)
            .bind(schedule_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_err)?;
        row.as_ref().map(record).transpose()
    }

    async fn update(&self, rec: &ScheduleRecord, expected: u64) -> Result<u64, ScheduleStoreError> {
        let expected_i = i64::try_from(expected)
            .map_err(|_| ScheduleStoreError::InvalidArgument("version out of range".into()))?;
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE temporal_schedules SET version = version + 1, schedule = $4, memo = $5, \
             search_attributes = $6, runtime = $7, next_wake_ms = $8, paused = $9, \
             updated_at = now() \
             WHERE namespace_id = $1 AND schedule_id = $2 AND version = $3 \
             RETURNING version",
        )
        .bind(&rec.namespace_id)
        .bind(&rec.schedule_id)
        .bind(expected_i)
        .bind(rec.schedule.encode_to_vec())
        .bind(opt_bytes(&rec.memo))
        .bind(opt_bytes(&rec.search_attributes))
        .bind(runtime_json(rec)?)
        .bind(rec.runtime.next_wake_ms)
        .bind(rec.paused())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        if let Some(v) = updated {
            return u64::try_from(v)
                .map_err(|_| ScheduleStoreError::Corrupt("negative version".into()));
        }
        let actual: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM temporal_schedules WHERE namespace_id = $1 AND schedule_id = $2",
        )
        .bind(&rec.namespace_id)
        .bind(&rec.schedule_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        match actual {
            None => Err(ScheduleStoreError::NotFound),
            Some(a) => Err(ScheduleStoreError::Conflict {
                expected,
                actual: u64::try_from(a).unwrap_or(0),
            }),
        }
    }

    async fn delete(
        &self,
        namespace_id: &str,
        schedule_id: &str,
    ) -> Result<bool, ScheduleStoreError> {
        let r = sqlx::query(
            "DELETE FROM temporal_schedules WHERE namespace_id = $1 AND schedule_id = $2",
        )
        .bind(namespace_id)
        .bind(schedule_id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(r.rows_affected() > 0)
    }

    async fn list(
        &self,
        namespace_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ScheduleRecord>, ScheduleStoreError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM temporal_schedules \
             WHERE namespace_id = $1 AND ($2::text IS NULL OR schedule_id > $2) \
             ORDER BY schedule_id LIMIT $3"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(namespace_id)
            .bind(after)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        rows.iter().map(|r| record(r).map(|(rec, _)| rec)).collect()
    }

    async fn due(
        &self,
        before_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, ScheduleStoreError> {
        let rows = sqlx::query(
            "SELECT namespace_id, schedule_id FROM temporal_schedules \
             WHERE next_wake_ms IS NOT NULL AND next_wake_ms <= $1 \
             ORDER BY next_wake_ms LIMIT $2",
        )
        .bind(before_ms)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("namespace_id").map_err(map_err)?,
                    r.try_get("schedule_id").map_err(map_err)?,
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::schedule::v1::ScheduleState;

    fn rec(id: &str, wake: Option<i64>) -> ScheduleRecord {
        let mut runtime = Runtime::new(0);
        runtime.next_wake_ms = wake;
        ScheduleRecord {
            namespace_id: "ns".into(),
            schedule_id: id.into(),
            generation: "g".into(),
            request_id: String::new(),
            schedule: Schedule {
                state: Some(ScheduleState {
                    paused: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            memo: None,
            search_attributes: None,
            runtime,
        }
    }

    #[tokio::test]
    async fn mem_store_cas_list_and_due() {
        let s = MemScheduleStore::new();
        assert_eq!(s.create(&rec("b", Some(30))).await.unwrap(), 1);
        assert_eq!(
            s.create(&rec("b", None)).await,
            Err(ScheduleStoreError::Exists)
        );
        s.create(&rec("a", Some(10))).await.unwrap();
        s.create(&rec("c", None)).await.unwrap();
        let (got, v) = s.get("ns", "b").await.unwrap().unwrap();
        assert!(got.paused());
        assert_eq!(s.update(&got, v).await.unwrap(), 2);
        assert_eq!(
            s.update(&got, v).await,
            Err(ScheduleStoreError::Conflict {
                expected: 1,
                actual: 2
            })
        );
        let ids: Vec<_> = s
            .list("ns", Some("a"), 10)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.schedule_id)
            .collect();
        assert_eq!(ids, ["b", "c"]);
        assert_eq!(
            s.due(30, 10).await.unwrap(),
            [
                ("ns".to_string(), "a".to_string()),
                ("ns".to_string(), "b".to_string())
            ]
        );
        assert!(s.delete("ns", "a").await.unwrap());
        assert!(!s.delete("ns", "a").await.unwrap());
        assert_eq!(
            s.update(&rec("a", None), 1).await,
            Err(ScheduleStoreError::NotFound)
        );
    }

    /// Against a real Postgres when `LOAMS_TEMPORAL_TEST_PG_URL` is set (the
    /// store crate's convention); skipped otherwise.
    #[tokio::test]
    async fn pg_store_cas_list_and_due() {
        let Ok(url) = std::env::var("LOAMS_TEMPORAL_TEST_PG_URL") else {
            return;
        };
        let s = PgScheduleStore::connect(&url).await.unwrap();
        s.migrate().await.unwrap();
        s.migrate().await.unwrap();
        let ns = format!("ns-{}", std::process::id());
        let mut r = rec("x", Some(5));
        r.namespace_id = ns.clone();
        assert_eq!(s.create(&r).await.unwrap(), 1);
        assert_eq!(s.create(&r).await, Err(ScheduleStoreError::Exists));
        let (got, v) = s.get(&ns, "x").await.unwrap().unwrap();
        assert_eq!(got, r);
        assert_eq!(s.update(&got, v).await.unwrap(), 2);
        assert!(matches!(
            s.update(&got, v).await,
            Err(ScheduleStoreError::Conflict { .. })
        ));
        assert!(
            s.due(5, 1000)
                .await
                .unwrap()
                .contains(&(ns.clone(), "x".to_string()))
        );
        assert_eq!(s.list(&ns, None, 10).await.unwrap().len(), 1);
        assert!(s.delete(&ns, "x").await.unwrap());
    }
}
