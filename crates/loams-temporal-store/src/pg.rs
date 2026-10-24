//! [`PgStore`]: the store on Postgres (design §74 §5.5).
//!
//! **Endpoint:** a session-pooled or direct endpoint only, never a
//! transaction-mode pooler (§5.5 rule 5). A commit holds a row lock across
//! several statements, and the outbox wake-up is a `LISTEN` on one
//! connection for the store's lifetime. Stock Postgres 17 and Loams Postgres
//! `compute1` are both fine; Loams SQL (TiDB) and MySQL are not (§5.5 rule 6).
//!
//! Queries are built at run time (`sqlx::query`), so a build needs no
//! database. The schema is `migrations/`, embedded and applied by
//! [`PgStore::migrate`].

use std::str::FromStr;
use std::sync::Arc;
#[cfg(feature = "faults")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions, PgRow};
use sqlx::{Postgres, Row, Transaction};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{
    Commit, CurrentRun, EventRow, FIRST_EVENT_ID, Loaded, NamespaceRecord, OutboxRow, RunKey,
    STATUS_RUNNING, StartExecution, StartOutcome, Store, StoreError, VisibilityRow, check_commit,
    check_shards, shard_of,
};

/// The `NOTIFY` channel a commit with outbox rows signals; the payload is the
/// shard ID. One channel for every shard: a relay listens once and filters.
pub const OUTBOX_CHANNEL: &str = "loams_temporal_outbox";

/// `pg_advisory_xact_lock` key that serialises migrations ("ltstore\0" as
/// big-endian bytes).
const MIGRATION_LOCK: i64 = 0x6c74_7374_6f72_6500;

/// The embedded migrations, in order: (version, SQL).
const MIGRATIONS: &[(i32, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

/// The store on Postgres. Cheap to clone; the clones share one pool and one
/// listener, which stops when the last clone is dropped.
#[derive(Clone, Debug)]
pub struct PgStore {
    pool: PgPool,
    shards: u32,
    wake: Arc<watch::Sender<u64>>,
    _listener: Arc<Listener>,
    #[cfg(feature = "faults")]
    fault: Arc<AtomicBool>,
}

#[derive(Debug)]
struct Listener(JoinHandle<()>);

impl Drop for Listener {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl PgStore {
    /// Connects to `url` (a session or direct endpoint) with `shards` shards,
    /// and starts the outbox listener. If the schema exists, its shard count
    /// must equal `shards` ([`StoreError::ShardCountMismatch`], X1.R6). Call
    /// [`PgStore::migrate`] before serving.
    pub async fn connect(url: &str, shards: u32) -> Result<Self, StoreError> {
        let opts = PgConnectOptions::from_str(url)
            .map_err(|e| StoreError::InvalidArgument(format!("database URL: {e}")))?;
        Self::connect_with(opts, shards).await
    }

    /// [`PgStore::connect`] with parsed options (TLS, application name, …).
    pub async fn connect_with(opts: PgConnectOptions, shards: u32) -> Result<Self, StoreError> {
        check_shards(shards)?;
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .map_err(map_err)?;
        if let Some(stored) = stored_shards(&pool).await?
            && stored != shards
        {
            pool.close().await;
            return Err(StoreError::ShardCountMismatch {
                stored,
                configured: shards,
            });
        }
        let wake = Arc::new(watch::Sender::new(0u64));
        let task = tokio::spawn(listen(pool.clone(), wake.clone()));
        Ok(Self {
            pool,
            shards,
            wake,
            _listener: Arc::new(Listener(task)),
            #[cfg(feature = "faults")]
            fault: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn shards(&self) -> u32 {
        self.shards
    }

    /// The pool, for callers that need their own queries (health checks).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Closes the pool. The store answers [`StoreError::Unavailable`] after.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Applies the embedded migrations not yet applied, in one transaction
    /// under an advisory lock, and records them in `schema_version` with the
    /// shard count. Idempotent and safe to run from several processes at
    /// once. Refuses a database created with another shard count.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        let exists: bool = sqlx::query_scalar("SELECT to_regclass('schema_version') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        let mut applied = 0i32;
        if exists {
            let rows = sqlx::query("SELECT version, shards FROM schema_version ORDER BY version")
                .fetch_all(&mut *tx)
                .await
                .map_err(map_err)?;
            for r in rows {
                let shards = u32::try_from(r.try_get::<i32, _>("shards").map_err(map_err)?)
                    .map_err(|_| StoreError::Corrupt("negative shard count".into()))?;
                if shards != self.shards {
                    return Err(StoreError::ShardCountMismatch {
                        stored: shards,
                        configured: self.shards,
                    });
                }
                applied = applied.max(r.try_get::<i32, _>("version").map_err(map_err)?);
            }
        }
        for (version, sql) in MIGRATIONS {
            if *version <= applied {
                continue;
            }
            sqlx::raw_sql(sqlx::AssertSqlSafe(*sql))
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
            sqlx::query("INSERT INTO schema_version (version, shards) VALUES ($1, $2)")
                .bind(version)
                .bind(self.shards as i32)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
            tracing::info!(
                version,
                shards = self.shards,
                "loams-temporal-store: migrated"
            );
        }
        tx.commit().await.map_err(map_err)
    }

    /// Makes the next [`Store::commit`] fail with a server-side error after
    /// it has written its events, state, current run and outbox rows, before
    /// the visibility row and the commit.
    #[cfg(feature = "faults")]
    pub fn fail_next_commit(&self) {
        self.fault.store(true, Ordering::SeqCst);
    }

    fn shard(&self, key: &RunKey) -> i32 {
        // Below MAX_SHARDS, which fits an i32.
        shard_of(&key.namespace_id, &key.workflow_id, self.shards) as i32
    }

    fn bump(&self) {
        self.wake.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Writes a checked commit's rows inside `tx`, after the run's row:
    /// events, the current run's status, outbox rows, visibility, `NOTIFY`.
    async fn write_rows(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        key: &RunKey,
        c: &Commit,
    ) -> Result<(), StoreError> {
        let shard = self.shard(key);
        if !c.events.is_empty() {
            let ids: Vec<i64> = c.events.iter().map(|e| e.event_id).collect();
            let batch: Vec<i64> = c.events.iter().map(|e| e.batch_first_event_id).collect();
            let types: Vec<i32> = c.events.iter().map(|e| e.event_type).collect();
            let blobs: Vec<&[u8]> = c.events.iter().map(|e| e.blob.as_slice()).collect();
            sqlx::query(
                "INSERT INTO history_events (shard_id, namespace_id, workflow_id, run_id, \
                 event_id, batch_first_event_id, event_type, event_blob) \
                 SELECT $1, $2, $3, $4, e.* FROM UNNEST($5::bigint[], $6::bigint[], \
                 $7::integer[], $8::bytea[]) AS e",
            )
            .bind(shard)
            .bind(&key.namespace_id)
            .bind(&key.workflow_id)
            .bind(&key.run_id)
            .bind(&ids)
            .bind(&batch)
            .bind(&types)
            .bind(&blobs)
            .execute(&mut **tx)
            .await
            .map_err(|e| match unique(&e) {
                true => StoreError::Corrupt(format!(
                    "history of {}/{}/{} has events past next_event_id",
                    key.namespace_id, key.workflow_id, key.run_id
                )),
                false => map_err(e),
            })?;
        }
        sqlx::query(
            "UPDATE current_runs SET status = $5 \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 AND run_id = $4",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .bind(c.status)
        .execute(&mut **tx)
        .await
        .map_err(map_err)?;
        if !c.outbox.is_empty() {
            let kinds: Vec<&str> = c.outbox.iter().map(|o| o.kind.as_str()).collect();
            let ids: Vec<&str> = c.outbox.iter().map(|o| o.resonate_id.as_str()).collect();
            let reqs: Vec<&serde_json::Value> = c.outbox.iter().map(|o| &o.request).collect();
            sqlx::query(
                "INSERT INTO outbox (shard_id, namespace_id, workflow_id, run_id, kind, \
                 resonate_id, request) \
                 SELECT $1, $2, $3, $4, o.kind, o.rid, o.req \
                 FROM UNNEST($5::text[], $6::text[], $7::jsonb[]) WITH ORDINALITY \
                 AS o(kind, rid, req, n) ORDER BY o.n",
            )
            .bind(shard)
            .bind(&key.namespace_id)
            .bind(&key.workflow_id)
            .bind(&key.run_id)
            .bind(&kinds)
            .bind(&ids)
            .bind(&reqs)
            .execute(&mut **tx)
            .await
            .map_err(map_err)?;
        }
        #[cfg(feature = "faults")]
        if self.fault.swap(false, Ordering::SeqCst) {
            // A real server-side error between statements: division by zero.
            sqlx::query("SELECT 1 / 0")
                .execute(&mut **tx)
                .await
                .map_err(map_err)?;
        }
        if let Some(v) = &c.visibility {
            sqlx::query(
                "INSERT INTO visibility (namespace_id, workflow_id, run_id, workflow_type, \
                 status, start_time, close_time, execution_time, task_queue, search_attrs, memo) \
                 VALUES ($1, $2, $3, $4, $5, \
                 to_timestamp(0) + $6 * interval '1 millisecond', \
                 to_timestamp(0) + $7 * interval '1 millisecond', \
                 to_timestamp(0) + $8 * interval '1 millisecond', $9, $10, $11) \
                 ON CONFLICT (namespace_id, workflow_id, run_id) DO UPDATE SET \
                 workflow_type = EXCLUDED.workflow_type, status = EXCLUDED.status, \
                 start_time = EXCLUDED.start_time, close_time = EXCLUDED.close_time, \
                 execution_time = EXCLUDED.execution_time, task_queue = EXCLUDED.task_queue, \
                 search_attrs = EXCLUDED.search_attrs, memo = EXCLUDED.memo",
            )
            .bind(&key.namespace_id)
            .bind(&key.workflow_id)
            .bind(&key.run_id)
            .bind(&v.workflow_type)
            .bind(v.status)
            .bind(v.start_time_ms as f64)
            .bind(v.close_time_ms.map(|t| t as f64))
            .bind(v.execution_time_ms as f64)
            .bind(&v.task_queue)
            .bind(&v.search_attrs)
            .bind(v.memo.as_deref())
            .execute(&mut **tx)
            .await
            .map_err(map_err)?;
        }
        if !c.outbox.is_empty() {
            sqlx::query("SELECT pg_notify($1, $2)")
                .bind(OUTBOX_CHANNEL)
                .bind(shard.to_string())
                .execute(&mut **tx)
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }
}

/// The shard count recorded in `schema_version`, if the schema exists.
async fn stored_shards(pool: &PgPool) -> Result<Option<u32>, StoreError> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('schema_version') IS NOT NULL")
        .fetch_one(pool)
        .await
        .map_err(map_err)?;
    if !exists {
        return Ok(None);
    }
    let shards: Option<i32> =
        sqlx::query_scalar("SELECT shards FROM schema_version ORDER BY version LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(map_err)?;
    shards
        .map(|s| u32::try_from(s).map_err(|_| StoreError::Corrupt("negative shard count".into())))
        .transpose()
}

/// The outbox listener: bumps `wake` on every notification, and once after
/// each reconnect (notifications sent while disconnected are lost, so the
/// relay rescans). Stops when the pool closes.
async fn listen(pool: PgPool, wake: Arc<watch::Sender<u64>>) {
    let bump = || wake.send_modify(|n| *n = n.wrapping_add(1));
    let mut backoff = Duration::from_millis(100);
    loop {
        if pool.is_closed() {
            return;
        }
        let mut listener = match PgListener::connect_with(&pool).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "loams-temporal-store: outbox LISTEN connect failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        if let Err(e) = listener.listen(OUTBOX_CHANNEL).await {
            tracing::warn!(error = %e, "loams-temporal-store: LISTEN failed");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(10));
            continue;
        }
        backoff = Duration::from_millis(100);
        bump();
        loop {
            match listener.try_recv().await {
                Ok(Some(_)) => bump(),
                // The connection dropped; the next try_recv reconnects and
                // listens again. Anything sent meanwhile was missed.
                Ok(None) => bump(),
                Err(sqlx::Error::PoolClosed) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "loams-temporal-store: outbox LISTEN failed");
                    tokio::time::sleep(backoff).await;
                    break;
                }
            }
        }
    }
}

fn db_code(e: &sqlx::Error) -> Option<String> {
    match e {
        sqlx::Error::Database(d) => d.code().map(|c| c.into_owned()),
        _ => None,
    }
}

fn unique(e: &sqlx::Error) -> bool {
    db_code(e).as_deref() == Some("23505")
}

fn map_err(e: sqlx::Error) -> StoreError {
    use sqlx::Error as E;
    match &e {
        E::Io(_)
        | E::Tls(_)
        | E::PoolTimedOut
        | E::PoolClosed
        | E::WorkerCrashed
        | E::Protocol(_) => StoreError::Unavailable(e.to_string()),
        E::ColumnDecode { .. } | E::Decode(_) | E::ColumnNotFound(_) | E::TypeNotFound { .. } => {
            StoreError::Corrupt(e.to_string())
        }
        E::Database(_) => match db_code(&e).as_deref() {
            // NUL in text or jsonb, a bad encoding.
            Some("22021" | "22P05" | "22P02") => StoreError::InvalidArgument(e.to_string()),
            // Connection exceptions, shutdown, too many connections.
            Some(c) if c.starts_with("08") || c.starts_with("57P") || c == "53300" => {
                StoreError::Unavailable(e.to_string())
            }
            _ => StoreError::Database(e.to_string()),
        },
        _ => StoreError::Database(e.to_string()),
    }
}

fn to_u64(v: i64, what: &str) -> Result<u64, StoreError> {
    u64::try_from(v).map_err(|_| StoreError::Corrupt(format!("negative {what}")))
}

fn namespace(r: &PgRow) -> Result<NamespaceRecord, StoreError> {
    Ok(NamespaceRecord {
        namespace_id: r.try_get("namespace_id").map_err(map_err)?,
        loams_namespace_id: r.try_get("loams_namespace_id").map_err(map_err)?,
        name: r.try_get("name").map_err(map_err)?,
        retention_secs: r.try_get("retention_secs").map_err(map_err)?,
        state: r.try_get("state").map_err(map_err)?,
        config: r.try_get("config").map_err(map_err)?,
    })
}

const NS_COLS: &str = "namespace_id, loams_namespace_id, name, retention_secs, state, config";

fn text_arg(field: &str, s: &str) -> Result<(), StoreError> {
    // Postgres refuses NUL in text; say which field instead of a bind error.
    if s.contains('\0') {
        return Err(StoreError::InvalidArgument(format!("{field} contains NUL")));
    }
    Ok(())
}

#[async_trait]
impl Store for PgStore {
    async fn create_namespace(&self, ns: NamespaceRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO namespaces (namespace_id, loams_namespace_id, name, retention_secs, \
             state, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&ns.namespace_id)
        .bind(&ns.loams_namespace_id)
        .bind(&ns.name)
        .bind(ns.retention_secs)
        .bind(ns.state)
        .bind(&ns.config)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if unique(&e) {
                StoreError::NamespaceExists
            } else {
                map_err(e)
            }
        })?;
        Ok(())
    }

    async fn namespace_by_name(
        &self,
        loams_namespace_id: &str,
        name: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError> {
        let q =
            format!("SELECT {NS_COLS} FROM namespaces WHERE loams_namespace_id = $1 AND name = $2");
        sqlx::query(sqlx::AssertSqlSafe(q))
            .bind(loams_namespace_id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_err)?
            .as_ref()
            .map(namespace)
            .transpose()
    }

    async fn namespace_by_id(
        &self,
        namespace_id: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError> {
        let q = format!("SELECT {NS_COLS} FROM namespaces WHERE namespace_id = $1");
        sqlx::query(sqlx::AssertSqlSafe(q))
            .bind(namespace_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_err)?
            .as_ref()
            .map(namespace)
            .transpose()
    }

    async fn list_namespaces(
        &self,
        loams_namespace_id: &str,
    ) -> Result<Vec<NamespaceRecord>, StoreError> {
        let q =
            format!("SELECT {NS_COLS} FROM namespaces WHERE loams_namespace_id = $1 ORDER BY name");
        sqlx::query(sqlx::AssertSqlSafe(q))
            .bind(loams_namespace_id)
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?
            .iter()
            .map(namespace)
            .collect()
    }

    async fn start(&self, s: StartExecution) -> Result<StartOutcome, StoreError> {
        check_commit(&s.commit, FIRST_EVENT_ID)?;
        let key = &s.key;
        let shard = self.shard(key);
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        // Claim the workflow ID: a fresh row, or lock the existing one. A
        // concurrent start of the same ID waits here until the other commits.
        let inserted = sqlx::query(
            "INSERT INTO current_runs (shard_id, namespace_id, workflow_id, run_id, status, \
             start_request_id) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (shard_id, namespace_id, workflow_id) DO NOTHING",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .bind(s.commit.status)
        .bind(&s.request_id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?
        .rows_affected()
            == 1;
        if !inserted {
            let r = sqlx::query(
                "SELECT run_id, status, start_request_id FROM current_runs \
                 WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 FOR UPDATE",
            )
            .bind(shard)
            .bind(&key.namespace_id)
            .bind(&key.workflow_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
            let run_id: String = r.try_get("run_id").map_err(map_err)?;
            let status: i32 = r.try_get("status").map_err(map_err)?;
            let request_id: String = r.try_get("start_request_id").map_err(map_err)?;
            let running = status == STATUS_RUNNING;
            if running || request_id == s.request_id {
                return Ok(StartOutcome::AlreadyStarted {
                    run_id,
                    request_id,
                    running,
                });
            }
            sqlx::query(
                "UPDATE current_runs SET run_id = $4, status = $5, start_request_id = $6 \
                 WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3",
            )
            .bind(shard)
            .bind(&key.namespace_id)
            .bind(&key.workflow_id)
            .bind(&key.run_id)
            .bind(s.commit.status)
            .bind(&s.request_id)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }
        sqlx::query(
            "INSERT INTO executions (shard_id, namespace_id, workflow_id, run_id, status, \
             next_event_id, db_record_version, state_blob) VALUES ($1, $2, $3, $4, $5, $6, 1, $7)",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .bind(s.commit.status)
        .bind(s.commit.next_event_id)
        .bind(&s.commit.state_blob)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if unique(&e) {
                StoreError::RunExists
            } else {
                map_err(e)
            }
        })?;
        self.write_rows(&mut tx, key, &s.commit).await?;
        tx.commit().await.map_err(map_err)?;
        if !s.commit.outbox.is_empty() {
            self.bump();
        }
        Ok(StartOutcome::Started { version: 1 })
    }

    async fn current_run(
        &self,
        namespace_id: &str,
        workflow_id: &str,
    ) -> Result<Option<CurrentRun>, StoreError> {
        let shard = shard_of(namespace_id, workflow_id, self.shards) as i32;
        let Some(r) = sqlx::query(
            "SELECT run_id, status, start_request_id FROM current_runs \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3",
        )
        .bind(shard)
        .bind(namespace_id)
        .bind(workflow_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?
        else {
            return Ok(None);
        };
        Ok(Some(CurrentRun {
            run_id: r.try_get("run_id").map_err(map_err)?,
            status: r.try_get("status").map_err(map_err)?,
            request_id: r.try_get("start_request_id").map_err(map_err)?,
        }))
    }

    async fn load(&self, key: &RunKey) -> Result<Option<Loaded>, StoreError> {
        let Some(r) = sqlx::query(
            "SELECT state_blob, db_record_version, next_event_id, status FROM executions \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 AND run_id = $4",
        )
        .bind(self.shard(key))
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?
        else {
            return Ok(None);
        };
        Ok(Some(Loaded {
            state_blob: r.try_get("state_blob").map_err(map_err)?,
            version: to_u64(r.try_get("db_record_version").map_err(map_err)?, "version")?,
            next_event_id: r.try_get("next_event_id").map_err(map_err)?,
            status: r.try_get("status").map_err(map_err)?,
        }))
    }

    async fn commit(
        &self,
        key: &RunKey,
        expected_version: u64,
        c: Commit,
    ) -> Result<u64, StoreError> {
        let shard = self.shard(key);
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let Some(r) = sqlx::query(
            "SELECT db_record_version, next_event_id FROM executions \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 AND run_id = $4 \
             FOR UPDATE",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_err)?
        else {
            return Err(StoreError::NotFound);
        };
        let actual = to_u64(r.try_get("db_record_version").map_err(map_err)?, "version")?;
        if actual != expected_version {
            return Err(StoreError::Conflict {
                expected: expected_version,
                actual,
            });
        }
        let stored_next: i64 = r.try_get("next_event_id").map_err(map_err)?;
        check_commit(&c, stored_next)?;
        let version = expected_version + 1;
        let new_version =
            i64::try_from(version).map_err(|_| StoreError::Corrupt("version overflow".into()))?;
        sqlx::query(
            "UPDATE executions SET state_blob = $5, status = $6, next_event_id = $7, \
             db_record_version = $8, last_write_ts = now() \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 AND run_id = $4",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .bind(&c.state_blob)
        .bind(c.status)
        .bind(c.next_event_id)
        .bind(new_version)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;
        self.write_rows(&mut tx, key, &c).await?;
        tx.commit().await.map_err(map_err)?;
        if !c.outbox.is_empty() {
            self.bump();
        }
        Ok(version)
    }

    async fn history(
        &self,
        key: &RunKey,
        from_event_id: i64,
        max_events: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        let shard = self.shard(key);
        let rows = sqlx::query(
            "SELECT event_id, batch_first_event_id, event_type, event_blob FROM history_events \
             WHERE shard_id = $1 AND namespace_id = $2 AND workflow_id = $3 AND run_id = $4 \
             AND event_id >= $5 ORDER BY event_id LIMIT $6",
        )
        .bind(shard)
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .bind(from_event_id)
        .bind(i64::try_from(max_events).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        if rows.is_empty() && self.load(key).await?.is_none() {
            return Err(StoreError::NotFound);
        }
        rows.iter()
            .map(|r| {
                Ok(EventRow {
                    event_id: r.try_get("event_id").map_err(map_err)?,
                    batch_first_event_id: r.try_get("batch_first_event_id").map_err(map_err)?,
                    event_type: r.try_get("event_type").map_err(map_err)?,
                    blob: r.try_get("event_blob").map_err(map_err)?,
                })
            })
            .collect()
    }

    async fn claim_outbox(
        &self,
        shards: &[u32],
        limit: usize,
        lease: Duration,
        owner: &str,
    ) -> Result<Vec<OutboxRow>, StoreError> {
        text_arg("owner", owner)?;
        if shards.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let shard_ids: Vec<i32> = shards
            .iter()
            .filter(|s| **s < self.shards)
            .map(|s| *s as i32)
            .collect();
        let rows = sqlx::query(
            "UPDATE outbox o SET claimed_by = $4, \
             claimed_until = now() + make_interval(secs => $3), attempts = o.attempts + 1 \
             WHERE o.id IN (SELECT id FROM outbox \
               WHERE shard_id = ANY($1) AND dispatched_at IS NULL \
               AND (claimed_until IS NULL OR claimed_until < now()) \
               ORDER BY id FOR UPDATE SKIP LOCKED LIMIT $2) \
             RETURNING o.id, o.shard_id, o.namespace_id, o.workflow_id, o.run_id, o.kind, \
             o.resonate_id, o.request, o.attempts",
        )
        .bind(&shard_ids)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(lease.as_secs_f64())
        .bind(owner)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        let mut out = rows
            .iter()
            .map(|r| {
                Ok(OutboxRow {
                    id: r.try_get("id").map_err(map_err)?,
                    shard_id: u32::try_from(r.try_get::<i32, _>("shard_id").map_err(map_err)?)
                        .map_err(|_| StoreError::Corrupt("negative shard_id".into()))?,
                    key: RunKey {
                        namespace_id: r.try_get("namespace_id").map_err(map_err)?,
                        workflow_id: r.try_get("workflow_id").map_err(map_err)?,
                        run_id: r.try_get("run_id").map_err(map_err)?,
                    },
                    kind: r.try_get("kind").map_err(map_err)?,
                    resonate_id: r.try_get("resonate_id").map_err(map_err)?,
                    request: r.try_get("request").map_err(map_err)?,
                    attempts: r.try_get("attempts").map_err(map_err)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        // RETURNING has no order.
        out.sort_by_key(|o| o.id);
        Ok(out)
    }

    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), StoreError> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE outbox SET dispatched_at = now(), claimed_until = NULL \
             WHERE id = ANY($1) AND dispatched_at IS NULL",
        )
        .bind(ids)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(())
    }

    fn outbox_wakeups(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    async fn visibility(&self, key: &RunKey) -> Result<Option<VisibilityRow>, StoreError> {
        let Some(r) = sqlx::query(
            "SELECT workflow_type, status, \
             floor(extract(epoch FROM start_time) * 1000)::bigint AS start_ms, \
             floor(extract(epoch FROM close_time) * 1000)::bigint AS close_ms, \
             floor(extract(epoch FROM execution_time) * 1000)::bigint AS exec_ms, \
             task_queue, memo, search_attrs FROM visibility \
             WHERE namespace_id = $1 AND workflow_id = $2 AND run_id = $3",
        )
        .bind(&key.namespace_id)
        .bind(&key.workflow_id)
        .bind(&key.run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?
        else {
            return Ok(None);
        };
        Ok(Some(VisibilityRow {
            workflow_type: r.try_get("workflow_type").map_err(map_err)?,
            status: r.try_get("status").map_err(map_err)?,
            start_time_ms: r.try_get("start_ms").map_err(map_err)?,
            close_time_ms: r.try_get("close_ms").map_err(map_err)?,
            execution_time_ms: r.try_get("exec_ms").map_err(map_err)?,
            task_queue: r.try_get("task_queue").map_err(map_err)?,
            memo: r.try_get("memo").map_err(map_err)?,
            search_attrs: r.try_get("search_attrs").map_err(map_err)?,
        }))
    }
}
