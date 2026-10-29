//! [`JobStore`] on Postgres (Loams Postgres, as the engines' metadata):
//! saved Flink SQL jobs survive a restart of the API (D1988).
//!
//! One table, `flink_saved_jobs`, keyed by `(namespace, id)`, holding the
//! whole [`SavedJob`] as `jsonb` plus the columns the store filters and
//! compares on. Updates are a compare-and-set on `version`, so two
//! lifecycle operations racing on one job cannot both win. The schema is
//! `migrations/`, embedded and applied by [`PgJobStore::migrate`] under an
//! advisory lock, recorded in `flink_schema_version`.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

use super::jobs::{JobStore, SavedJob, StoreError};

/// `pg_advisory_xact_lock` key that serialises migrations ("lflink\0\0" as
/// big-endian bytes).
const MIGRATION_LOCK: i64 = 0x6c66_6c69_6e6b_0000;

/// The embedded migrations, in order: (version, SQL).
const MIGRATIONS: &[(i32, &str)] = &[(1, include_str!("../../migrations/0001_saved_jobs.sql"))];

/// The schema version [`PgJobStore::migrate`] brings a database to.
pub const SCHEMA_VERSION: i32 = 1;

/// Saved jobs on Postgres. Cheap to clone; clones share one pool.
#[derive(Clone, Debug)]
pub struct PgJobStore {
    pool: PgPool,
}

fn backend(e: sqlx::Error) -> StoreError {
    StoreError::Backend(e.to_string())
}

fn state_name(job: &SavedJob) -> Result<String, StoreError> {
    match serde_json::to_value(job.state) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        Ok(other) => Err(StoreError::Backend(format!("job state as {other}"))),
        Err(e) => Err(StoreError::Backend(e.to_string())),
    }
}

fn version_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::Backend(format!("version {v} is too large")))
}

fn decode(row: &sqlx::postgres::PgRow) -> Result<SavedJob, StoreError> {
    let body: serde_json::Value = row.try_get("body").map_err(backend)?;
    let version: i64 = row.try_get("version").map_err(backend)?;
    let mut job: SavedJob = serde_json::from_value(body)
        .map_err(|e| StoreError::Backend(format!("a stored job does not decode: {e}")))?;
    // The column is the truth for the compare-and-set.
    job.version = u64::try_from(version)
        .map_err(|_| StoreError::Backend(format!("negative version {version}")))?;
    Ok(job)
}

impl PgJobStore {
    /// Connect to `url` (`postgres://…`) with a small pool and a 30 s
    /// statement timeout. Call [`Self::migrate`] before use.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let opts = PgConnectOptions::from_str(url).map_err(backend)?;
        Self::connect_with(opts).await
    }

    pub async fn connect_with(opts: PgConnectOptions) -> Result<Self, StoreError> {
        let opts = opts.options([("statement_timeout", "30000")]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .map_err(backend)?;
        Ok(Self { pool })
    }

    /// A store on an existing pool (the host's).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply the embedded migrations not yet applied, in one transaction
    /// under an advisory lock. Idempotent; safe from several processes.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query("SET LOCAL statement_timeout = 0")
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS flink_schema_version (
                 version    integer PRIMARY KEY,
                 applied_at timestamptz NOT NULL DEFAULT now()
             )",
        )
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        let applied: Option<i32> =
            sqlx::query_scalar("SELECT max(version) FROM flink_schema_version")
                .fetch_one(&mut *tx)
                .await
                .map_err(backend)?;
        let applied = applied.unwrap_or(0);
        if applied > SCHEMA_VERSION {
            return Err(StoreError::Backend(format!(
                "the database is at schema version {applied}, newer than this build's {SCHEMA_VERSION}"
            )));
        }
        for (version, sql) in MIGRATIONS {
            if *version <= applied {
                continue;
            }
            sqlx::raw_sql(sqlx::AssertSqlSafe(*sql))
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
            sqlx::query("INSERT INTO flink_schema_version (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
            tracing::info!(version, "loams-flink: saved-job store migrated");
        }
        tx.commit().await.map_err(backend)
    }

    /// The schema version the database is at (0 before [`Self::migrate`]).
    pub async fn schema_version(&self) -> Result<i32, StoreError> {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('flink_schema_version') IS NOT NULL")
                .fetch_one(&self.pool)
                .await
                .map_err(backend)?;
        if !exists {
            return Ok(0);
        }
        let v: Option<i32> = sqlx::query_scalar("SELECT max(version) FROM flink_schema_version")
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?;
        Ok(v.unwrap_or(0))
    }

    /// Every saved job in the `Running` state, across instances, for a
    /// namespace and instance (what the controller restores).
    pub async fn running(
        &self,
        namespace: &str,
        instance: &str,
    ) -> Result<Vec<SavedJob>, StoreError> {
        let rows = sqlx::query(
            "SELECT body, version FROM flink_saved_jobs
             WHERE namespace = $1 AND instance = $2 AND state = 'running'
             ORDER BY created_at, id",
        )
        .bind(namespace)
        .bind(instance)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(decode).collect()
    }
}

#[async_trait]
impl JobStore for PgJobStore {
    async fn insert(&self, job: SavedJob) -> Result<SavedJob, StoreError> {
        let body = serde_json::to_value(&job).map_err(|e| StoreError::Backend(e.to_string()))?;
        let done = sqlx::query(
            "INSERT INTO flink_saved_jobs (namespace, id, instance, state, version, created_at, body)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (namespace, id) DO NOTHING",
        )
        .bind(&job.namespace)
        .bind(&job.id)
        .bind(&job.instance)
        .bind(state_name(&job)?)
        .bind(version_i64(job.version)?)
        .bind(&job.created_at)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        if done.rows_affected() == 0 {
            return Err(StoreError::Exists(job.id));
        }
        Ok(job)
    }

    async fn get(&self, namespace: &str, id: &str) -> Result<Option<SavedJob>, StoreError> {
        let row = sqlx::query(
            "SELECT body, version FROM flink_saved_jobs WHERE namespace = $1 AND id = $2",
        )
        .bind(namespace)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.as_ref().map(decode).transpose()
    }

    async fn list(&self, namespace: &str, instance: &str) -> Result<Vec<SavedJob>, StoreError> {
        let rows = sqlx::query(
            "SELECT body, version FROM flink_saved_jobs
             WHERE namespace = $1 AND instance = $2
             ORDER BY created_at, id",
        )
        .bind(namespace)
        .bind(instance)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(decode).collect()
    }

    async fn update(&self, mut job: SavedJob) -> Result<SavedJob, StoreError> {
        let expected = version_i64(job.version)?;
        job.version += 1;
        job.updated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let body = serde_json::to_value(&job).map_err(|e| StoreError::Backend(e.to_string()))?;
        let done = sqlx::query(
            "UPDATE flink_saved_jobs
             SET state = $3, version = $4, body = $5
             WHERE namespace = $1 AND id = $2 AND version = $6",
        )
        .bind(&job.namespace)
        .bind(&job.id)
        .bind(state_name(&job)?)
        .bind(version_i64(job.version)?)
        .bind(body)
        .bind(expected)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        if done.rows_affected() == 1 {
            return Ok(job);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM flink_saved_jobs WHERE namespace = $1 AND id = $2)",
        )
        .bind(&job.namespace)
        .bind(&job.id)
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        Err(if exists {
            StoreError::Conflict(job.id)
        } else {
            StoreError::NotFound(job.id)
        })
    }

    async fn delete(&self, namespace: &str, id: &str) -> Result<bool, StoreError> {
        let done = sqlx::query("DELETE FROM flink_saved_jobs WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(done.rows_affected() == 1)
    }
}
