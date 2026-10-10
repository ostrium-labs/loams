//! The deny list: L1 of design §49 §13.2 (HS1 Task 5; §32 §7.8, FL2 Task 8).
//!
//! Every statement chDB runs is first analysed by its worker (`hsw1` `Analyze`):
//! ClickHouse's own parser explains its syntax tree, and [`check`] walks every
//! node of it. A table function, an engine or a function the House does not allow
//! is refused with `344 SUPPORT_IS_DISABLED` wherever it stands — in a subquery, a
//! `view()`, a CTE, a `JOIN`, an `IN`, an `INSERT … SELECT`, a `CREATE … AS
//! SELECT`, a `WITH` — and a denied setting named in a `SETTINGS` clause is `164`,
//! as the front answers for a URL setting or a `SET`.
//!
//! # Which tree
//!
//! The design names `EXPLAIN QUERY TREE`, chDB's analysed tree. Resolving it
//! opens what it names: `EXPLAIN QUERY TREE SELECT * FROM url('http://…')`
//! connects to infer the schema (measured, Task 5), so the check would itself be
//! the SSRF it guards against. The worker therefore explains **syntax only**:
//!
//! * `EXPLAIN AST` for every statement (it is the only explain that takes
//!   `INSERT`, `CREATE` and the rest), and
//! * `EXPLAIN QUERY TREE run_passes = 0` for a query: the query tree built and not
//!   resolved, which also names the settings of every `SETTINGS` clause (the AST
//!   prints them as a bare `Set`).
//!
//! Both trees are checked; a node is refused if either shows it. What neither
//! shows is left to L2 (the worker's grants and pinned settings) and L3 (the OS
//! sandbox): the expressions inside `INSERT … VALUES` data (L2 turns their
//! evaluation off) and the settings of a non-query's `SETTINGS` clause (L2 pins
//! every denied one, `452`).
//!
//! # Allow-lists
//!
//! Deny by default (HS1 Global Constraints): table functions are allowed by name
//! ([`ALLOWED_TABLE_FUNCTIONS`]), temporary tables by engine
//! ([`ALLOWED_TEMPORARY_ENGINES`]) and `system` tables by name
//! ([`ALLOWED_SYSTEM_TABLES`]). The `DENIED_*` lists name what is refused, each
//! entry with a test answering `344`, and a test fails when chDB grows a table
//! function that is on neither list.
//!
//! # Host functions
//!
//! [`HOST_FUNCTIONS`] would describe the host. Those with a Loams value
//! (`hostName()`, `displayName()`, `serverTimezone()`, `currentUser()`) are
//! rewritten in the text before it reaches a worker ([`rewrite_host_functions`]);
//! any call the rewrite did not take (another spelling, an argument, a quoted name)
//! is then still in the tree and refused, as are those with no Loams value.

use std::ops::Range;

use crate::classify::{Lexeme, lex};
use crate::errors::{ChError, HouseError};

/// Table functions a statement may use (§32 §7.8), compared without case.
/// `SQLStandardValues` is what `FROM (VALUES …)` parses to; the generators read
/// nothing. `merge` is allowed over the session's own database only.
pub const ALLOWED_TABLE_FUNCTIONS: &[&str] = &[
    "numbers",
    "numbers_mt",
    "zeros",
    "zeros_mt",
    "values",
    "SQLStandardValues",
    "generateRandom",
    "generate_series",
    "generateSeries",
    "primes",
    "format",
    "null",
    "view",
    "merge",
];

/// Table functions allowed only inside an `INSERT` (§32 §7.8: `input` outside
/// `INSERT` is denied).
pub const INSERT_ONLY_TABLE_FUNCTIONS: &[&str] = &["input"];

/// The table functions chDB 26.9 has that the House refuses (`344`): every one
/// reaches the host's files, the network, another process, or storage the House
/// does not own. The worker reads the bucket with `icebergS3` in views the front
/// writes, never in user text.
pub const DENIED_TABLE_FUNCTIONS: &[&str] = &[
    "file",
    "fileCluster",
    "filesystem",
    "url",
    "urlCluster",
    "remote",
    "remoteSecure",
    "cluster",
    "clusterAllReplicas",
    "mysql",
    "postgresql",
    "odbc",
    "jdbc",
    "sqlite",
    "mongodb",
    "redis",
    "hdfs",
    "hdfsCluster",
    "hive",
    "s3",
    "s3Cluster",
    "gcs",
    "oss",
    "cosn",
    "azureBlobStorage",
    "azureBlobStorageCluster",
    "iceberg",
    "icebergS3",
    "icebergAzure",
    "icebergHDFS",
    "icebergLocal",
    "icebergCluster",
    "icebergS3Cluster",
    "icebergAzureCluster",
    "icebergHDFSCluster",
    "icebergLocalCluster",
    "deltaLake",
    "deltaLakeS3",
    "deltaLakeAzure",
    "deltaLakeLocal",
    "deltaLakeCluster",
    "deltaLakeS3Cluster",
    "deltaLakeAzureCluster",
    "hudi",
    "hudiCluster",
    "paimon",
    "paimonS3",
    "paimonAzure",
    "paimonHDFS",
    "paimonLocal",
    "paimonCluster",
    "paimonS3Cluster",
    "paimonAzureCluster",
    "paimonHDFSCluster",
    "executable",
    "dictionary",
    "ytsaurus",
    "arrowFlight",
    "arrowflight",
    "prometheusQuery",
    "prometheusQueryRange",
    "timeSeriesData",
    "timeSeriesMetrics",
    "timeSeriesSamples",
    "timeSeriesSelector",
    "timeSeriesTags",
    "mergeTreeIndex",
    "mergeTreeProjection",
    "mergeTreeAnalyzeIndexes",
    "mergeTreeAnalyzeIndexesUUID",
    "mergeTreeTextIndex",
    "loop",
    "viewIfPermitted",
    "viewExplain",
    "eval",
    "fuzzJSON",
    "fuzzQuery",
    // New in chDB 26.9 (`the_lists_follow_chdb`).
    "timeSeriesMetricFamilies",
    "mergeTreeCodecBlockCounts",
    "bigquery",
    "arrowstream",
];

/// Engines a session-local temporary table may have (HS1 Global Constraints: no
/// MergeTree inside chDB, only `Memory` and `Null` temporary tables).
pub const ALLOWED_TEMPORARY_ENGINES: &[&str] = &["Memory", "Null"];

/// Engines no `CREATE` may name (`344`), temporary or not: they read or write the
/// host, the network or another system (§32 §7.8). A lake table's Tier 1 engine
/// (`ReplacingMergeTree` …) is not here: the front maps it onto Iceberg (HS1 Task
/// 10) and chDB never sees it. A pipe's `S3Queue` is the front's own (HS1 Task
/// 18) and is refused only inside chDB, where no engine outside
/// [`ALLOWED_TEMPORARY_ENGINES`] is allowed.
pub const DENIED_ENGINES: &[&str] = &[
    "URL",
    "File",
    "FileLog",
    "S3",
    "S3Queue",
    "AzureBlobStorage",
    "AzureQueue",
    "HDFS",
    "Hive",
    "MySQL",
    "MaterializedMySQL",
    "PostgreSQL",
    "MaterializedPostgreSQL",
    "MongoDB",
    "Redis",
    "SQLite",
    "ODBC",
    "JDBC",
    "Executable",
    "ExecutablePool",
    "Kafka",
    "RabbitMQ",
    "NATS",
    "Distributed",
    "Dictionary",
    "Iceberg",
    "IcebergS3",
    "IcebergAzure",
    "IcebergHDFS",
    "IcebergLocal",
    "DeltaLake",
    "DeltaLakeAzure",
    "DeltaLakeLocal",
    "Hudi",
    "EmbeddedRocksDB",
    "KeeperMap",
    "YTsaurus",
    "ArrowFlight",
    // Fix round 1: another table's data under a new name (`Merge` over
    // `system`, `Alias`, `Buffer`'s destination), object stores under other
    // names, and the rest chDB 26.9 has (`the_engine_lists_follow_chdb`).
    // `ExternalDistributed` is ClickHouse's, not in chDB 26.9: listed for the
    // next version.
    "Merge",
    "Buffer",
    "Alias",
    "ExternalDistributed",
    "COSN",
    "OSS",
    "GCS",
    "DeltaLakeS3",
    "TimeSeries",
    "Remote",
    "RemoteSecure",
    "ArrowStream",
    "BigQuery",
    "Paimon",
    "PaimonS3",
    "PaimonAzure",
    "PaimonHDFS",
    "PaimonLocal",
    "QueryRunner",
    "Loop",
    "FuzzJSON",
    "FuzzQuery",
];

/// The engines chDB has that are not on [`DENIED_ENGINES`], reviewed (fix round
/// 1): a `CREATE` may name them, and `the_engine_lists_follow_chdb` fails when a
/// version adds an engine on neither list. In chDB a temporary table is still
/// held to [`ALLOWED_TEMPORARY_ENGINES`]; a lake table's engine (the MergeTree
/// family) is the front's, mapped onto Iceberg (HS1 Task 10); views are the
/// front's own (HS1 Tasks 11 and 17); the rest keep their data in the session.
pub const ALLOWED_ENGINES: &[&str] = &[
    "Memory",
    "Null",
    "View",
    "MaterializedView",
    "MergeTree",
    "ReplacingMergeTree",
    "SummingMergeTree",
    "AggregatingMergeTree",
    "CollapsingMergeTree",
    "VersionedCollapsingMergeTree",
    "CoalescingMergeTree",
    "GraphiteMergeTree",
    "ReplicatedMergeTree",
    "ReplicatedReplacingMergeTree",
    "ReplicatedSummingMergeTree",
    "ReplicatedAggregatingMergeTree",
    "ReplicatedCollapsingMergeTree",
    "ReplicatedVersionedCollapsingMergeTree",
    "ReplicatedCoalescingMergeTree",
    "ReplicatedGraphiteMergeTree",
    "Log",
    "TinyLog",
    "StripeLog",
    "Set",
    "Join",
    "GenerateRandom",
];

/// Functions no statement may call (`344`): they read a file (`file`, a model's
/// path), reach another server (`hasColumnInTable` with a host), read the
/// process's memory (introspection, also pinned off in L2), or read the request's
/// own headers, which carry the credentials.
pub const DENIED_FUNCTIONS: &[&str] = &[
    "file",
    "catboostEvaluate",
    "naiveBayesClassifier",
    "hasColumnInTable",
    "addressToLine",
    "addressToLineWithInlines",
    "addressToSymbol",
    "demangle",
    "getClientHTTPHeader",
];

/// Clauses no statement may carry (`344`, HS1 R1.10): chDB's client layer writes
/// and reads the host's files for them whatever the grants say. Found by
/// `classify::check_text` with ClickHouse's own lexing.
pub const DENIED_CLAUSES: &[(&str, &str)] = &[("INTO", "OUTFILE"), ("FROM", "INFILE")];

/// Statements refused outright (`344`, §49 §13.2), by the classifier before any
/// worker is asked, and again by their AST labels in [`check`]. `CREATE
/// DICTIONARY` is refused whatever its source: no source is a Loams one yet.
pub const DENIED_STATEMENTS: &[&str] = &[
    "SYSTEM",
    "ATTACH",
    "BACKUP",
    "RESTORE",
    "CREATE FUNCTION",
    "CREATE DICTIONARY",
    "CREATE NAMED COLLECTION",
];

/// The AST labels of [`DENIED_STATEMENTS`], as chDB's `EXPLAIN AST` prints them.
const DENIED_AST_LABELS: &[(&str, &str)] = &[
    ("SYSTEM query", "SYSTEM"),
    ("AttachQuery", "ATTACH"),
    ("BackupQuery", "BACKUP"),
    ("RestoreQuery", "RESTORE"),
    ("CreateSQLFunctionQuery", "CREATE FUNCTION"),
    ("CreateFunctionQuery", "CREATE FUNCTION"),
    ("Dictionary definition", "CREATE DICTIONARY"),
    ("CreateNamedCollectionQuery", "CREATE NAMED COLLECTION"),
    ("AlterNamedCollectionQuery", "ALTER NAMED COLLECTION"),
];

/// Settings no request, `SET` or `SETTINGS` clause may change (`164`): they name
/// a file, a directory, a schema source or a URL; switch the evaluation of
/// `VALUES` expressions back on; or pick an engine other than `Memory` for tables
/// created without one. A trailing `*` is a prefix. The worker's profile pins
/// every one chDB has (`452` for a `SETTINGS` clause the front cannot see), and a
/// test keeps the two lists together.
pub const DENIED_SETTINGS: &[&str] = &[
    "format_schema*",
    "user_files_path",
    "output_format_schema",
    "input_format_record_errors_file_path",
    "format_template_resultset",
    "format_template_row",
    "format_avro_schema_registry_url",
    "rename_files_after_processing",
    "s3queue_default_zookeeper_path",
    "input_format_values_interpret_expressions",
    "input_format_values_deduce_templates_of_expressions",
    "into_outfile_create_parent_directories",
    "default_temporary_table_engine",
    "default_table_engine",
    "allow_introspection_functions",
    // Fix round 1 (`path_like_settings_are_denied`): the request's headers,
    // which carry the credentials; secrets in `SHOW` and `SELECT`; the scripts
    // directory (a server setting: refused, never unknown); every switch the
    // worker pins (a pinned setting is a denied one); and the switches that
    // open the host, the network or another dialect.
    "allow_get_client_http_header",
    "format_display_secrets_in_show_and_select",
    "user_scripts_path",
    "url_base",
    "allow_insert_into_iceberg",
    "allow_experimental_insert_into_iceberg",
    "allow_experimental_iceberg_compaction",
    "allow_iceberg_remove_orphan_files",
    "allow_experimental_expire_snapshots",
    "allow_experimental_cleanup_old_data_files_compaction",
    "allow_custom_error_code_in_throwif",
    "allow_experimental_ai_functions",
    "ai_function_allow_insecure_endpoint",
    "allow_experimental_eval_table_function",
    "allow_python_table_function",
    "allow_fuzz_query_functions",
    "allow_unrestricted_reads_from_keeper",
    "allow_named_collection_override_by_default",
    "allow_distributed_ddl",
    "allow_experimental_kusto_dialect",
    "allow_experimental_prql_dialect",
    "allow_experimental_polyglot_dialect",
    "jemalloc_enable_profiler",
    "jemalloc_collect_profile_samples_in_trace_log",
];

/// What a host function answers on the House.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// The House's display name (`X-ClickHouse-Server-Display-Name`).
    DisplayName,
    /// `UTC`, the House's timezone (`X-ClickHouse-Timezone`).
    Timezone,
    /// The House user the request authenticated as.
    User,
    /// Nothing: the call is refused (`344`).
    Disabled,
}

/// A function that would describe the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFunction {
    /// The name, as chDB registers it (an alias is its own entry).
    pub name: &'static str,
    /// Whether chDB takes the name in any case (`system.functions`'
    /// `case_insensitive`, measured).
    pub case_insensitive: bool,
    /// What the House answers.
    pub answer: Host,
}

const fn host(name: &'static str, case_insensitive: bool, answer: Host) -> HostFunction {
    HostFunction {
        name,
        case_insensitive,
        answer,
    }
}

/// The host functions (§32 §8.6, §49 §13.1): rewritten to a Loams value, or `344`.
pub const HOST_FUNCTIONS: &[HostFunction] = &[
    host("hostName", false, Host::DisplayName),
    host("hostname", false, Host::DisplayName),
    host("FQDN", true, Host::DisplayName),
    host("fullHostName", false, Host::DisplayName),
    host("displayName", false, Host::DisplayName),
    host("serverTimezone", false, Host::Timezone),
    host("serverTimeZone", false, Host::Timezone),
    host("currentUser", false, Host::User),
    host("user", true, Host::User),
    host("current_user", true, Host::User),
    host("session_user", true, Host::User),
    host("authenticatedUser", false, Host::User),
    host("uptime", false, Host::Disabled),
    host("logTrace", false, Host::Disabled),
    host("getMacro", false, Host::Disabled),
    host("filesystemAvailable", false, Host::Disabled),
    host("filesystemCapacity", false, Host::Disabled),
    host("filesystemUnreserved", false, Host::Disabled),
    host("getServerSetting", false, Host::Disabled),
    host("getMergeTreeSetting", false, Host::Disabled),
    host("getServerPort", false, Host::Disabled),
    host("tcpPort", false, Host::Disabled),
    host("serverUUID", false, Host::Disabled),
    host("getOSKernelVersion", false, Host::Disabled),
    host("showCertificate", false, Host::Disabled),
    host("zookeeperSessionUptime", false, Host::Disabled),
    host("currentProfiles", false, Host::Disabled),
    host("enabledProfiles", false, Host::Disabled),
    host("defaultProfiles", false, Host::Disabled),
    host("currentRoles", false, Host::Disabled),
    host("enabledRoles", false, Host::Disabled),
    host("defaultRoles", false, Host::Disabled),
];

/// The `system` tables a statement may read: catalogs of what the engine can do,
/// the namespace's own objects, and generators. The rest describe the host
/// (`disks`, `server_settings`, `asynchronous_metrics`, `macros`,
/// `user_directories`, `certificates`, `stack_trace`, …) or the engine's
/// internals, and are `344` (§49 §13.1). `databases`, `tables`, `columns` and
/// `parts` become Loams's own in HS1 Task 11. `information_schema` is allowed
/// whole: its views name only these.
pub const ALLOWED_SYSTEM_TABLES: &[&str] = &[
    "one",
    "numbers",
    "numbers_mt",
    "zeros",
    "zeros_mt",
    "databases",
    "tables",
    "columns",
    "data_skipping_indices",
    "parts",
    "parts_columns",
    "settings",
    "settings_changes",
    "functions",
    "formats",
    "data_type_families",
    "table_engines",
    "table_functions",
    "database_engines",
    "aggregate_function_combinators",
    "collations",
    "time_zones",
    "keywords",
    "build_options",
    "contributors",
    "licenses",
    "processes",
    "dictionaries",
    "errors",
    "events",
    "metrics",
];

/// The database a session reads, which `merge()` may name: `default` until HS1
/// Task 10 maps databases onto the namespace's catalog.
pub const OWN_DATABASE: &str = "default";

/// The statements an `EXPLAIN AST` may open with (fix round 1): what the surface
/// sends to chDB. The first line of the explain must be one of these, as chDB
/// prints it in TSV, or nothing in the output is trusted: a `FORMAT` the front
/// did not strip would print JSON, CSV or one raw line instead, and the check
/// would read none of it. `SHOW` forms are [`ALLOWED_SHOW_QUERIES`].
pub const STATEMENT_ROOTS: &[&str] = &[
    "SelectWithUnionQuery",
    "InsertQuery",
    "CreateQuery",
    "Explain",
    "DescribeQuery",
    "ExistsTableQuery",
    "ExistsDatabaseQuery",
    "ExistsViewQuery",
    "ExistsDictionaryQuery",
    "DropQuery",
];

/// The `SHOW` forms a statement may use, by the label chDB's `EXPLAIN AST` gives
/// them (fix round 1, measured on chDB 26.9). Each is rewritten by the engine
/// into a read of a `system` table, which no explain shows, so the form itself is
/// held to what [`ALLOWED_SYSTEM_TABLES`] allows: tables, databases, columns and
/// indexes, `CREATE` of a table, database or view, settings, engines, functions
/// and the process list. The access forms (`ACCESS`, `GRANTS`, `USERS`, `ROLES`,
/// `PROFILES`, `QUOTAS`, `POLICIES`, `PRIVILEGES`, `CREATE USER` …) and `SHOW
/// CREATE DICTIONARY` are `344`. `ShowTables` also covers `CLUSTERS`,
/// `FILESYSTEM CACHES`, `MERGES` and `DICTIONARIES`: its form is read from the
/// text ([`ALLOWED_SHOW_LISTS`]).
pub const ALLOWED_SHOW_QUERIES: &[&str] = &[
    "ShowTables",
    "ShowColumns",
    "ShowIndexes",
    "ShowCreateTableQuery",
    "ShowCreateDatabaseQuery",
    "ShowCreateViewQuery",
    "ShowSetting",
    "ShowEngineQuery",
    "ShowFunctions",
    "ShowProcesslistQuery",
];

/// What a `ShowTables` statement may list: the word after `SHOW` (and `FULL`,
/// `EXTENDED`, `TEMPORARY` or `CHANGED`).
pub const ALLOWED_SHOW_LISTS: &[&str] = &["TABLES", "DATABASES", "SETTINGS"];

/// Whether a setting is on [`DENIED_SETTINGS`].
pub fn is_denied_setting(name: &str) -> bool {
    DENIED_SETTINGS
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => {
                name.len() >= prefix.len()
                    && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            }
            None => name.eq_ignore_ascii_case(pattern),
        })
}

fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(name))
}

/// `344` for an engine a `CREATE` names: one on [`DENIED_ENGINES`] always, and,
/// for a temporary table, anything outside [`ALLOWED_TEMPORARY_ENGINES`].
pub fn check_engine(engine: &str, temporary: bool) -> Result<(), ChError> {
    if listed(DENIED_ENGINES, engine) || (temporary && !listed(ALLOWED_TEMPORARY_ENGINES, engine)) {
        return Err(disabled(format!(
            "Engine {engine} is disabled on the House: {}",
            if temporary {
                "a temporary table is Memory or Null"
            } else {
                "it reads or writes outside the namespace's tables"
            }
        )));
    }
    Ok(())
}

/// `344` for a statement on [`DENIED_STATEMENTS`].
pub fn denied_statement(what: &str) -> ChError {
    disabled(format!(
        "{what} is disabled on the House (see the surface page)"
    ))
}

fn disabled(message: String) -> ChError {
    ChError::support_is_disabled(message)
}

/// A refusal of a denied setting, whoever noticed it. A `SETTINGS` clause is
/// applied while chDB explains the statement, so a setting the worker's profile
/// pins answers the engine's `452` before [`check`] sees the tree; for a setting
/// on [`DENIED_SETTINGS`] that is the front's `164`, the code a URL setting or a
/// `SET` gets. Anything else is returned as it came.
pub fn as_setting_refusal(err: HouseError) -> HouseError {
    if err.code() != 452 {
        return err;
    }
    let name = err
        .message()
        .split("Setting ")
        .filter_map(|rest| rest.split_once(" should not be changed"))
        .map(|(name, _)| name)
        .find(|name| is_denied_setting(name));
    match name {
        Some(name) => HouseError::from(setting_refused(name)),
        None => err,
    }
}

fn setting_refused(name: &str) -> ChError {
    ChError::readonly(format!(
        "Cannot modify '{name}' setting: it is not allowed on the House (see the surface page)"
    ))
}

/// One node of an explained tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// A function call; every name the line could hold.
    Function(Vec<String>),
    /// A table function: in a `FROM`, an `INSERT INTO FUNCTION`, or a `CREATE …
    /// AS`.
    TableFunction(Vec<String>),
    /// A `CREATE`'s engine.
    Engine(String),
    /// An identifier, possibly qualified (`system.disks`).
    Identifier(String),
    /// A literal, as written (`'default'`, `UInt64_1`).
    Constant(String),
    /// The names of a `SETTINGS` clause (query tree only).
    Settings(Vec<String>),
    /// Anything else, by its label: statements, lists, sections.
    Other(String),
}

/// A node and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The node.
    pub node: Node,
    /// The line's indentation.
    pub depth: usize,
    /// The nearest earlier line with less indentation, in the same tree.
    pub parent: Option<usize>,
}

/// The worker's explains of one statement, parsed (HS1 Task 5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryTree {
    entries: Vec<Entry>,
    /// The statement is an `INSERT` (its AST root is `InsertQuery`).
    insert: bool,
    /// The statement creates a lake table or a view, not a temporary table: its
    /// engine is held to [`DENIED_ENGINES`] only (the AST does not say
    /// `TEMPORARY`; the front reads it from the text).
    shared_create: bool,
    /// How many entries the `EXPLAIN AST` gave (the query tree's follow).
    ast_entries: usize,
    /// Whether a query tree was given.
    has_query_tree: bool,
    /// What a `SHOW` lists (`TABLES`, `CLUSTERS` …), read from the statement's
    /// text, when it is one: `ShowTables` does not say.
    show_list: Option<String>,
}

impl QueryTree {
    /// Parses `EXPLAIN AST` and, when there is one, `EXPLAIN QUERY TREE
    /// run_passes = 0` output, each one TSV-escaped row per line.
    pub fn from_explain(ast: &str, query_tree: Option<&str>) -> Self {
        let mut tree = Self::default();
        let ast_from = tree.entries.len();
        tree.push_lines(ast, Format::Ast);
        tree.insert = tree.entries[ast_from..]
            .iter()
            .find(|e| e.depth == 0)
            .is_some_and(
                |e| matches!(&e.node, Node::Other(label) if label.starts_with("InsertQuery")),
            );
        tree.ast_entries = tree.entries.len();
        if let Some(text) = query_tree {
            tree.has_query_tree = true;
            tree.push_lines(text, Format::QueryTree);
        }
        tree
    }

    /// The statement's text, for what its trees do not say: the list a `SHOW`
    /// names. Without it a `ShowTables` node is refused.
    pub fn statement(mut self, sql: &str) -> Self {
        self.show_list = show_list(sql);
        self
    }

    /// Marks the statement as a `CREATE` of something other than a temporary
    /// table (`classify::creates_shared_object`). Unmarked, every engine is held
    /// to [`ALLOWED_TEMPORARY_ENGINES`]: what chDB runs is a temporary table.
    pub fn shared_create(mut self, yes: bool) -> Self {
        self.shared_create = yes;
        self
    }

    /// The nodes, in the explains' order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether the statement is an `INSERT`.
    pub fn is_insert(&self) -> bool {
        self.insert
    }

    fn push_lines(&mut self, text: &str, format: Format) {
        let first = self.entries.len();
        for raw in text.lines() {
            let line = unescape_tsv(raw);
            let depth = line.len() - line.trim_start_matches(' ').len();
            let label = line[depth..].trim_end();
            if label.is_empty() {
                continue;
            }
            let parent = self.entries[first..]
                .iter()
                .rposition(|e| e.depth < depth)
                .map(|at| first + at);
            let node = match format {
                Format::Ast => self.ast_node(label, parent),
                Format::QueryTree => query_tree_node(label),
            };
            self.entries.push(Entry {
                node,
                depth,
                parent,
            });
        }
    }

    /// An `EXPLAIN AST` line. A `Function` is a table function under a
    /// `TableExpression`, directly under an `InsertQuery` (`INSERT INTO
    /// FUNCTION`) or a `CreateQuery` (`CREATE … AS f(…)`); the first `Function`
    /// under a `Storage definition` is the engine.
    fn ast_node(&self, label: &str, parent: Option<usize>) -> Node {
        let parent_label = parent.and_then(|at| match &self.entries[at].node {
            Node::Other(label) => Some(label.as_str()),
            _ => None,
        });
        if let Some(rest) = label.strip_prefix("Function ") {
            // Function names hold no spaces: what follows is `(alias …)` and
            // `(children n)`, which a user's alias cannot move in front of it.
            let name = rest.split(' ').next().unwrap_or_default().to_string();
            let under = |prefix: &str| parent_label.is_some_and(|l| l.starts_with(prefix));
            if under("TableExpression") || under("InsertQuery") || under("CreateQuery") {
                return Node::TableFunction(vec![name]);
            }
            if under("Storage definition") {
                let first = !self.entries.iter().any(|e| {
                    e.parent == parent && matches!(e.node, Node::Engine(_) | Node::Function(_))
                });
                if first {
                    return Node::Engine(name);
                }
            }
            return Node::Function(vec![name]);
        }
        for prefix in ["TableIdentifier ", "Identifier "] {
            if let Some(rest) = label.strip_prefix(prefix) {
                let name = rest.split(" (alias ").next().unwrap_or_default();
                let name = name.strip_suffix(')').map_or(name, |n| {
                    n.rsplit_once(" (children ").map_or(name, |(n, _)| n)
                });
                return Node::Identifier(name.to_string());
            }
        }
        if let Some(rest) = label.strip_prefix("Literal ") {
            return Node::Constant(
                rest.split(" (alias ")
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        Node::Other(label.to_string())
    }

    /// The argument nodes of a function or table function: an AST
    /// `ExpressionList`'s children, or a query tree's `ARGUMENTS` → `LIST`'s.
    fn arguments(&self, at: usize) -> Vec<usize> {
        let children = |of: usize| {
            self.entries
                .iter()
                .enumerate()
                .filter(move |(_, e)| e.parent == Some(of))
                .map(|(i, _)| i)
        };
        let is = |i: usize, prefix: &str| matches!(&self.entries[i].node, Node::Other(label) if label.starts_with(prefix));
        for child in children(at) {
            if is(child, "ExpressionList") {
                return children(child).collect();
            }
            if is(child, "ARGUMENTS")
                && let Some(list) = children(child).find(|c| is(*c, "LIST"))
            {
                return children(list).collect();
            }
        }
        Vec::new()
    }
}

#[derive(Clone, Copy)]
enum Format {
    Ast,
    QueryTree,
}

/// An `EXPLAIN QUERY TREE` line: `KIND id: n, key: value, …`, a `SETTINGS a=1
/// b=2` line, or a section header (`PROJECTION`, `JOIN TREE`, `ARGUMENTS`, …).
fn query_tree_node(label: &str) -> Node {
    if let Some(rest) = label.strip_prefix("SETTINGS ") {
        return Node::Settings(
            rest.split(' ')
                .filter_map(|piece| piece.split_once('=').map(|(name, _)| name))
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
        );
    }
    let Some((kind, fields)) = label.split_once(" id: ") else {
        return Node::Other(label.to_string());
    };
    match kind {
        "FUNCTION" => Node::Function(field_values(fields, "function_name: ")),
        "TABLE_FUNCTION" => Node::TableFunction(field_values(fields, "table_function_name: ")),
        "IDENTIFIER" => Node::Identifier(last_field(fields, "identifier: ")),
        "TABLE" => Node::Identifier(last_field(fields, "table_name: ")),
        "CONSTANT" => Node::Constant(
            fields
                .split_once("constant_value: ")
                .map(|(_, v)| {
                    v.rsplit_once(", constant_value_type: ")
                        .map_or(v, |(value, _)| value)
                })
                .unwrap_or_default()
                .to_string(),
        ),
        other => Node::Other(other.to_string()),
    }
}

/// Every value a `key: ` could start on a line, up to the next `,`. A user's
/// alias comes before the real field and may imitate it; the real value is always
/// among these (names hold no commas), and an imitation can only add a name to
/// refuse, never hide one.
fn field_values(fields: &str, key: &str) -> Vec<String> {
    fields
        .match_indices(key)
        .map(|(at, _)| {
            let rest = &fields[at + key.len()..];
            rest.split(',').next().unwrap_or_default().to_string()
        })
        .collect()
}

/// The value of the line's last field, `key: value` to the end. An alias that
/// imitates the key comes before it.
fn last_field(fields: &str, key: &str) -> String {
    fields
        .rfind(key)
        .map(|at| fields[at + key.len()..].to_string())
        .unwrap_or_default()
}

/// TSV's escapes undone (`\'`, `\\`, `\t`, `\n`, …). Rows were split on raw
/// newlines first, so an escaped newline cannot start a line of its own.
fn unescape_tsv(row: &str) -> String {
    let mut out = String::with_capacity(row.len());
    let mut chars = row.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The deny list over a statement's explained trees: `Ok` only when nothing in
/// them is refused. A tree with no nodes (nothing was explained) is refused: the
/// House never runs what it has not checked.
pub fn check(tree: &QueryTree) -> Result<(), ChError> {
    if tree.entries.is_empty() {
        return Err(disabled(
            "the statement could not be analysed, and the House runs nothing unchecked".to_string(),
        ));
    }
    check_shape(tree)?;
    for (at, entry) in tree.entries.iter().enumerate() {
        match &entry.node {
            Node::Other(label) => {
                if let Some((_, what)) = DENIED_AST_LABELS
                    .iter()
                    .find(|(prefix, _)| label.starts_with(prefix))
                {
                    return Err(denied_statement(what));
                }
                if is_show(label) {
                    check_show(tree, at, label)?;
                }
            }
            Node::TableFunction(names) => {
                for name in names {
                    check_table_function(tree, at, name)?;
                }
            }
            Node::Function(names) => {
                for name in names {
                    check_function(name)?;
                }
            }
            Node::Engine(name) => check_engine(name, !tree.shared_create)?,
            Node::Identifier(name) => check_identifier(name)?,
            Node::Settings(names) => {
                if let Some(name) = names.iter().find(|n| is_denied_setting(n)) {
                    return Err(setting_refused(name));
                }
            }
            Node::Constant(_) => {}
        }
    }
    Ok(())
}

/// The explains are what the worker asked for, or nothing in them is read (fix
/// round 1): the AST opens with a statement root as chDB prints it in TSV — the
/// bare label, or the label and its `(children n)` with those children on the
/// lines after it — and a query tree opens with its `QUERY` or `UNION` node.
fn check_shape(tree: &QueryTree) -> Result<(), ChError> {
    let unreadable = || {
        disabled(
            "the statement's syntax tree is not in the form the House reads, and the House \
             runs nothing unchecked"
                .to_string(),
        )
    };
    let ast = &tree.entries[..tree.ast_entries];
    let Some(Entry {
        node: Node::Other(root),
        depth: 0,
        ..
    }) = ast.first()
    else {
        return Err(unreadable());
    };
    if let Some((_, what)) = DENIED_AST_LABELS
        .iter()
        .find(|(prefix, _)| root.starts_with(prefix))
    {
        return Err(denied_statement(what));
    }
    let known = STATEMENT_ROOTS.iter().any(|name| is_label(root, name)) || is_show(root);
    let children = root
        .rsplit_once(" (children ")
        .and_then(|(_, n)| n.strip_suffix(')'))
        .map(|n| n.parse::<usize>());
    let whole = match children {
        // A bare root (`ShowTables`): nothing more on its line. An access
        // form's `SHOW USERS query` is refused as a `SHOW` below.
        None => !root.contains(' ') || root.starts_with("SHOW "),
        Some(Ok(n)) => n > 0 && ast.iter().skip(1).any(|e| e.depth > 0),
        Some(Err(_)) => false,
    };
    if !known || !whole {
        return Err(unreadable());
    }
    if tree.has_query_tree {
        let first = tree.entries.get(tree.ast_entries);
        let is_root = first.is_some_and(|e| {
            e.depth == 0
                && matches!(&e.node, Node::Other(kind) if kind == "QUERY" || kind == "UNION")
        });
        if !is_root {
            return Err(unreadable());
        }
    }
    Ok(())
}

/// `label` is the node `name`: the name alone, or followed by a space.
fn is_label(label: &str, name: &str) -> bool {
    label
        .strip_prefix(name)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

/// A `SHOW` statement's node (`ShowTables`, `SHOW USERS query`).
fn is_show(label: &str) -> bool {
    label.starts_with("Show") || label.starts_with("SHOW ")
}

/// `344` for a `SHOW` form outside [`ALLOWED_SHOW_QUERIES`] and
/// [`ALLOWED_SHOW_LISTS`]; a `SHOW CREATE` of a `system` table is held to
/// [`ALLOWED_SYSTEM_TABLES`].
fn check_show(tree: &QueryTree, at: usize, label: &str) -> Result<(), ChError> {
    let refused = |what: &str| {
        disabled(format!(
            "SHOW {what} is disabled on the House: it describes the host, its access or the \
             engine's internals (see the surface page)"
        ))
    };
    let Some(form) = ALLOWED_SHOW_QUERIES
        .iter()
        .find(|name| is_label(label, name))
    else {
        return Err(refused(label));
    };
    if *form == "ShowTables" {
        match &tree.show_list {
            Some(list) if listed(ALLOWED_SHOW_LISTS, list) => {}
            Some(list) => return Err(refused(list)),
            None => return Err(refused("(a form the House could not read)")),
        }
    }
    if form.starts_with("ShowCreate") {
        // `SHOW CREATE TABLE system.disks` names its database and table as two
        // identifiers.
        let names: Vec<&str> = tree
            .entries
            .iter()
            .filter(|e| e.parent == Some(at))
            .filter_map(|e| match &e.node {
                Node::Identifier(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        for pair in names.windows(2) {
            check_identifier(&format!("{}.{}", pair[0], pair[1]))?;
        }
    }
    Ok(())
}

/// The list a `SHOW` names: the first word after `SHOW` other than `FULL`,
/// `EXTENDED`, `TEMPORARY` and `CHANGED`, upper-cased, found with ClickHouse's
/// lexing. None when the text is no `SHOW`, or the list is not a bare word.
fn show_list(sql: &str) -> Option<String> {
    let lexemes = lex(sql);
    let word = |lexeme: &Lexeme| match lexeme {
        Lexeme::Word(start, end) => Some(sql[*start..*end].to_ascii_uppercase()),
        _ => None,
    };
    let at = lexemes
        .iter()
        .position(|(l, _)| word(l).as_deref() == Some("SHOW"))?;
    for (lexeme, _) in &lexemes[at + 1..] {
        let next = word(lexeme)?;
        if !matches!(next.as_str(), "FULL" | "EXTENDED" | "TEMPORARY" | "CHANGED") {
            return Some(next);
        }
    }
    None
}

fn check_table_function(tree: &QueryTree, at: usize, name: &str) -> Result<(), ChError> {
    if listed(INSERT_ONLY_TABLE_FUNCTIONS, name) {
        if tree.insert {
            return Ok(());
        }
        return Err(disabled(format!(
            "Table function {name} is disabled on the House outside an INSERT"
        )));
    }
    if !listed(ALLOWED_TABLE_FUNCTIONS, name) {
        return Err(disabled(format!(
            "Table function {name} is disabled on the House: it reads outside the \
             namespace's tables (allowed: {})",
            ALLOWED_TABLE_FUNCTIONS.join(", ")
        )));
    }
    if name.eq_ignore_ascii_case("merge") {
        let args = tree.arguments(at);
        // `merge(regex)` reads the current database; `merge(db, regex)` must name
        // it, plainly: no `REGEXP(…)`, no other database.
        if args.len() >= 2 && !names_own_database(tree, args[0]) {
            return Err(disabled(format!(
                "merge() over a database other than {OWN_DATABASE} is disabled on the House"
            )));
        }
    }
    Ok(())
}

fn names_own_database(tree: &QueryTree, at: usize) -> bool {
    let quoted = format!("'{OWN_DATABASE}'");
    match &tree.entries[at].node {
        Node::Constant(value) => value == &quoted,
        Node::Identifier(name) => name == OWN_DATABASE,
        Node::Function(names) => {
            names.len() == 1 && names[0] == "currentDatabase" && tree.arguments(at).is_empty()
        }
        _ => false,
    }
}

fn check_function(name: &str) -> Result<(), ChError> {
    if listed(DENIED_FUNCTIONS, name)
        || (listed(DENIED_TABLE_FUNCTIONS, name) && !listed(ALLOWED_TABLE_FUNCTIONS, name))
    {
        return Err(disabled(format!(
            "Function {name} is disabled on the House: it reads or runs outside the query"
        )));
    }
    if let Some(found) = HOST_FUNCTIONS
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
    {
        return Err(disabled(match found.answer {
            Host::Disabled => {
                format!("Function {name} describes the host and is disabled on the House")
            }
            _ => format!(
                "Function {name} answers the House's value only when called as {}()",
                found.name
            ),
        }));
    }
    Ok(())
}

fn check_identifier(name: &str) -> Result<(), ChError> {
    let Some((database, table)) = name.split_once('.') else {
        return Ok(());
    };
    if database.eq_ignore_ascii_case("system") && !listed(ALLOWED_SYSTEM_TABLES, table) {
        return Err(disabled(format!(
            "Table system.{table} describes the host or the engine's internals and is \
             disabled on the House"
        )));
    }
    Ok(())
}

/// The values the host functions answer with.
#[derive(Debug, Clone, Copy)]
pub struct HostValues<'a> {
    /// `hostName()`, `FQDN()`, `displayName()`.
    pub display_name: &'a str,
    /// `serverTimezone()`.
    pub timezone: &'a str,
    /// `currentUser()`.
    pub user: &'a str,
}

/// The statement with each `name()` call of a [`HOST_FUNCTIONS`] entry that has
/// a Loams value replaced by that value, under the column name the call would
/// have had: `hostName()` becomes ``('loams-house' AS `hostName()`)``. Calls are
/// found with ClickHouse's own lexing, so a name in a string, a quoted
/// identifier or a comment is left alone; a call it does not take stays in the
/// text, and [`check`] refuses it.
pub fn rewrite_host_functions(sql: &str, values: &HostValues<'_>) -> String {
    let lexemes = lex(sql);
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut i = 0;
    while i < lexemes.len() {
        let call = match (&lexemes[i], lexemes.get(i + 1), lexemes.get(i + 2)) {
            (
                (Lexeme::Word(start, end), _),
                Some((Lexeme::Symbol(b'('), _)),
                Some((Lexeme::Symbol(b')'), close)),
            ) if i == 0 || lexemes[i - 1].0 != Lexeme::Symbol(b'.') => {
                Some((*start, *end, close.clone()))
            }
            _ => None,
        };
        if let Some((start, end, close)) = call {
            let word = &sql[start..end];
            let answer = HOST_FUNCTIONS
                .iter()
                .find(|h| {
                    if h.case_insensitive {
                        h.name.eq_ignore_ascii_case(word)
                    } else {
                        h.name == word
                    }
                })
                .and_then(|h| match h.answer {
                    Host::DisplayName => Some(values.display_name),
                    Host::Timezone => Some(values.timezone),
                    Host::User => Some(values.user),
                    Host::Disabled => None,
                });
            if let Some(value) = answer {
                out.push_str(&sql[copied..start]);
                out.push_str(&format!("({} AS `{word}()`)", sql_string(value)));
                copied = span_end(&close, sql.len());
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&sql[copied..]);
    out
}

fn span_end(range: &Range<usize>, len: usize) -> usize {
    range.end.min(len)
}

/// A ClickHouse string literal.
fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: HostValues<'static> = HostValues {
        display_name: "loams-house",
        timezone: "UTC",
        user: "alice",
    };

    fn tree(ast: &str, query_tree: Option<&str>) -> QueryTree {
        QueryTree::from_explain(ast, query_tree)
    }

    fn code(result: Result<(), ChError>) -> Option<i32> {
        result.err().map(|e| e.code)
    }

    // `EXPLAIN AST` and `EXPLAIN QUERY TREE run_passes = 0` as chDB prints them
    // (TSV, so quotes are escaped). The worker tests check the real ones.
    const JOIN_URL_AST: &str = "SelectWithUnionQuery (children 1)
 ExpressionList (children 1)
  SelectQuery (children 3)
   ExpressionList (children 1)
    Asterisk
   TablesInSelectQuery (children 2)
    TablesInSelectQueryElement (children 1)
     TableExpression (children 1)
      Function numbers (children 1)
       ExpressionList (children 1)
        Literal UInt64_3
    TablesInSelectQueryElement (children 2)
     TableExpression (children 1)
      Function url (alias b) (children 1)
       ExpressionList (children 1)
        Literal \\'http://x/y\\'
     TableJoin";

    const IN_FILE_QUERY_TREE: &str = "QUERY id: 0
  PROJECTION
    LIST id: 1, nodes: 1
      MATCHER id: 2, matcher_type: ASTERISK
  JOIN TREE
    TABLE_FUNCTION id: 3, table_function_name: numbers
      ARGUMENTS
        LIST id: 4, nodes: 1
          CONSTANT id: 5, constant_value: UInt64_3, constant_value_type: UInt8
  WHERE
    FUNCTION id: 6, function_name: in, function_type: ordinary
      ARGUMENTS
        LIST id: 7, nodes: 2
          IDENTIFIER id: 8, identifier: number
          QUERY id: 9, is_subquery: 1
            PROJECTION
              LIST id: 10, nodes: 1
                MATCHER id: 11, matcher_type: ASTERISK
            JOIN TREE
              TABLE_FUNCTION id: 12, alias: x, table_function_name: file
                ARGUMENTS
                  LIST id: 13, nodes: 1
                    CONSTANT id: 14, constant_value: \\'/etc/passwd\\', constant_value_type: String
  SETTINGS max_threads=2";

    #[test]
    fn table_functions_anywhere_are_refused() {
        assert_eq!(code(check(&tree(JOIN_URL_AST, None))), Some(344));
        assert_eq!(
            code(check(&tree(
                "SelectWithUnionQuery",
                Some(IN_FILE_QUERY_TREE)
            ))),
            Some(344)
        );
        let allowed = JOIN_URL_AST.replace("Function url", "Function generateRandom");
        assert_eq!(code(check(&tree(&allowed, None))), None);
    }

    #[test]
    fn an_alias_cannot_hide_a_name() {
        let line = "    TABLE_FUNCTION id: 3, alias: x, table_function_name: numbers, table_function_name: file";
        let t = tree(
            "SelectWithUnionQuery",
            Some(&format!("QUERY id: 0\n  JOIN TREE\n{line}")),
        );
        assert_eq!(code(check(&t)), Some(344));
        let ast = "SelectWithUnionQuery (children 1)\n TableExpression (children 1)\n  Function file (alias numbers (children 1)) (children 1)";
        assert_eq!(code(check(&tree(ast, None))), Some(344));
        let f = "QUERY id: 0\n  PROJECTION\n    FUNCTION id: 9, alias: function_name: plus, function_name: hostName, function_type: ordinary";
        assert_eq!(
            code(check(&tree("SelectWithUnionQuery", Some(f)))),
            Some(344)
        );
    }

    #[test]
    fn engines_insert_targets_and_create_as() {
        let create = "CreateQuery t (children 3)
 Identifier t
 Columns definition (children 1)
 Storage definition (children 1)
  Function URL (children 1)
   ExpressionList (children 2)";
        assert_eq!(code(check(&tree(create, None))), Some(344));
        let lake = "CreateQuery t (children 2)\n Identifier t\n Storage definition (children 2)\n  Function MergeTree";
        assert_eq!(
            code(check(&tree(lake, None))),
            Some(344),
            "a temporary table's engine"
        );
        assert_eq!(
            code(check(&tree(lake, None).shared_create(true))),
            None,
            "a lake table's"
        );
        assert_eq!(
            code(check(&tree(create, None).shared_create(true))),
            Some(344),
            "URL in any CREATE"
        );
        let memory = "CreateQuery t (children 3)\n Identifier t\n Storage definition (children 2)\n  Function Memory\n  Function tuple";
        assert_eq!(code(check(&tree(memory, None))), None);
        let insert = "InsertQuery   (children 1)\n Function file (children 1)\n  ExpressionList (children 2)";
        assert_eq!(code(check(&tree(insert, None))), Some(344));
        let create_as = "CreateQuery t (children 2)\n Identifier t\n Function s3 (children 1)";
        assert_eq!(code(check(&tree(create_as, None))), Some(344));
    }

    #[test]
    fn input_only_inside_an_insert() {
        let select = "SelectWithUnionQuery (children 1)\n TableExpression (children 1)\n  Function input (children 1)";
        assert_eq!(code(check(&tree(select, None))), Some(344));
        let insert = "InsertQuery   (children 2)\n Identifier t\n SelectWithUnionQuery (children 1)\n  TableExpression (children 1)\n   Function input (children 1)";
        assert_eq!(code(check(&tree(insert, None))), None);
    }

    #[test]
    fn merge_reads_only_the_own_database() {
        let merge = |arg: &str| {
            format!(
                "SelectWithUnionQuery (children 1)\n TableExpression (children 1)\n  Function merge (children 1)\n   ExpressionList (children 2)\n    {arg}\n    Literal \\'^t\\'"
            )
        };
        assert_eq!(
            code(check(&tree(&merge("Literal \\'default\\'"), None))),
            None
        );
        assert_eq!(
            code(check(&tree(&merge("Function currentDatabase"), None))),
            None
        );
        assert_eq!(
            code(check(&tree(&merge("Literal \\'system\\'"), None))),
            Some(344)
        );
        assert_eq!(
            code(check(&tree(&merge("Function REGEXP (children 1)"), None))),
            Some(344)
        );
        let qt = "QUERY id: 0\n  JOIN TREE\n    TABLE_FUNCTION id: 1, table_function_name: merge\n      ARGUMENTS\n        LIST id: 2, nodes: 2\n          CONSTANT id: 3, constant_value: \\'system\\', constant_value_type: String\n          CONSTANT id: 4, constant_value: \\'^st\\', constant_value_type: String";
        assert_eq!(
            code(check(&tree("SelectWithUnionQuery", Some(qt)))),
            Some(344)
        );
    }

    #[test]
    fn system_tables_settings_and_statements() {
        let disks = "SelectWithUnionQuery (children 1)\n TableExpression (children 1)\n  TableIdentifier system.disks (alias d)";
        assert_eq!(code(check(&tree(disks, None))), Some(344));
        let one = disks.replace("system.disks", "system.one");
        assert_eq!(code(check(&tree(&one, None))), None);
        let schema = "QUERY id: 0\n  SETTINGS format_schema=/etc/x max_threads=2";
        assert_eq!(
            code(check(&tree("SelectWithUnionQuery", Some(schema)))),
            Some(164)
        );
        assert_eq!(code(check(&tree("SYSTEM query", None))), Some(344));
        assert_eq!(
            code(check(&tree("CreateSQLFunctionQuery f (children 2)", None))),
            Some(344)
        );
        assert_eq!(
            code(check(&tree(
                "CreateQuery d (children 3)\n Identifier d\n Dictionary definition (children 4)",
                None
            ))),
            Some(344)
        );
        assert_eq!(
            code(check(&QueryTree::default())),
            Some(344),
            "nothing explained"
        );
    }

    #[test]
    fn denied_settings_match_patterns() {
        for name in [
            "format_schema",
            "format_schema_source",
            "user_files_path",
            "format_template_row",
            "format_avro_schema_registry_url",
            "default_temporary_table_engine",
        ] {
            assert!(is_denied_setting(name), "{name}");
        }
        for name in [
            "max_threads",
            "format_csv_delimiter",
            "output_format_json_quote_64bit_integers",
            "format_template_row_format",
            "format_avro_schema_registry_connection_timeout",
        ] {
            assert!(!is_denied_setting(name), "{name}");
        }
    }

    #[test]
    fn host_functions_are_rewritten_by_clickhouse_lexing() {
        assert_eq!(
            rewrite_host_functions("SELECT hostName(), FQDN ( ) AS f, fqdn()", &VALUES),
            "SELECT ('loams-house' AS `hostName()`), ('loams-house' AS `FQDN()`) AS f, ('loams-house' AS `fqdn()`)"
        );
        assert_eq!(
            rewrite_host_functions("SELECT serverTimezone(), currentUser(), USER()", &VALUES),
            "SELECT ('UTC' AS `serverTimezone()`), ('alice' AS `currentUser()`), ('alice' AS `USER()`)"
        );
        // Strings, quoted names, comments, other cases, arguments and qualified
        // names are left for the check.
        for untouched in [
            "SELECT 'hostName()'",
            "SELECT `hostName`()",
            "SELECT 1 -- hostName()",
            "SELECT HOSTNAME()",
            "SELECT hostName(1)",
            "SELECT t.hostName()",
            "SELECT getMacro()",
            "SELECT $$hostName()$$",
        ] {
            assert_eq!(rewrite_host_functions(untouched, &VALUES), untouched);
        }
        let quoted = HostValues {
            user: "o'brien\\",
            ..VALUES
        };
        assert_eq!(
            rewrite_host_functions("SELECT user()", &quoted),
            "SELECT ('o\\'brien\\\\' AS `user()`)"
        );
    }

    #[test]
    fn host_functions_left_in_the_tree_are_refused() {
        for name in [
            "hostName",
            "HOSTNAME",
            "getMacro",
            "filesystemAvailable",
            "file",
        ] {
            let qt = format!(
                "QUERY id: 0\n  PROJECTION\n    FUNCTION id: 1, function_name: {name}, function_type: ordinary"
            );
            assert_eq!(
                code(check(&tree("SelectWithUnionQuery", Some(&qt)))),
                Some(344),
                "{name}"
            );
        }
    }

    #[test]
    fn pinned_setting_refusals_become_164() {
        use loams_house_ipc::EngineError;
        let engine = |message: &str| {
            HouseError::from(EngineError {
                code: 452,
                name: "SETTING_CONSTRAINT_VIOLATION".to_string(),
                message: message.to_string(),
            })
        };
        let pinned =
            engine("Code: 452. DB::Exception: Setting format_schema should not be changed.");
        assert_eq!(as_setting_refusal(pinned).code(), 164);
        let capped = engine("Setting max_memory_usage shouldn't be greater than 4294967296");
        assert_eq!(as_setting_refusal(capped).code(), 452);
    }

    #[test]
    fn explain_output_is_held_to_its_shape() {
        // chDB's explains as the worker asks for them: TSV, a statement root
        // first. JSON, CSV or a one-line RawBLOB (a top-level FORMAT the front
        // did not strip) is refused, not read.
        let json = "{\n\t\"meta\":\n\t[\n\t\t{\n\t\t\t\"explain\": \"  Function url (children 1)\"";
        assert_eq!(code(check(&tree(json, None))), Some(344));
        let csv = "\"SelectWithUnionQuery (children 1)\"\n\" TableExpression (children 1)\"";
        assert_eq!(code(check(&tree(csv, None))), Some(344));
        let vertical = "Row 1:\n──────\nexplain: SelectWithUnionQuery (children 1)";
        assert_eq!(code(check(&tree(vertical, None))), Some(344));
        assert_eq!(code(check(&tree("SettingsQuery x", None))), Some(344));
        // A one-line RawBLOB: a root that claims children it does not have, or
        // ends on a leaf.
        let raw = "SelectWithUnionQuery (children 1) ExpressionList (children 1) Literal UInt64_1";
        assert_eq!(code(check(&tree(raw, None))), Some(344));
        let claims = "SelectWithUnionQuery (children 1) TableIdentifier `x (children 1)";
        assert_eq!(code(check(&tree(claims, None))), Some(344));
        // A query tree that is not one (the AST was TSV, the query tree JSON).
        let ok_ast = "SelectWithUnionQuery (children 1)";
        let json_qt = "{\"explain\":\"QUERY id: 0\"}\n{\"explain\":\"    TABLE_FUNCTION id: 3, table_function_name: url\"}";
        assert_eq!(code(check(&tree(ok_ast, Some(json_qt)))), Some(344));
        assert_eq!(code(check(&tree(ok_ast, Some("")))), Some(344));
        // Every root the surface reaches is known.
        for (ast, query_tree) in [
            ("SelectWithUnionQuery (children 1)", Some("QUERY id: 0")),
            (
                "SelectWithUnionQuery (children 1)",
                Some("UNION id: 0, union_mode: INTERSECT_ALL"),
            ),
            ("InsertQuery   (children 1)\n Identifier t", None),
            ("CreateQuery x (children 2)\n Identifier x", None),
            ("Explain EXPLAIN AST (children 1)", None),
            ("DescribeQuery (children 1)", None),
            ("ExistsTableQuery  t (children 1)", None),
            ("ExistsDatabaseQuery default  (children 1)", None),
            ("DropQuery  t (children 1)", None),
            ("ShowColumns", None),
            ("ShowCreateTableQuery  t (children 1)\n Identifier t", None),
        ] {
            let ast = if ast.contains("(children") && !ast.contains('\n') {
                format!("{ast}\n ExpressionList")
            } else {
                ast.to_string()
            };
            assert_eq!(code(check(&tree(&ast, query_tree))), None, "{ast}");
        }
    }

    #[test]
    fn show_forms_follow_the_allow_list() {
        let show = |label: &str, sql: &str| check(&tree(label, None).statement(sql));
        for sql in [
            "SHOW TABLES",
            "show full tables from default like 't%'",
            "SHOW TEMPORARY TABLES",
            "SHOW DATABASES",
            "SHOW SETTINGS LIKE 'max%'",
            "/* c */ SHOW CHANGED SETTINGS ILIKE '%x'",
        ] {
            assert_eq!(code(show("ShowTables", sql)), None, "{sql}");
        }
        for sql in [
            "SHOW CLUSTERS",
            "SHOW CLUSTER 'default'",
            "SHOW FILESYSTEM CACHES",
            "SHOW MERGES",
            "SHOW DICTIONARIES",
            "SHOW `TABLES`",
        ] {
            assert_eq!(code(show("ShowTables", sql)), Some(344), "{sql}");
        }
        assert_eq!(
            code(check(&tree("ShowTables", None))),
            Some(344),
            "a SHOW whose text is unknown"
        );
        for label in [
            "ShowColumns",
            "ShowIndexes",
            "ShowCreateTableQuery  t (children 1)",
            "ShowCreateDatabaseQuery default  (children 1)",
            "ShowCreateViewQuery  v (children 1)",
            "ShowSetting",
            "ShowEngineQuery",
            "ShowFunctions",
            "ShowProcesslistQuery",
        ] {
            let ast = if label.contains("(children") {
                format!("{label}\n Identifier t")
            } else {
                label.to_string()
            };
            assert_eq!(code(check(&tree(&ast, None))), None, "{label}");
        }
        for label in [
            "ShowAccessQuery",
            "ShowGrantsQuery",
            "ShowPrivilegesQuery",
            "SHOW USERS query",
            "SHOW ROLES query",
            "SHOW SETTINGS PROFILES query",
            "SHOW QUOTAS query",
            "SHOW CURRENT QUOTA query",
            "SHOW ROW POLICIES query",
            "SHOW CREATE USER query",
            "SHOW CURRENT ROLES query",
            "ShowCreateDictionaryQuery  d (children 1)",
            "ShowSomethingNew",
        ] {
            assert_eq!(code(check(&tree(label, None))), Some(344), "{label}");
        }
        // Inside an EXPLAIN, too.
        let explain = "Explain EXPLAIN AST (children 1)\n ShowTables";
        assert_eq!(
            code(check(
                &tree(explain, None).statement("EXPLAIN AST SHOW CLUSTERS")
            )),
            Some(344)
        );
        // SHOW CREATE TABLE of a system table is held to the system allow-list.
        let create = |db: &str, t: &str| {
            format!("ShowCreateTableQuery {db} {t} (children 2)\n Identifier {db}\n Identifier {t}")
        };
        assert_eq!(
            code(check(&tree(&create("system", "disks"), None))),
            Some(344)
        );
        assert_eq!(code(check(&tree(&create("system", "one"), None))), None);
        assert_eq!(code(check(&tree(&create("default", "disks"), None))), None);
        let substituted =
            "ShowCreateTableQuery  system.disks (children 1)\n Identifier system.disks";
        assert_eq!(code(check(&tree(substituted, None))), Some(344));
    }

    #[test]
    fn host_functions_cover_uptime_logs_and_the_authenticated_user() {
        assert_eq!(
            rewrite_host_functions("SELECT authenticatedUser()", &VALUES),
            "SELECT ('alice' AS `authenticatedUser()`)"
        );
        for name in ["uptime", "logTrace"] {
            let qt = format!(
                "QUERY id: 0\n  PROJECTION\n    FUNCTION id: 1, function_name: {name}, function_type: ordinary"
            );
            assert_eq!(
                code(check(&tree("SelectWithUnionQuery", Some(&qt)))),
                Some(344),
                "{name}"
            );
        }
    }

    #[test]
    fn denied_settings_include_the_headers_secrets_and_scripts() {
        for name in [
            "allow_get_client_http_header",
            "format_display_secrets_in_show_and_select",
            "user_scripts_path",
            "allow_custom_error_code_in_throwif",
            "allow_insert_into_iceberg",
        ] {
            assert!(is_denied_setting(name), "{name}");
        }
    }

    #[test]
    fn engines_include_the_merge_buffer_and_object_store_ones() {
        for engine in [
            "Merge",
            "Buffer",
            "Alias",
            "ExternalDistributed",
            "COSN",
            "OSS",
            "GCS",
            "DeltaLakeS3",
            "TimeSeries",
        ] {
            assert_eq!(code(check_engine(engine, false)), Some(344), "{engine}");
        }
        for engine in ALLOWED_ENGINES {
            assert!(!listed(DENIED_ENGINES, engine), "{engine}");
        }
    }

    #[test]
    fn lists_do_not_overlap() {
        for name in ALLOWED_TABLE_FUNCTIONS {
            assert!(!listed(DENIED_TABLE_FUNCTIONS, name), "{name}");
        }
        for name in ALLOWED_TEMPORARY_ENGINES {
            assert!(!listed(DENIED_ENGINES, name), "{name}");
        }
    }
}
