//! Rendered configuration (§47 §5.1).

mod tidb;

pub use tidb::{
    CLUSTER_TLS_DIR, CONFIG_DIR, CONFIG_PATH, INIT_SQL_PATH, SERVER_VERSION, TLS_DIR, tidb,
    tidb_globals, tidb_init_sql,
};
