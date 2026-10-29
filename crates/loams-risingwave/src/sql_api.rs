//! The RisingWave SQL API over pgwire (plan RW1, lane RW-b, assumption A10).
//!
//! RisingWave's frontend speaks the Postgres wire protocol (port 4566,
//! database `dev`). This module submits statements and reads the
//! `rw_catalog` system tables to list materialized views, sources, sinks and
//! tables and to follow a streaming job's backfill (`rw_ddl_progress`).
//!
//! Every query here uses the simple query protocol and reads text, so the
//! column types of `rw_catalog` (which have changed between RisingWave
//! releases) never decide whether a read works: values are cast to
//! `varchar` in SQL and parsed here.
//!
//! Transport ([`crate::tls`]): a remote host always gets TLS with a
//! verified certificate (design 81 R8: loams-gateway terminates TLS); plain
//! TCP is allowed only to loopback, which is how the compose
//! `engine-gateway` is reached. Status: preview, not yet run end to end.

use std::fmt;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::tls::{self, TlsError, TlsOptions, Transport};

use crate::connectors::pipeline::Rendered;
use crate::connectors::{Ddl, DdlError, quote_ident, quote_literal};

/// The frontend through the compose `engine-gateway` (design 81 R7: the only
/// host port, on loopback). The gateway checks the password as a Loams API
/// key or token and logs in upstream as the instance's service user (R8).
pub const DEFAULT_URL: &str = "postgres://loams@127.0.0.1:4566/dev";

/// What went wrong talking to RisingWave.
#[derive(Debug, thiserror::Error)]
pub enum SqlApiError {
    #[error("connecting to RisingWave: {0}")]
    Connect(#[source] tokio_postgres::Error),
    #[error("connecting to RisingWave: {0}")]
    Tls(#[from] TlsError),
    #[error("{context}: {message}")]
    Query {
        context: String,
        message: String,
        code: Option<String>,
    },
    #[error(transparent)]
    Ddl(#[from] DdlError),
    #[error("statement {index} failed ({display}): {source}")]
    Apply {
        index: usize,
        display: String,
        source: Box<SqlApiError>,
    },
    #[error("job {id} did not finish within {waited:?}")]
    Timeout { id: i64, waited: Duration },
    #[error("unexpected catalog row: {0}")]
    Catalog(String),
}

impl SqlApiError {
    fn query(context: impl Into<String>, err: tokio_postgres::Error) -> Self {
        let (message, code) = match err.as_db_error() {
            Some(db) => {
                let mut message = db.message().to_owned();
                if let Some(detail) = db.detail() {
                    message.push_str(": ");
                    message.push_str(detail);
                }
                (message, Some(db.code().code().to_owned()))
            }
            None => (err.to_string(), None),
        };
        Self::Query {
            context: context.into(),
            message,
            code,
        }
    }
}

/// The result of one statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Outcome {
    /// A statement that returned rows (`SELECT`, `SHOW`, `DESCRIBE`).
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
    },
    /// A statement that returned none (DDL, DML): RisingWave's row count.
    Command { rows_affected: u64 },
}

/// Groups the simple query protocol's messages per statement.
fn outcomes(messages: Vec<SimpleQueryMessage>) -> Vec<Outcome> {
    let mut out = Vec::new();
    let mut columns: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(described) => {
                columns = Some(described.iter().map(|c| c.name().to_owned()).collect());
            }
            SimpleQueryMessage::Row(row) => {
                if columns.is_none() {
                    columns = Some(row.columns().iter().map(|c| c.name().to_owned()).collect());
                }
                rows.push(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_owned))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(count) => match columns.take() {
                Some(columns) => out.push(Outcome::Rows {
                    columns,
                    rows: std::mem::take(&mut rows),
                }),
                None => out.push(Outcome::Command {
                    rows_affected: count,
                }),
            },
            _ => {}
        }
    }
    out
}

/// Which catalog to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    MaterializedView,
    Source,
    Sink,
    Table,
}

impl RelationKind {
    /// The `rw_catalog` table and the extra columns read from it.
    fn catalog(self) -> (&'static str, &'static str) {
        match self {
            Self::MaterializedView => ("rw_materialized_views", "NULL::varchar"),
            Self::Source => ("rw_sources", "r.connector::varchar"),
            Self::Sink => ("rw_sinks", "r.connector::varchar"),
            Self::Table => ("rw_tables", "NULL::varchar"),
        }
    }

    /// The `SELECT` that lists this kind, user schemas only.
    pub fn list_sql(self) -> String {
        let (table, connector) = self.catalog();
        format!(
            "SELECT r.id::varchar, s.name::varchar, r.name::varchar, {connector}, r.definition::varchar \
             FROM rw_catalog.{table} r JOIN rw_catalog.rw_schemas s ON s.id = r.schema_id \
             WHERE s.name NOT IN ('rw_catalog', 'pg_catalog', 'information_schema') \
             ORDER BY s.name, r.name"
        )
    }
}

/// One materialized view, source, sink or table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relation {
    pub kind: RelationKind,
    pub id: i64,
    pub schema: String,
    pub name: String,
    /// The connector of a source or sink (`kafka`, `iceberg`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    /// The statement that created it, as RisingWave stores it.
    pub definition: String,
}

/// A streaming job that is still being created (backfilling).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobProgress {
    pub id: i64,
    pub statement: String,
    /// RisingWave's progress text, such as `42.15%` or, for a job reading
    /// an unbounded source, the rows consumed so far.
    pub progress: String,
    /// The progress as a fraction, when the text is a percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialized_at: Option<String>,
}

/// `rw_ddl_progress`, one row per job being created.
pub const JOBS_SQL: &str = "SELECT ddl_id::varchar, ddl_statement::varchar, progress::varchar, \
     initialized_at::varchar FROM rw_catalog.rw_ddl_progress ORDER BY ddl_id";

/// Parses `42.15%` into 0.4215.
pub fn parse_percent(text: &str) -> Option<f64> {
    let number = text.trim().strip_suffix('%')?.trim();
    let value: f64 = number.parse().ok()?;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value / 100.0)
}

fn parse_id(text: Option<String>, what: &str) -> Result<i64, SqlApiError> {
    let text = text.ok_or_else(|| SqlApiError::Catalog(format!("{what} is NULL")))?;
    text.trim()
        .parse()
        .map_err(|_| SqlApiError::Catalog(format!("{what} {text:?} is not an integer")))
}

fn relations(kind: RelationKind, outcome: Outcome) -> Result<Vec<Relation>, SqlApiError> {
    let Outcome::Rows { rows, .. } = outcome else {
        return Err(SqlApiError::Catalog(
            "the listing returned no rows".to_owned(),
        ));
    };
    rows.into_iter()
        .map(|row| {
            let mut cells = row.into_iter();
            let mut next = || cells.next().flatten();
            Ok(Relation {
                kind,
                id: parse_id(next(), "id")?,
                schema: next().unwrap_or_default(),
                name: next().unwrap_or_default(),
                connector: next(),
                definition: next().unwrap_or_default(),
            })
        })
        .collect()
}

fn jobs(outcome: Outcome) -> Result<Vec<JobProgress>, SqlApiError> {
    let Outcome::Rows { rows, .. } = outcome else {
        return Err(SqlApiError::Catalog(
            "the progress query returned no rows".to_owned(),
        ));
    };
    rows.into_iter()
        .map(|row| {
            let mut cells = row.into_iter();
            let mut next = || cells.next().flatten();
            let id = parse_id(next(), "ddl_id")?;
            let statement = next().unwrap_or_default();
            let progress = next().unwrap_or_default();
            Ok(JobProgress {
                id,
                fraction: parse_percent(&progress),
                statement,
                progress,
                initialized_at: next(),
            })
        })
        .collect()
}

/// A connection to a RisingWave frontend.
pub struct SqlApi {
    client: Client,
    connection: tokio::task::JoinHandle<()>,
}

impl fmt::Debug for SqlApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqlApi")
            .field("closed", &self.client.is_closed())
            .finish_non_exhaustive()
    }
}

impl Drop for SqlApi {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

impl SqlApi {
    /// Connects to `url` (`postgres://user[:password]@host:port/db`). Needs
    /// a Tokio runtime.
    pub async fn connect(url: &str) -> Result<Self, SqlApiError> {
        let config: tokio_postgres::Config = url.parse().map_err(SqlApiError::Connect)?;
        Self::connect_config(config, &TlsOptions::default()).await
    }

    /// Connects with a parsed configuration (the CLI adds the password from
    /// the environment this way, so it never sits in a URL). TLS follows
    /// [`tls::choose`]: required and verified for any non-loopback host.
    pub async fn connect_config(
        mut config: tokio_postgres::Config,
        options: &TlsOptions,
    ) -> Result<Self, SqlApiError> {
        let (client, connection) = match tls::choose(&mut config, options)? {
            Transport::Plain => {
                let (client, connection) =
                    config.connect(NoTls).await.map_err(SqlApiError::Connect)?;
                let connection = tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        tracing::warn!(%err, "RisingWave connection closed");
                    }
                });
                (client, connection)
            }
            Transport::Tls(make) => {
                let (client, connection) =
                    config.connect(make).await.map_err(SqlApiError::Connect)?;
                let connection = tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        tracing::warn!(%err, "RisingWave connection closed");
                    }
                });
                (client, connection)
            }
        };
        Ok(Self { client, connection })
    }

    /// Runs one or more `;`-separated statements and returns one outcome per
    /// statement.
    pub async fn submit(&self, sql: &str) -> Result<Vec<Outcome>, SqlApiError> {
        let messages = self
            .client
            .simple_query(sql)
            .await
            .map_err(|err| SqlApiError::query("running the statement", err))?;
        Ok(outcomes(messages))
    }

    /// Renders and runs one builder's statement.
    pub async fn execute(&self, ddl: &impl Ddl) -> Result<Vec<Outcome>, SqlApiError> {
        let sql = ddl.to_sql()?;
        self.submit(&sql).await
    }

    /// Runs a rendered pipeline in order, stopping at the first failure.
    /// Errors show the statement's redacted text, never a secret's value.
    pub async fn apply(&self, statements: &[Rendered]) -> Result<Vec<Outcome>, SqlApiError> {
        let mut out = Vec::with_capacity(statements.len());
        for (index, statement) in statements.iter().enumerate() {
            match self.submit(&statement.sql).await {
                Ok(outcomes) => out.extend(outcomes),
                Err(source) => {
                    return Err(SqlApiError::Apply {
                        index,
                        display: first_line(&statement.display),
                        source: Box::new(source),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn single(&self, sql: &str, context: &str) -> Result<Outcome, SqlApiError> {
        let messages = self
            .client
            .simple_query(sql)
            .await
            .map_err(|err| SqlApiError::query(context, err))?;
        outcomes(messages)
            .into_iter()
            .next()
            .ok_or_else(|| SqlApiError::Catalog(format!("{context}: no result")))
    }

    /// Lists one kind of relation in the user schemas.
    pub async fn list(&self, kind: RelationKind) -> Result<Vec<Relation>, SqlApiError> {
        let outcome = self.single(&kind.list_sql(), "listing the catalog").await?;
        relations(kind, outcome)
    }

    pub async fn materialized_views(&self) -> Result<Vec<Relation>, SqlApiError> {
        self.list(RelationKind::MaterializedView).await
    }

    pub async fn sources(&self) -> Result<Vec<Relation>, SqlApiError> {
        self.list(RelationKind::Source).await
    }

    pub async fn sinks(&self) -> Result<Vec<Relation>, SqlApiError> {
        self.list(RelationKind::Sink).await
    }

    pub async fn tables(&self) -> Result<Vec<Relation>, SqlApiError> {
        self.list(RelationKind::Table).await
    }

    /// The jobs being created now, with their backfill progress.
    pub async fn jobs(&self) -> Result<Vec<JobProgress>, SqlApiError> {
        let outcome = self.single(JOBS_SQL, "reading rw_ddl_progress").await?;
        jobs(outcome)
    }

    /// One job's progress, or `None` once it has finished (or never existed).
    pub async fn job(&self, id: i64) -> Result<Option<JobProgress>, SqlApiError> {
        Ok(self.jobs().await?.into_iter().find(|job| job.id == id))
    }

    /// The job creating `name` (a materialized view, sink, table or index),
    /// found by the name in its statement.
    pub async fn job_for(&self, name: &str) -> Result<Option<JobProgress>, SqlApiError> {
        let quoted = quote_ident(name)?;
        Ok(self
            .jobs()
            .await?
            .into_iter()
            .find(|job| job.statement.contains(&quoted) || statement_names(&job.statement, name)))
    }

    /// `CANCEL JOBS <id>`: stops a job that is being created.
    pub async fn cancel_job(&self, id: i64) -> Result<(), SqlApiError> {
        self.submit(&format!("CANCEL JOBS {id}")).await?;
        Ok(())
    }

    /// Polls until job `id` leaves `rw_ddl_progress` or `timeout` passes.
    pub async fn wait_for_job(
        &self,
        id: i64,
        poll: Duration,
        timeout: Duration,
    ) -> Result<(), SqlApiError> {
        let start = Instant::now();
        loop {
            if self.job(id).await?.is_none() {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(SqlApiError::Timeout {
                    id,
                    waited: start.elapsed(),
                });
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// `SHOW CREATE <kind> name`: the stored definition.
    pub async fn show_create(
        &self,
        kind: RelationKind,
        name: &str,
    ) -> Result<Option<String>, SqlApiError> {
        let keyword = match kind {
            RelationKind::MaterializedView => "MATERIALIZED VIEW",
            RelationKind::Source => "SOURCE",
            RelationKind::Sink => "SINK",
            RelationKind::Table => "TABLE",
        };
        let outcome = self
            .single(
                &format!("SHOW CREATE {keyword} {}", quote_ident(name)?),
                "SHOW CREATE",
            )
            .await?;
        Ok(match outcome {
            Outcome::Rows { rows, .. } => rows
                .into_iter()
                .next()
                .and_then(|row| row.into_iter().nth(1).flatten()),
            Outcome::Command { .. } => None,
        })
    }

    /// Whether a secret of this name exists (`rw_catalog.rw_secrets`).
    pub async fn secret_exists(&self, name: &str) -> Result<bool, SqlApiError> {
        let sql = format!(
            "SELECT count(*)::varchar FROM rw_catalog.rw_secrets WHERE name = {}",
            quote_literal(name)?
        );
        let outcome = self.single(&sql, "reading rw_secrets").await?;
        match outcome {
            Outcome::Rows { rows, .. } => {
                let count = rows
                    .into_iter()
                    .next()
                    .and_then(|row| row.into_iter().next().flatten());
                Ok(count.as_deref().map(str::trim) != Some("0"))
            }
            Outcome::Command { .. } => Err(SqlApiError::Catalog(
                "rw_secrets returned no rows".to_owned(),
            )),
        }
    }
}

/// Whether `statement` creates `name` unquoted (`CREATE MATERIALIZED VIEW
/// name AS …`), matched case-insensitively as RisingWave folds it.
fn statement_names(statement: &str, name: &str) -> bool {
    let lower = statement.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    lower
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .any(|word| word == name || word.rsplit('.').next() == Some(name.as_str()))
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("42.5%"), Some(0.425));
        assert_eq!(parse_percent(" 100% "), Some(1.0));
        assert_eq!(parse_percent("0.00%"), Some(0.0));
        assert_eq!(parse_percent("12345 rows consumed"), None);
        assert_eq!(parse_percent("120%"), None);
        assert_eq!(parse_percent("NaN%"), None);
    }

    #[test]
    fn list_sql_reads_user_schemas_only() {
        let sql = RelationKind::Sink.list_sql();
        assert!(
            sql.contains("FROM rw_catalog.rw_sinks r JOIN rw_catalog.rw_schemas s"),
            "{sql}"
        );
        assert!(sql.contains("r.connector::varchar"), "{sql}");
        assert!(
            sql.contains("NOT IN ('rw_catalog', 'pg_catalog', 'information_schema')"),
            "{sql}"
        );
        assert!(
            RelationKind::MaterializedView
                .list_sql()
                .contains("NULL::varchar")
        );
    }

    fn rows(rows: &[&[Option<&str>]]) -> Outcome {
        Outcome::Rows {
            columns: vec![],
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| c.map(str::to_owned)).collect())
                .collect(),
        }
    }

    #[test]
    fn relations_parse_from_text_rows() {
        let outcome = rows(&[&[
            Some("1001"),
            Some("public"),
            Some("clicks"),
            Some("kafka"),
            Some("CREATE SOURCE clicks (...)"),
        ]]);
        let parsed = relations(RelationKind::Source, outcome).unwrap();
        assert_eq!(
            parsed,
            vec![Relation {
                kind: RelationKind::Source,
                id: 1001,
                schema: "public".to_owned(),
                name: "clicks".to_owned(),
                connector: Some("kafka".to_owned()),
                definition: "CREATE SOURCE clicks (...)".to_owned(),
            }]
        );
        assert!(relations(RelationKind::Source, rows(&[&[Some("x")]])).is_err());
        assert!(relations(RelationKind::Source, Outcome::Command { rows_affected: 0 }).is_err());
    }

    #[test]
    fn jobs_parse_with_fractions() {
        let outcome = rows(&[
            &[
                Some("7"),
                Some("CREATE MATERIALIZED VIEW m AS SELECT 1"),
                Some("12.50%"),
                Some("2026-10-27 10:00:00+00:00"),
            ],
            &[
                Some("8"),
                Some("CREATE SINK s FROM m"),
                Some("3000 rows consumed"),
                None,
            ],
        ]);
        let parsed = jobs(outcome).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, 7);
        assert_eq!(parsed[0].fraction, Some(0.125));
        assert_eq!(parsed[1].fraction, None);
        assert_eq!(parsed[1].initialized_at, None);
    }

    #[test]
    fn job_statements_match_names() {
        assert!(statement_names(
            "CREATE MATERIALIZED VIEW clicks_per_minute AS SELECT",
            "clicks_per_minute"
        ));
        assert!(statement_names(
            "create sink web.Clicks_Lake from m",
            "clicks_lake"
        ));
        assert!(!statement_names(
            "CREATE MATERIALIZED VIEW clicks2 AS SELECT",
            "clicks"
        ));
    }

    #[test]
    fn outcomes_split_per_statement() {
        assert_eq!(
            outcomes(vec![
                SimpleQueryMessage::CommandComplete(0),
                SimpleQueryMessage::CommandComplete(3)
            ]),
            vec![
                Outcome::Command { rows_affected: 0 },
                Outcome::Command { rows_affected: 3 }
            ]
        );
    }
}
