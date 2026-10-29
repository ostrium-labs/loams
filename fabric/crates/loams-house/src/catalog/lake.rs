//! [`Lake`]: a namespace's ClickHouse databases and tables over a [`HouseCatalog`].
//!
//! * ClickHouse database `d` of namespace 7 is the Iceberg namespace `ns_7.d`, and
//!   `default` is `ns_7` itself (§49 §5.3; nested namespaces, which Lakekeeper
//!   and the House's own catalogs support, so the flat fallback is not needed).
//! * A table lives at `s3://<bucket>/<prefix>/ns/7/house/<db>/<table>-<uuid>`
//!   ([`LakeRoot::table_location`]): the store's bucket and prefix as
//!   `house-cache` names them to workers, so a pinned metadata file's absolute
//!   paths read through the proxy as they are (R9.5). The uuid keeps a table made
//!   after a `DROP` of the same name out of the dropped one's files.
//! * `DROP TABLE` keeps the table for the retention of §49 §11 so `UNDROP TABLE`
//!   can bring it back: it is renamed into `ns_7.__loams_dropped` with the time and
//!   its old name as properties; HS1 Task 15 purges it after the retention.
//! * Tables another writer made (`external`, §49 §5.2) are listed and read, and
//!   their locations are vouched to `house-cache` ([`PrefixGrants`]) so the
//!   namespace's workers may read them (R9.5); DDL on them is `497`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent, TableUpdate};

use super::{CatalogError, HouseCatalog, LakeTable};
use crate::ddl::{self, TableClass};
use crate::errors::{ChError, HouseError};

/// The namespace that keeps dropped tables until they are purged.
pub const DROPPED: &str = "__loams_dropped";
/// Names starting with this are the House's own.
pub const RESERVED_PREFIX: &str = "__loams";
/// The House's own database of a namespace's SQL functions (HE1 R14.11).
pub const FUNCTIONS_DATABASE: &str = "__loams_functions";
/// Property: the database a dropped table was in.
pub const DROPPED_DATABASE: &str = "loams.dropped.database";
/// Property: its name there.
pub const DROPPED_NAME: &str = "loams.dropped.name";
/// Property: when it was dropped, in Unix milliseconds.
pub const DROPPED_AT_MS: &str = "loams.dropped.at_ms";
/// How long a dropped table can be brought back (`loams_snapshot_retention`'s
/// default, §49 §11).
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where the House's tables live in the store, as workers name it:
/// `s3://<bucket>/<prefix>`.
#[derive(Clone, PartialEq, Eq)]
pub struct LakeRoot {
    bucket: String,
    prefix: String,
}

impl fmt::Debug for LakeRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LakeRoot({self})")
    }
}

impl fmt::Display for LakeRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.prefix.is_empty() {
            write!(f, "s3://{}", self.bucket)
        } else {
            write!(f, "s3://{}/{}", self.bucket, self.prefix)
        }
    }
}

impl LakeRoot {
    /// The root of a store URL, as `house-cache` names it to workers:
    /// `s3://bucket/prefix` (also `gs://`, `az://`) keeps its bucket and prefix;
    /// `file://` and `memory://` stores are bucket `local` with no prefix.
    pub fn from_store_url(url: &str) -> Result<Self, String> {
        let bad =
            || format!("the store URL {url:?} is not s3://, gs://, az://, file:// or memory://");
        let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
        match scheme {
            "s3" | "s3a" | "gs" | "az" | "abfs" | "abfss" => {
                let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
                if bucket.is_empty() {
                    return Err(bad());
                }
                Ok(Self {
                    bucket: bucket.to_string(),
                    prefix: prefix.trim_matches('/').to_string(),
                })
            }
            "file" | "memory" => Ok(Self::local()),
            _ => Err(bad()),
        }
    }

    /// `s3://local`: a `file://` or `memory://` store.
    pub fn local() -> Self {
        Self {
            bucket: "local".to_string(),
            prefix: String::new(),
        }
    }

    /// The bucket name workers use.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The key prefix (no slashes at either end).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Where a new table goes: `<root>/ns/<id>/house/<db>/<table>-<uuid>`, names
    /// reduced to `[A-Za-z0-9_-]` (the uuid keeps them apart).
    pub fn table_location(&self, namespace: u64, database: &str, table: &str) -> String {
        format!(
            "{self}/ns/{namespace}/house/{}/{}-{}",
            path_safe(database),
            path_safe(table),
            uuid::Uuid::new_v4().simple()
        )
    }

    /// A location's key relative to the store (what `house-cache` grants), or
    /// `None` when it is outside the root.
    pub fn relative<'a>(&self, location: &'a str) -> Option<&'a str> {
        let (scheme, rest) = location.split_once("://")?;
        if !matches!(scheme, "s3" | "s3a") {
            return None;
        }
        let rest = rest.strip_prefix(self.bucket.as_str())?;
        let rest = if rest.is_empty() {
            rest
        } else {
            rest.strip_prefix('/')?
        };
        if self.prefix.is_empty() {
            return Some(rest);
        }
        let rest = rest.strip_prefix(self.prefix.as_str())?;
        if rest.is_empty() {
            Some(rest)
        } else {
            rest.strip_prefix('/')
        }
    }
}

fn path_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "_".to_string()
    } else {
        safe
    }
}

/// Where the catalog vouches that a namespace's workers may read: `house-cache`
/// adds the prefix to those workers' grants (R9.5). `prefix` is relative to the
/// store and ends in `/`.
pub trait PrefixGrants: Send + Sync + fmt::Debug {
    /// Lets namespace `namespace`'s workers read under `prefix`.
    fn vouch(&self, namespace: &str, prefix: &str);
}

/// A database of a namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    /// Its ClickHouse name.
    pub name: String,
    /// Its comment.
    pub comment: String,
}

/// A namespace's ClickHouse databases and tables over a catalog.
pub struct Lake {
    catalog: Arc<dyn HouseCatalog>,
    root: LakeRoot,
    retention: Duration,
    grants: Option<Arc<dyn PrefixGrants>>,
    store: Option<loams_store::Store>,
    /// The tables' commit queues (HS1 Task 12), made at the first write.
    committer: std::sync::OnceLock<Arc<crate::commit::Committer>>,
    /// This front's state of the pipes' views (HS1 Task 16).
    pipe_states: Arc<crate::pipes::PipeStates>,
    /// Pipes and materialized views per namespace (§49 §12, HS1 Task 21).
    object_caps: ObjectCaps,
    /// The front's quotas (HS1 Task 22): the `pipes` quota, checked where
    /// pipes are made. Set once, when the front starts serving.
    quotas: std::sync::OnceLock<Arc<crate::quota::Quotas>>,
    /// The front's metrics (HS1 Task 23): its commits and compactions count
    /// here, and its surfaces share them.
    metrics: Arc<crate::metrics::Metrics>,
}

/// How many pipes and materialized views a namespace may hold (§49 §12: 32 and
/// 64; `36` past either).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectCaps {
    /// Pipes per namespace.
    pub pipes: usize,
    /// Materialized views per namespace.
    pub views: usize,
}

impl Default for ObjectCaps {
    fn default() -> Self {
        Self {
            pipes: crate::limits::MAX_PIPES_PER_NAMESPACE,
            views: crate::limits::MAX_VIEWS_PER_NAMESPACE,
        }
    }
}

impl fmt::Debug for Lake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lake")
            .field("catalog", &self.catalog)
            .field("root", &self.root)
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn access_denied(what: &str) -> HouseError {
    HouseError::from(ChError::access_denied(format!(
        "{what}: the table belongs to another writer (an external table), so the House reads \
         it and never changes it"
    )))
}

impl Lake {
    /// The lake of `catalog`, with tables under `root`.
    pub fn new(catalog: Arc<dyn HouseCatalog>, root: LakeRoot) -> Self {
        Self {
            catalog,
            root,
            retention: DEFAULT_RETENTION,
            grants: None,
            store: None,
            committer: std::sync::OnceLock::new(),
            pipe_states: Arc::default(),
            object_caps: ObjectCaps::default(),
            quotas: std::sync::OnceLock::new(),
            metrics: Arc::default(),
        }
    }

    /// The front's metrics (HS1 Task 23).
    pub fn metrics(&self) -> &Arc<crate::metrics::Metrics> {
        &self.metrics
    }

    /// Holds the namespaces' pipes to `quotas` (HS1 Task 22). The first call
    /// wins: one lake is served by one front.
    pub fn set_quotas(&self, quotas: Arc<crate::quota::Quotas>) {
        let _ = self.quotas.set(quotas);
    }

    /// The quotas its pipes are held to, once a front set them.
    pub fn quotas(&self) -> Option<&Arc<crate::quota::Quotas>> {
        self.quotas.get()
    }

    /// This front's state of the pipes' views (HS1 Task 16).
    pub fn pipe_states(&self) -> Arc<crate::pipes::PipeStates> {
        Arc::clone(&self.pipe_states)
    }

    /// The commit queues of this lake's tables (HS1 Task 12).
    pub fn committer(&self) -> Arc<crate::commit::Committer> {
        Arc::clone(self.committer.get_or_init(|| {
            Arc::new(
                crate::commit::Committer::new(crate::commit::CommitIo {
                    catalog: Arc::clone(&self.catalog),
                    store: self.store.clone(),
                    root: self.root.clone(),
                })
                .with_metrics(Arc::clone(&self.metrics)),
            )
        }))
    }

    /// The store the tables live in, which the front reads manifests from
    /// (`system.parts` and the `FINAL` views, HS1 Task 11).
    pub fn with_store(mut self, store: loams_store::Store) -> Self {
        self.store = Some(store);
        self
    }

    /// A file of a table (a manifest list, a manifest), by its absolute
    /// location: `210` when the House has no store or the store fails, `36` when
    /// the location is outside the store.
    pub async fn read_file(&self, location: &str) -> Result<bytes::Bytes, HouseError> {
        let store = self.store.as_ref().ok_or_else(|| {
            HouseError::from(ChError::network_error(
                "this House reads no table files: it was started without --store",
            ))
        })?;
        let key = self.root.relative(location).ok_or_else(|| {
            HouseError::from(ChError::bad_arguments(format!(
                "{location} is outside the House's store ({})",
                self.root
            )))
        })?;
        store
            .get(key)
            .await
            .map(|(bytes, _)| bytes)
            // The store's own words may name its endpoint or account (R9.2).
            .map_err(|_| {
                HouseError::from(ChError::network_error(
                    "the store failed to read a table file",
                ))
            })
    }

    /// How long snapshots and dropped tables are kept (§49 §11).
    pub fn retention(&self) -> Duration {
        self.retention
    }

    /// The namespaces the catalog holds tables for (`ns_<id>`), for the
    /// retention sweep (HS1 Task 15).
    pub async fn namespaces(&self) -> Result<Vec<u64>, HouseError> {
        let roots = self.catalog.list_root_databases().await?;
        let mut out: Vec<u64> = roots
            .iter()
            .filter_map(|ns| match ns.as_ref().as_slice() {
                [only] => only.strip_prefix("ns_")?.parse().ok(),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    /// The name a dropped table is kept under (HS1 Task 15's reconciliation
    /// gives a table dropped after a catalog's restore point the same one).
    pub fn dropped_name(table_uuid: &uuid::Uuid, dropped_at_ms: u64) -> String {
        format!("t_{}_{dropped_at_ms}", table_uuid.simple())
    }

    /// How many pipes and materialized views a namespace may hold.
    pub fn with_object_caps(mut self, caps: ObjectCaps) -> Self {
        self.object_caps = caps;
        self
    }

    /// [`Lake::with_object_caps`]'s caps.
    pub fn object_caps(&self) -> ObjectCaps {
        self.object_caps
    }

    /// How long `UNDROP TABLE` can bring a table back.
    pub fn with_retention(mut self, retention: Duration) -> Self {
        self.retention = retention;
        self
    }

    /// Where external tables' locations are vouched (`house-cache`).
    pub fn with_grants(mut self, grants: Arc<dyn PrefixGrants>) -> Self {
        self.grants = Some(grants);
        self
    }

    /// The catalog.
    pub fn catalog(&self) -> &Arc<dyn HouseCatalog> {
        &self.catalog
    }

    /// The tables' root.
    pub fn root(&self) -> &LakeRoot {
        &self.root
    }

    /// `ns_<id>`: the namespace's root and its `default` database.
    pub fn root_namespace(namespace: u64) -> NamespaceIdent {
        NamespaceIdent::new(format!("ns_{namespace}"))
    }

    /// The namespace, database and table of a table's identifier (the inverse
    /// of [`Lake::namespace`]), unless it is not one of the House's.
    pub fn names_of(table: &TableIdent) -> Option<(u64, String, String)> {
        let levels = table.namespace().as_ref();
        let namespace = levels.first()?.strip_prefix("ns_")?.parse().ok()?;
        let database = match levels.len() {
            1 => "default".to_string(),
            2 => levels[1].clone(),
            _ => return None,
        };
        if database.starts_with(RESERVED_PREFIX) {
            return None;
        }
        Some((namespace, database, table.name().to_string()))
    }

    /// The Iceberg namespace of a ClickHouse database.
    pub fn namespace(namespace: u64, database: &str) -> NamespaceIdent {
        if database == "default" {
            return Self::root_namespace(namespace);
        }
        let mut levels = Self::root_namespace(namespace).inner();
        levels.push(database.to_string());
        NamespaceIdent::from_vec(levels).unwrap_or_else(|_| Self::root_namespace(namespace))
    }

    /// The Iceberg identifier of a ClickHouse table.
    pub fn table_ident(namespace: u64, database: &str, table: &str) -> TableIdent {
        TableIdent::new(Self::namespace(namespace, database), table.to_string())
    }

    /// `ns_<id>.__loams_dropped`: where dropped tables wait for `UNDROP` or
    /// their purge (HS1 Task 15).
    pub fn dropped_namespace(namespace: u64) -> NamespaceIdent {
        Self::namespace(namespace, DROPPED)
    }

    /// Creates `ns_<id>` (and its `default` database) the first time it is needed.
    pub async fn ensure_root(&self, namespace: u64) -> Result<(), HouseError> {
        match self
            .catalog
            .create_database(&Self::root_namespace(namespace), HashMap::new())
            .await
        {
            Ok(()) | Err(CatalogError::NamespaceExists(_)) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Whether the namespace has the database (`default` always exists).
    pub async fn database_exists(
        &self,
        namespace: u64,
        database: &str,
    ) -> Result<bool, HouseError> {
        if database == "default" {
            return Ok(true);
        }
        if database.starts_with(RESERVED_PREFIX) {
            return Ok(false);
        }
        match self
            .catalog
            .database_properties(&Self::namespace(namespace, database))
            .await
        {
            Ok(_) => Ok(true),
            Err(CatalogError::NoSuchNamespace(_)) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// `81` unless the database exists.
    pub async fn require_database(&self, namespace: u64, database: &str) -> Result<(), HouseError> {
        if self.database_exists(namespace, database).await? {
            Ok(())
        } else {
            Err(unknown_database(database))
        }
    }

    /// The namespace's databases, by name (`default` always among them).
    pub async fn databases(&self, namespace: u64) -> Result<Vec<DatabaseInfo>, HouseError> {
        let mut out = vec![DatabaseInfo {
            name: "default".to_string(),
            comment: String::new(),
        }];
        let children = match self
            .catalog
            .list_databases(&Self::root_namespace(namespace))
            .await
        {
            Ok(children) => children,
            Err(CatalogError::NoSuchNamespace(_)) => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        for child in children {
            let Some(name) = child.as_ref().last().cloned() else {
                continue;
            };
            // `_house` (the query log, HS1 Task 23) is the House's own: never
            // listed; [`Lake::house_tables`] finds it for the House's sweeps.
            if name.starts_with(RESERVED_PREFIX) || name == crate::querylog::HOUSE_DATABASE {
                continue;
            }
            let comment = self
                .catalog
                .database_properties(&child)
                .await
                .ok()
                .and_then(|p| p.get("comment").cloned())
                .unwrap_or_default();
            out.push(DatabaseInfo { name, comment });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// `CREATE DATABASE`.
    pub async fn create_database(
        &self,
        namespace: u64,
        database: &str,
        comment: Option<&str>,
        if_not_exists: bool,
    ) -> Result<(), HouseError> {
        ddl::check_new_name("database", database)?;
        if database == "default" {
            return if if_not_exists {
                Ok(())
            } else {
                Err(CatalogError::NamespaceExists(database.to_string()).into())
            };
        }
        self.ensure_root(namespace).await?;
        let mut properties = HashMap::new();
        if let Some(comment) = comment {
            properties.insert("comment".to_string(), comment.to_string());
        }
        match self
            .catalog
            .create_database(&Self::namespace(namespace, database), properties)
            .await
        {
            Ok(()) => Ok(()),
            Err(CatalogError::NamespaceExists(_)) if if_not_exists => Ok(()),
            Err(CatalogError::NamespaceExists(_)) => Err(ddl::clickhouse_error(
                82,
                "DATABASE_ALREADY_EXISTS",
                format!("Database {database} already exists"),
            )),
            Err(err) => Err(err.into()),
        }
    }

    /// `DROP DATABASE`: its tables are dropped (and can be brought back with
    /// `UNDROP` once the database exists again), then the database. `default`
    /// stays.
    pub async fn drop_database(
        &self,
        namespace: u64,
        database: &str,
        if_exists: bool,
    ) -> Result<(), HouseError> {
        if database == "default" {
            return Err(HouseError::from(ChError::bad_arguments(
                "Database default cannot be dropped on the House: drop its tables instead",
            )));
        }
        if !self.database_exists(namespace, database).await? {
            return if if_exists {
                Ok(())
            } else {
                Err(unknown_database(database))
            };
        }
        let names = self.table_names(namespace, database).await?;
        let mut tables = Vec::with_capacity(names.len());
        for name in &names {
            if let Some(table) = self.load(namespace, database, name).await? {
                if ddl::class_of(&table.metadata) == TableClass::External {
                    return Err(access_denied(&format!(
                        "DROP DATABASE {database} would drop {database}.{name}"
                    )));
                }
                tables.push(table);
            }
        }
        for table in tables {
            self.soft_drop(namespace, database, table).await?;
        }
        self.catalog
            .drop_database(&Self::namespace(namespace, database))
            .await
            .or_else(|err| match err {
                CatalogError::NoSuchNamespace(_) if if_exists => Ok(()),
                other => Err(other),
            })
            .map_err(HouseError::from)
    }

    /// A database's table names, sorted; `81` when it does not exist.
    pub async fn table_names(
        &self,
        namespace: u64,
        database: &str,
    ) -> Result<Vec<String>, HouseError> {
        match self
            .catalog
            .list_tables(&Self::namespace(namespace, database))
            .await
        {
            Ok(tables) => Ok(tables.into_iter().map(|t| t.name().to_string()).collect()),
            Err(CatalogError::NoSuchNamespace(_))
                if database == "default" || database == crate::querylog::HOUSE_DATABASE =>
            {
                Ok(Vec::new())
            }
            Err(CatalogError::NoSuchNamespace(_)) => Err(unknown_database(database)),
            Err(err) => Err(err.into()),
        }
    }

    /// A table, or `None` when it (or its database) does not exist. An external
    /// table's location is vouched to `house-cache`.
    pub async fn load(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
    ) -> Result<Option<LakeTable>, HouseError> {
        if database.starts_with(RESERVED_PREFIX) {
            return Ok(None);
        }
        match self
            .catalog
            .load_table(&Self::table_ident(namespace, database, table))
            .await
        {
            Ok(loaded) => {
                if ddl::class_of(&loaded.metadata) == TableClass::External {
                    self.vouch(namespace, &loaded);
                }
                Ok(Some(loaded))
            }
            Err(CatalogError::NoSuchTable(_) | CatalogError::NoSuchNamespace(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn vouch(&self, namespace: u64, table: &LakeTable) {
        let Some(grants) = &self.grants else {
            return;
        };
        if let Some(relative) = self.root.relative(table.metadata.location()) {
            let prefix = format!("{}/", relative.trim_end_matches('/'));
            // The `ns/` tree is the namespaces' own: a location in this
            // namespace's part is granted already, and one in another's must never
            // be (PR #448 review).
            if prefix != "/" && !prefix.starts_with("ns/") {
                grants.vouch(&namespace.to_string(), &prefix);
            }
        }
    }

    /// `CREATE TABLE`: `None` when it exists and `if_not_exists` says so.
    pub async fn create_table(
        &self,
        namespace: u64,
        database: &str,
        creation: TableCreation,
        if_not_exists: bool,
    ) -> Result<Option<LakeTable>, HouseError> {
        ddl::check_new_name("table", &creation.name)?;
        self.ensure_root(namespace).await?;
        self.require_database(namespace, database).await?;
        let name = creation.name.clone();
        match self
            .catalog
            .create_table(&Self::namespace(namespace, database), creation)
            .await
        {
            Ok(table) => Ok(Some(table)),
            Err(CatalogError::TableExists(_)) if if_not_exists => Ok(None),
            Err(CatalogError::TableExists(_)) => Err(HouseError::from(
                ChError::table_already_exists(format!("Table {database}.{name} already exists")),
            )),
            Err(err) => Err(err.into()),
        }
    }

    /// The table, or `60`; `497` when it is external.
    pub async fn lake_table(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
        what: &str,
    ) -> Result<LakeTable, HouseError> {
        self.require_database(namespace, database).await?;
        let loaded = self
            .load(namespace, database, table)
            .await?
            .ok_or_else(|| unknown_table(database, table))?;
        if ddl::class_of(&loaded.metadata) == TableClass::External {
            return Err(access_denied(&format!("{what} {database}.{table}")));
        }
        if ddl::class_of(&loaded.metadata) == TableClass::View {
            return Err(HouseError::from(crate::errors::ChError::not_implemented(
                format!("{what} {database}.{table}: it is a view, which holds no rows"),
            )));
        }
        Ok(loaded)
    }

    /// `DROP TABLE`: kept for the retention, then purged by HS1 Task 15.
    pub async fn drop_table(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
        if_exists: bool,
    ) -> Result<(), HouseError> {
        if !self.database_exists(namespace, database).await? {
            return if if_exists {
                Ok(())
            } else {
                Err(unknown_database(database))
            };
        }
        let Some(loaded) = self.load(namespace, database, table).await? else {
            return if if_exists {
                Ok(())
            } else {
                Err(unknown_table(database, table))
            };
        };
        if ddl::class_of(&loaded.metadata) == TableClass::External {
            return Err(access_denied(&format!("DROP TABLE {database}.{table}")));
        }
        self.soft_drop(namespace, database, loaded).await
    }

    async fn soft_drop(
        &self,
        namespace: u64,
        database: &str,
        table: LakeTable,
    ) -> Result<(), HouseError> {
        let at = now_ms();
        let marked = self
            .catalog
            .commit(
                &table.ident,
                Vec::new(),
                vec![TableUpdate::SetProperties {
                    updates: HashMap::from([
                        (DROPPED_DATABASE.to_string(), database.to_string()),
                        (DROPPED_NAME.to_string(), table.ident.name().to_string()),
                        (DROPPED_AT_MS.to_string(), at.to_string()),
                    ]),
                }],
            )
            .await?;
        let dropped = Self::dropped_namespace(namespace);
        match self.catalog.create_database(&dropped, HashMap::new()).await {
            Ok(()) | Err(CatalogError::NamespaceExists(_)) => {}
            Err(err) => return Err(err.into()),
        }
        let kept = TableIdent::new(dropped, Self::dropped_name(&marked.metadata.uuid(), at));
        self.catalog
            .rename_table(&marked.ident, &kept)
            .await
            .map_err(HouseError::from)
    }

    /// `UNDROP TABLE`: the newest table dropped under that name within the
    /// retention comes back.
    pub async fn undrop_table(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
    ) -> Result<(), HouseError> {
        self.require_database(namespace, database).await?;
        let not_found = || {
            HouseError::from(ChError::unknown_table(format!(
                "Table {database}.{table} was not dropped within the retention ({} s), so UNDROP \
                 cannot bring it back",
                self.retention.as_secs()
            )))
        };
        let dropped = match self
            .catalog
            .list_tables(&Self::dropped_namespace(namespace))
            .await
        {
            Ok(tables) => tables,
            Err(CatalogError::NoSuchNamespace(_)) => return Err(not_found()),
            Err(err) => return Err(err.into()),
        };
        let now = now_ms();
        let mut newest: Option<(u64, LakeTable)> = None;
        for ident in dropped {
            let loaded = match self.catalog.load_table(&ident).await {
                Ok(loaded) => loaded,
                Err(CatalogError::NoSuchTable(_)) => continue,
                Err(err) => return Err(err.into()),
            };
            let properties = loaded.metadata.properties();
            let at = properties
                .get(DROPPED_AT_MS)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let matches = properties.get(DROPPED_DATABASE).map(String::as_str) == Some(database)
                && properties.get(DROPPED_NAME).map(String::as_str) == Some(table)
                && now.saturating_sub(at) <= self.retention.as_millis() as u64;
            if matches && newest.as_ref().is_none_or(|(best, _)| at > *best) {
                newest = Some((at, loaded));
            }
        }
        let (_, found) = newest.ok_or_else(not_found)?;
        let back = Self::table_ident(namespace, database, table);
        self.catalog
            .rename_table(&found.ident, &back)
            .await
            .map_err(|err| match err {
                CatalogError::TableExists(_) => HouseError::from(ChError::table_already_exists(
                    format!("Table {database}.{table} already exists"),
                )),
                other => other.into(),
            })?;
        self.catalog
            .commit(
                &back,
                Vec::new(),
                vec![TableUpdate::RemoveProperties {
                    removals: vec![
                        DROPPED_DATABASE.to_string(),
                        DROPPED_NAME.to_string(),
                        DROPPED_AT_MS.to_string(),
                    ],
                }],
            )
            .await?;
        Ok(())
    }

    /// The names of the namespace's SQL functions (HE1 R14.11).
    pub async fn function_names(&self, namespace: u64) -> Result<Vec<String>, HouseError> {
        match self
            .catalog
            .list_tables(&Self::namespace(namespace, FUNCTIONS_DATABASE))
            .await
        {
            Ok(tables) => Ok(tables.into_iter().map(|t| t.name().to_string()).collect()),
            Err(CatalogError::NoSuchNamespace(_)) => Ok(Vec::new()),
            Err(err) => Err(err.into()),
        }
    }

    /// A SQL function's lambda (`(a, b) -> …`), as `CREATE FUNCTION` wrote it.
    pub async fn function(&self, namespace: u64, name: &str) -> Result<Option<String>, HouseError> {
        match self
            .catalog
            .load_table(&Self::table_ident(namespace, FUNCTIONS_DATABASE, name))
            .await
        {
            Ok(loaded) => Ok(loaded
                .metadata
                .properties()
                .get(ddl::PROP_FUNCTION_LAMBDA)
                .cloned()),
            Err(CatalogError::NoSuchTable(_) | CatalogError::NoSuchNamespace(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// `CREATE [OR REPLACE] FUNCTION [IF NOT EXISTS] name AS lambda` (HE1
    /// R14.11): a SQL function of the namespace, a catalog entry with no files
    /// in the House's own database `__loams_functions`, which no statement
    /// names. `609` when it exists and neither `OR REPLACE` nor `IF NOT
    /// EXISTS` is given.
    pub async fn create_function(
        &self,
        namespace: u64,
        function: &ddl::CreateFunction,
    ) -> Result<(), HouseError> {
        self.ensure_root(namespace).await?;
        match self
            .catalog
            .create_database(
                &Self::namespace(namespace, FUNCTIONS_DATABASE),
                HashMap::from([(
                    "comment".to_string(),
                    "The House's own: the namespace's SQL functions".to_string(),
                )]),
            )
            .await
        {
            Ok(()) | Err(CatalogError::NamespaceExists(_)) => {}
            Err(err) => return Err(err.into()),
        }
        let properties = HashMap::from([
            (ddl::PROP_DDL.to_string(), function.render()),
            (ddl::PROP_ENGINE.to_string(), "Function".to_string()),
            (ddl::PROP_CLASS.to_string(), "view".to_string()),
            (
                ddl::PROP_FUNCTION_LAMBDA.to_string(),
                function.lambda.clone(),
            ),
        ]);
        let ident = Self::table_ident(namespace, FUNCTIONS_DATABASE, &function.name);
        match self.catalog.load_table(&ident).await {
            Ok(existing) => {
                if function.if_not_exists {
                    return Ok(());
                }
                if !function.or_replace {
                    return Err(ddl::clickhouse_error(
                        609,
                        "FUNCTION_ALREADY_EXISTS",
                        format!(
                            "User-defined object '{}' already exists: while adding user \
                             defined function `{}`",
                            function.name, function.name
                        ),
                    ));
                }
                self.catalog
                    .commit(
                        &existing.ident,
                        Vec::new(),
                        vec![TableUpdate::SetProperties {
                            updates: properties,
                        }],
                    )
                    .await?;
                Ok(())
            }
            Err(CatalogError::NoSuchTable(_) | CatalogError::NoSuchNamespace(_)) => {
                let location =
                    self.root()
                        .table_location(namespace, FUNCTIONS_DATABASE, &function.name);
                let creation = ddl::view_creation(&function.name, location, properties)?;
                match self
                    .catalog
                    .create_table(&Self::namespace(namespace, FUNCTIONS_DATABASE), creation)
                    .await
                {
                    Ok(_) => Ok(()),
                    // Another front made it first: as ClickHouse answers a race.
                    Err(CatalogError::TableExists(_)) if function.if_not_exists => Ok(()),
                    Err(err) => Err(err.into()),
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// `DROP FUNCTION [IF EXISTS] name` (HE1 R14.11): `46` for a function the
    /// namespace does not have.
    pub async fn drop_function(
        &self,
        namespace: u64,
        name: &str,
        if_exists: bool,
    ) -> Result<(), HouseError> {
        let ident = Self::table_ident(namespace, FUNCTIONS_DATABASE, name);
        match self.catalog.drop_table(&ident).await {
            Ok(()) => Ok(()),
            Err(CatalogError::NoSuchTable(_) | CatalogError::NoSuchNamespace(_)) if if_exists => {
                Ok(())
            }
            Err(CatalogError::NoSuchTable(_) | CatalogError::NoSuchNamespace(_)) => {
                Err(ddl::clickhouse_error(
                    46,
                    "UNKNOWN_FUNCTION",
                    format!(
                        "User-defined object '{name}' doesn't exist: while removing user \
                         defined function `{name}`"
                    ),
                ))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// `EXCHANGE TABLES a AND b` within the namespace (HE1 R14.9): each name
    /// then holds the other's table. Answers the tables now under `a` and `b`.
    ///
    /// The catalogs rename one table at a time (Iceberg REST has no swap), so
    /// the exchange is three renames through a name of the House's own (`a` →
    /// `__loams_exchange_…`, `b` → `a`, that → `b`), undone in reverse when a
    /// step fails. It is not atomic as ClickHouse's is: a statement that
    /// resolves `a` or `b` between the steps can find no table (`60`), and a
    /// front that dies between them leaves the moved table under the House's
    /// name, which `SHOW TABLES` does not list.
    pub async fn exchange_tables(
        &self,
        namespace: u64,
        a: (&str, &str),
        b: (&str, &str),
    ) -> Result<(LakeTable, LakeTable), HouseError> {
        let first = self.exchanged(namespace, a).await?;
        let second = self.exchanged(namespace, b).await?;
        // A table exchanged with itself stays (the 26.9.2.8 oracle answers it).
        if first.ident == second.ident {
            return Ok((first, second));
        }
        let parked = Self::table_ident(
            namespace,
            a.0,
            &format!(
                "{RESERVED_PREFIX}_exchange_{}",
                uuid::Uuid::new_v4().simple()
            ),
        );
        let a_ident = Self::table_ident(namespace, a.0, a.1);
        let b_ident = Self::table_ident(namespace, b.0, b.1);
        self.catalog.rename_table(&first.ident, &parked).await?;
        if let Err(err) = self.catalog.rename_table(&second.ident, &a_ident).await {
            let _ = self.catalog.rename_table(&parked, &first.ident).await;
            return Err(err.into());
        }
        if let Err(err) = self.catalog.rename_table(&parked, &b_ident).await {
            let _ = self.catalog.rename_table(&a_ident, &second.ident).await;
            let _ = self.catalog.rename_table(&parked, &first.ident).await;
            return Err(err.into());
        }
        Ok((
            self.catalog.load_table(&a_ident).await?,
            self.catalog.load_table(&b_ident).await?,
        ))
    }

    /// A table `EXCHANGE TABLES` names: `60` when missing, `497` when it is
    /// another writer's.
    async fn exchanged(
        &self,
        namespace: u64,
        (database, table): (&str, &str),
    ) -> Result<LakeTable, HouseError> {
        self.require_database(namespace, database).await?;
        let loaded = self
            .load(namespace, database, table)
            .await?
            .ok_or_else(|| unknown_table(database, table))?;
        if ddl::class_of(&loaded.metadata) == TableClass::External {
            return Err(access_denied(&format!(
                "EXCHANGE TABLES {database}.{table}"
            )));
        }
        Ok(loaded)
    }

    /// `RENAME TABLE a TO b` within the namespace.
    pub async fn rename_table(
        &self,
        namespace: u64,
        from: (&str, &str),
        to: (&str, &str),
    ) -> Result<LakeTable, HouseError> {
        ddl::check_new_name("table", to.1)?;
        let table = self
            .lake_table(namespace, from.0, from.1, "RENAME TABLE")
            .await?;
        self.require_database(namespace, to.0).await?;
        let target = Self::table_ident(namespace, to.0, to.1);
        self.catalog
            .rename_table(&table.ident, &target)
            .await
            .map_err(|err| match err {
                CatalogError::TableExists(_) => HouseError::from(ChError::table_already_exists(
                    format!("Table {}.{} already exists", to.0, to.1),
                )),
                other => other.into(),
            })?;
        self.catalog
            .load_table(&target)
            .await
            .map_err(HouseError::from)
    }
}

fn unknown_database(database: &str) -> HouseError {
    HouseError::from(ChError::unknown_database(format!(
        "Database {database} does not exist"
    )))
}

fn unknown_table(database: &str, table: &str) -> HouseError {
    HouseError::from(ChError::unknown_table(format!(
        "Table {database}.{table} does not exist"
    )))
}

/// Registers every table of a namespace's catalog in another catalog (moving a
/// `--single-node` House's local catalog into Lakekeeper, §49 §17): each Iceberg
/// namespace under `ns_<id>` is created where missing, and each table is
/// registered with its current metadata file (Iceberg REST `register`). Tables the
/// target already has are left alone. Answers how many were registered.
pub async fn register_all(
    from: &dyn HouseCatalog,
    to: &dyn HouseCatalog,
    namespace: u64,
) -> Result<usize, CatalogError> {
    let mut pending = vec![Lake::root_namespace(namespace)];
    let mut registered = 0;
    while let Some(ns) = pending.pop() {
        match to
            .create_database(&ns, from.database_properties(&ns).await?)
            .await
        {
            Ok(()) | Err(CatalogError::NamespaceExists(_)) => {}
            Err(err) => return Err(err),
        }
        for table in from.list_tables(&ns).await? {
            let loaded = from.load_table(&table).await?;
            match to.register_table(&table, &loaded.metadata_location).await {
                Ok(_) => registered += 1,
                Err(CatalogError::TableExists(_)) => {}
                Err(err) => return Err(err),
            }
        }
        pending.extend(from.list_databases(&ns).await?);
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_and_relative_keys() {
        let root = LakeRoot::from_store_url("s3://bkt/some/prefix/").expect("root");
        assert_eq!(root.to_string(), "s3://bkt/some/prefix");
        let location = root.table_location(7, "sales db", "t/1");
        assert!(
            location.starts_with("s3://bkt/some/prefix/ns/7/house/sales_db/t_1-"),
            "{location}"
        );
        assert_eq!(
            root.relative(&location)
                .map(|r| r.starts_with("ns/7/house/")),
            Some(true)
        );
        assert_eq!(root.relative("s3://bkt/other/x"), None);
        assert_eq!(root.relative("s3://bkt2/some/prefix/x"), None);
        assert_eq!(root.relative("gs://bkt/some/prefix/x"), None);
        let local = LakeRoot::from_store_url("file:///tmp/x").expect("root");
        assert_eq!(local, LakeRoot::local());
        assert_eq!(local.relative("s3://local/ns/1/a"), Some("ns/1/a"));
        assert!(LakeRoot::from_store_url("http://x").is_err());
        assert_eq!(Lake::namespace(7, "default").to_url_string(), "ns_7");
        assert_eq!(Lake::namespace(7, "d").to_url_string(), "ns_7\u{1f}d");
    }
}
