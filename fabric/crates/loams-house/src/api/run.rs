//! Running one statement for `loams.house.v1` (HS1 Task 8): the front's checks,
//! a worker, the header, and the frames a result streams as ([`super`]'s docs say
//! what each step is for).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use connectrpc::{ConnectError, ErrorCode};
use loams_house_ipc::{Execute, InputSpec, Limits, Progress, QueryClass};
use loams_house_proto::loams::house::v1 as pb;
use loams_house_proto::loams::house::v1::__buffa::oneof::{
    execute_query_response, preview_table_response, result_batch,
};

use super::frames::{self, ArrowFramer, Caps, Column, Framer, JsonFramer};
use super::{
    DRAIN_BUDGET, EXPLAIN_MAX_BYTES, HouseApi, KILL_GRACE, MAX_TIMEOUT_MS, invalid,
    read_only_refusal, read_only_user_refusal, to_connect,
};
use crate::admission::{Event, Outcome, WorkerLease, WorkerPool};
use crate::classify::{Classified, Expect, Stmt, check_text, classify};
use crate::config::UserMap;
use crate::deny;
use crate::errors::{ChError, HouseError};
use crate::gate;
use crate::request;
use crate::watchdog::{Deadline, ExitReason};

/// A statement's entry in the front's process list (HS1 Task 21: one list for
/// both surfaces, which `CancelQuery` and `KILL QUERY` both read).
type RunningGuard = crate::watchdog::ProcessGuard;

impl HouseApi {
    /// Enters `st` into the process list: an id already running is
    /// `invalid_argument` (R8.8).
    fn register(
        &self,
        query_id: &str,
        user: &UserMap,
        sql: &str,
    ) -> Result<RunningGuard, ConnectError> {
        self.governance
            .processes
            .register(query_id, user, sql)
            .map_err(|err| invalid("query_id", err.message().to_string()))
    }

    /// Admits a statement with `settings` under the namespace queues and node
    /// memory (§49 §12, HS1 Task 21).
    async fn admit(
        &self,
        user: &UserMap,
        settings: &[(String, String)],
        access: crate::quota::Access,
    ) -> Result<crate::admission::Ticket, ConnectError> {
        let memory = crate::limits::statement_memory(settings, &self.config.session_limits);
        self.governance
            .admission
            .admit(&user.namespace.to_string(), memory, access)
            .await
            .map_err(|err| to_connect(&err))
    }
}

/// A refusal's ClickHouse code: the one its `ErrorInfo` names, else the
/// Connect code's counterpart (`reason_for`'s inverse, coarsely).
fn connect_code(err: &ConnectError) -> i32 {
    match err.code {
        ErrorCode::Unauthenticated => 516,
        ErrorCode::PermissionDenied => 497,
        ErrorCode::NotFound => 60,
        ErrorCode::InvalidArgument => 36,
        ErrorCode::Unimplemented => 48,
        ErrorCode::Unavailable => 210,
        _ => 1,
    }
}

fn cancelled_error(query_id: &str) -> HouseError {
    ExitReason::Cancel.client_error(query_id, None)
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// How a statement's output comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Output {
    /// `ARROW_IPC` frames.
    Arrow,
    /// `JSON_ROWS` frames.
    Json,
    /// The whole output, collected (`ExplainQuery`).
    Collect,
}

/// A statement as the API asks for it.
#[derive(Debug)]
pub(super) struct Statement {
    pub(super) sql: String,
    pub(super) database: String,
    pub(super) settings: Vec<(String, String)>,
    pub(super) params: Vec<(String, String)>,
    pub(super) output: Output,
    pub(super) caps: Caps,
    pub(super) timeout_ms: u32,
    pub(super) query_id: String,
    pub(super) read_only: bool,
    /// `ExplainQuery`: the `EXPLAIN` form to run the query under.
    pub(super) explain: Option<&'static str>,
}

/// A statement that passed the front's checks and holds a worker.
#[derive(Debug)]
struct Started {
    /// `None` for a statement the front answered itself (`SET`, `USE`).
    lease: Option<WorkerLease>,
    header: Option<Vec<Column>>,
    framer: Framer,
    running: RunningGuard,
    deadline: Option<Deadline>,
    /// Its admission (HS1 Task 21), held until the result ends.
    ticket: Option<crate::admission::Ticket>,
    started: Instant,
    /// What an `INSERT` into a lake table wrote, which the front ran to its
    /// commit before answering (HS1 Task 12).
    inserted: Option<crate::insert::Inserted>,
    /// Its query log row, metrics, operator line and trace (HS1 Task 23).
    observed: Option<Arc<crate::querylog::Statement>>,
}

pub(super) fn sorted<'a>(
    map: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = map
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    pairs.sort();
    pairs
}

/// One `Execute` of a statement: its id, parameters and settings.
fn execute(
    st: &Statement,
    settings: &[(String, String)],
    views: &[String],
    sql: String,
    format: &str,
    limits: Limits,
) -> Execute {
    Execute {
        query_id: st.query_id.clone(),
        session: None,
        settings: settings.to_vec(),
        views: views.to_vec(),
        sql,
        format: format.to_string(),
        params: st.params.clone(),
        limits,
        input: None,
        output: loams_house_ipc::Output::Blocks,
        database: Some(if st.database.is_empty() {
            "default".to_string()
        } else {
            st.database.clone()
        }),
    }
}

/// Runs a header step (`DESCRIBE`, or one row of names and types) on the
/// statement's worker and collects its output, at most [`EXPLAIN_MAX_BYTES`]:
/// `CancelQuery` and the statement's deadline reach it as they reach the statement.
async fn header_step(
    lease: &mut WorkerLease,
    execute: Execute,
    running: &RunningGuard,
    deadline_at: tokio::time::Instant,
) -> Result<Vec<u8>, HouseError> {
    let query_id = execute.query_id.clone();
    lease.start(execute).await?;
    let kill = lease.kill_handle();
    let _deadline = kill
        .as_ref()
        .map(|kill| kill.kill_at(deadline_at, ExitReason::Timeout));
    running.started(kill.clone());
    let mut out = Vec::new();
    loop {
        match lease.next_event().await? {
            Event::Chunk(chunk) => {
                if out.len() + chunk.bytes.len() > EXPLAIN_MAX_BYTES {
                    if let Some(kill) = &kill {
                        let _ = kill.kill(ExitReason::Cancel);
                    }
                    return Err(HouseError::from(ChError::bad_arguments(format!(
                        "query {query_id}: the result's header is larger than \
                         {EXPLAIN_MAX_BYTES} bytes"
                    ))));
                }
                out.extend_from_slice(&chunk.bytes);
            }
            Event::Progress(_) => {}
            Event::Done(_) => return Ok(out),
        }
    }
}

impl HouseApi {
    /// [`HouseApi::start_checked`], with the statement's audit record (HS1
    /// Task 20): its identity, kind and how its start ended.
    async fn start(&self, user: &UserMap, st: Statement) -> Result<Started, ConnectError> {
        let audit = self.config.audit.as_ref().map(|sink| {
            let mut event = crate::audit::AuditEvent::now("api", &st.query_id);
            event.user = user.user.clone();
            event.principal = user.principal.clone();
            event.namespace = Some(user.namespace);
            event.statement = Some(
                match classify(&request::plan(st.sql.as_bytes(), false).sql) {
                    Ok(Classified::Known { stmt, .. }) => stmt.kind().to_string(),
                    Ok(Classified::Unparsed { .. }) | Err(_) => "Unparsed".to_string(),
                },
            );
            (sink, event)
        });
        // Its query log row, metrics, operator line and trace (HS1 Task 23):
        // its checks and worker run under its root span.
        let observed = self.governance.observer.begin("api", &st.query_id, None);
        observed.identify(user);
        observed.describe(
            &match classify(&request::plan(st.sql.as_bytes(), false).sql) {
                Ok(Classified::Known { stmt, .. }) => stmt.kind().to_string(),
                Ok(Classified::Unparsed { .. }) | Err(_) => "Unparsed".to_string(),
            },
            super::database_or_default(&st.database),
            &st.sql,
        );
        let started = crate::trace::scope(observed.current(), self.start_checked(user, st)).await;
        let started = match started {
            Ok(mut started) => {
                if started.lease.is_some() {
                    observed.started_on_worker();
                }
                started.observed = Some(observed);
                Ok(started)
            }
            Err(err) => {
                observed.fail(connect_code(&err), err.message.as_deref().unwrap_or(""));
                Err(err)
            }
        };
        if let Some((sink, mut event)) = audit {
            event.code = match &started {
                Ok(_) => 0,
                Err(err) => connect_code(err),
            };
            sink.record(&event);
        }
        started
    }

    /// Checks a statement, takes a worker, analyses it, and starts it.
    async fn start_checked(&self, user: &UserMap, st: Statement) -> Result<Started, ConnectError> {
        let config = &self.config;
        if st.sql.trim().is_empty() {
            return Err(invalid("sql", "sql is empty"));
        }
        if st.sql.len() > config.max_query_size {
            return Err(to_connect(&HouseError::from(ChError::syntax_error(
                format!(
                    "Max query size exceeded: the statement is longer than {} bytes",
                    config.max_query_size
                ),
            ))));
        }
        let database = if st.database.is_empty() {
            "default"
        } else {
            st.database.as_str()
        };
        match &self.lake {
            Some(lake) => lake
                .require_database(user.namespace, database)
                .await
                .map_err(|err| to_connect(&err))?,
            None if database != "default" => {
                return Err(to_connect(&HouseError::from(ChError::unknown_database(
                    format!("Database {database} does not exist"),
                ))));
            }
            None => {}
        }
        let known = self.pool.known_settings();
        for (name, value) in &st.settings {
            crate::settings::check(name, value, &config.session_limits, &known)
                .map_err(|err| to_connect(&err))?;
        }
        let read_only = st.read_only || user.readonly;
        let refuse_write = || {
            if user.readonly {
                read_only_user_refusal(&user.user)
            } else {
                read_only_refusal()
            }
        };

        // What the statement is (the HTTP interface's `prepare`, without a body).
        let mut plan = request::plan(st.sql.as_bytes(), false);
        let mut classified = match &plan.input {
            Some(spec) => {
                check_text(&spec.insert).map_err(|err| to_connect(&err))?;
                Classified::Known {
                    stmt: Stmt::Insert(crate::classify::InsertStmt {
                        text: spec.insert.clone(),
                        format: Some(spec.format.clone()),
                        table: None,
                    }),
                    format: None,
                }
            }
            None => classify(&plan.sql).map_err(|err| to_connect(&err))?,
        };
        // A read-only request first (`house_read_only`), then the caller's
        // scopes (HS1 Task 20).
        if read_only && matches!(&classified, Classified::Known { stmt, .. } if !stmt.is_read()) {
            return Err(refuse_write());
        }
        gate::authorize(user, &classified).map_err(|err| to_connect(&err))?;
        let trailing_format = match &classified {
            Classified::Known { format, .. } | Classified::Unparsed { format, .. } => {
                format.is_some()
            }
        };
        if plan.input.is_none() && (trailing_format || plan.format.is_some()) {
            return Err(invalid(
                "sql",
                "the result format is the request's `format`: drop the FORMAT clause",
            ));
        }
        // `SHOW TABLES`, `SHOW DATABASES`, `SHOW PROCESSLIST`: ClickHouse's own
        // `SELECT` over the House's `system` tables (HS1 Task 11).
        if let Classified::Known { stmt, .. } = &classified
            && let Some(query) =
                gate::show_answer(self.lake.as_deref(), user.namespace, database, stmt)
                    .await
                    .map_err(|err| to_connect(&err))?
        {
            plan.sql = query.clone();
            classified = Classified::Known {
                stmt: Stmt::Query { text: query },
                format: None,
            };
        }
        // The catalog's statements (HS1 Task 10), as over HTTP: DDL and `USE`
        // answer here, `DESCRIBE`/`SHOW CREATE`/`EXISTS` of a catalog table read
        // their rows.
        if st.explain.is_none()
            && let Classified::Known { stmt, .. } = &classified
        {
            if read_only && !stmt.is_read() {
                return Err(refuse_write());
            }
            let outcome = gate::catalog_answer(
                self.lake.as_deref(),
                &self.pool,
                user,
                database,
                stmt,
                &plan.sql,
                None,
                &st.query_id,
            )
            .await
            .map_err(|err| to_connect(&err))?;
            match outcome {
                gate::CatalogOutcome::Done => {
                    let running = self.register(&st.query_id, user, &st.sql)?;
                    return Ok(Started {
                        lease: None,
                        header: Some(Vec::new()),
                        framer: Framer::Json(JsonFramer::new(st.caps, false)),
                        running,
                        deadline: None,
                        ticket: None,
                        started: Instant::now(),
                        inserted: None,
                        observed: None,
                    });
                }
                gate::CatalogOutcome::Rows(query) => {
                    plan.sql = query.clone();
                    classified = Classified::Known {
                        stmt: Stmt::Query { text: query },
                        format: None,
                    };
                }
                gate::CatalogOutcome::NotOurs => {}
                outcome @ (gate::CatalogOutcome::Insert(_)
                | gate::CatalogOutcome::CreateAs(_)
                | gate::CatalogOutcome::Optimize(_)
                | gate::CatalogOutcome::Mutate(_)) => {
                    // An `INSERT` into a lake table (HS1 Task 12): run to its
                    // commit, then answered with what it wrote.
                    let Some(lake) = self.lake.as_deref() else {
                        return Err(to_connect(&HouseError::from(ChError::not_implemented(
                            "this House has no catalog",
                        ))));
                    };
                    let running = self.register(&st.query_id, user, &st.sql)?;
                    // The front carries a lake write to its commit (HS1 Task 12).
                    running.uncancellable();
                    let ticket = self
                        .admit(user, &st.settings, crate::quota::Access::Write)
                        .await?;
                    let started = Instant::now();
                    let lease = self
                        .pool
                        .acquire(&user.namespace.to_string())
                        .await
                        .map_err(|err| to_connect(&err))?;
                    let known = self.pool.known_settings();
                    let ctx = crate::insert::InsertContext {
                        lake,
                        pool: &self.pool,
                        namespace: user.namespace,
                        database,
                        user,
                        query_id: &st.query_id,
                        params: &st.params,
                        settings: &st.settings,
                        limits: &config.session_limits,
                        known: &known,
                        max_insert_bytes: config.lake_insert_max_bytes,
                        pipe_batch: None,
                        admitted: Some(&ticket),
                    };
                    let inserted_into = match &outcome {
                        gate::CatalogOutcome::Insert(target) => {
                            Some((target.database.clone(), target.name.clone()))
                        }
                        _ => None,
                    };
                    let inserted = match outcome {
                        gate::CatalogOutcome::Insert(target) => {
                            let streams =
                                matches!(target.source, crate::insert::Source::Stream { .. });
                            let mut feed =
                                crate::insert::BytesFeed(std::mem::take(&mut plan.first_data));
                            let feed: Option<&mut dyn crate::insert::Feed> =
                                if streams { Some(&mut feed) } else { None };
                            crate::insert::run(&ctx, lease, *target, feed).await
                        }
                        gate::CatalogOutcome::CreateAs(create) => {
                            crate::insert::create_as(&ctx, lease, *create).await
                        }
                        gate::CatalogOutcome::Optimize(plan) => {
                            self.pool.release(lease, Outcome::Completed);
                            crate::compact::optimize(&ctx, &plan).await
                        }
                        gate::CatalogOutcome::Mutate(plan) => {
                            self.pool.release(lease, Outcome::Completed);
                            crate::mutate::run(&ctx, &plan).await
                        }
                        _ => unreachable!("matched above"),
                    };
                    // The source's materialized views run before the answer
                    // (HS1 Task 17).
                    let inserted = match (inserted, inserted_into, self.lake.as_ref()) {
                        (Ok(inserted), Some((db, name)), Some(lake))
                            if inserted.snapshot_id.is_some() && inserted.written_rows > 0 =>
                        {
                            crate::mv::after_insert(
                                lake,
                                &self.pool,
                                config.session_limits,
                                config.lake_insert_max_bytes,
                                user.namespace,
                                &db,
                                &name,
                                &st.settings,
                            )
                            .await
                            .map(|()| inserted)
                        }
                        (other, _, _) => other,
                    }
                    .map_err(|err| to_connect(&err))?;
                    return Ok(Started {
                        lease: None,
                        header: Some(Vec::new()),
                        framer: Framer::Json(JsonFramer::new(st.caps, false)),
                        running,
                        deadline: None,
                        ticket: Some(ticket),
                        started,
                        inserted: Some(inserted),
                        observed: None,
                    });
                }
            }
        }
        let (expect, unparsed, is_read, is_query) = match &classified {
            Classified::Known { stmt, .. } => {
                if st.explain.is_some() && !matches!(stmt, Stmt::Query { .. }) {
                    return Err(invalid(
                        "sql",
                        "ExplainQuery takes a query (SELECT, WITH or a parenthesised query)",
                    ));
                }
                if read_only && !stmt.is_read() {
                    return Err(refuse_write());
                }
                match gate::front_answer(stmt, None, &config.session_limits, &known) {
                    Some(Ok(())) => {
                        // `SET` and `USE` keep nothing without a session: an empty
                        // result, as in ClickHouse.
                        let running = self.register(&st.query_id, user, &st.sql)?;
                        return Ok(Started {
                            lease: None,
                            header: Some(Vec::new()),
                            framer: Framer::Json(JsonFramer::new(st.caps, false)),
                            running,
                            deadline: None,
                            ticket: None,
                            started: Instant::now(),
                            inserted: None,
                            observed: None,
                        });
                    }
                    Some(Err(err)) => return Err(to_connect(&err)),
                    None => {}
                }
                (
                    stmt.expect().unwrap_or(Expect::Read),
                    None,
                    stmt.is_read(),
                    matches!(stmt, Stmt::Query { .. }),
                )
            }
            Classified::Unparsed { message, .. } => {
                if st.explain.is_some() {
                    return Err(invalid(
                        "sql",
                        "ExplainQuery takes a query (SELECT, WITH or a parenthesised query)",
                    ));
                }
                (Expect::Read, Some(message.clone()), true, false)
            }
        };

        let sql = gate::statement_text(&plan.sql, &user.user);
        let input = plan.input.map(|spec| InputSpec {
            insert: deny::rewrite_host_functions(&spec.insert, &gate::host_values(&user.user)),
            format: spec.format,
        });
        let running = self.register(&st.query_id, user, &st.sql)?;
        let access = match &classified {
            Classified::Known { stmt, .. } => crate::quota::Access::of(stmt),
            Classified::Unparsed { .. } => crate::quota::Access::Read,
        };
        let ticket = self.admit(user, &st.settings, access).await?;
        let started = Instant::now();

        let mut lease = self
            .pool
            .acquire(&user.namespace.to_string())
            .await
            .map_err(|err| to_connect(&err))?;

        // ClickHouse's class and the deny list, on exactly the text chDB parses.
        let parsed_text = match &input {
            Some(spec) => format!("{} FORMAT {}", spec.insert, spec.format),
            None => sql.clone(),
        };
        let analysis = match lease.analyze(&parsed_text, &st.params).await {
            Ok(analysis) => analysis,
            Err(err) => {
                self.pool.release(lease, Outcome::Completed);
                return Err(to_connect(&deny::as_setting_refusal(err)));
            }
        };
        // HE1 R14.5: calls of the namespace's own views are allowed.
        let views =
            match gate::view_calls(self.lake.as_deref(), user.namespace, database, &analysis).await
            {
                Ok(views) => views,
                Err(err) => {
                    self.pool.release(lease, Outcome::Completed);
                    return Err(to_connect(&err));
                }
            };
        let admitted = gate::admit_views(
            &analysis,
            &parsed_text,
            expect,
            false,
            unparsed.as_deref(),
            &views,
        )
        .map_err(|err| to_connect(&err))
        .and_then(|()| {
            if read_only && analysis.classification.class != QueryClass::ReadOnly {
                Err(refuse_write())
            } else {
                Ok(())
            }
        });
        if let Err(err) = admitted {
            self.pool.release(lease, Outcome::Completed);
            return Err(err);
        }
        // What it reads (HS1 Task 11): lake tables pinned, `system` tables the
        // House's, chosen by the request's settings (Loams's own included).
        let reads = if input.is_some() {
            crate::read::ReadPlan::default()
        } else {
            match gate::read_plan(
                self.lake.as_deref(),
                user.namespace,
                database,
                user,
                &mut lease,
                &analysis,
                &sql,
                &st.settings,
                None,
                Some(crate::read::Front {
                    processes: &self.governance.processes,
                    query_log: self.governance.observer.log.as_deref(),
                }),
            )
            .await
            {
                Ok(reads) => reads,
                Err(err) => {
                    self.pool.release(lease, Outcome::Completed);
                    return Err(to_connect(&err));
                }
            }
        };
        let sql = if input.is_some() {
            sql
        } else {
            reads.sql.clone()
        };
        if running.cancelled() {
            self.pool.release(lease, Outcome::Completed);
            return Err(to_connect(&cancelled_error(&st.query_id)));
        }

        let limit_ms = if st.timeout_ms == 0 {
            MAX_TIMEOUT_MS
        } else {
            st.timeout_ms
        };
        let deadline_at =
            tokio::time::Instant::now() + Duration::from_millis(u64::from(limit_ms)) + KILL_GRACE;
        let mut settings: Vec<(String, String)> = st
            .settings
            .iter()
            .filter(|(k, _)| !crate::settings::is_front_only(k))
            .cloned()
            .collect();
        let limits = Limits {
            max_execution_time_ms: (st.timeout_ms > 0).then_some(u64::from(st.timeout_ms)),
            max_result_rows: (is_read && st.output != Output::Collect)
                .then_some(st.caps.max_rows.saturating_add(1)),
            ..Limits::default()
        };
        if limits.max_result_rows.is_some() {
            settings.push(("result_overflow_mode".to_string(), "break".to_string()));
        }

        // The header, when the output will not carry it (see the module's docs).
        let mut header = None;
        if st.output == Output::Arrow && is_read {
            let pre = if is_query {
                execute(
                    &st,
                    &settings,
                    &reads.views,
                    format!("DESCRIBE TABLE (\n{sql}\n) SETTINGS describe_compact_output = 1"),
                    "JSONCompactEachRow",
                    Limits {
                        max_execution_time_ms: limits.max_execution_time_ms,
                        ..Limits::default()
                    },
                )
            } else {
                execute(
                    &st,
                    &settings,
                    &reads.views,
                    sql.clone(),
                    "JSONCompactEachRowWithNamesAndTypes",
                    Limits {
                        max_result_rows: Some(1),
                        ..limits
                    },
                )
            };
            // Cancellable and under the statement's deadline, like the statement.
            let described = header_step(&mut lease, pre, &running, deadline_at).await;
            match described.and_then(|out| frames::header_from_json(&out, !is_query)) {
                Ok(columns) => header = Some(columns),
                Err(err) => {
                    self.pool.release(lease, Outcome::Completed);
                    return Err(to_connect(&err));
                }
            }
        } else if !is_read {
            header = Some(Vec::new());
        }

        let (text, format, framer) = match st.output {
            Output::Arrow => {
                settings.push((
                    "output_format_arrow_compression_method".to_string(),
                    "none".to_string(),
                ));
                (sql, "ArrowStream", Framer::Arrow(ArrowFramer::new(st.caps)))
            }
            Output::Json if is_read => (
                sql,
                "JSONCompactEachRowWithNamesAndTypes",
                Framer::Json(JsonFramer::new(st.caps, true)),
            ),
            Output::Json => (
                sql,
                "JSONCompactEachRow",
                Framer::Json(JsonFramer::new(st.caps, false)),
            ),
            Output::Collect => (
                match st.explain {
                    Some(kind) => format!("EXPLAIN {kind}\n{sql}"),
                    None => sql,
                },
                "TabSeparatedRaw",
                Framer::Json(JsonFramer::new(st.caps, false)),
            ),
        };
        let mut main = execute(
            &st,
            &settings,
            &reads.views,
            if input.is_some() { String::new() } else { text },
            format,
            limits,
        );
        if input.is_some() {
            // As over HTTP: a streamed `INSERT` gets no views, so no database.
            main.database = None;
        }
        main.input = input;
        let has_input = main.input.is_some();
        if let Err(err) = lease.start(main).await {
            self.pool.release(lease, Outcome::Completed);
            return Err(to_connect(&err));
        }
        let kill = lease.kill_handle();
        let deadline = kill
            .as_ref()
            .map(|kill| kill.kill_at(deadline_at, ExitReason::Timeout));
        running.started(kill);
        if has_input {
            let sent = async {
                if !plan.first_data.is_empty() {
                    lease
                        .send_input(bytes::Bytes::from(plan.first_data.clone()))
                        .await?;
                }
                lease.end_input().await
            }
            .await;
            if let Err(err) = sent {
                self.pool.kill(lease, ExitReason::Cancel);
                return Err(to_connect(&err));
            }
        }
        Ok(Started {
            lease: Some(lease),
            header,
            framer,
            running,
            deadline,
            ticket: Some(ticket),
            started,
            inserted: None,
            observed: None,
        })
    }

    /// A statement's frames, as a stream.
    pub(super) async fn frames(
        &self,
        user: &UserMap,
        st: Statement,
    ) -> Result<
        impl futures::Stream<Item = Result<Frame, ConnectError>> + Send + 'static,
        ConnectError,
    > {
        let query_id = st.query_id.clone();
        let format = st.output;
        let started = self.start(user, st).await?;
        let run = Run::new(self.pool.clone(), started, query_id, format);
        Ok(futures::stream::unfold(run, |mut run| async move {
            run.next().await.map(|frame| (frame, run))
        }))
    }

    /// A statement's whole output (`ExplainQuery`), at most [`EXPLAIN_MAX_BYTES`].
    pub(super) async fn collect(
        &self,
        user: &UserMap,
        st: Statement,
    ) -> Result<Vec<u8>, ConnectError> {
        let query_id = st.query_id.clone();
        let Started {
            lease,
            running,
            deadline,
            ticket,
            ..
        } = self.start(user, st).await?;
        let Some(mut lease) = lease else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let result = loop {
            match lease.next_event().await {
                Ok(Event::Chunk(chunk)) => {
                    if out.len() + chunk.bytes.len() > EXPLAIN_MAX_BYTES {
                        self.pool.kill(lease, ExitReason::Cancel);
                        return Err(to_connect(&HouseError::from(ChError::bad_arguments(
                            format!(
                                "query {query_id}: the answer is larger than {EXPLAIN_MAX_BYTES} bytes"
                            ),
                        ))));
                    }
                    out.extend_from_slice(&chunk.bytes);
                }
                Ok(Event::Progress(_)) => {}
                Ok(Event::Done(_)) => break Ok(out),
                Err(err) => break Err(to_connect(&err)),
            }
        };
        self.pool.release(lease, Outcome::Completed);
        drop((running, deadline, ticket));
        result
    }
}

/// One frame of a result, before it is an `ExecuteQueryResponse` or a
/// `PreviewTableResponse`.
#[derive(Debug, Clone)]
pub(super) enum Frame {
    Header(pb::ResultHeader),
    Batch(pb::ResultBatch),
    Progress(pb::QueryProgress),
    Summary(pb::QuerySummary),
}

impl From<Frame> for pb::ExecuteQueryResponse {
    fn from(frame: Frame) -> Self {
        use execute_query_response::Frame as F;
        let frame = match frame {
            Frame::Header(v) => F::Header(Box::new(v)),
            Frame::Batch(v) => F::Batch(Box::new(v)),
            Frame::Progress(v) => F::Progress(Box::new(v)),
            Frame::Summary(v) => F::Summary(Box::new(v)),
        };
        Self {
            frame: Some(frame),
            ..Default::default()
        }
    }
}

impl From<Frame> for pb::PreviewTableResponse {
    fn from(frame: Frame) -> Self {
        use preview_table_response::Frame as F;
        let frame = match frame {
            Frame::Header(v) => F::Header(Box::new(v)),
            Frame::Batch(v) => F::Batch(Box::new(v)),
            Frame::Progress(v) => F::Progress(Box::new(v)),
            Frame::Summary(v) => F::Summary(Box::new(v)),
        };
        Self {
            frame: Some(frame),
            ..Default::default()
        }
    }
}

/// A started statement, read into frames.
#[derive(Debug)]
struct Run {
    pool: WorkerPool,
    lease: Option<WorkerLease>,
    framer: Framer,
    header: Option<Vec<Column>>,
    header_sent: bool,
    queue: VecDeque<Frame>,
    query_id: String,
    format: pb::ResultFormat,
    started: Instant,
    last: Progress,
    drained: u64,
    /// The next chunk opens an engine block (the last one did not say `continued`).
    block_start: bool,
    failed: Option<ConnectError>,
    done: bool,
    /// An `INSERT` into a lake table the front ran (HS1 Task 12).
    inserted: Option<crate::insert::Inserted>,
    /// Its query log row (HS1 Task 23), ended when the run is.
    observed: Option<Arc<crate::querylog::Statement>>,
    _running: RunningGuard,
    _deadline: Option<Deadline>,
    _ticket: Option<crate::admission::Ticket>,
}

impl Drop for Run {
    fn drop(&mut self) {
        // A stream dropped before its end: the client went away (HS1 Task 23).
        if !self.done
            && let Some(observed) = &self.observed
        {
            observed.fail(
                crate::querylog::CANCELLED,
                "the client went away before the result ended",
            );
        }
    }
}

impl Run {
    fn new(pool: WorkerPool, started: Started, query_id: String, output: Output) -> Self {
        let Started {
            lease,
            header,
            framer,
            running,
            deadline,
            ticket,
            started,
            inserted,
            observed,
        } = started;
        let front_only = lease.is_none();
        let mut run = Self {
            pool,
            lease,
            framer,
            header,
            header_sent: false,
            queue: VecDeque::new(),
            query_id,
            format: match output {
                Output::Arrow => pb::ResultFormat::RESULT_FORMAT_ARROW_IPC,
                _ => pb::ResultFormat::RESULT_FORMAT_JSON_ROWS,
            },
            started,
            last: Progress::default(),
            drained: 0,
            block_start: true,
            failed: None,
            done: false,
            inserted,
            observed,
            _running: running,
            _deadline: deadline,
            _ticket: ticket,
        };
        run.send_header_if_known();
        if front_only {
            run.finish(Vec::new());
        }
        run
    }

    async fn next(&mut self) -> Option<Result<Frame, ConnectError>> {
        loop {
            if let Some(frame) = self.queue.pop_front() {
                return Some(Ok(frame));
            }
            if let Some(err) = self.failed.take() {
                self.done = true;
                return Some(Err(err));
            }
            if self.done {
                return None;
            }
            self.step().await;
        }
    }

    fn send_header_if_known(&mut self) {
        if self.header_sent {
            return;
        }
        if let Some(columns) = self.framer.take_header() {
            self.header = Some(columns);
        }
        let Some(columns) = self.header.take() else {
            return;
        };
        self.header_sent = true;
        self.queue.push_back(Frame::Header(pb::ResultHeader {
            columns: columns
                .into_iter()
                .map(|c| pb::ResultColumn {
                    name: c.name,
                    clickhouse_type: c.clickhouse_type,
                    ..Default::default()
                })
                .collect(),
            query_id: self.query_id.clone(),
            format: self.format.into(),
            ..Default::default()
        }));
    }

    fn batches(&mut self, batches: Vec<frames::Batch>) {
        let arrow = matches!(self.framer, Framer::Arrow(_));
        for batch in batches {
            let data = if arrow {
                result_batch::Data::ArrowIpc(batch.bytes)
            } else {
                result_batch::Data::RowsJson(batch.bytes)
            };
            self.queue.push_back(Frame::Batch(pb::ResultBatch {
                data: Some(data),
                rows: batch.rows,
                ..Default::default()
            }));
        }
    }

    /// The statement ended well (or was cut): the rest, then the summary.
    fn finish(&mut self, rest: Vec<frames::Batch>) {
        if !self.header_sent {
            self.header.get_or_insert_with(Vec::new);
            self.send_header_if_known();
        }
        self.batches(rest);
        if let Some(observed) = &self.observed {
            observed.progress(&self.last);
            let (rows, bytes) = (self.framer.rows(), self.framer.bytes());
            let written = self
                .inserted
                .as_ref()
                .map(|i| (i.written_rows, i.written_bytes));
            observed.update(|e| {
                e.result_rows = rows;
                e.result_bytes = bytes;
                if let Some((rows, bytes)) = written {
                    e.written_rows = rows;
                    e.written_bytes = bytes;
                }
            });
        }
        self.queue.push_back(Frame::Summary(pb::QuerySummary {
            result_rows: self.framer.rows(),
            result_bytes: self.framer.bytes(),
            read_rows: self.last.rows_read,
            read_bytes: self.last.bytes_read,
            written_rows: self
                .inserted
                .as_ref()
                .map_or(self.last.written_rows, |i| i.written_rows),
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            truncated: self.framer.truncated(),
            query_id: self.query_id.clone(),
            snapshot_ids: self
                .inserted
                .as_ref()
                .and_then(|i| i.snapshot_id.map(|id| (i.table.clone(), id)))
                .into_iter()
                .collect(),
            ..Default::default()
        }));
        self.done = true;
    }

    fn fail(&mut self, err: &HouseError) {
        if let Some(observed) = &self.observed {
            observed.progress(&self.last);
            observed.fail(err.code(), err.message());
        }
        self.failed = Some(to_connect(err));
    }

    async fn step(&mut self) {
        let Some(lease) = self.lease.as_mut() else {
            self.done = true;
            return;
        };
        match lease.next_event().await {
            Ok(Event::Chunk(chunk)) => {
                if self.framer.truncated() {
                    // Past the cap: drained, and stopped past the budget.
                    self.drained += chunk.bytes.len() as u64;
                    if self.drained > DRAIN_BUDGET
                        && let Some(lease) = self.lease.take()
                    {
                        self.pool.kill(lease, ExitReason::Cancel);
                        let rest = self.framer.finish().unwrap_or_default();
                        self.finish(rest);
                    }
                    return;
                }
                let block_start = std::mem::replace(&mut self.block_start, !chunk.continued);
                match self.framer.push(&chunk.bytes, block_start) {
                    Ok(batches) => {
                        self.send_header_if_known();
                        if !self.header_sent && !self.framer.header_pending() {
                            self.header.get_or_insert_with(Vec::new);
                            self.send_header_if_known();
                        }
                        self.batches(batches);
                    }
                    Err(err) => {
                        if let Some(lease) = self.lease.take() {
                            self.pool.kill(lease, ExitReason::Cancel);
                        }
                        self.fail(&err);
                    }
                }
            }
            Ok(Event::Progress(progress)) => {
                self.last = progress;
                if self.header_sent {
                    self.queue.push_back(Frame::Progress(pb::QueryProgress {
                        read_rows: progress.rows_read,
                        read_bytes: progress.bytes_read,
                        total_rows_to_read: 0,
                        elapsed_ms: progress.elapsed_ns / 1_000_000,
                        ..Default::default()
                    }));
                }
            }
            Ok(Event::Done(stats)) => {
                self.last = stats;
                if let Some(lease) = self.lease.take() {
                    if let Some(observed) = &self.observed {
                        observed.cpu(lease.measured().cpu);
                    }
                    self.pool.release(lease, Outcome::Completed);
                }
                match self.framer.finish() {
                    Ok(rest) => self.finish(rest),
                    Err(err) => self.fail(&err),
                }
            }
            Err(err) => {
                if let Some(lease) = self.lease.take() {
                    self.pool.release(lease, Outcome::Completed);
                }
                if self.framer.truncated() {
                    // The client has every row it asked for; what failed after the
                    // cut (a drained block, a `throw` overflow mode) is not its.
                    let rest = self.framer.finish().unwrap_or_default();
                    self.finish(rest);
                } else {
                    self.fail(&err);
                }
            }
        }
    }
}
