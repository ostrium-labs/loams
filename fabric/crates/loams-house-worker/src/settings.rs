//! The worker's own controls: L2 of design §49 §13.2 (HS1 Task 5).
//!
//! L1, the front's deny list (`loams_house::deny`), refuses what a statement
//! names. L2 is what chDB itself is told, so that what L1 cannot see is refused
//! by the engine anyway:
//!
//! * **Every user connection** is `readonly = 2` (no DDL, settings may change),
//!   `allow_ddl = 0` (DDL is the front's; session-local temporary tables still
//!   work, measured) and in UTC. Both `readonly` and `allow_ddl` are sticky: a
//!   statement cannot set them back.
//! * **The `worker` profile** pins with `<readonly/>` constraints the settings
//!   that open the host or the network ([`PINNED_OFF`], [`PINNED_VALUES`],
//!   [`PINNED_PATHS`]), so a query's `SETTINGS` clause, which the front does not
//!   parse for every statement, answers `452`.
//! * **The grants** (HS1 R1.8, `config::GRANTS`) withhold `FILE`, `URL`, `REMOTE`,
//!   `S3` outside the forwarder, `SYSTEM`, `CREATE FUNCTION` and dictionaries.
//! * **`display_name`** is the House's, so `displayName()` never names the host.
//!
//! R1.10's finding stands: `INTO OUTFILE` and `FROM INFILE` are carried out by
//! chDB's client layer whatever these say, so they rest on L1 and L3.

/// The query-level arguments of every user connection (HS1 R1.9).
///
/// `session_timezone=UTC` because the House answers `X-ClickHouse-Timezone: UTC`
/// (FL2 Task 2) and chDB takes its *server* timezone from the host: an empty
/// environment leaves it on `/etc/localtime`, and neither `<timezone>` in the
/// config file nor `--timezone=UTC` changes it (measured, HS1 Task 3). The session
/// setting is what `timezone()` and every DateTime conversion use, and a user may
/// still `SET` it, as in ClickHouse.
///
/// `allow_ddl=0` (§49 §13.2): `CREATE VIEW`, `CREATE TABLE`, `ALTER` and the rest
/// answer `392`, and `SET allow_ddl = 1` is refused; `CREATE` and `DROP
/// TEMPORARY TABLE` still work (measured, HS1 Task 5).
pub const USER_CONNECTION_ARGS: &[&str] =
    &["--readonly=2", "--allow_ddl=0", "--session_timezone=UTC"];

/// Settings the worker's profile sets to 0 and pins with `<readonly/>`
/// constraints, so a user's `SET` or `SETTINGS` answers `452` (HS1 R1.5, R1.8,
/// Task 5).
pub const PINNED_OFF: &[&str] = &[
    "allow_introspection_functions",
    "allow_insert_into_iceberg",
    "allow_experimental_iceberg_compaction",
    "allow_iceberg_remove_orphan_files",
    // A user-chosen code in `throwIf` could impersonate a Loams or chDB error
    // (`236 ABORTED`, a crash) to the front (HS1 Task 2 review M2).
    "allow_custom_error_code_in_throwif",
    // `INSERT … VALUES` evaluates expressions in its data, which no `EXPLAIN`
    // sees: `VALUES (file('/etc/passwd'))` read the file and `VALUES ((SELECT …
    // FROM url(…)))` would open the URL (measured, HS1 Task 5). Both switches are
    // needed: with only the first, the template path still evaluated
    // `hostName()`. Literal values insert as before; an expression answers `344`.
    "input_format_values_interpret_expressions",
    "input_format_values_deduce_templates_of_expressions",
    // `INTO OUTFILE` is refused by L1; this keeps it from making directories if
    // L1 is ever bypassed.
    "into_outfile_create_parent_directories",
    // Fix round 1 (`path_like_settings_are_denied`): the request's headers
    // carry the credentials; secrets stay hidden in `SHOW` and `SELECT`; the
    // Iceberg writers' other names; functions that reach an AI endpoint, run
    // Python, evaluate text or read Keeper; other dialects, which the deny list
    // would not parse as chDB does; and the memory profiler.
    "allow_get_client_http_header",
    "format_display_secrets_in_show_and_select",
    "allow_experimental_insert_into_iceberg",
    "allow_experimental_expire_snapshots",
    "allow_experimental_cleanup_old_data_files_compaction",
    "allow_experimental_ai_functions",
    "ai_function_allow_insecure_endpoint",
    "allow_experimental_eval_table_function",
    "allow_python_table_function",
    "allow_fuzz_query_functions",
    "allow_unrestricted_reads_from_keeper",
    "allow_experimental_kusto_dialect",
    "allow_experimental_prql_dialect",
    "allow_experimental_polyglot_dialect",
    "jemalloc_enable_profiler",
    "jemalloc_collect_profile_samples_in_trace_log",
];

/// Switches the profile pins at their defaults with `<readonly/>` constraints
/// (fix round 1): what they open is refused already, and turning them off would
/// change nothing the House does.
pub const PINNED_SWITCHES: &[&str] = &[
    "allow_named_collection_override_by_default",
    "allow_distributed_ddl",
];

/// Settings the profile pins to a value with `<readonly/>` constraints.
///
/// No MergeTree inside chDB, ever (D762): a temporary table without an `ENGINE`
/// is a `Memory` one, and so would be any table (none is created by a user
/// connection, which has `allow_ddl = 0`).
pub const PINNED_VALUES: &[(&str, &str)] = &[
    ("default_temporary_table_engine", "Memory"),
    ("default_table_engine", "Memory"),
];

/// Settings that name a file, a directory, a schema source or a URL the engine
/// would read or write: pinned at their defaults with `<readonly/>` (HS1 Task 4
/// fix round 1, I4; Task 5), so a query's `SETTINGS` clause cannot set them
/// (`452`). `format_schema_source` can be `query`, which would make the schema a
/// statement of its own.
pub const PINNED_PATHS: &[&str] = &[
    "format_schema",
    "format_schema_source",
    "format_schema_message_name",
    "output_format_schema",
    "input_format_record_errors_file_path",
    "format_template_resultset",
    "format_template_row",
    "format_avro_schema_registry_url",
    "rename_files_after_processing",
    "s3queue_default_zookeeper_path",
    "url_base",
];

/// The largest `max_memory_usage` a statement may set: §49 §12's 4 GiB per query
/// (FL2 Ruling 10). It is also the profile's value, so `0` (unlimited) is a change
/// the constraint sees: ClickHouse does not check a value equal to the current one.
pub const MAX_QUERY_MEMORY: u64 = 4 * 1024 * 1024 * 1024;

/// The largest `max_execution_time` a statement may set, in seconds (FL2 Ruling
/// 10); also the profile's value, for the same reason.
pub const MAX_EXECUTION_TIME_S: u64 = 300;

/// The smallest `max_execution_time`: one microsecond, ClickHouse's unit. The
/// constraint is checked after the engine's conversion, so a value that truncates
/// to 0 (unlimited) is under it (measured, fix round 1).
pub const MIN_EXECUTION_TIME_S: &str = "0.000001";

/// What `displayName()` answers: the House's `X-ClickHouse-Server-Display-Name`
/// (`loams_house::config::DISPLAY_NAME`; a test keeps the two equal).
pub const DISPLAY_NAME: &str = "loams-house";

/// The `max_threads` cap: the node's cores, as the front's (FL2 Ruling 10).
pub fn max_threads_cap() -> u64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u64)
}

/// Every setting the profile pins, whatever its value.
pub fn pinned() -> impl Iterator<Item = &'static str> {
    PINNED_OFF
        .iter()
        .copied()
        .chain(PINNED_VALUES.iter().map(|(name, _)| *name))
        .chain(PINNED_PATHS.iter().copied())
        .chain(PINNED_SWITCHES.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_connections_are_readonly_without_ddl() {
        assert!(USER_CONNECTION_ARGS.contains(&"--readonly=2"));
        assert!(USER_CONNECTION_ARGS.contains(&"--allow_ddl=0"));
    }

    #[test]
    fn pinned_names_are_distinct() {
        let names: Vec<_> = pinned().collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "{names:?}");
    }
}
