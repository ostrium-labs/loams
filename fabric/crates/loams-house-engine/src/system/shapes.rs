//! The columns of the `system` tables the engine serves (plan HE1 Task 38),
//! ClickHouse 26.9.2.8's: names, types and order as `SHOW CREATE TABLE
//! system.<t>` prints them (ClickHouse's Apache-2.0
//! `tests/queries/0_stateless/02117_show_create_table_system.reference` at
//! v26.9.2.8). An `ALIAS` column is listed as a column (the front's chDB
//! views do the same, from `DESCRIBE`; R38.2), with its target in
//! [`ALIASES`].
//!
//! `clusters`, `macros`, `mutations`, `distributed_ddl_queue` and
//! `query_log` are the front's (`loams-house/src/system.rs`, HE1 R9.62 and
//! HS1 Task 23), copied because the engine never depends on the front
//! (`engine_crates_stay_apart`; `shapes_match_the_front` keeps them equal).

/// A table's columns: (name, ClickHouse type).
pub type Shape = &'static [(&'static str, &'static str)];

/// `system.tables`.
pub const TABLES: Shape = &[
    ("database", "String"),
    ("name", "String"),
    ("uuid", "UUID"),
    ("engine", "String"),
    ("is_temporary", "UInt8"),
    ("data_paths", "Array(String)"),
    ("metadata_path", "String"),
    ("metadata_modification_time", "DateTime"),
    ("metadata_version", "Int32"),
    ("dependencies_database", "Array(String)"),
    ("dependencies_table", "Array(String)"),
    ("create_table_query", "String"),
    ("engine_full", "String"),
    ("as_select", "String"),
    (
        "parameterized_view_parameters",
        "Array(Tuple(name String, type String))",
    ),
    ("partition_key", "String"),
    ("sorting_key", "String"),
    ("primary_key", "String"),
    ("sampling_key", "String"),
    ("unique_key", "String"),
    ("skipping_indices_types", "Array(String)"),
    ("storage_policy", "String"),
    ("total_rows", "Nullable(UInt64)"),
    ("total_bytes", "Nullable(UInt64)"),
    ("total_bytes_uncompressed", "Nullable(UInt64)"),
    ("parts", "Nullable(UInt64)"),
    ("active_parts", "Nullable(UInt64)"),
    ("total_marks", "Nullable(UInt64)"),
    ("active_on_fly_data_mutations", "UInt64"),
    ("active_on_fly_alter_mutations", "UInt64"),
    ("active_on_fly_metadata_mutations", "UInt64"),
    ("columns_descriptions_cache_size", "UInt64"),
    ("lifetime_rows", "Nullable(UInt64)"),
    ("lifetime_bytes", "Nullable(UInt64)"),
    ("comment", "String"),
    ("has_own_data", "UInt8"),
    ("loading_dependencies_database", "Array(String)"),
    ("loading_dependencies_table", "Array(String)"),
    ("loading_dependent_database", "Array(String)"),
    ("loading_dependent_table", "Array(String)"),
    ("target_database", "String"),
    ("target_table", "String"),
    ("definer", "String"),
    ("table", "String"),
];

/// `system.columns`.
pub const COLUMNS: Shape = &[
    ("database", "String"),
    ("table", "String"),
    ("name", "String"),
    ("type", "String"),
    ("position", "UInt64"),
    ("default_kind", "String"),
    ("default_expression", "String"),
    ("data_compressed_bytes", "UInt64"),
    ("data_uncompressed_bytes", "UInt64"),
    ("marks_bytes", "UInt64"),
    ("comment", "String"),
    ("is_in_partition_key", "UInt8"),
    ("is_in_sorting_key", "UInt8"),
    ("is_in_primary_key", "UInt8"),
    ("is_in_sampling_key", "UInt8"),
    ("compression_codec", "String"),
    ("character_octet_length", "Nullable(UInt64)"),
    ("numeric_precision", "Nullable(UInt64)"),
    ("numeric_precision_radix", "Nullable(UInt64)"),
    ("numeric_scale", "Nullable(UInt64)"),
    ("datetime_precision", "Nullable(UInt64)"),
    ("serialization_hint", "Nullable(String)"),
    ("statistics", "String"),
    ("column", "String"),
];

/// `system.databases`.
pub const DATABASES: Shape = &[
    ("name", "String"),
    ("engine", "String"),
    ("data_path", "String"),
    ("metadata_path", "String"),
    ("uuid", "UUID"),
    ("engine_full", "String"),
    ("comment", "String"),
    ("is_external", "UInt8"),
    ("database", "String"),
];

/// `system.parts`.
pub const PARTS: Shape = &[
    ("partition", "String"),
    ("name", "String"),
    ("uuid", "UUID"),
    ("part_type", "String"),
    ("part_storage_type", "String"),
    ("active", "UInt8"),
    ("marks", "UInt64"),
    ("rows", "UInt64"),
    ("files", "UInt64"),
    ("bytes_on_disk", "UInt64"),
    ("data_compressed_bytes", "UInt64"),
    ("data_uncompressed_bytes", "UInt64"),
    ("primary_key_size", "UInt64"),
    ("marks_bytes", "UInt64"),
    ("secondary_indices_compressed_bytes", "UInt64"),
    ("secondary_indices_uncompressed_bytes", "UInt64"),
    ("secondary_indices_marks_bytes", "UInt64"),
    ("modification_time", "DateTime"),
    ("remove_time", "DateTime"),
    ("refcount", "UInt32"),
    ("min_date", "Date"),
    ("max_date", "Date"),
    ("min_time", "DateTime"),
    ("max_time", "DateTime"),
    ("partition_id", "String"),
    ("min_block_number", "Int64"),
    ("max_block_number", "Int64"),
    ("level", "UInt32"),
    ("data_version", "UInt64"),
    ("primary_key_bytes_in_memory", "UInt64"),
    ("primary_key_bytes_in_memory_allocated", "UInt64"),
    ("index_granularity_bytes_in_memory", "UInt64"),
    ("index_granularity_bytes_in_memory_allocated", "UInt64"),
    ("is_frozen", "UInt8"),
    ("database", "String"),
    ("table", "String"),
    ("engine", "String"),
    ("disk_name", "String"),
    ("path", "String"),
    ("hash_of_all_files", "String"),
    ("hash_of_uncompressed_files", "String"),
    ("uncompressed_hash_of_compressed_files", "String"),
    ("delete_ttl_info_min", "DateTime"),
    ("delete_ttl_info_max", "DateTime"),
    ("move_ttl_info.expression", "Array(String)"),
    ("move_ttl_info.min", "Array(DateTime)"),
    ("move_ttl_info.max", "Array(DateTime)"),
    ("default_compression_codec", "String"),
    ("recompression_ttl_info.expression", "Array(String)"),
    ("recompression_ttl_info.min", "Array(DateTime)"),
    ("recompression_ttl_info.max", "Array(DateTime)"),
    ("group_by_ttl_info.expression", "Array(String)"),
    ("group_by_ttl_info.min", "Array(DateTime)"),
    ("group_by_ttl_info.max", "Array(DateTime)"),
    ("rows_where_ttl_info.expression", "Array(String)"),
    ("rows_where_ttl_info.min", "Array(DateTime)"),
    ("rows_where_ttl_info.max", "Array(DateTime)"),
    ("projections", "Array(String)"),
    ("visible", "UInt8"),
    ("creation_tid", "Tuple(UInt64, UInt64, UUID)"),
    ("removal_tid_lock", "UInt64"),
    ("removal_tid", "Tuple(UInt64, UInt64, UUID)"),
    ("creation_csn", "UInt64"),
    ("removal_csn", "UInt64"),
    ("has_lightweight_delete", "UInt8"),
    ("last_removal_attempt_time", "DateTime"),
    ("removal_state", "String"),
    ("bytes", "UInt64"),
    ("marks_size", "UInt64"),
    ("part_name", "String"),
];

/// `system.settings`.
pub const SETTINGS: Shape = &[
    ("name", "String"),
    ("value", "String"),
    ("changed", "UInt8"),
    ("description", "String"),
    ("min", "Nullable(String)"),
    ("max", "Nullable(String)"),
    ("disallowed_values", "Array(String)"),
    ("readonly", "UInt8"),
    ("type", "String"),
    ("default", "String"),
    ("alias_for", "String"),
    ("is_obsolete", "UInt8"),
    (
        "tier",
        "Enum8('Production' = 0, 'Obsolete' = 4, 'Experimental' = 8, 'Beta' = 12, 'PrivatePreview' = 16)",
    ),
];

/// `system.functions`.
pub const FUNCTIONS: Shape = &[
    ("name", "String"),
    ("is_aggregate", "UInt8"),
    ("case_insensitive", "UInt8"),
    ("alias_to", "String"),
    ("create_query", "String"),
    (
        "origin",
        "Enum8('System' = 0, 'SQLUserDefined' = 1, 'ExecutableUserDefined' = 2, 'WasmUserDefined' = 3)",
    ),
    ("description", "String"),
    ("syntax", "String"),
    ("arguments", "String"),
    ("parameters", "String"),
    ("returned_value", "String"),
    ("examples", "String"),
    ("introduced_in", "String"),
    ("categories", "String"),
    ("deterministic", "Nullable(UInt8)"),
    ("higher_order", "Nullable(UInt8)"),
];

/// `system.one`.
pub const ONE: Shape = &[("dummy", "UInt8")];

/// `system.numbers`.
pub const NUMBERS: Shape = &[("number", "UInt64")];

/// `system.numbers_mt`.
pub const NUMBERS_MT: Shape = &[("number", "UInt64")];

/// `system.zeros`.
pub const ZEROS: Shape = &[("zero", "UInt8")];

/// `system.zeros_mt`.
pub const ZEROS_MT: Shape = &[("zero", "UInt8")];

/// `system.clusters` (the front's `CLUSTERS_COLUMNS`).
pub const CLUSTERS: Shape = &[
    ("cluster", "String"),
    ("shard_num", "UInt32"),
    ("shard_name", "String"),
    ("shard_weight", "UInt32"),
    ("internal_replication", "UInt8"),
    ("replica_num", "UInt32"),
    ("host_name", "String"),
    ("host_address", "String"),
    ("port", "UInt16"),
    ("is_local", "UInt8"),
    ("user", "String"),
    ("default_database", "String"),
    ("errors_count", "UInt32"),
    ("slowdowns_count", "UInt32"),
    ("estimated_recovery_time", "UInt32"),
    ("database_shard_name", "String"),
    ("database_replica_name", "String"),
    ("is_shared_catalog_cluster", "UInt8"),
    ("is_active", "Nullable(UInt8)"),
    ("unsynced_after_recovery", "Nullable(UInt8)"),
    ("replication_lag", "Nullable(UInt32)"),
    ("recovery_time", "Nullable(UInt64)"),
    ("name", "String"),
];

/// `system.macros` (the front's `MACROS_COLUMNS`).
pub const MACROS: Shape = &[("macro", "String"), ("substitution", "String")];

/// `system.mutations` (the front's `MUTATIONS_COLUMNS`).
pub const MUTATIONS: Shape = &[
    ("database", "String"),
    ("table", "String"),
    ("mutation_id", "String"),
    ("command", "String"),
    ("create_time", "DateTime"),
    ("finish_time", "DateTime"),
    ("block_numbers.partition_id", "Array(String)"),
    ("block_numbers.number", "Array(Int64)"),
    ("parts_in_progress_names", "Array(String)"),
    ("parts_to_do_names", "Array(String)"),
    ("parts_to_do", "Int64"),
    ("parts_postpone_reasons", "Map(String, String)"),
    ("is_done", "UInt8"),
    ("is_killed", "UInt8"),
    ("latest_failed_part", "String"),
    ("latest_fail_time", "DateTime"),
    ("latest_fail_reason", "String"),
    ("latest_fail_error_code_name", "String"),
];

/// `system.distributed_ddl_queue` (the front's `DDL_QUEUE_COLUMNS`).
pub const DISTRIBUTED_DDL_QUEUE: Shape = &[
    ("entry", "String"),
    ("entry_version", "Nullable(UInt8)"),
    ("initiator_host", "Nullable(String)"),
    ("initiator_port", "Nullable(UInt16)"),
    ("cluster", "String"),
    ("query", "String"),
    ("settings", "Map(String, String)"),
    ("query_create_time", "DateTime"),
    ("host", "Nullable(String)"),
    ("port", "Nullable(UInt16)"),
    (
        "status",
        "Nullable(Enum8('Inactive' = 0, 'Active' = 1, 'Finished' = 2, 'Removing' = 3, 'Unknown' = 4))",
    ),
    ("exception_code", "Nullable(UInt16)"),
    ("exception_text", "Nullable(String)"),
    ("query_finish_time", "Nullable(DateTime)"),
    ("query_duration_ms", "Nullable(UInt64)"),
];

/// `system.query_log` (the front's `QUERY_LOG_COLUMNS`).
pub const QUERY_LOG: Shape = &[
    ("type", "String"),
    ("event_date", "Date"),
    ("event_time", "DateTime('UTC')"),
    ("event_time_microseconds", "DateTime64(6, 'UTC')"),
    ("query_start_time", "DateTime('UTC')"),
    ("query_start_time_microseconds", "DateTime64(6, 'UTC')"),
    ("query_duration_ms", "UInt64"),
    ("read_rows", "UInt64"),
    ("read_bytes", "UInt64"),
    ("written_rows", "UInt64"),
    ("written_bytes", "UInt64"),
    ("result_rows", "UInt64"),
    ("result_bytes", "UInt64"),
    ("memory_usage", "UInt64"),
    ("current_database", "String"),
    ("query", "String"),
    ("normalized_query_hash", "UInt64"),
    ("query_kind", "String"),
    ("is_initial_query", "UInt8"),
    ("user", "String"),
    ("query_id", "String"),
    ("initial_user", "String"),
    ("initial_query_id", "String"),
    ("exception_code", "Int32"),
    ("exception", "String"),
    ("ProfileEvents", "Map(String, UInt64)"),
    ("loams_principal", "String"),
    ("loams_surface", "String"),
];

/// `ALIAS` columns: (table, column, the column it aliases).
pub const ALIASES: &[(&str, &str, &str)] = &[
    ("tables", "table", "name"),
    ("columns", "column", "name"),
    ("databases", "database", "name"),
    ("parts", "bytes", "bytes_on_disk"),
    ("parts", "marks_size", "marks_bytes"),
    ("parts", "part_name", "name"),
];
