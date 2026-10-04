//! Sessions: one chDB connection each, which is where a House `session_id`'s
//! isolation comes from.
//!
//! A `Session` owns a connection of its own, and Task 1 measured that the
//! isolation is real: a setting changed on one connection is not visible on
//! another, and a temporary table created on one does not exist on the other. That
//! matters because the House's `session_id` has to keep settings and the current
//! database (Task 4), and chDB has no session concept of its own.
//!
//! Two measured facts shape the rest of it:
//!
//! * **`chdb_connect` refuses a second connection whose arguments differ from the
//!   first one's** — the second call returns null. A session therefore takes the
//!   engine's arguments unchanged, and settings cannot be connection arguments even
//!   though the header says query-level settings are applied at connect. They are
//!   applied per statement instead, as a `SET` before each one.
//! * **A cancellation spends the connection it happened on.** After
//!   `chdb_stream_cancel_query`, the next streaming statement on that connection
//!   answers `"No active streaming query"`, with no code and no way to recover the
//!   connection (measured: the connection was reusable for `SELECT`, but not for a
//!   stream). So a cancelled session opens a fresh connection for its next
//!   statement, with the same arguments, which is the one connection shape chDB
//!   allows.

use std::sync::{Arc, Mutex, MutexGuard};

use loams_chdb_sys::ffi;

use crate::arrow::{ArrowHandle, ArrowStream, RecordBatchReader};
use crate::engine::{EngineConfig, SessionId, Settings, engine_error};
use crate::error::ChdbError;
use crate::query::{self, QueryStream, Registry};

/// A session: one chDB connection, its own settings, its own statements.
#[derive(Debug)]
pub struct Session {
    id: SessionId,
    settings: Settings,
    /// The arguments a connection of this session must be opened with, which are
    /// the engine's own: a second `chdb_connect` with any other argument returns
    /// null, Task 1 measured.
    config: EngineConfig,
    /// The session's connection, or `None` when a cancellation has spent it.
    connection: Mutex<Option<Arc<ffi::Connection>>>,
    registry: Arc<Registry>,
}

impl Session {
    /// Opens a session on a connection of its own.
    pub(crate) fn open(
        id: SessionId,
        config: &EngineConfig,
        settings: &Settings,
    ) -> Result<Self, ChdbError> {
        let connection = Arc::new(ffi::Connection::open(&config.to_args()).map_err(engine_error)?);
        Ok(Self {
            id,
            settings: settings.clone(),
            config: config.clone(),
            connection: Mutex::new(Some(connection)),
            registry: Arc::new(Registry::default()),
        })
    }

    /// The session id, which is the House's `session_id`.
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// The settings this session applies before each statement.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// A new query id: a UUID v4, as Task 2's `X-ClickHouse-Query-Id` carries.
    pub fn new_query_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// The session's connection, opening a new one if a cancellation spent it.
    fn connection(&self) -> Result<Arc<ffi::Connection>, ChdbError> {
        let mut guard: MutexGuard<'_, Option<Arc<ffi::Connection>>> = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(connection) = guard.as_ref() {
            return Ok(Arc::clone(connection));
        }
        let connection =
            Arc::new(ffi::Connection::open(&self.config.to_args()).map_err(engine_error)?);
        *guard = Some(Arc::clone(&connection));
        Ok(connection)
    }

    /// Runs a query and streams its result.
    ///
    /// `format` is the output format name the statement is asked for —
    /// `TSV`, `CSV`, `JSONEachRow`, `RowBinary`, `Native`, `Parquet`,
    /// `ArrowStream` — and `params` are the statement's parameters, which go
    /// through the ABI's parameter form so a value with an interior NUL is safe.
    ///
    /// The session's settings are applied first, one `SET` each, because they
    /// cannot be connection arguments (see the module docs). A statement that
    /// cannot be started answers with the error the engine gave, parsed: an unknown
    /// table is `60 UNKNOWN_TABLE`, not a null handle.
    pub fn execute(
        &self,
        sql: &str,
        format: &str,
        params: &[(String, String)],
    ) -> Result<QueryStream, ChdbError> {
        self.execute_with_id(&Self::new_query_id(), sql, format, params)
    }

    /// [`Session::execute`] with the caller's query id.
    ///
    /// The House's HTTP interface has the id in `X-ClickHouse-Query-Id` before it
    /// has a stream to attach it to, so `KILL QUERY` can name a statement that is
    /// still being set up; [`Session::cancel`] takes the same id.
    ///
    /// chDB itself ignores the id — `SET query_id` answers 115 `UNKNOWN_SETTING`
    /// at v26.9.0 — so this is Loams' correlation between a statement and the name
    /// the client gave it.
    pub fn execute_with_id(
        &self,
        query_id: &str,
        sql: &str,
        format: &str,
        params: &[(String, String)],
    ) -> Result<QueryStream, ChdbError> {
        let connection = self.connection()?;
        Self::apply_settings_of(&connection, &self.settings)?;
        let stream = connection
            .stream(sql, format, params)
            .map_err(engine_error)?;
        Ok(QueryStream::new(
            query_id.to_string(),
            Arc::new(stream),
            Arc::clone(&self.registry),
        ))
    }

    /// Runs a statement that produces no rows: `SET`, `USE`, and the `SET`s this
    /// session applies to itself.
    pub fn execute_simple(&self, sql: &str) -> Result<(), ChdbError> {
        self.connection()?.execute(sql).map_err(engine_error)
    }

    /// Runs a query and streams the result as Arrow record batches.
    ///
    /// This is the `ArrowStream` output format's C ABI path: chDB hands back a
    /// caller-allocated cell whose `internal_data` is its own `ArrowArrayStream`,
    /// which is read in place — Task 1 measured that driving it through a copy, which
    /// is what arrow-rs's own importer does, segfaults.
    pub fn execute_arrow(&self, sql: &str) -> Result<ArrowStream, ChdbError> {
        self.execute_arrow_with_id(&Self::new_query_id(), sql)
    }

    /// [`Session::execute_arrow`] with the caller's query id.
    pub fn execute_arrow_with_id(
        &self,
        query_id: &str,
        sql: &str,
    ) -> Result<ArrowStream, ChdbError> {
        let connection = self.connection()?;
        Self::apply_settings_of(&connection, &self.settings)?;
        ArrowStream::start(query_id.to_string(), &connection, sql)
    }

    /// Exports an Arrow reader to the engine as a table of the given name.
    ///
    /// The reader is exported through the Arrow C stream interface: the
    /// `RecordBatchReader` becomes an `ArrowArrayStream`, which goes into the cell
    /// `chdb_arrow_scan` reads. The handle owns both, so the reader stays alive for
    /// as long as the engine could pull from it — a reader dropped early would leave
    /// the engine reading freed memory — and the table is unregistered when the
    /// handle drops.
    ///
    /// The table is named the way chDB names it: `ArrowStream('<name>')` in a
    /// statement.
    pub fn register_arrow(
        &self,
        name: &str,
        reader: Box<dyn RecordBatchReader + Send>,
    ) -> Result<ArrowHandle, ChdbError> {
        let connection = self.connection()?;
        Self::apply_settings_of(&connection, &self.settings)?;
        crate::arrow::scan(connection, name, reader)
    }

    /// Cancels a statement of this session by query id.
    ///
    /// Returns as soon as the cancellation is registered, which is what Task 2's
    /// `KILL QUERY` needs: the next block of that statement answers `394
    /// CANCELLED` at once, while the engine is asked to stop in the background.
    /// See [`crate::query`] for what Task 1 measured about the ABI's own cancel, and
    /// [`Session`] for what the cancellation costs the connection it happened on.
    pub fn cancel(&self, query_id: &str) -> Result<(), ChdbError> {
        query::cancel(&self.registry, query_id)?;
        self.spend_connection();
        Ok(())
    }

    /// Forgets the session's connection, so the next statement opens a new one.
    fn spend_connection(&self) {
        let mut guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }

    /// [`Session::execute`] on a blocking thread.
    pub async fn execute_async(
        &self,
        sql: String,
        format: String,
        params: Vec<(String, String)>,
    ) -> Result<QueryStream, ChdbError> {
        let connection = self.connection()?;
        let registry = Arc::clone(&self.registry);
        let settings = self.settings.clone();
        tokio::task::spawn_blocking(move || {
            Session::apply_settings_of(&connection, &settings)?;
            let stream = connection
                .stream(&sql, &format, &params)
                .map_err(engine_error)?;
            Ok(QueryStream::new(
                Session::new_query_id(),
                Arc::new(stream),
                registry,
            ))
        })
        .await
        .unwrap_or_else(|err| Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string())))
    }

    /// [`Session::execute_arrow`] on a blocking thread.
    pub async fn execute_arrow_async(&self, sql: String) -> Result<ArrowStream, ChdbError> {
        let connection = self.connection()?;
        let settings = self.settings.clone();
        tokio::task::spawn_blocking(move || {
            Session::apply_settings_of(&connection, &settings)?;
            ArrowStream::start(Session::new_query_id(), &connection, &sql)
        })
        .await
        .unwrap_or_else(|err| Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string())))
    }

    /// [`Session::cancel`] on a blocking thread.
    pub async fn cancel_async(&self, query_id: String) -> Result<(), ChdbError> {
        let registry = Arc::clone(&self.registry);
        tokio::task::spawn_blocking(move || query::cancel(&registry, &query_id))
            .await
            .unwrap_or_else(|err| Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string())))
    }

    /// Applies this session's settings, one `SET` each.
    fn apply_settings_of(
        connection: &ffi::Connection,
        settings: &Settings,
    ) -> Result<(), ChdbError> {
        for (name, value) in settings.as_slice() {
            // A setting's name and value come from the House's own allowlist and
            // the client's URL, so neither may end the statement early: the value is
            // quoted the way ClickHouse quotes a setting value.
            let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
            connection
                .execute(&format!("SET {name} = '{escaped}'"))
                .map_err(engine_error)?;
        }
        Ok(())
    }
}
