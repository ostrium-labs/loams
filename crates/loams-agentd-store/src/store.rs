//! `DocsStore` — local SQLite persistence for doc snapshots and the
//! processed-command ledger (ARCHITECTURE §2 command plane: entries are marked
//! processed BEFORE execution so a crash can never double-execute a command).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// Errors surfaced by [`DocsStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Synchronous command/doc callbacks still need commit-before-send semantics.
/// Relinquish a multithread runtime's core BEFORE either SQLite or its mutex
/// can block; moving only the snapshot writer leaves these contenders fatal.
pub(crate) fn store_blocking<T>(f: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}

/// Ordered, append-only migrations. Each entry runs once inside a transaction;
/// `schema_migrations` records what has been applied.
const MIGRATIONS: &[&str] = &[
    // v1 — snapshots + processed-command ledger
    "CREATE TABLE snapshots (
        doc_id   TEXT PRIMARY KEY,
        bytes    BLOB NOT NULL,
        saved_at INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE processed_commands (
        command_id   TEXT PRIMARY KEY,
        processed_at INTEGER NOT NULL
     ) STRICT;",
    // v2 — chat2 room cursor + doc epoch (docs/chat2-sync.md C2). The cursor
    // is persisted in the SAME transaction as the snapshot bytes, so content
    // and cursor cannot diverge (restored backups / copied devices simply
    // redownload from their honest cursor). `epoch` marks rebuild lineage
    // (M1/M3): 2 = thin chat2 rebuild; NULL/0 = pre-migration s2 doc.
    "ALTER TABLE snapshots ADD COLUMN cursor INTEGER;
     ALTER TABLE snapshots ADD COLUMN epoch INTEGER;",
    // v3 — publication survives actor eviction and process restart.
    "CREATE TABLE chat_outbox (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        batch_id TEXT NOT NULL UNIQUE,
        bytes BLOB NOT NULL,
        needs_checkpoint INTEGER NOT NULL DEFAULT 0
     ) STRICT;
     CREATE INDEX chat_outbox_doc ON chat_outbox(doc_id,ordinal);
     CREATE TABLE chat_outbox_initialized (doc_id TEXT PRIMARY KEY) STRICT;",
    // Older cursors may have advanced over parked imports. Trust only writes
    // made by the causal-aware persister, not every legacy epoch-2 snapshot.
    "ALTER TABLE snapshots ADD COLUMN cursor_verified INTEGER NOT NULL DEFAULT 0;",
    // Durable wake/recovery receipts, independent of live document handles.
    "CREATE TABLE chat_sync_jobs (
        doc_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        version INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(doc_id, kind)
    ) STRICT;
    CREATE TABLE sync_job_clock (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL) STRICT;
    INSERT INTO sync_job_clock VALUES (1,0);",
    "ALTER TABLE chat_sync_jobs ADD COLUMN cursor TEXT NOT NULL DEFAULT '';",
    // Loams Studio's review comments (SD1 Task 11, §53 §5.6), keyed by
    // checkout. `outdated` is computed from the diff, never stored.
    "CREATE TABLE review_comments (
        id            TEXT PRIMARY KEY,
        checkout_id   TEXT NOT NULL,
        path          TEXT NOT NULL,
        side          TEXT NOT NULL CHECK (side IN ('old', 'new')),
        line          INTEGER NOT NULL,
        end_line      INTEGER,
        body          TEXT NOT NULL,
        diff_checksum TEXT NOT NULL,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL,
        resolved      INTEGER NOT NULL DEFAULT 0
     ) STRICT;
     CREATE INDEX review_comments_checkout ON review_comments(checkout_id, created_at, id);",
    // Run groups (plan SD1 Task 12, Ruling 5): one row per group, its record
    // as JSON (`crate::run_groups`).
    "CREATE TABLE run_groups (
        group_id   TEXT PRIMARY KEY,
        space_id   TEXT,
        created_at INTEGER NOT NULL,
        record     TEXT NOT NULL
     ) STRICT;",
];

/// Tables created with `IF NOT EXISTS` after [`MIGRATIONS`], so they need no
/// version number of their own: several SD1 tasks add tables at once, and a
/// numbered step would depend on which merged first. Only new tables go here;
/// a change to an existing one is a migration.
const TABLES: &[&str] = &[
    // SD1 Task 14 (G20): each turn's tokens and duration, as the harness
    // reported them, for the thread header. Informational, never billing.
    "CREATE TABLE IF NOT EXISTS turn_usage (
        chat_id       TEXT NOT NULL,
        turn_id       TEXT NOT NULL,
        input_tokens  INTEGER,
        output_tokens INTEGER,
        cache_tokens  INTEGER,
        cost_usd      REAL,
        duration_ms   INTEGER NOT NULL,
        ended_at      INTEGER NOT NULL,
        PRIMARY KEY (chat_id, turn_id)
     ) STRICT;",
    // SD1 Task 13 (G22): a thread's approval mode, when one was set.
    "CREATE TABLE IF NOT EXISTS approval_modes (
        chat_id TEXT PRIMARY KEY NOT NULL,
        mode    TEXT NOT NULL
     ) STRICT;",
    // SD1 Task 13 (G18): "always" decisions, per thread and tool.
    "CREATE TABLE IF NOT EXISTS approval_always (
        chat_id TEXT NOT NULL,
        tool    TEXT NOT NULL,
        PRIMARY KEY (chat_id, tool)
     ) STRICT;",
    // SD1 Task 13 (G17): approvals waiting for a person, so they survive a
    // restart. `args` are stored redacted, as JSON.
    "CREATE TABLE IF NOT EXISTS pending_approvals (
        chat_id    TEXT NOT NULL,
        call_id    TEXT NOT NULL,
        turn_id    TEXT,
        tool       TEXT NOT NULL,
        args       TEXT NOT NULL,
        risk       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (chat_id, call_id)
     ) STRICT;",
    // FT1 FT0 Task 4 (design §72 §5.2): the factory loop's typed approvals
    // (plan, merge, deploy). Only the payload's hash is kept; `state` is
    // `pending`, `approved`, `denied` or `used`.
    "CREATE TABLE IF NOT EXISTS typed_approvals (
        id           TEXT PRIMARY KEY NOT NULL,
        chat_id      TEXT NOT NULL,
        kind         TEXT NOT NULL,
        summary      TEXT NOT NULL,
        payload_hash TEXT NOT NULL,
        state        TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        decided_at   INTEGER
     ) STRICT;",
];

/// One pending approval's row (SD1 Task 13, `pending_approvals`). `args` is
/// the redacted arguments' JSON and `risk` its wire name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApprovalRow {
    pub chat_id: String,
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool: String,
    pub args: String,
    pub risk: String,
    pub created_at: i64,
}

/// One turn's usage row (SD1 Task 14, `turn_usage`). A count the harness did
/// not report is `None`; `cost_usd` is only ever the harness's own figure.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsageRow {
    pub turn_id: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: i64,
}

/// SQLite-backed store under a data directory (`{data_dir}/docs.sqlite3`).
///
/// Holds warm-open doc snapshots (the DO room is authoritative; these make
/// cold starts instant and offline restarts possible) and the command ledger
/// that gives command execution mark-BEFORE-execute idempotence.
pub struct DocsStore {
    conn: Mutex<Connection>,
    failed_publications: Mutex<HashSet<String>>,
    /// Snapshot jobs wait here asynchronously before entering the blocking pool.
    pub snapshot_writer: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for DocsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocsStore").finish_non_exhaustive()
    }
}

impl DocsStore {
    /// Open (creating directory, database, and schema as needed).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let mut conn = Connection::open(data_dir.join("docs.sqlite3"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            failed_publications: Mutex::new(HashSet::new()),
            snapshot_writer: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Insert before sending. Stable IDs make crash-after-ACK replay idempotent.
    pub fn enqueue_chat_update(
        &self,
        doc_id: &str,
        batch_id: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        store_blocking(|| self.enqueue_chat_update_blocking(doc_id, batch_id, bytes))
    }

    fn enqueue_chat_update_blocking(
        &self,
        doc_id: &str,
        batch_id: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let result = self.conn().execute(
            "INSERT OR IGNORE INTO chat_outbox(doc_id,batch_id,bytes) VALUES (?1,?2,?3)",
            params![doc_id, batch_id, bytes],
        );
        if result.is_err() {
            self.failed_publications
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(doc_id.to_string());
        }
        result?;
        Ok(())
    }

    pub fn pending_chat_updates(&self, doc_id: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| self.pending_chat_updates_blocking(doc_id))
    }

    /// Admission/lifetime checks must not copy the pending payloads.
    pub fn has_pending_chat_updates(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_outbox WHERE doc_id=?1)",
                params![doc_id],
                |row| row.get(0),
            )?)
        })
    }

    fn pending_chat_updates_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT batch_id,bytes FROM chat_outbox WHERE doc_id=?1 ORDER BY ordinal")?;
        Ok(stmt
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn reject_chat_update(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.reject_chat_update_blocking(doc_id, batch_id))
    }

    fn reject_chat_update_blocking(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE chat_outbox SET needs_checkpoint=1 WHERE doc_id=?1 AND batch_id=?2",
            params![doc_id, batch_id],
        )?;
        Ok(())
    }

    pub fn rejected_chat_updates(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| self.rejected_chat_updates_blocking(doc_id))
    }

    fn rejected_chat_updates_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT batch_id,bytes FROM chat_outbox WHERE doc_id=?1 AND needs_checkpoint=1 ORDER BY ordinal")?;
        Ok(stmt
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn acknowledge_chat_update(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.acknowledge_chat_update_blocking(doc_id, batch_id))
    }

    fn acknowledge_chat_update_blocking(
        &self,
        doc_id: &str,
        batch_id: &str,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "DELETE FROM chat_outbox WHERE doc_id=?1 AND batch_id=?2",
            params![doc_id, batch_id],
        )?;
        Ok(())
    }

    pub fn chat_outbox_initialized(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.chat_outbox_initialized_blocking(doc_id))
    }

    fn chat_outbox_initialized_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM chat_outbox_initialized WHERE doc_id=?1",
                params![doc_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Upgrade legacy snapshots (including orphaned local history) once. The
    /// marker and complete replay obligation commit together; never reset cursors.
    pub fn initialize_chat_outbox(
        &self,
        doc_id: &str,
        updates: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        store_blocking(|| self.initialize_chat_outbox_blocking(doc_id, updates))
    }

    fn initialize_chat_outbox_blocking(
        &self,
        doc_id: &str,
        updates: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if tx.execute(
            "INSERT OR IGNORE INTO chat_outbox_initialized(doc_id) VALUES (?1)",
            params![doc_id],
        )? != 0
        {
            for bytes in updates {
                tx.execute(
                    "INSERT INTO chat_outbox(doc_id,batch_id,bytes) VALUES (?1,?2,?3)",
                    params![doc_id, uuid::Uuid::new_v4().to_string(), bytes],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Latest saved snapshot for `doc_id`, if any.
    pub fn load_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| self.load_snapshot_blocking(doc_id))
    }

    fn load_snapshot_blocking(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = self
            .conn()
            .query_row(
                "SELECT bytes FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bytes)
    }

    /// Save (upsert) the snapshot for `doc_id`.
    pub fn save_snapshot(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        store_blocking(|| self.save_snapshot_blocking(doc_id, bytes))
    }

    fn save_snapshot_blocking(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at",
            params![doc_id, bytes, now_ms()],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        Ok(())
    }

    /// Save the snapshot together with its chat2 room cursor and doc epoch —
    /// ONE transaction, so bytes and cursor can never disagree (the C2 rule;
    /// a divergent pair is exactly the restored-backup redownload bug).
    pub fn save_snapshot_with_cursor(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        store_blocking(|| self.save_snapshot_with_cursor_blocking(doc_id, bytes, cursor, epoch))
    }

    fn save_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 0",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_verified_snapshot_with_cursor(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            self.save_verified_snapshot_with_cursor_blocking(doc_id, bytes, cursor, epoch)
        })
    }

    fn save_verified_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch, cursor_verified) VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 1",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn snapshot_cursor(&self, doc_id: &str) -> Result<u64, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    "SELECT COALESCE(cursor, 0) FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0) as u64)
        })
    }

    pub fn snapshot_cursor_verified(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.snapshot_cursor_verified_blocking(doc_id))
    }

    fn snapshot_cursor_verified_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT cursor_verified FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Snapshot + chat2 cursor + epoch. Pre-migration rows (or rows written
    /// by [`Self::save_snapshot`]) read back as `(bytes, 0, 0)`.
    pub fn load_snapshot_with_cursor(
        &self,
        doc_id: &str,
    ) -> Result<Option<(Vec<u8>, u64, u32)>, StoreError> {
        store_blocking(|| self.load_snapshot_with_cursor_blocking(doc_id))
    }

    /// Read admission metadata without fetching or decoding the snapshot blob.
    pub fn snapshot_epoch(&self, doc_id: &str) -> Result<u32, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    "SELECT COALESCE(epoch, 0) FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| row.get::<_, u32>(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    fn load_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Option<(Vec<u8>, u64, u32)>, StoreError> {
        let row = self
            .conn()
            .query_row(
                "SELECT bytes, cursor, epoch FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                    ))
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Delete the snapshot row for `doc_id` (destructive schema breaks: the
    /// legacy `workspace` row is dropped on open). Missing rows are a no-op.
    pub fn delete_snapshot(&self, doc_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.delete_snapshot_blocking(doc_id))
    }

    fn delete_snapshot_blocking(&self, doc_id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM snapshots WHERE doc_id = ?1", params![doc_id])?;
        tx.execute("DELETE FROM chat_outbox WHERE doc_id = ?1", params![doc_id])?;
        tx.execute(
            "DELETE FROM chat_sync_jobs WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM chat_outbox_initialized WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Whether a snapshot row exists for `doc_id` — presence only, no blob read.
    pub fn has_snapshot(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.has_snapshot_blocking(doc_id))
    }

    fn has_snapshot_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        let hit = self
            .conn()
            .query_row(
                "SELECT 1 FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |_| Ok(()),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    /// The full command ledger — profile-import reads the source's claims so
    /// imported pending commands can never re-execute under the new profile.
    pub fn processed_commands(&self) -> Result<Vec<(String, i64)>, StoreError> {
        store_blocking(|| self.processed_commands_blocking())
    }

    fn processed_commands_blocking(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT command_id, processed_at FROM processed_commands")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Merge foreign ledger claims (profile import). Existing claims win;
    /// returns how many rows were newly inserted.
    pub fn import_processed_commands(&self, rows: &[(String, i64)]) -> Result<usize, StoreError> {
        store_blocking(|| self.import_processed_commands_blocking(rows))
    }

    fn import_processed_commands_blocking(
        &self,
        rows: &[(String, i64)],
    ) -> Result<usize, StoreError> {
        let mut inserted = 0;
        let conn = self.conn();
        for (command_id, processed_at) in rows {
            inserted += conn.execute(
                "INSERT OR IGNORE INTO processed_commands (command_id, processed_at) VALUES (?1, ?2)",
                params![command_id, processed_at],
            )?;
        }
        Ok(inserted)
    }

    /// Whether `command_id` has already been claimed for execution.
    pub fn is_processed(&self, command_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.is_processed_blocking(command_id))
    }

    fn is_processed_blocking(&self, command_id: &str) -> Result<bool, StoreError> {
        let hit = self
            .conn()
            .query_row(
                "SELECT 1 FROM processed_commands WHERE command_id = ?1",
                params![command_id],
                |_| Ok(()),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    /// Claim `command_id` for execution — call BEFORE executing (ledger rule:
    /// a crash mid-execution must never re-run the command). Returns `true`
    /// if this call claimed it, `false` if it was already processed.
    pub fn mark_processed(&self, command_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.mark_processed_blocking(command_id))
    }

    fn mark_processed_blocking(&self, command_id: &str) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO processed_commands (command_id, processed_at) VALUES (?1, ?2)",
            params![command_id, now_ms()],
        )?;
        Ok(changed > 0)
    }

    // Every snapshot path, including ACK/cursor persistence, must retain a
    // replay obligation for operations whose outbox write failed. Invalidate
    // the marker atomically with those operations becoming durable in a snapshot.
    fn invalidate_failed_publication(
        &self,
        tx: &rusqlite::Transaction<'_>,
        doc_id: &str,
    ) -> Result<(), StoreError> {
        if self
            .failed_publications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(doc_id)
        {
            tx.execute(
                "DELETE FROM chat_outbox_initialized WHERE doc_id=?1",
                params![doc_id],
            )?;
        }
        Ok(())
    }

    /// Record a turn's usage. A turn recorded again (a resumed turn) adds
    /// its counts and duration to the row.
    pub fn record_turn_usage(&self, chat_id: &str, row: &TurnUsageRow) -> Result<(), StoreError> {
        let count = |n: Option<u64>| n.map(|n| i64::try_from(n).unwrap_or(i64::MAX));
        store_blocking(|| {
            self.conn().execute(
                "INSERT INTO turn_usage
                    (chat_id, turn_id, input_tokens, output_tokens, cache_tokens, cost_usd,
                     duration_ms, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (chat_id, turn_id) DO UPDATE SET
                    input_tokens = CASE WHEN excluded.input_tokens IS NULL THEN input_tokens
                        ELSE COALESCE(input_tokens, 0) + excluded.input_tokens END,
                    output_tokens = CASE WHEN excluded.output_tokens IS NULL THEN output_tokens
                        ELSE COALESCE(output_tokens, 0) + excluded.output_tokens END,
                    cache_tokens = CASE WHEN excluded.cache_tokens IS NULL THEN cache_tokens
                        ELSE COALESCE(cache_tokens, 0) + excluded.cache_tokens END,
                    cost_usd = CASE WHEN excluded.cost_usd IS NULL THEN cost_usd
                        ELSE COALESCE(cost_usd, 0) + excluded.cost_usd END,
                    duration_ms = duration_ms + excluded.duration_ms,
                    ended_at = excluded.ended_at",
                params![
                    chat_id,
                    row.turn_id,
                    count(row.input_tokens),
                    count(row.output_tokens),
                    count(row.cache_tokens),
                    row.cost_usd,
                    row.duration_ms,
                    now_ms(),
                ],
            )?;
            Ok(())
        })
    }

    /// A chat's turns' usage, in the order they ended.
    pub fn turn_usage(&self, chat_id: &str) -> Result<Vec<TurnUsageRow>, StoreError> {
        let count = |n: Option<i64>| n.map(|n| u64::try_from(n).unwrap_or(0));
        store_blocking(|| {
            let conn = self.conn();
            let mut statement = conn.prepare(
                "SELECT turn_id, input_tokens, output_tokens, cache_tokens, cost_usd, duration_ms
                 FROM turn_usage WHERE chat_id = ?1 ORDER BY ended_at, rowid",
            )?;
            let rows = statement.query_map(params![chat_id], |row| {
                Ok(TurnUsageRow {
                    turn_id: row.get(0)?,
                    input_tokens: count(row.get(1)?),
                    output_tokens: count(row.get(2)?),
                    cache_tokens: count(row.get(3)?),
                    cost_usd: row.get(4)?,
                    duration_ms: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Forget a deleted chat's usage.
    pub fn delete_turn_usage(&self, chat_id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "DELETE FROM turn_usage WHERE chat_id = ?1",
                params![chat_id],
            )?;
            Ok(())
        })
    }

    /// Set a thread's approval mode (SD1 Task 13).
    pub fn set_approval_mode(&self, chat_id: &str, mode: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "INSERT INTO approval_modes (chat_id, mode) VALUES (?1, ?2)
                 ON CONFLICT (chat_id) DO UPDATE SET mode = excluded.mode",
                params![chat_id, mode],
            )?;
            Ok(())
        })
    }

    /// Every thread's approval mode that was set, as `(chat_id, mode)`.
    pub fn approval_modes(&self) -> Result<Vec<(String, String)>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut statement = conn.prepare("SELECT chat_id, mode FROM approval_modes")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Remember an "always" decision for a thread's tool.
    pub fn add_approval_always(&self, chat_id: &str, tool: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "INSERT OR IGNORE INTO approval_always (chat_id, tool) VALUES (?1, ?2)",
                params![chat_id, tool],
            )?;
            Ok(())
        })
    }

    /// Every remembered "always", as `(chat_id, tool)`.
    pub fn approval_always(&self) -> Result<Vec<(String, String)>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut statement = conn.prepare("SELECT chat_id, tool FROM approval_always")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Record an approval waiting for a person (replacing one with its ids).
    pub fn put_pending_approval(&self, row: &PendingApprovalRow) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "INSERT OR REPLACE INTO pending_approvals
                    (chat_id, call_id, turn_id, tool, args, risk, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.chat_id,
                    row.call_id,
                    row.turn_id,
                    row.tool,
                    row.args,
                    row.risk,
                    row.created_at,
                ],
            )?;
            Ok(())
        })
    }

    /// Forget an approval once it is answered, denied or gone with its turn.
    pub fn delete_pending_approval(&self, chat_id: &str, call_id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "DELETE FROM pending_approvals WHERE chat_id = ?1 AND call_id = ?2",
                params![chat_id, call_id],
            )?;
            Ok(())
        })
    }

    /// Every pending approval, oldest first.
    pub fn pending_approvals(&self) -> Result<Vec<PendingApprovalRow>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut statement = conn.prepare(
                "SELECT chat_id, call_id, turn_id, tool, args, risk, created_at
                 FROM pending_approvals ORDER BY created_at, rowid",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(PendingApprovalRow {
                    chat_id: row.get(0)?,
                    call_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    tool: row.get(3)?,
                    args: row.get(4)?,
                    risk: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Forget a deleted thread's mode, "always" decisions and approvals.
    pub fn delete_chat_approvals(&self, chat_id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            for table in ["approval_modes", "approval_always", "pending_approvals"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE chat_id = ?1"),
                    params![chat_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock only means another thread panicked mid-query; the
        // connection itself is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
         ) STRICT",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, now_ms()],
        )?;
        tx.commit()?;
    }
    for sql in TABLES {
        conn.execute_batch(sql)?;
    }
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn contended_connection_does_not_starve_a_two_worker_runtime() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let holder = store.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let holding = std::thread::spawn(move || {
            let _connection = holder.conn();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
        });
        locked_rx.recv().unwrap();
        let started = std::time::Instant::now();
        let write = store.clone();
        let a = tokio::spawn(async move {
            write.save_snapshot("whale", &[0; 1024]).unwrap();
        });
        let read = store.clone();
        let b = tokio::spawn(async move {
            read.load_snapshot("registry1").unwrap();
        });
        // Both contenders have a chance to enter the shared connection wait.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(
            started.elapsed() < std::time::Duration::from_millis(300),
            "SQLite contenders monopolized both runtime workers"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let _accepted = listener.accept().await.unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(300),
            "network progress waited for SQLite"
        );
        drop(socket);
        a.await.unwrap();
        b.await.unwrap();
        holding.join().unwrap();
    }

    #[test]
    fn verified_cursor_marker_tracks_atomic_snapshot_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store
            .save_snapshot_with_cursor("chat", b"old", 69000, 2)
            .unwrap();
        assert!(!store.snapshot_cursor_verified("chat").unwrap());
        store
            .save_verified_snapshot_with_cursor("chat", b"safe", 69001, 2)
            .unwrap();
        assert!(store.snapshot_cursor_verified("chat").unwrap());
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        assert!(store.snapshot_cursor_verified("chat").unwrap());
        assert_eq!(
            store.load_snapshot_with_cursor("chat").unwrap(),
            Some((b"safe".to_vec(), 69001, 2))
        );
        store
            .save_snapshot_with_cursor("chat", b"replacement", 0, 2)
            .unwrap();
        assert!(
            !store.snapshot_cursor_verified("chat").unwrap(),
            "unverified replacement invalidates trust"
        );
    }

    #[test]
    fn snapshot_roundtrip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        assert_eq!(store.load_snapshot("chat-1").unwrap(), None);
        store.save_snapshot("chat-1", b"v1").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v1"[..])
        );
        store.save_snapshot("chat-1", b"v2-longer-bytes").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2-longer-bytes"[..])
        );
        // Distinct docs do not collide.
        store.save_snapshot("chat-2", b"other").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2-longer-bytes"[..])
        );
    }

    #[test]
    fn cursor_rides_the_snapshot_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        // Plain saves (pre-chat2 path) read back with cursor/epoch 0.
        store.save_snapshot("chat-1", b"v1").unwrap();
        assert_eq!(
            store.load_snapshot_with_cursor("chat-1").unwrap(),
            Some((b"v1".to_vec(), 0, 0))
        );
        // Cursor and bytes land together; a re-save moves both.
        store
            .save_snapshot_with_cursor("chat-1", b"v2", 41, 2)
            .unwrap();
        assert_eq!(
            store.load_snapshot_with_cursor("chat-1").unwrap(),
            Some((b"v2".to_vec(), 41, 2))
        );
        // Plain load still works for cursor-written rows.
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2"[..])
        );
        // A plain save (legacy caller) clears nothing — cursor persists…
        store.save_snapshot("chat-1", b"v3").unwrap();
        let (bytes, cursor, epoch) = store.load_snapshot_with_cursor("chat-1").unwrap().unwrap();
        assert_eq!((bytes.as_slice(), cursor, epoch), (&b"v3"[..], 41, 2));
    }

    #[test]
    fn processed_ledger_claims_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        assert!(!store.is_processed("cmd-1").unwrap());
        assert!(store.mark_processed("cmd-1").unwrap(), "first mark claims");
        assert!(store.is_processed("cmd-1").unwrap());
        assert!(
            !store.mark_processed("cmd-1").unwrap(),
            "second mark must not re-claim"
        );
    }

    #[test]
    fn reopen_preserves_data_and_migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store.save_snapshot("chat-1", b"persisted").unwrap();
            store.mark_processed("cmd-1").unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap(); // re-runs migrate()
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"persisted"[..])
        );
        assert!(store.is_processed("cmd-1").unwrap());
        assert!(!store.mark_processed("cmd-1").unwrap());
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn outbox_survives_restart_initializes_once_and_preserves_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let original;
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .save_snapshot_with_cursor("chat", b"snapshot", 40229, 2)
                .unwrap();
            store
                .initialize_chat_outbox("chat", &[b"legacy-tail".to_vec()])
                .unwrap();
            original = store.pending_chat_updates("chat").unwrap();
            store
                .enqueue_chat_update("chat", "stable-id", b"new-turn")
                .unwrap();
            store
                .enqueue_chat_update("chat", "stable-id", b"new-turn")
                .unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        store
            .initialize_chat_outbox("chat", &[b"must-not-reseed".to_vec()])
            .unwrap();
        let pending = store.pending_chat_updates("chat").unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0], original[0]);
        assert_eq!(pending[1], ("stable-id".into(), b"new-turn".to_vec()));
        assert_eq!(
            store.load_snapshot_with_cursor("chat").unwrap().unwrap(),
            (b"snapshot".to_vec(), 40229, 2)
        );
        store
            .acknowledge_chat_update("other-chat", "stable-id")
            .unwrap();
        assert_eq!(store.pending_chat_updates("chat").unwrap().len(), 2);
        store.acknowledge_chat_update("chat", "stable-id").unwrap();
        assert_eq!(store.pending_chat_updates("chat").unwrap(), original);
        store.delete_snapshot("chat").unwrap();
        assert!(store.pending_chat_updates("chat").unwrap().is_empty());
        assert!(!store.chat_outbox_initialized("chat").unwrap());
    }
}

#[cfg(test)]
mod publication_failure_tests {
    use super::*;
    #[test]
    fn cursor_save_after_failed_outbox_write_keeps_a_durable_replay_obligation() {
        for cursor_save in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .save_snapshot_with_cursor("chat", b"before", 42, 2)
                .unwrap();
            store.initialize_chat_outbox("chat", &[]).unwrap();
            store.conn().execute_batch("CREATE TRIGGER fail_publication BEFORE INSERT ON chat_outbox BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;").unwrap();
            assert!(
                store
                    .enqueue_chat_update("chat", "batch", b"cleanup")
                    .is_err()
            );
            if cursor_save {
                store
                    .save_snapshot_with_cursor("chat", b"after", 43, 2)
                    .unwrap();
            } else {
                store.save_snapshot("chat", b"after").unwrap();
            }
            drop(store);
            let reopened = DocsStore::open(dir.path()).unwrap();
            assert!(!reopened.chat_outbox_initialized("chat").unwrap());
            assert_eq!(
                reopened.load_snapshot_with_cursor("chat").unwrap().unwrap(),
                (b"after".to_vec(), if cursor_save { 43 } else { 42 }, 2)
            );
        }
    }
}
