//! The checks a statement passes on every House surface (HS1 Tasks 4, 5 and 8):
//! what the front answers itself, which worker runs it, and whether a statement a
//! worker analysed may run. The ClickHouse HTTP interface ([`crate::http`]) and
//! `loams.house.v1` ([`crate::api`]) both call these, so a statement refused on
//! one surface is refused on the other with the same code.

use std::collections::HashSet;

use loams_house_ipc::QueryClass;

use async_trait::async_trait;
use loams_house_ipc::{Execute, Limits, Output};

use crate::admission::{Outcome, WorkerLease, WorkerPool};
use crate::catalog::Lake;
use crate::classify::{Classified, Expect, Stmt, decide};
use crate::config::UserMap;
use crate::ddl::{self, ChColumn, Described, Kind, Oracle};
use crate::deny;
use crate::errors::{ChError, HouseError};
use crate::read::{ReadContext, ReadPlan};
use crate::session::SessionGuard;

/// The text chDB runs for a statement: the host functions answer the House's
/// values (HS1 Task 5), rewritten in the text and refused by the deny list in any
/// form the rewrite did not take; and what runs is what the worker explains,
/// which it ends with its own `FORMAT`, so no trailing `;` or comment (Task 5
/// fix round 1).
pub fn statement_text(sql: &str, user: &str) -> String {
    let sql = deny::rewrite_host_functions(sql, &host_values(user));
    let sql = crate::system::rewrite_cluster_reads(&sql);
    crate::classify::without_trailer(&sql).to_string()
}

/// What the host functions answer for `user`.
pub fn host_values(user: &str) -> deny::HostValues<'_> {
    deny::HostValues {
        display_name: crate::config::DISPLAY_NAME,
        timezone: "UTC",
        user,
    }
}

/// Whether a statement a worker analysed may run: the deny list over its trees
/// (HS1 Task 5: `344`, or `164` for a denied setting), then ClickHouse's class
/// ([`decide`]). A statement refused on its class alone (more than one, or one
/// ClickHouse cannot parse) has no trees, and `decide` refuses it.
pub fn admit(
    analysis: &loams_house_ipc::Analysis,
    sql: &str,
    expect: Expect,
    readonly: bool,
    unparsed: Option<&str>,
) -> Result<(), HouseError> {
    admit_views(analysis, sql, expect, readonly, unparsed, &[])
}

/// [`admit`], with the table functions that call the namespace's own views
/// ([`view_calls`], HE1 R14.5) allowed.
pub fn admit_views(
    analysis: &loams_house_ipc::Analysis,
    sql: &str,
    expect: Expect,
    readonly: bool,
    unparsed: Option<&str>,
    views: &[String],
) -> Result<(), HouseError> {
    let classification = analysis.classification;
    if classification.statements == 1 && classification.class != QueryClass::Unknown {
        // A `CREATE` of a lake table or a view never runs in chDB (the front owns
        // it, or `decide` refuses the form): its engine is held to the denied
        // engines, not to a temporary table's Memory or Null.
        let tree = deny::QueryTree::from_explain(&analysis.ast, analysis.query_tree.as_deref())
            .shared_create(crate::classify::creates_shared_object(sql))
            .catalog_views(views)
            .statement(sql);
        deny::check(&tree).map_err(HouseError::from)?;
    }
    decide(classification, expect, readonly, unparsed)
}

/// The table functions an analysed statement calls that are the namespace's
/// own views, as its trees print them (HE1 R14.5): ClickHouse reads
/// `v(p = 1)`, a name that is no table function, as a call of the
/// parameterised view `v` in the current database (or `db.v(…)`), and so does
/// the House, resolving the name in the catalog. Names that are not views of
/// the namespace are left out, so the deny list still refuses them.
pub async fn view_calls(
    lake: Option<&Lake>,
    namespace: u64,
    current: &str,
    analysis: &loams_house_ipc::Analysis,
) -> Result<Vec<String>, HouseError> {
    let Some(lake) = lake else {
        return Ok(Vec::new());
    };
    let tree = deny::QueryTree::from_explain(&analysis.ast, analysis.query_tree.as_deref());
    let mut out = Vec::new();
    for printed in tree.view_calls() {
        if let Some((_, _, table)) =
            crate::read::resolve(lake, namespace, current, &printed).await?
            && ddl::class_of(&table.metadata) == ddl::TableClass::View
        {
            out.push(printed);
        }
    }
    Ok(out)
}

/// The scopes a statement needs (HS1 Task 20, §49 §14).
pub fn required(stmt: &Stmt) -> &'static [crate::identity::Scope] {
    use crate::identity::Scope::{Ddl, Read, Write};
    match stmt {
        Stmt::Query { .. }
        | Stmt::Show { .. }
        | Stmt::Describe { .. }
        | Stmt::Exists { .. }
        | Stmt::Explain { .. }
        | Stmt::Use(_)
        | Stmt::Set(_)
        | Stmt::CreateTemporaryTable { .. }
        | Stmt::KillQuery(_)
        | Stmt::Unsupported { .. } => &[Read],
        Stmt::Drop(drop) if drop.temporary => &[Read],
        Stmt::Insert(_) | Stmt::Optimize { .. } | Stmt::Mutation { .. } => &[Write],
        // A materialized view writes into its target (HS1 Task 16).
        Stmt::CreateTableAs { .. } | Stmt::CreateMaterializedView { .. } => &[Ddl, Write],
        Stmt::CreateDatabase { .. }
        | Stmt::CreateTable { .. }
        | Stmt::CreatePipe { .. }
        | Stmt::Drop(_)
        | Stmt::Truncate { .. }
        | Stmt::Alter { .. }
        | Stmt::CreateView { .. }
        | Stmt::CreateFunction { .. }
        | Stmt::CreateDictionary { .. }
        | Stmt::Rename { .. }
        | Stmt::Exchange { .. }
        | Stmt::Undrop { .. } => &[Ddl],
    }
}

/// `497` unless the identity holds every scope the statement needs (HS1 Task
/// 20). A statement only ClickHouse's parser can read is a query here (it is
/// held to reads further on).
pub fn authorize(user: &UserMap, classified: &Classified) -> Result<(), HouseError> {
    let (needed, what): (&[crate::identity::Scope], String) = match classified {
        Classified::Known { stmt, .. } => (required(stmt), stmt.kind().to_string()),
        Classified::Unparsed { .. } => (&[crate::identity::Scope::Read], "Query".to_string()),
    };
    match needed.iter().find(|scope| !user.scopes.has(**scope)) {
        Some(scope) => Err(crate::identity::lacks(user, *scope, &what)),
        None => Ok(()),
    }
}

/// The task that will serve an owned statement this front does not yet (`48`).
/// The catalog's statements reach here only when the House has no catalog
/// ([`catalog_answer`] takes them otherwise).
fn later(stmt: &Stmt) -> Option<String> {
    if catalog_kind(stmt).is_some_and(|kind| kind.needs_catalog()) {
        return Some(
            "This House has no catalog (start it with --catalog or --single-node), so it holds \
             no lake tables"
                .to_string(),
        );
    }
    Some(match stmt {
        Stmt::CreateTableAs { .. } => {
            "CREATE TABLE … AS SELECT makes a lake table, and this House has no catalog \
                 (start it with --catalog or --single-node)"
                .to_string()
        }
        Stmt::Truncate { .. } => {
            "TRUNCATE serves the catalog's lake tables only (not temporary tables)".to_string()
        }
        Stmt::Optimize { .. } => {
            "OPTIMIZE TABLE compacts lake tables, and this House has no catalog (start it \
                 with --catalog or --single-node)"
                .to_string()
        }
        Stmt::Mutation { .. } => {
            "Mutations change lake tables, and this House has no catalog (start it with \
                 --catalog or --single-node)"
                .to_string()
        }
        // The HTTP front answers `KILL QUERY` before it gets here (HS1 Task 21);
        // `loams.house.v1` has `CancelQuery` instead.
        Stmt::KillQuery(_) => {
            "KILL QUERY is served on the ClickHouse HTTP interface; loams.house.v1 \
                 cancels a query with CancelQuery"
                .to_string()
        }
        Stmt::Unsupported { kind } => {
            format!("{kind} is not on the House's ClickHouse surface")
        }
        _ => return None,
    })
}

/// What the front answers itself: `Some(Ok)` for an empty success (`SET`, `USE`),
/// `Some(Err)` for an error, `None` for a statement a worker runs.
pub fn front_answer(
    stmt: &Stmt,
    session: Option<&SessionGuard>,
    limits: &crate::settings::SessionLimits,
    known: &HashSet<String>,
) -> Option<Result<(), HouseError>> {
    if let Some(message) = later(stmt) {
        return Some(Err(HouseError::from(ChError::not_implemented(message))));
    }
    match stmt {
        Stmt::Set(pairs) => Some((|| {
            for (name, value) in pairs {
                crate::settings::check(name, value, limits, known)?;
            }
            // Without a session a SET has nothing to keep it, as in ClickHouse.
            if let Some(guard) = session {
                let mut state = guard.state();
                for (name, value) in pairs {
                    state.settings.apply(name, value, limits, known)?;
                }
            }
            Ok(())
        })()),
        Stmt::Use(database) => Some(if database != "default" {
            Err(HouseError::from(ChError::unknown_database(format!(
                "Database {database} does not exist"
            ))))
        } else {
            if let Some(guard) = session {
                guard.state().database = database.clone();
            }
            Ok(())
        }),
        _ => None,
    }
}

/// The worker for a statement: the session's pinned worker (a lost pin falls back
/// to any worker: its temporary tables are gone), a newly pinned one for the
/// session's first temporary table, or any worker of the namespace.
pub async fn acquire_for(
    pool: &WorkerPool,
    namespace: &str,
    session: Option<&mut SessionGuard>,
    pins: bool,
) -> Result<WorkerLease, HouseError> {
    let Some(guard) = session else {
        return pool.acquire(namespace).await;
    };
    let pinned = guard.state().pinned.clone();
    if let Some(worker) = pinned {
        if let Some(lease) = pool.acquire_pinned(namespace, &worker).await? {
            return Ok(lease);
        }
        guard.state().pinned = None;
    }
    if pins {
        let lease = pool.acquire_and_pin(namespace).await?;
        // Kept only if the statement succeeds (fix round 1, M1), and only within
        // the user's cap (HS1 Task 21).
        if let Err(err) = guard.pin_provisionally(lease.worker_id().to_string()) {
            pool.unpin(lease.worker_id());
            pool.release(lease, crate::admission::Outcome::Completed);
            return Err(err);
        }
        return Ok(lease);
    }
    pool.acquire(namespace).await
}

/// The catalog statement a [`Stmt`] is (HS1 Task 10), if any.
pub fn catalog_kind(stmt: &Stmt) -> Option<Kind> {
    Some(match stmt {
        Stmt::CreateDatabase { .. } => Kind::CreateDatabase,
        Stmt::CreateTable { .. } | Stmt::CreatePipe { .. } => Kind::CreateTable,
        Stmt::CreateMaterializedView { .. } => Kind::CreateMaterializedView,
        Stmt::Drop(drop) if !drop.temporary => Kind::Drop,
        Stmt::Undrop { .. } => Kind::Undrop,
        Stmt::Rename { .. } => Kind::Rename,
        Stmt::Exchange { .. } => Kind::Exchange,
        Stmt::Alter { .. } => Kind::Alter,
        Stmt::CreateView { .. } => Kind::CreateView,
        Stmt::CreateFunction { .. } => Kind::CreateFunction,
        Stmt::CreateDictionary { .. } => Kind::CreateDictionary,
        Stmt::Describe { .. } => Kind::Describe,
        Stmt::Show { .. } => Kind::Show,
        Stmt::Exists { .. } => Kind::Exists,
        Stmt::Insert(_) => Kind::Insert,
        _ => return None,
    })
}

impl Kind {
    /// Whether the statement means nothing without a catalog (`48` then); the
    /// others fall through to chDB.
    pub fn needs_catalog(self) -> bool {
        !matches!(
            self,
            Kind::Describe | Kind::Show | Kind::Exists | Kind::Insert
        )
    }
}

/// What the catalog made of a statement.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogOutcome {
    /// Done: an empty success.
    Done,
    /// Answer with this query's rows (a `SELECT` over constants).
    Rows(String),
    /// An `INSERT` into a lake table, for [`crate::insert::run`] (HS1 Task 12).
    Insert(Box<crate::insert::Target>),
    /// `CREATE TABLE … AS SELECT` of a lake table, for
    /// [`crate::insert::create_as`] (HS1 Task 12).
    CreateAs(Box<crate::insert::CreateAs>),
    /// `OPTIMIZE TABLE` of a lake table, for [`crate::compact::optimize`]
    /// (HS1 Task 13).
    Optimize(Box<crate::compact::OptimizePlan>),
    /// A mutation of a lake table, for [`crate::mutate::run`] (HE1 R9.5).
    Mutate(Box<crate::mutate::MutationPlan>),
    /// Not the catalog's: on to [`front_answer`] and chDB.
    NotOurs,
}

/// The catalog's answer to a statement (HS1 Task 10): DDL, `USE`, and
/// `DESCRIBE`/`SHOW CREATE`/`EXISTS`/`INSERT` naming a catalog table. `text` is
/// the statement without its `FORMAT`; `current` the request's database.
#[allow(clippy::too_many_arguments)]
pub async fn catalog_answer(
    lake: Option<&Lake>,
    pool: &WorkerPool,
    user: &UserMap,
    current: &str,
    stmt: &Stmt,
    text: &str,
    session: Option<&SessionGuard>,
    query_id: &str,
) -> Result<CatalogOutcome, HouseError> {
    let namespace = user.namespace;
    let Some(lake) = lake else {
        return Ok(CatalogOutcome::NotOurs);
    };
    if let Stmt::Use(database) = stmt {
        lake.require_database(namespace, database).await?;
        if let Some(guard) = session {
            guard.state().database = database.clone();
        }
        return Ok(CatalogOutcome::Done);
    }
    if let Stmt::Optimize { .. } = stmt {
        let plan = crate::compact::OptimizePlan::parse(text, current)?;
        if !crate::identity::granted(user, &plan.database, &plan.name) {
            return Err(crate::identity::not_granted(
                user,
                &plan.database,
                &plan.name,
            ));
        }
        // A table the catalog lacks is `60`, an external one `497`, as for DDL.
        lake.lake_table(namespace, &plan.database, &plan.name, "OPTIMIZE TABLE")
            .await?;
        return Ok(CatalogOutcome::Optimize(Box::new(plan)));
    }
    // HE1 R9.14: an inner-table materialized view makes its table from its
    // SELECT's columns, as CREATE TABLE … AS SELECT does, then the view.
    if let Stmt::CreateMaterializedView { .. } = stmt
        && let Ok(mv) = crate::ddl::parse_create_mv(text)
        && mv.to.is_none()
        && mv.inner_table
    {
        let plan = crate::insert::CreateAs::inner_view(text, current)?;
        if !crate::identity::granted(user, &plan.database, &plan.name) {
            return Err(crate::identity::not_granted(
                user,
                &plan.database,
                &plan.name,
            ));
        }
        return Ok(CatalogOutcome::CreateAs(Box::new(plan)));
    }
    if let Stmt::Mutation { .. } = stmt {
        let Some(plan) = crate::mutate::MutationPlan::parse(text, current)? else {
            return Err(HouseError::from(ChError::syntax_error("Not a mutation")));
        };
        if !crate::identity::granted(user, &plan.database, &plan.name) {
            return Err(crate::identity::not_granted(
                user,
                &plan.database,
                &plan.name,
            ));
        }
        // A table the catalog lacks is `60`, an external one `497`, as for DDL.
        lake.lake_table(namespace, &plan.database, &plan.name, "ALTER TABLE")
            .await?;
        return Ok(CatalogOutcome::Mutate(Box::new(plan)));
    }
    if let Stmt::CreateTableAs { .. } = stmt {
        let plan = crate::insert::CreateAs::parse(text, current)?;
        if !crate::identity::granted(user, &plan.database, &plan.name) {
            return Err(crate::identity::not_granted(
                user,
                &plan.database,
                &plan.name,
            ));
        }
        return Ok(CatalogOutcome::CreateAs(Box::new(plan)));
    }
    if let Stmt::Truncate { .. } = stmt
        && let Some(((db, name), if_exists)) = crate::ddl::parse_truncate(text)
    {
        let database = db.unwrap_or_else(|| current.to_string());
        if !crate::identity::granted(user, &database, &name) {
            return Err(crate::identity::not_granted(user, &database, &name));
        }
        let table = if lake.database_exists(namespace, &database).await? {
            lake.load(namespace, &database, &name).await?
        } else {
            None
        };
        return match table {
            None if if_exists => Ok(CatalogOutcome::Done),
            // A temporary table, or no table: as before (chDB, or `48`).
            None => Ok(CatalogOutcome::NotOurs),
            Some(table) => {
                if crate::ddl::class_of(&table.metadata) == crate::ddl::TableClass::Pipe {
                    return Err(HouseError::from(ChError::not_implemented(format!(
                        "TRUNCATE {database}.{name}: the table is a pipe, which holds no rows"
                    ))));
                }
                if crate::ddl::class_of(&table.metadata) == crate::ddl::TableClass::External {
                    return Err(HouseError::from(ChError::access_denied(format!(
                        "TRUNCATE {database}.{name}: the table belongs to another writer (an \
                         external table), so the House reads it and never changes it"
                    ))));
                }
                let committer = lake.committer();
                crate::commit::truncate(committer.io(), &table.ident).await?;
                Ok(CatalogOutcome::Done)
            }
        };
    }
    if let Stmt::Insert(insert) = stmt {
        return Ok(
            match crate::insert::target(lake, user, current, insert).await? {
                Some(target) => CatalogOutcome::Insert(Box::new(target)),
                None => CatalogOutcome::NotOurs,
            },
        );
    }
    let Some(kind) = catalog_kind(stmt) else {
        return Ok(CatalogOutcome::NotOurs);
    };
    // The identity's grants (HS1 Task 20): DDL on a table or database outside
    // them is `497`; a table outside them is as unknown to `DESCRIBE`, `SHOW
    // CREATE` and `EXISTS` as a missing one.
    for object in ddl::touched(kind, text, current) {
        let allowed = match &object {
            ddl::Object::Table((db, name)) => {
                crate::identity::granted(user, db.as_deref().unwrap_or(current), name)
            }
            ddl::Object::Database(database) => crate::identity::granted_database(user, database),
        };
        if allowed {
            continue;
        }
        return match (kind, object) {
            (Kind::Exists, _) => Ok(CatalogOutcome::Rows(
                "SELECT toUInt8(0) AS result".to_string(),
            )),
            (Kind::Describe | Kind::Show, ddl::Object::Table((db, name))) => {
                Err(HouseError::from(ChError::unknown_table(format!(
                    "Table {}.{name} does not exist",
                    db.as_deref().unwrap_or(current)
                ))))
            }
            (_, ddl::Object::Table((db, name))) => Err(crate::identity::not_granted(
                user,
                db.as_deref().unwrap_or(current),
                &name,
            )),
            (_, ddl::Object::Database(database)) => {
                Err(crate::identity::not_granted(user, &database, "*"))
            }
        };
    }
    let mut oracle = WorkerOracle::new(pool, Some(lake), namespace, current, query_id);
    let answer = ddl::execute(lake, namespace, current, kind, text, &mut oracle).await;
    oracle.finish();
    Ok(match answer? {
        ddl::Answer::Done => CatalogOutcome::Done,
        ddl::Answer::Rows(query) => CatalogOutcome::Rows(query),
        ddl::Answer::PassThrough => CatalogOutcome::NotOurs,
    })
}

/// The deny list over a catalog statement's syntax tree (HS1 Tasks 5 and 10):
/// one statement that ClickHouse can parse, nothing denied anywhere in it.
pub fn admit_ddl(analysis: &loams_house_ipc::Analysis, sql: &str) -> Result<(), HouseError> {
    admit_ddl_views(analysis, sql, &[])
}

/// [`admit_ddl`], with calls of the namespace's own views allowed (HE1 R14.5).
pub fn admit_ddl_views(
    analysis: &loams_house_ipc::Analysis,
    sql: &str,
    views: &[String],
) -> Result<(), HouseError> {
    let classification = analysis.classification;
    if classification.statements > 1 {
        return Err(HouseError::from(ChError::syntax_error(
            "Multi-statements are not allowed: send one statement per request",
        )));
    }
    if classification.class == QueryClass::Unknown {
        return Err(HouseError::from(ChError::syntax_error(format!(
            "ClickHouse's parser cannot parse this statement: {}",
            crate::classify::OUTSIDE_SURFACE
        ))));
    }
    let tree = deny::QueryTree::from_explain(&analysis.ast, analysis.query_tree.as_deref())
        .shared_create(crate::classify::creates_shared_object(sql))
        .catalog_views(views)
        .statement(sql);
    deny::check(&tree).map_err(HouseError::from)
}

/// [`Oracle`] on a worker of the namespace, taken at the first question and
/// given back by [`WorkerOracle::finish`] (or when dropped).
struct WorkerOracle<'a> {
    pool: &'a WorkerPool,
    namespace: String,
    query_id: String,
    lease: Option<WorkerLease>,
    /// The catalog, the namespace and the current database: where a call of a
    /// view in a statement's text is resolved (HE1 R14.5).
    lake: Option<&'a Lake>,
    namespace_id: u64,
    current: String,
}

impl<'a> WorkerOracle<'a> {
    fn new(
        pool: &'a WorkerPool,
        lake: Option<&'a Lake>,
        namespace: u64,
        current: &str,
        query_id: &str,
    ) -> Self {
        Self {
            pool,
            namespace: namespace.to_string(),
            query_id: query_id.to_string(),
            lease: None,
            lake,
            namespace_id: namespace,
            current: current.to_string(),
        }
    }

    async fn lease(&mut self) -> Result<&mut WorkerLease, HouseError> {
        if self.lease.is_none() {
            self.lease = Some(self.pool.acquire(&self.namespace).await?);
        }
        self.lease
            .as_mut()
            .ok_or_else(|| HouseError::from(ChError::network_error("no worker")))
    }

    fn finish(&mut self) {
        if let Some(lease) = self.lease.take() {
            self.pool.release(lease, Outcome::Completed);
        }
    }

    /// Runs an internal read and answers its `JSONCompactEachRow` lines.
    async fn rows(
        &mut self,
        sql: &str,
        params: Vec<(String, String)>,
        settings: Vec<(String, String)>,
    ) -> Result<Vec<Vec<String>>, HouseError> {
        let execute = Execute {
            query_id: format!("{}-ddl", self.query_id),
            session: None,
            settings,
            views: Vec::new(),
            sql: sql.to_string(),
            format: "JSONCompactEachRow".to_string(),
            params,
            limits: Limits::default(),
            input: None,
            output: Output::Blocks,
            database: None,
        };
        let collected = self.lease().await?.run(execute).await?;
        let text = String::from_utf8_lossy(&collected.bytes);
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Vec<serde_json::Value>>(line)
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s,
                                other => other.to_string(),
                            })
                            .collect()
                    })
                    .map_err(|err| {
                        HouseError::from(ChError::network_error(format!(
                            "the worker's answer is unreadable: {err}"
                        )))
                    })
            })
            .collect()
    }
}

impl Drop for WorkerOracle<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

impl std::fmt::Debug for WorkerOracle<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerOracle")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

/// `Array(String)` as a query parameter's text.
fn string_array(values: &[String]) -> String {
    let quoted: Vec<String> = values
        .iter()
        .map(|v| format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect();
    format!("[{}]", quoted.join(","))
}

#[async_trait]
impl Oracle for WorkerOracle<'_> {
    async fn check(&mut self, sql: &str) -> Result<(), HouseError> {
        let analysis = self
            .lease()
            .await?
            .analyze(sql, &[])
            .await
            .map_err(deny::as_setting_refusal)?;
        let views = view_calls(self.lake, self.namespace_id, &self.current, &analysis).await?;
        admit_ddl_views(&analysis, sql, &views)
    }

    async fn describe(&mut self, columns: &str) -> Result<Vec<Described>, HouseError> {
        // `flatten_nested = 0`: a `Nested` column stays one column, which the type
        // table refuses, rather than arrays under dotted names. The column list
        // is a `String` parameter, read as `TabSeparated` text: escaped, so a
        // type written over several lines (`Array(Tuple(\n …))`, Laminar's
        // `spans`) or a default holding a backslash arrives as written (HE1
        // R14.6).
        let rows = self
            .rows(
                "DESCRIBE TABLE null({columns:String})",
                vec![("columns".to_string(), crate::http::tsv_escape(columns))],
                vec![("flatten_nested".to_string(), "0".to_string())],
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                let get = |i: usize| row.get(i).cloned().unwrap_or_default();
                if row.len() < 7 {
                    return Err(HouseError::from(ChError::network_error(
                        "the worker's DESCRIBE answered too few columns",
                    )));
                }
                Ok(Described {
                    column: ChColumn {
                        name: get(0),
                        ch_type: get(1),
                        default_kind: get(2),
                        default_expression: get(3),
                        comment: get(4),
                        codec: get(5),
                    },
                    ttl: get(6),
                })
            })
            .collect()
    }

    async fn builtin_function(&mut self, name: &str) -> Result<bool, HouseError> {
        let rows = self
            .rows(
                "SELECT count() FROM system.functions WHERE origin != 'SQLUserDefined' \
                 AND (name = {name:String} OR (case_insensitive AND lower(name) = lower({name:String})))",
                vec![("name".to_string(), name.to_string())],
                Vec::new(),
            )
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .is_some_and(|count| count != "0"))
    }

    async fn merge_tree_settings(&mut self, names: &[String]) -> Result<Vec<String>, HouseError> {
        let rows = self
            .rows(
                "SELECT name FROM system.merge_tree_settings WHERE name IN {names:Array(String)}",
                vec![("names".to_string(), string_array(names))],
                Vec::new(),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next())
            .collect())
    }
}

/// `SHOW TABLES`, `SHOW DATABASES` and `SHOW PROCESSLIST` as the `SELECT`
/// ClickHouse rewrites them into, over the `system` tables the House replaces
/// (HS1 Task 11): `Some(text)` to run instead. `SHOW TABLES FROM d` of a
/// database the namespace lacks is `81`, as in ClickHouse.
pub async fn show_answer(
    lake: Option<&Lake>,
    namespace: u64,
    current: &str,
    stmt: &Stmt,
) -> Result<Option<String>, HouseError> {
    let Stmt::Show { text } = stmt else {
        return Ok(None);
    };
    let Some(answer) = crate::system::show_query(text, current) else {
        return Ok(None);
    };
    let (query, database) = answer?;
    if let Some(database) = database
        && !matches!(
            database.as_str(),
            "system" | "information_schema" | "INFORMATION_SCHEMA"
        )
    {
        match lake {
            Some(lake) => lake.require_database(namespace, &database).await?,
            None if database == "default" => {}
            None => {
                return Err(HouseError::from(ChError::unknown_database(format!(
                    "Database {database} does not exist"
                ))));
            }
        }
    }
    Ok(Some(query))
}

/// What an admitted statement reads, pinned, and the views its worker makes
/// first (HS1 Task 11, [`crate::read::plan`]). `settings` are the session's and
/// the request's, Loams's own included.
#[allow(clippy::too_many_arguments)]
pub async fn read_plan(
    lake: Option<&Lake>,
    namespace: u64,
    database: &str,
    user: &UserMap,
    lease: &mut WorkerLease,
    analysis: &loams_house_ipc::Analysis,
    sql: &str,
    settings: &[(String, String)],
    batch: Option<&crate::pipes::PipeBatch>,
    front: Option<crate::read::Front<'_>>,
) -> Result<ReadPlan, HouseError> {
    crate::read::plan(
        ReadContext {
            lake,
            namespace,
            database,
            user: &user.user,
            grants: user.grants.as_ref(),
            batch,
            front,
            admin: user.scopes.admin,
        },
        lease,
        analysis,
        sql,
        settings,
    )
    .await
}
