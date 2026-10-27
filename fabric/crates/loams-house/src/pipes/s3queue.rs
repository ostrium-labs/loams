//! `S3Queue` pipes (HS1 Task 18, design §49 §9): objects that land in the
//! namespace's own prefix of the bucket go into a lake table exactly once,
//! through a materialized view.
//!
//! ```sql
//! CREATE TABLE inbox (k UInt64, v String)
//!     ENGINE = S3Queue('ns/2/incoming/*.jsonl', 'JSONEachRow')
//!     SETTINGS mode = 'ordered', after_processing = 'keep';
//! CREATE MATERIALIZED VIEW inbox_mv TO t AS SELECT k, v FROM inbox;
//! ```
//!
//! * **The path** is a key pattern under the namespace's own prefix,
//!   `ns/<id>/`, of the House's store, with ClickHouse's globs (`*`, `**`,
//!   `?`, `{a,b}`, `{1..9}`). Anything else is `344`: a URL, another
//!   namespace's prefix, `..`, or the House's own tables under `ns/<id>/house/`
//!   (ruling R18.2). Workers read the objects through `house-cache`, whose
//!   grant is that same prefix.
//! * **Ordered** (ClickHouse's `mode = 'ordered'`, the only mode served): the
//!   view's cursor is the greatest key it has processed,
//!   `loams.s3queue.<view id>` on its target, and a run takes the matching keys
//!   after it in key order, at most `max_processed_files_before_commit` (100)
//!   objects and `max_processed_bytes_before_commit` (256 MiB) of them. It
//!   commits the view's rows **with the moved cursor in the same commit**,
//!   through the pipes' [`commit_cursor`](super::commit_cursor): the snapshot's
//!   summary names the processed keys (`loams.s3queue.<view id>.files`). So a
//!   front that dies before the commit leaves the objects for the next run, a
//!   second front that loses the race abandons its batch, and an object is
//!   never read twice. An object whose key sorts before the cursor when it
//!   lands is never read: writers name objects in increasing order, as
//!   ClickHouse's ordered mode asks (ruling R18.1).
//! * **`after_processing`**: `keep` (the default) or `delete`, which removes
//!   the objects read once their commit landed (skipped ones, never read, are
//!   kept).
//! * **A poison object** — one the view cannot write (a malformed row, rows
//!   past the insert cap) — fails its batch before the commit, so the view
//!   stays at it, the error in its `last_error`, until it is fixed or removed
//!   (ruling R18.3). The first object of a batch is taken whatever its size.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    Hooks, MaterializedView, Pipe, PipeBatch, PipeFeed, Ran, Step, ViewWriter, commit_cursor,
};
use crate::admission::WorkerPool;
use crate::catalog::{Lake, LakeRoot};
use crate::config::HouseConfig;
use crate::ddl::quote_string;
use crate::errors::{ChError, HouseError};

/// Property of an `S3Queue` pipe: its [`QueueSource`].
pub const PROP_QUEUE: &str = "loams.pipe_s3queue";
/// Properties (and summary keys) of a view's target: the cursor,
/// `loams.s3queue.<view id>` = the greatest key processed.
pub const QUEUE_CURSOR_PREFIX: &str = "loams.s3queue.";
/// `max_processed_files_before_commit`'s default (ClickHouse's).
pub const DEFAULT_MAX_FILES: u64 = 100;
/// `max_processed_bytes_before_commit`'s default.
pub const DEFAULT_MAX_BYTES: u64 = super::MAX_BATCH_BYTES as u64;
/// Keys one listing page holds.
const LIST_PAGE: usize = 1_000;
/// Keys a run lists past a full batch, to count the lag.
const LAG_SCAN: usize = 10_000;
/// Rounds between two listings of the namespaces' queues.
const REFRESH_EVERY: u64 = 30;

/// Input formats an `S3Queue` reads (chDB's `s3()` reads them).
pub const FORMATS: &[&str] = &[
    "JSONEachRow",
    "JSONLines",
    "NDJSON",
    "JSONCompactEachRow",
    "CSV",
    "CSVWithNames",
    "CSVWithNamesAndTypes",
    "TSV",
    "TabSeparated",
    "TSVWithNames",
    "TabSeparatedWithNames",
    "TSVWithNamesAndTypes",
    "Parquet",
    "ORC",
    "Arrow",
    "Avro",
];

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

fn outside(path: &str, namespace: u64) -> HouseError {
    HouseError::from(ChError::support_is_disabled(format!(
        "S3Queue reads only the namespace's own prefix of the House's store: {path:?} is not \
         under ns/{namespace}/ (or is under ns/{namespace}/house/, the House's own tables)"
    )))
}

/// What `after_processing` does with an object once its rows committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterProcessing {
    /// Leaves it.
    Keep,
    /// Deletes it.
    Delete,
}

/// Where an `S3Queue` pipe reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSource {
    /// The key pattern, store-relative, under `ns/<id>/`.
    pub path: String,
    /// The objects' input format.
    pub format: String,
    /// `after_processing`.
    pub after_processing: AfterProcessing,
    /// `max_processed_files_before_commit`.
    pub max_files: u64,
    /// `max_processed_bytes_before_commit`.
    pub max_bytes: u64,
}

impl QueueSource {
    /// `S3Queue('<path>', '<format>')` with its `SETTINGS`, for `namespace`.
    pub fn parse(
        namespace: u64,
        args: &[String],
        settings: &[(String, String)],
    ) -> Result<Self, HouseError> {
        let literal = |raw: &String| -> Result<String, HouseError> {
            if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
                Ok(crate::ddl::unquote(raw))
            } else {
                Err(bad(format!(
                    "S3Queue's arguments are string literals, not {raw}"
                )))
            }
        };
        match args.len() {
            2 => {}
            3 | 4 => {
                return Err(HouseError::from(ChError::support_is_disabled(
                    "S3Queue takes no credentials: it reads the namespace's own prefix of the \
                     House's store with the House's",
                )));
            }
            _ => {
                return Err(bad(
                    "S3Queue takes a path under the namespace's prefix and a format: \
                     S3Queue('ns/<id>/<path>', '<format>')",
                ));
            }
        }
        let path = literal(&args[0])?;
        check_path(&path, namespace)?;
        Glob::parse(&path)?;
        let format = literal(&args[1])?;
        if !FORMATS.contains(&format.as_str()) {
            return Err(HouseError::from(ChError::unknown_format(format!(
                "Unknown format {format} for an S3Queue pipe: {}",
                FORMATS.join(", ")
            ))));
        }
        let mut source = Self {
            path,
            format,
            after_processing: AfterProcessing::Keep,
            max_files: DEFAULT_MAX_FILES,
            max_bytes: DEFAULT_MAX_BYTES,
        };
        for (name, value) in settings {
            let short = name.strip_prefix("s3queue_").unwrap_or(name);
            match short {
                "mode" => match value.as_str() {
                    "ordered" => {}
                    "unordered" => {
                        return Err(HouseError::from(ChError::not_implemented(
                            "S3Queue's unordered mode is not served: the House keeps one \
                             cursor per view, the greatest key processed (mode = 'ordered')",
                        )));
                    }
                    other => {
                        return Err(bad(format!(
                            "S3Queue's mode is 'ordered' or 'unordered', not {other:?}"
                        )));
                    }
                },
                "after_processing" => {
                    source.after_processing = match value.as_str() {
                        "keep" => AfterProcessing::Keep,
                        "delete" => AfterProcessing::Delete,
                        other => {
                            return Err(bad(format!(
                                "after_processing is 'keep' or 'delete', not {other:?}"
                            )));
                        }
                    }
                }
                "max_processed_files_before_commit" => {
                    source.max_files = number(name, value, 1, 10_000)?;
                }
                "max_processed_bytes_before_commit" => {
                    source.max_bytes = number(name, value, 1, DEFAULT_MAX_BYTES)?;
                }
                _ => {
                    return Err(HouseError::from(ChError::unknown_setting(format!(
                        "Unknown setting '{name}': for an S3Queue pipe (mode, after_processing, \
                         max_processed_files_before_commit, max_processed_bytes_before_commit)"
                    ))));
                }
            }
        }
        Ok(source)
    }

    /// The engine as `SHOW CREATE` prints it.
    pub fn render(&self) -> String {
        format!(
            "S3Queue({}, {})",
            quote_string(&self.path),
            quote_string(&self.format)
        )
    }
}

fn number(name: &str, value: &str, min: u64, max: u64) -> Result<u64, HouseError> {
    match value.parse::<u64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(bad(format!(
            "{name} is a number from {min} to {max}, not {value:?}"
        ))),
    }
}

/// `344` unless `path` is a key pattern under `ns/<namespace>/` and outside
/// `ns/<namespace>/house/`.
pub fn check_path(path: &str, namespace: u64) -> Result<(), HouseError> {
    let own = format!("ns/{namespace}/");
    let Some(rest) = path.strip_prefix(own.as_str()) else {
        return Err(outside(path, namespace));
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let sound = !rest.is_empty()
        && !path.contains('\\')
        && !without_ranges(path).contains("..")
        && !path.contains("://")
        && segments.iter().all(|s| !s.is_empty() && *s != ".")
        && segments[0] != "house";
    if !sound {
        return Err(outside(path, namespace));
    }
    // A pattern whose first segment is a glob could reach `house/`; listing
    // leaves those keys out (`Glob::matches` is not asked about them).
    Ok(())
}

/// `path` with each `{N..M}` range read as `N`: the only `..` a sound path
/// holds.
fn without_ranges(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let body = &after[..close];
        match body.split_once("..") {
            Some((from, to))
                if !from.is_empty()
                    && !to.is_empty()
                    && from.bytes().chain(to.bytes()).all(|b| b.is_ascii_digit()) =>
            {
                out.push_str(from);
            }
            _ => {
                out.push('{');
                out.push_str(body);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A key may be read by its name in an `s3()` URL: no glob or quote
/// characters, nothing that needs escaping.
fn readable(key: &str) -> bool {
    key.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'=' | b'+'))
}

// ---------------------------------------------------------------------------
// Globs.

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Char(char),
    /// `*`: any run of characters but `/`.
    Star,
    /// `**`: any run of characters.
    Stars,
    /// `?`: one character but `/`.
    One,
    /// `{a,b}` or `{1..9}`: one of these.
    Either(Vec<String>),
}

/// A ClickHouse key pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    tokens: Vec<Token>,
    /// The part before the first glob character.
    literal: String,
}

impl Glob {
    /// `pattern` read as ClickHouse reads an `s3()` path.
    pub fn parse(pattern: &str) -> Result<Self, HouseError> {
        let mut tokens = Vec::new();
        let mut literal = String::new();
        let mut globbed = false;
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '*' if chars.get(i + 1) == Some(&'*') => {
                    tokens.push(Token::Stars);
                    i += 2;
                    globbed = true;
                    continue;
                }
                '*' => tokens.push(Token::Star),
                '?' => tokens.push(Token::One),
                '{' => {
                    let close = chars[i..]
                        .iter()
                        .position(|&c| c == '}')
                        .map(|p| p + i)
                        .ok_or_else(|| bad(format!("{pattern:?} opens a {{ it never closes")))?;
                    let body: String = chars[i + 1..close].iter().collect();
                    if body.contains('{') {
                        return Err(bad(format!("{pattern:?} nests braces")));
                    }
                    tokens.push(Token::Either(alternatives(&body, pattern)?));
                    i = close + 1;
                    globbed = true;
                    continue;
                }
                '}' => return Err(bad(format!("{pattern:?} closes a }} it never opened"))),
                c => {
                    tokens.push(Token::Char(c));
                    if !globbed {
                        literal.push(c);
                    }
                }
            }
            if c == '*' || c == '?' {
                globbed = true;
            }
            i += 1;
        }
        Ok(Self { tokens, literal })
    }

    /// The directory a listing starts from: the literal part up to its last
    /// `/`, with that `/`.
    pub fn directory(&self) -> &str {
        match self.literal.rfind('/') {
            Some(at) => &self.literal[..=at],
            None => "",
        }
    }

    /// Whether `key` matches.
    pub fn matches(&self, key: &str) -> bool {
        let key: Vec<char> = key.chars().collect();
        matches_from(&self.tokens, &key)
    }
}

fn alternatives(body: &str, pattern: &str) -> Result<Vec<String>, HouseError> {
    if let Some((from, to)) = body.split_once("..")
        && let (Ok(from), Ok(to)) = (from.parse::<u64>(), to.parse::<u64>())
    {
        let (low, high) = (from.min(to), from.max(to));
        if high - low > 10_000 {
            return Err(bad(format!(
                "{pattern:?} names a range of over 10 000 numbers"
            )));
        }
        return Ok((low..=high).map(|n| n.to_string()).collect());
    }
    let parts: Vec<String> = body.split(',').map(str::to_string).collect();
    if parts
        .iter()
        .any(|p| p.contains(['*', '?']) || p.contains(".."))
    {
        return Err(bad(format!(
            "{pattern:?}: a {{…}} holds literal alternatives or a range of numbers"
        )));
    }
    Ok(parts)
}

fn matches_from(tokens: &[Token], key: &[char]) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        return key.is_empty();
    };
    match first {
        Token::Char(c) => key.first() == Some(c) && matches_from(rest, &key[1..]),
        Token::One => key.first().is_some_and(|&c| c != '/') && matches_from(rest, &key[1..]),
        Token::Star => {
            for n in 0..=key.len() {
                if matches_from(rest, &key[n..]) {
                    return true;
                }
                if key.get(n) == Some(&'/') {
                    break;
                }
            }
            false
        }
        Token::Stars => (0..=key.len()).any(|n| matches_from(rest, &key[n..])),
        Token::Either(options) => options.iter().any(|option| {
            let option: Vec<char> = option.chars().collect();
            key.starts_with(&option) && matches_from(rest, &key[option.len()..])
        }),
    }
}

// ---------------------------------------------------------------------------
// The batch.

/// The view that makes `pipe`'s name read `keys` (store-relative, all
/// [`readable`]) through the worker's `endpoint`.
pub fn batch(
    pipe: &Pipe,
    source: &QueueSource,
    keys: &[String],
    endpoint: &str,
    root: &LakeRoot,
) -> Result<PipeBatch, HouseError> {
    let names = match keys {
        [] => return Err(bad("an S3Queue batch names at least one object")),
        [one] => one.clone(),
        many => format!("{{{}}}", many.join(",")),
    };
    let base = format!("{}/{}", endpoint.trim_end_matches('/'), root.bucket());
    let url = format!("{base}/{names}");
    PipeBatch::over(pipe, &url, &source.format)
}

/// The cursor key of `view`.
pub fn cursor_key(view: &MaterializedView) -> String {
    format!("{QUEUE_CURSOR_PREFIX}{}", view.id)
}

/// The keys one run takes, in key order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Keys read by name.
    pub keys: Vec<String>,
    /// Keys that cannot be named in a read; the cursor moves past them.
    pub skipped: Vec<String>,
    /// The greatest of both.
    pub last: Option<String>,
    /// Matching keys past the batch, as far as the run listed.
    pub lag: u64,
}

/// The matching keys after `cursor`, a batch's worth.
pub async fn list(
    store: &loams_store::Store,
    namespace: u64,
    source: &QueueSource,
    cursor: Option<&str>,
) -> Result<Listed, HouseError> {
    check_path(&source.path, namespace)?;
    let glob = Glob::parse(&source.path)?;
    let directory = glob.directory().to_string();
    let house = format!("ns/{namespace}/house/");
    let mut out = Listed::default();
    let mut bytes = 0u64;
    let mut full = false;
    let mut scanned_past = 0usize;
    let mut after = cursor.map(str::to_string);
    loop {
        let page = store
            .list_page(directory.trim_end_matches('/'), after.as_deref(), LIST_PAGE)
            .await
            .map_err(|_| {
                HouseError::from(ChError::network_error(
                    "the store failed to list an S3Queue's objects",
                ))
            })?;
        for info in &page {
            let key = &info.path;
            if !key.starts_with(directory.as_str())
                || key.starts_with(house.as_str())
                || !glob.matches(key)
            {
                continue;
            }
            if !full {
                let taken = out.keys.len() + out.skipped.len();
                let over = taken > 0
                    && (taken as u64 >= source.max_files
                        || bytes.saturating_add(info.size) > source.max_bytes);
                if over {
                    full = true;
                } else {
                    if readable(key) {
                        out.keys.push(key.clone());
                        bytes += info.size;
                    } else {
                        out.skipped.push(key.clone());
                    }
                    out.last = Some(key.clone());
                    continue;
                }
            }
            out.lag += 1;
        }
        if full {
            scanned_past += page.len();
        }
        if page.len() < LIST_PAGE || scanned_past >= LAG_SCAN {
            break;
        }
        after = page.last().map(|info| info.path.clone());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The runner.

/// The `S3Queue` pipes' runner, in every front with a store (see the module
/// docs). It needs no engine link.
#[derive(Debug, Clone)]
pub struct QueueRunner {
    lake: Arc<Lake>,
    writer: ViewWriter,
    hooks: Option<Arc<dyn Hooks>>,
    known: Arc<Mutex<Vec<(u64, String, String)>>>,
}

impl QueueRunner {
    /// A runner over `lake`'s `S3Queue` pipes.
    pub fn new(lake: Arc<Lake>, pool: WorkerPool, config: HouseConfig) -> Self {
        Self {
            writer: ViewWriter {
                lake: Arc::clone(&lake),
                pool,
                limits: config.session_limits,
                max_insert_bytes: config.lake_insert_max_bytes,
            },
            lake,
            hooks: None,
            known: Arc::default(),
        }
    }

    /// With a test's hooks.
    pub fn with_hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Runs every `every` until the returned task is aborted.
    pub fn spawn(self, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut round: u64 = 0;
            loop {
                tick.tick().await;
                if round.is_multiple_of(REFRESH_EVERY) {
                    self.refresh().await;
                }
                self.round().await;
                round += 1;
            }
        })
    }

    /// Lists the namespaces' `S3Queue` pipes (every `REFRESH_EVERY` rounds:
    /// a pipe made on another front is found within that).
    pub async fn refresh(&self) {
        let namespaces = match self.lake.namespaces().await {
            Ok(namespaces) => namespaces,
            Err(err) => {
                eprintln!(
                    "loams-house: S3Queue pipes cannot list namespaces: {}",
                    err.message()
                );
                return;
            }
        };
        let mut known = Vec::new();
        for namespace in namespaces {
            match super::pipes(&self.lake, namespace).await {
                Ok(pipes) => known.extend(
                    pipes
                        .into_iter()
                        .filter(|p| matches!(p.source, PipeFeed::Queue(_)))
                        .map(|p| (namespace, p.database, p.name)),
                ),
                Err(err) => eprintln!(
                    "loams-house: S3Queue pipes of namespace {namespace} cannot be listed: {}",
                    err.message()
                ),
            }
        }
        *self.known.lock().unwrap_or_else(|e| e.into_inner()) = known;
    }

    /// One round: every view of every known `S3Queue` pipe, once.
    pub async fn round(&self) {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (namespace, database, name) in known {
            let table = match self.lake.load(namespace, &database, &name).await {
                Ok(Some(table)) => table,
                Ok(None) => continue,
                Err(err) => {
                    eprintln!(
                        "loams-house: S3Queue pipe {database}.{name} cannot be loaded: {}",
                        err.message()
                    );
                    continue;
                }
            };
            let Some(pipe) = Pipe::from_table(&database, &name, table) else {
                continue;
            };
            for view in &pipe.views {
                let _ = self.run(namespace, &pipe, view).await;
            }
        }
    }

    async fn hook(&self, step: Step, view: &MaterializedView) -> Result<(), HouseError> {
        match &self.hooks {
            Some(hooks) => hooks.at(step, view).await,
            None => Ok(()),
        }
    }

    /// One run of `view`, its state recorded. `records` are objects.
    pub async fn run(
        &self,
        namespace: u64,
        pipe: &Pipe,
        view: &MaterializedView,
    ) -> Result<Ran, HouseError> {
        let result = self.run_inner(namespace, pipe, view).await;
        self.lake
            .pipe_states()
            .update(namespace, view, |state| match &result {
                Ok((ran, skipped)) => {
                    if let Ran::Committed { records, rows } = ran {
                        state.last_commit_ms = Some(super::now_ms());
                        state.committed_records += records;
                        state.committed_rows += rows;
                    }
                    state.last_error = match skipped.as_slice() {
                        [] => String::new(),
                        keys => format!(
                            "{} object(s) skipped: their keys hold characters a read cannot \
                             name (letters, digits and . _ - / = + only), first {:?}",
                            keys.len(),
                            keys[0]
                        ),
                    };
                }
                Err(err) => state.last_error = err.message().to_string(),
            });
        result.map(|(ran, _)| ran)
    }

    async fn run_inner(
        &self,
        namespace: u64,
        pipe: &Pipe,
        view: &MaterializedView,
    ) -> Result<(Ran, Vec<String>), HouseError> {
        let PipeFeed::Queue(source) = &pipe.source else {
            return Ok((Ran::Idle, Vec::new()));
        };
        let lake = &self.lake;
        let target = lake
            .lake_table(
                namespace,
                &view.target_database,
                &view.target,
                "a materialized view's target",
            )
            .await?;
        let key = cursor_key(view);
        let cursor = target.metadata.properties().get(&key).cloned();
        let committer = lake.committer();
        let io = committer.io();
        let listed = list(io.store()?, namespace, source, cursor.as_deref()).await?;
        lake.pipe_states()
            .update(namespace, view, |state| state.lag_records = listed.lag);
        let Some(last) = listed.last.clone() else {
            return Ok((Ran::Idle, Vec::new()));
        };
        self.hook(Step::Fetched, view).await?;

        let mut files = Vec::new();
        let mut rows = 0;
        let mut bytes = 0;
        if !listed.keys.is_empty() {
            let root = lake.root().clone();
            let make = |endpoint: &str| batch(pipe, source, &listed.keys, endpoint, &root);
            let written = self
                .writer
                .write(namespace, view, &target, Some(&make))
                .await?;
            files = written.data_files;
            rows = written.rows;
            bytes = written.input_bytes;
            self.hook(Step::Written, view).await?;
        }
        self.hook(Step::BeforeCommit, view).await?;
        let read: BTreeMap<String, String> =
            cursor.iter().map(|c| (key.clone(), c.clone())).collect();
        let next = BTreeMap::from([(key.clone(), last.clone())]);
        let mut summary = vec![
            (key.clone(), last),
            (
                format!("{key}.files"),
                serde_json::to_string(&listed.keys).map_err(|e| bad(e.to_string()))?,
            ),
        ];
        if !listed.skipped.is_empty() {
            summary.push((
                format!("{key}.skipped"),
                serde_json::to_string(&listed.skipped).map_err(|e| bad(e.to_string()))?,
            ));
        }
        let outcome = commit_cursor(io, &target.ident, &read, &next, summary, files).await?;
        if let Ran::Committed { .. } = outcome {
            self.writer.report_insert(namespace, view, rows, bytes);
            self.hook(Step::Committed, view).await?;
            committer.mark_dirty(&target.ident);
            // The target's own views (HS1 Task 17); their errors are theirs.
            let views = crate::mv::ViewRunner::new(self.writer.clone());
            let _ = views
                .after_commit(namespace, &view.target_database, &view.target)
                .await;
            if source.after_processing == AfterProcessing::Delete {
                let store = io.store()?;
                for object in &listed.keys {
                    let _ = store.delete(object).await;
                }
            }
            return Ok((
                Ran::Committed {
                    records: (listed.keys.len() + listed.skipped.len()) as u64,
                    rows,
                },
                listed.skipped,
            ));
        }
        Ok((outcome, Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_as_clickhouse_reads_them() {
        let g = Glob::parse("ns/2/in/*.jsonl").expect("glob");
        assert_eq!(g.directory(), "ns/2/in/");
        assert!(g.matches("ns/2/in/a.jsonl"));
        assert!(!g.matches("ns/2/in/sub/a.jsonl"));
        assert!(!g.matches("ns/2/in/a.csv"));
        let g = Glob::parse("ns/2/in/**/*.csv").expect("glob");
        assert!(g.matches("ns/2/in/x/y/a.csv"));
        let g = Glob::parse("ns/2/in/{a,b}-?.csv").expect("glob");
        assert!(g.matches("ns/2/in/b-1.csv"));
        assert!(!g.matches("ns/2/in/c-1.csv"));
        assert!(!g.matches("ns/2/in/a-12.csv"));
        let g = Glob::parse("ns/2/day{1..3}/x").expect("glob");
        assert_eq!(g.directory(), "ns/2/");
        assert!(g.matches("ns/2/day3/x"));
        assert!(!g.matches("ns/2/day4/x"));
        assert!(Glob::parse("ns/2/{a").is_err());
        assert!(Glob::parse("ns/2/{a,{b}}").is_err());
        assert!(Glob::parse("ns/2/a}").is_err());
    }

    #[test]
    fn paths_outside_the_namespace_are_344() {
        for path in [
            "s3://other/ns/2/x.csv",
            "https://example.com/ns/2/x.csv",
            "/ns/2/x.csv",
            "ns/3/x.csv",
            "ns/20/x.csv",
            "ns/2/../3/x.csv",
            "ns/2/a/../../3/x",
            "ns/2/house/default/t/data/*.parquet",
            "ns/2/",
            "ns/2//x",
            "ns/2/./x",
            "ns/*/x.csv",
            "in/*.csv",
            "ns/2/a\\b",
        ] {
            assert_eq!(check_path(path, 2).expect_err(path).code(), 344, "{path}");
        }
        check_path("ns/2/in/*.csv", 2).expect("own prefix");
        check_path("ns/2/day{1..3}/*.csv", 2).expect("a range is no parent");
        assert_eq!(
            check_path("ns/2/{..,a}/x", 2).expect_err("parent").code(),
            344
        );
        check_path("ns/2/housekeeping/*.csv", 2).expect("not the House's tables");
    }

    #[test]
    fn arguments_and_settings() {
        let args = |p: &str, f: &str| vec![format!("'{p}'"), format!("'{f}'")];
        let s = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let source = QueueSource::parse(
            2,
            &args("ns/2/in/*.csv", "CSV"),
            &s(&[("mode", "ordered"), ("s3queue_after_processing", "delete")]),
        )
        .expect("source");
        assert_eq!(source.after_processing, AfterProcessing::Delete);
        assert_eq!(source.max_files, DEFAULT_MAX_FILES);
        assert_eq!(source.render(), "S3Queue('ns/2/in/*.csv', 'CSV')");
        let code = |args: Vec<String>, settings: Vec<(String, String)>| {
            QueueSource::parse(2, &args, &settings)
                .expect_err("refused")
                .code()
        };
        assert_eq!(code(args("ns/2/in/*", "Native"), s(&[])), 73);
        assert_eq!(
            code(args("ns/2/in/*", "CSV"), s(&[("mode", "unordered")])),
            48
        );
        assert_eq!(
            code(args("ns/2/in/*", "CSV"), s(&[("keeper_path", "/x")])),
            115
        );
        assert_eq!(
            code(
                args("ns/2/in/*", "CSV"),
                s(&[("max_processed_files_before_commit", "0")])
            ),
            36
        );
        assert_eq!(code(args("ns/3/in/*", "CSV"), s(&[])), 344);
        let mut with_keys = args("ns/2/in/*", "CSV");
        with_keys.insert(1, "'AKIA'".to_string());
        with_keys.insert(2, "'secret'".to_string());
        assert_eq!(code(with_keys, s(&[])), 344);
        assert!(readable("ns/2/in/day=1/a-b_c.v1+x.csv"));
        assert!(!readable("ns/2/in/a,b.csv"));
        assert!(!readable("ns/2/in/a b.csv"));
    }

    #[tokio::test]
    async fn listing_takes_keys_after_the_cursor_in_order() {
        let store = loams_store::Store::from_url("memory://", Vec::<(String, String)>::new())
            .expect("store");
        for key in [
            "ns/2/in/a.csv",
            "ns/2/in/b.csv",
            "ns/2/in/b.txt",
            "ns/2/in/c d.csv",
            "ns/2/in/e.csv",
            "ns/2/house/t.csv",
            "ns/3/in/a.csv",
        ] {
            store
                .put(key, bytes::Bytes::from_static(b"1\n"))
                .await
                .expect("put");
        }
        let mut source = QueueSource::parse(
            2,
            &["'ns/2/in/*.csv'".to_string(), "'CSV'".to_string()],
            &[],
        )
        .expect("source");
        source.max_files = 2;
        let first = list(&store, 2, &source, None).await.expect("list");
        assert_eq!(first.keys, vec!["ns/2/in/a.csv", "ns/2/in/b.csv"]);
        assert_eq!(first.lag, 2);
        let second = list(&store, 2, &source, first.last.as_deref())
            .await
            .expect("list");
        assert_eq!(second.keys, vec!["ns/2/in/e.csv"]);
        assert_eq!(second.skipped, vec!["ns/2/in/c d.csv"]);
        assert_eq!(second.last.as_deref(), Some("ns/2/in/e.csv"));
        let done = list(&store, 2, &source, second.last.as_deref())
            .await
            .expect("list");
        assert_eq!(done, Listed::default());
        // A pattern over the whole prefix never reaches the House's tables.
        let all = QueueSource::parse(2, &["'ns/2/**'".to_string(), "'CSV'".to_string()], &[])
            .expect("source");
        let listed = list(&store, 2, &all, None).await.expect("list");
        assert!(listed.keys.iter().all(|k| !k.starts_with("ns/2/house/")));
        assert_eq!(listed.keys.len() + listed.skipped.len(), 5);
    }
}
