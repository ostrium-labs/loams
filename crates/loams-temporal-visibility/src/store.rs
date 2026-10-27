//! Where visibility queries run, and the search-attribute registry
//! (D2075).
//!
//! - [`PgVisibility`] reads the `visibility` table `loams-temporal-store`
//!   writes in each run's commit (so a list sees a run as soon as its start
//!   or close commits), with the SQL of [`crate::sql`]. Its migration adds
//!   only indexes and the registry table.
//! - [`MemVisibility`] keeps records the gateway hands it
//!   ([`VisibilityStore::upsert`]) and filters them with [`crate::eval`].

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::Row;
use sqlx::postgres::{PgArguments, PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::query::Query;

use crate::eval::{self, Record};
use crate::filter::{Resolved, ValueType, is_system};
use crate::sql::{Builder, Param};

/// The most custom search attributes per namespace (Temporal's limit,
/// plan DX1 R0.9).
pub const MAX_CUSTOM_ATTRIBUTES: usize = 100;

const MIGRATION_LOCK: i64 = 0x6c74_7669_7369_6200; // "ltvisib\0"
const MIGRATION: &str = include_str!("../migrations/0001_visibility.sql");

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VisibilityError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("search attribute {0} already exists with another type")]
    Conflict(String),
    #[error("search attribute {0} not found")]
    NotFound(String),
    #[error("visibility unavailable: {0}")]
    Unavailable(String),
    #[error("visibility data corrupt: {0}")]
    Corrupt(String),
    #[error("database error: {0}")]
    Database(String),
}

/// Checks a custom attribute's name: letters, digits and `_`, starting with
/// a letter, at most 64 bytes, not a system name.
pub fn check_name(name: &str) -> Result<(), VisibilityError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(VisibilityError::InvalidArgument(format!(
            "search attribute name {name:?}: use letters, digits and _, starting with a letter, at most 64 bytes"
        )));
    }
    if is_system(name) {
        return Err(VisibilityError::InvalidArgument(format!(
            "{name} is a system search attribute"
        )));
    }
    Ok(())
}

/// The visibility backend.
#[async_trait]
pub trait VisibilityStore: Send + Sync + 'static {
    /// Records a run's visibility. The Postgres backend ignores it (the
    /// history store's commit wrote the row); the memory one keeps it.
    async fn upsert(&self, namespace_id: &str, rec: Record) -> Result<(), VisibilityError>;
    /// A page of the runs that pass `q`, in its order.
    async fn list(
        &self,
        namespace_id: &str,
        q: &Resolved,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Record>, VisibilityError>;
    /// How many runs pass `q`.
    async fn count(&self, namespace_id: &str, q: &Resolved) -> Result<i64, VisibilityError>;
    /// How many runs pass `q`, by status.
    async fn count_by_status(
        &self,
        namespace_id: &str,
        q: &Resolved,
    ) -> Result<Vec<(i32, i64)>, VisibilityError>;
    /// The namespace's custom search attributes.
    async fn custom_attributes(
        &self,
        namespace_id: &str,
    ) -> Result<BTreeMap<String, ValueType>, VisibilityError>;
    /// Registers attributes; one that exists with the same type is fine.
    async fn add_attributes(
        &self,
        namespace_id: &str,
        attrs: &BTreeMap<String, ValueType>,
    ) -> Result<(), VisibilityError>;
    /// Unregisters attributes (stored values stay, unqueryable).
    async fn remove_attributes(
        &self,
        namespace_id: &str,
        names: &[String],
    ) -> Result<(), VisibilityError>;
}

fn check_add(
    existing: &BTreeMap<String, ValueType>,
    attrs: &BTreeMap<String, ValueType>,
) -> Result<(), VisibilityError> {
    let mut new = 0;
    for (name, ty) in attrs {
        check_name(name)?;
        match existing.get(name) {
            Some(t) if t == ty => {}
            Some(_) => return Err(VisibilityError::Conflict(name.clone())),
            None => new += 1,
        }
    }
    if existing.len() + new > MAX_CUSTOM_ATTRIBUTES {
        return Err(VisibilityError::InvalidArgument(format!(
            "a namespace has at most {MAX_CUSTOM_ATTRIBUTES} custom search attributes"
        )));
    }
    Ok(())
}

/// Visibility in memory.
#[derive(Debug, Default)]
pub struct MemVisibility {
    runs: Mutex<BTreeMap<(String, String, String), Record>>,
    attrs: Mutex<BTreeMap<String, BTreeMap<String, ValueType>>>,
}

impl MemVisibility {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a run's visibility at once (what the gateway's commit tap
    /// calls, so records land in commit order).
    pub fn put(&self, namespace_id: &str, rec: Record) {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (
                    namespace_id.to_string(),
                    rec.workflow_id.clone(),
                    rec.run_id.clone(),
                ),
                rec,
            );
    }

    fn matching(&self, namespace_id: &str, q: &Resolved) -> Vec<Record> {
        let runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out: Vec<Record> = runs
            .iter()
            .filter(|((ns, _, _), r)| ns == namespace_id && eval::passes(q, r))
            .map(|(_, r)| r.clone())
            .collect();
        eval::sort(q, &mut out);
        out
    }
}

#[async_trait]
impl VisibilityStore for MemVisibility {
    async fn upsert(&self, namespace_id: &str, rec: Record) -> Result<(), VisibilityError> {
        self.put(namespace_id, rec);
        Ok(())
    }

    async fn list(
        &self,
        namespace_id: &str,
        q: &Resolved,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Record>, VisibilityError> {
        Ok(self
            .matching(namespace_id, q)
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    async fn count(&self, namespace_id: &str, q: &Resolved) -> Result<i64, VisibilityError> {
        Ok(self.matching(namespace_id, q).len() as i64)
    }

    async fn count_by_status(
        &self,
        namespace_id: &str,
        q: &Resolved,
    ) -> Result<Vec<(i32, i64)>, VisibilityError> {
        let mut by: BTreeMap<i32, i64> = BTreeMap::new();
        for r in self.matching(namespace_id, q) {
            *by.entry(r.status).or_default() += 1;
        }
        Ok(by.into_iter().collect())
    }

    async fn custom_attributes(
        &self,
        namespace_id: &str,
    ) -> Result<BTreeMap<String, ValueType>, VisibilityError> {
        Ok(self
            .attrs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(namespace_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn add_attributes(
        &self,
        namespace_id: &str,
        attrs: &BTreeMap<String, ValueType>,
    ) -> Result<(), VisibilityError> {
        let mut all = self
            .attrs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let existing = all.entry(namespace_id.to_string()).or_default();
        check_add(existing, attrs)?;
        existing.extend(attrs.iter().map(|(k, v)| (k.clone(), *v)));
        Ok(())
    }

    async fn remove_attributes(
        &self,
        namespace_id: &str,
        names: &[String],
    ) -> Result<(), VisibilityError> {
        let mut all = self
            .attrs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let existing = all.entry(namespace_id.to_string()).or_default();
        if let Some(missing) = names.iter().find(|n| !existing.contains_key(*n)) {
            return Err(VisibilityError::NotFound(missing.clone()));
        }
        for n in names {
            existing.remove(n);
        }
        Ok(())
    }
}

/// Visibility on Postgres.
#[derive(Debug, Clone)]
pub struct PgVisibility {
    pool: PgPool,
}

fn map_err(e: sqlx::Error) -> VisibilityError {
    use sqlx::Error as E;
    match &e {
        E::Io(_)
        | E::Tls(_)
        | E::PoolTimedOut
        | E::PoolClosed
        | E::WorkerCrashed
        | E::Protocol(_) => VisibilityError::Unavailable(e.to_string()),
        E::ColumnDecode { .. } | E::Decode(_) | E::ColumnNotFound(_) | E::TypeNotFound { .. } => {
            VisibilityError::Corrupt(e.to_string())
        }
        E::Database(d) => match d.code().as_deref() {
            // A stored value that does not cast to the attribute's type, a
            // bad time: the query's fault, not the server's.
            Some("22P02" | "22007" | "22008" | "22003" | "22021") => {
                VisibilityError::InvalidArgument(e.to_string())
            }
            Some(c) if c.starts_with("08") || c.starts_with("57P") || c == "53300" => {
                VisibilityError::Unavailable(e.to_string())
            }
            _ => VisibilityError::Database(e.to_string()),
        },
        _ => VisibilityError::Database(e.to_string()),
    }
}

fn bind_all<'q>(
    mut q: Query<'q, sqlx::Postgres, PgArguments>,
    params: Vec<Param>,
) -> Query<'q, sqlx::Postgres, PgArguments> {
    for p in params {
        q = match p {
            Param::Text(s) => q.bind(s),
            Param::Int(i) => q.bind(i),
            Param::Float(x) => q.bind(x),
            Param::Bool(b) => q.bind(b),
        };
    }
    q
}

fn record(r: &PgRow) -> Result<Record, VisibilityError> {
    let attrs: serde_json::Value = r.try_get("search_attrs").map_err(map_err)?;
    Ok(Record {
        workflow_id: r.try_get("workflow_id").map_err(map_err)?,
        run_id: r.try_get("run_id").map_err(map_err)?,
        workflow_type: r.try_get("workflow_type").map_err(map_err)?,
        status: r.try_get("status").map_err(map_err)?,
        task_queue: r.try_get("task_queue").map_err(map_err)?,
        start_ms: r.try_get("start_ms").map_err(map_err)?,
        close_ms: r.try_get("close_ms").map_err(map_err)?,
        execution_ms: r.try_get("exec_ms").map_err(map_err)?,
        search_attrs: match attrs {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        },
        memo: r.try_get("memo").map_err(map_err)?,
    })
}

const SELECT: &str = "SELECT workflow_id, run_id, workflow_type, status, task_queue, \
     floor(extract(epoch FROM start_time) * 1000)::bigint AS start_ms, \
     floor(extract(epoch FROM close_time) * 1000)::bigint AS close_ms, \
     floor(extract(epoch FROM execution_time) * 1000)::bigint AS exec_ms, \
     search_attrs, memo FROM visibility";

impl PgVisibility {
    /// Connects with a small pool of its own.
    pub async fn connect(url: &str) -> Result<Self, VisibilityError> {
        let opts = PgConnectOptions::from_str(url)
            .map_err(|e| VisibilityError::InvalidArgument(format!("database URL: {e}")))?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .map_err(map_err)?;
        Ok(PgVisibility { pool })
    }

    /// Over an existing pool (the history store's).
    pub fn with_pool(pool: PgPool) -> Self {
        PgVisibility { pool }
    }

    /// Adds the registry table and the list indexes (idempotent). Run after
    /// the history store's migration, which creates `visibility`.
    pub async fn migrate(&self) -> Result<(), VisibilityError> {
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

    /// The SQL and parameters of a list (exposed for tests).
    pub fn list_sql(q: &Resolved, limit: usize, offset: usize) -> (String, Vec<Param>) {
        let mut b = Builder::new(1);
        let w = match &q.filter {
            Some(f) => b.filter(f),
            None => "TRUE".into(),
        };
        let order = b.order_by(q);
        let mut params = b.params;
        params.push(Param::Int(i64::try_from(limit).unwrap_or(i64::MAX)));
        let l = params.len() + 1;
        params.push(Param::Int(i64::try_from(offset).unwrap_or(i64::MAX)));
        let o = params.len() + 1;
        (
            format!(
                "{SELECT} WHERE namespace_id = $1 AND {w} ORDER BY {order} LIMIT ${l} OFFSET ${o}"
            ),
            params,
        )
    }
}

#[async_trait]
impl VisibilityStore for PgVisibility {
    async fn upsert(&self, _namespace_id: &str, _rec: Record) -> Result<(), VisibilityError> {
        Ok(())
    }

    async fn list(
        &self,
        namespace_id: &str,
        q: &Resolved,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Record>, VisibilityError> {
        let (sql, params) = Self::list_sql(q, limit, offset);
        let query = bind_all(
            sqlx::query(sqlx::AssertSqlSafe(sql)).bind(namespace_id.to_string()),
            params,
        );
        let rows = query.fetch_all(&self.pool).await.map_err(map_err)?;
        rows.iter().map(record).collect()
    }

    async fn count(&self, namespace_id: &str, q: &Resolved) -> Result<i64, VisibilityError> {
        let (w, params) = crate::sql::where_clause(q, 1);
        let sql = format!("SELECT count(*) AS n FROM visibility WHERE namespace_id = $1 AND {w}");
        let query = bind_all(
            sqlx::query(sqlx::AssertSqlSafe(sql)).bind(namespace_id.to_string()),
            params,
        );
        let row = query.fetch_one(&self.pool).await.map_err(map_err)?;
        row.try_get("n").map_err(map_err)
    }

    async fn count_by_status(
        &self,
        namespace_id: &str,
        q: &Resolved,
    ) -> Result<Vec<(i32, i64)>, VisibilityError> {
        let (w, params) = crate::sql::where_clause(q, 1);
        let sql = format!(
            "SELECT status, count(*) AS n FROM visibility WHERE namespace_id = $1 AND {w} \
             GROUP BY status ORDER BY status"
        );
        let query = bind_all(
            sqlx::query(sqlx::AssertSqlSafe(sql)).bind(namespace_id.to_string()),
            params,
        );
        let rows = query.fetch_all(&self.pool).await.map_err(map_err)?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("status").map_err(map_err)?,
                    r.try_get("n").map_err(map_err)?,
                ))
            })
            .collect()
    }

    async fn custom_attributes(
        &self,
        namespace_id: &str,
    ) -> Result<BTreeMap<String, ValueType>, VisibilityError> {
        let rows = sqlx::query(
            "SELECT name, value_type FROM temporal_search_attributes WHERE namespace_id = $1",
        )
        .bind(namespace_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        let mut out = BTreeMap::new();
        for r in rows {
            let name: String = r.try_get("name").map_err(map_err)?;
            let t: i32 = r.try_get("value_type").map_err(map_err)?;
            let ty = ValueType::from_i32(t)
                .ok_or_else(|| VisibilityError::Corrupt(format!("{name}: value type {t}")))?;
            out.insert(name, ty);
        }
        Ok(out)
    }

    async fn add_attributes(
        &self,
        namespace_id: &str,
        attrs: &BTreeMap<String, ValueType>,
    ) -> Result<(), VisibilityError> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        // One registry change per namespace at a time.
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind((MIGRATION_LOCK >> 32) as i32)
            .bind(namespace_id)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        let rows = sqlx::query(
            "SELECT name, value_type FROM temporal_search_attributes WHERE namespace_id = $1",
        )
        .bind(namespace_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_err)?;
        let mut existing = BTreeMap::new();
        for r in rows {
            let name: String = r.try_get("name").map_err(map_err)?;
            let t: i32 = r.try_get("value_type").map_err(map_err)?;
            if let Some(ty) = ValueType::from_i32(t) {
                existing.insert(name, ty);
            }
        }
        check_add(&existing, attrs)?;
        for (name, ty) in attrs {
            sqlx::query(
                "INSERT INTO temporal_search_attributes (namespace_id, name, value_type) \
                 VALUES ($1, $2, $3) ON CONFLICT (namespace_id, name) DO NOTHING",
            )
            .bind(namespace_id)
            .bind(name)
            .bind(ty.to_i32())
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }
        tx.commit().await.map_err(map_err)
    }

    async fn remove_attributes(
        &self,
        namespace_id: &str,
        names: &[String],
    ) -> Result<(), VisibilityError> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        for name in names {
            let r = sqlx::query(
                "DELETE FROM temporal_search_attributes WHERE namespace_id = $1 AND name = $2",
            )
            .bind(namespace_id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
            if r.rows_affected() == 0 {
                return Err(VisibilityError::NotFound(name.clone()));
            }
        }
        tx.commit().await.map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::compile;

    #[tokio::test]
    async fn registry_rules() {
        let v = MemVisibility::new();
        let one = |n: &str, t| BTreeMap::from([(n.to_string(), t)]);
        v.add_attributes("ns", &one("CustomerId", ValueType::Keyword))
            .await
            .unwrap();
        // Same type again is fine; another type is a conflict.
        v.add_attributes("ns", &one("CustomerId", ValueType::Keyword))
            .await
            .unwrap();
        assert_eq!(
            v.add_attributes("ns", &one("CustomerId", ValueType::Int))
                .await,
            Err(VisibilityError::Conflict("CustomerId".into()))
        );
        assert!(
            v.add_attributes("ns", &one("WorkflowId", ValueType::Keyword))
                .await
                .is_err()
        );
        assert!(
            v.add_attributes("ns", &one("bad name", ValueType::Keyword))
                .await
                .is_err()
        );
        assert!(
            v.add_attributes("ns", &one("9lives", ValueType::Keyword))
                .await
                .is_err()
        );
        assert_eq!(v.custom_attributes("ns").await.unwrap().len(), 1);
        assert!(v.custom_attributes("other").await.unwrap().is_empty());
        let many: BTreeMap<String, ValueType> = (0..MAX_CUSTOM_ATTRIBUTES)
            .map(|i| (format!("A{i}"), ValueType::Int))
            .collect();
        assert!(v.add_attributes("ns", &many).await.is_err());
        assert_eq!(
            v.remove_attributes("ns", &["Nope".into()]).await,
            Err(VisibilityError::NotFound("Nope".into()))
        );
        v.remove_attributes("ns", &["CustomerId".into()])
            .await
            .unwrap();
        assert!(v.custom_attributes("ns").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mem_list_count_and_group() {
        let v = MemVisibility::new();
        for (i, status) in [1, 1, 2, 3].into_iter().enumerate() {
            v.upsert(
                "ns",
                Record {
                    workflow_id: format!("w{i}"),
                    run_id: format!("r{i}"),
                    status,
                    start_ms: i as i64,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        v.upsert("other", Record::default()).await.unwrap();
        let all = compile("", &BTreeMap::new()).unwrap();
        assert_eq!(v.count("ns", &all).await.unwrap(), 4);
        let page = v.list("ns", &all, 2, 1).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(
            v.count_by_status("ns", &all).await.unwrap(),
            vec![(1, 2), (2, 1), (3, 1)]
        );
        let running = compile("ExecutionStatus = 'Running'", &BTreeMap::new()).unwrap();
        assert_eq!(v.count("ns", &running).await.unwrap(), 2);
    }

    #[test]
    fn list_sql_numbers_limit_and_offset_last() {
        let q = compile("WorkflowType = 'a' ORDER BY StartTime", &BTreeMap::new()).unwrap();
        let (sql, params) = PgVisibility::list_sql(&q, 10, 20);
        assert!(
            sql.ends_with("ORDER BY start_time ASC NULLS LAST, run_id LIMIT $3 OFFSET $4"),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE namespace_id = $1 AND (workflow_type = $2::text)"),
            "{sql}"
        );
        assert_eq!(params.len(), 3);
        assert_eq!(params[1], Param::Int(10));
        assert_eq!(params[2], Param::Int(20));
    }

    /// Against a real Postgres with the history store's schema when
    /// `LOAMS_TEMPORAL_TEST_PG_URL` is set; skipped otherwise.
    #[tokio::test]
    async fn pg_registry_and_queries() {
        let Ok(url) = std::env::var("LOAMS_TEMPORAL_TEST_PG_URL") else {
            return;
        };
        let v = PgVisibility::connect(&url).await.unwrap();
        let has_table: bool = sqlx::query_scalar("SELECT to_regclass('visibility') IS NOT NULL")
            .fetch_one(&v.pool)
            .await
            .unwrap();
        if !has_table {
            return;
        }
        v.migrate().await.unwrap();
        v.migrate().await.unwrap();
        let ns = format!("vis-{}", std::process::id());
        let attrs = BTreeMap::from([("CustomerId".to_string(), ValueType::Keyword)]);
        v.add_attributes(&ns, &attrs).await.unwrap();
        assert_eq!(v.custom_attributes(&ns).await.unwrap(), attrs);
        let q = compile(
            "CustomerId = 'x' AND StartTime > '2026-01-01T00:00:00Z' ORDER BY CustomerId",
            &attrs,
        )
        .unwrap();
        assert!(v.list(&ns, &q, 10, 0).await.unwrap().is_empty());
        assert_eq!(v.count(&ns, &q).await.unwrap(), 0);
        assert!(v.count_by_status(&ns, &q).await.unwrap().is_empty());
        v.remove_attributes(&ns, &["CustomerId".into()])
            .await
            .unwrap();
    }
}
